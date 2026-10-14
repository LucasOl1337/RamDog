//! Modos da máquina: botões no cabeçalho que ligam e desligam perfis (jogo, silencioso,
//! noite, desempenho...).
//!
//! O RamDog não sabe o que cada modo faz. Ele lê `modes.json` ao lado do `config.json` e
//! roda os comandos no shell: `on` liga, `off` desliga, `state` sai com 0 quando o modo
//! está ligado. Sem o arquivo o botão não aparece.
//!
//! ```json
//! { "modes": [
//!   { "name": "Jogo", "hint": "Reserva CPU e RAM pro jogo",
//!     "on": "jogarsim", "off": "jogarnao",
//!     "state": "test -e \"$XDG_RUNTIME_DIR/jogar/active\"" }
//! ] }
//! ```
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Deserialize)]
pub struct Mode {
    pub name: String,
    #[serde(default)]
    pub hint: String,
    pub on: String,
    pub off: String,
    pub state: String,
}

#[derive(Debug, Default, Deserialize)]
struct File {
    #[serde(default)]
    modes: Vec<Mode>,
}

pub fn path() -> PathBuf {
    crate::config::config_path().with_file_name("modes.json")
}

/// Um modo pode desligar outro (desempenho para o silencioso), então o estado de todos é
/// relido junto depois de cada clique.
const POLL: Duration = Duration::from_secs(5);

pub struct Modes {
    pub list: Vec<Mode>,
    /// `None` enquanto a primeira leitura não voltou.
    pub active: Vec<Option<bool>>,
    /// Índice do modo cujo comando está rodando.
    pub busy: Option<usize>,
    pub error: Option<String>,
    poll: Option<Receiver<Vec<Option<bool>>>>,
    polled: Option<Instant>,
    toggle: Option<Receiver<Result<String, String>>>,
}

impl Modes {
    pub fn load() -> Self {
        let (list, error) = match std::fs::read_to_string(path()) {
            Ok(text) => match serde_json::from_str::<File>(&text) {
                Ok(f) => (f.modes, None),
                Err(e) => (Vec::new(), Some(format!("modes.json: {e}"))),
            },
            Err(_) => (Vec::new(), None),
        };
        Self {
            active: vec![None; list.len()],
            list,
            busy: None,
            error,
            poll: None,
            polled: None,
            toggle: None,
        }
    }

    /// Recolhe resultados prontos e dispara a próxima leitura de estado. Devolve a
    /// mensagem do comando que acabou de terminar: `(texto, erro)`.
    pub fn tick(&mut self) -> Option<(String, bool)> {
        let mut done = None;
        if let Some(rx) = &self.toggle {
            if let Ok(r) = rx.try_recv() {
                let name = self
                    .busy
                    .and_then(|i| self.list.get(i))
                    .map(|m| m.name.clone());
                let name = name.unwrap_or_default();
                done = Some(match r {
                    Ok(out) if out.trim().is_empty() => (name, false),
                    Ok(out) => (format!("{name}: {}", last_line(&out)), false),
                    Err(e) => (format!("{name}: {}", last_line(&e)), true),
                });
                self.toggle = None;
                self.busy = None;
                self.polled = None;
            }
        }
        if let Some(rx) = &self.poll {
            if let Ok(states) = rx.try_recv() {
                self.active = states;
                self.poll = None;
            }
        }
        let due = self.polled.is_none_or(|t| t.elapsed() >= POLL);
        if !self.list.is_empty() && self.poll.is_none() && self.toggle.is_none() && due {
            let cmds: Vec<String> = self.list.iter().map(|m| m.state.clone()).collect();
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let states = cmds
                    .iter()
                    .map(|c| shell(c).map(|_| true).ok().or(Some(false)))
                    .collect();
                let _ = tx.send(states);
            });
            self.poll = Some(rx);
            self.polled = Some(Instant::now());
        }
        done
    }

    pub fn toggle(&mut self, i: usize) {
        if self.busy.is_some() {
            return;
        }
        let Some(m) = self.list.get(i) else { return };
        let cmd = if self.active.get(i).copied().flatten() == Some(true) {
            m.off.clone()
        } else {
            m.on.clone()
        };
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(shell(&cmd));
        });
        self.busy = Some(i);
        self.toggle = Some(rx);
    }

    /// Nomes dos modos ligados, na ordem do arquivo.
    pub fn on_names(&self) -> Vec<&str> {
        self.list
            .iter()
            .zip(&self.active)
            .filter(|(_, a)| **a == Some(true))
            .map(|(m, _)| m.name.as_str())
            .collect()
    }
}

fn last_line(text: &str) -> &str {
    text.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
}

/// Roda no shell do sistema; erro traz o stderr (ou o stdout, se o stderr veio vazio).
fn shell(cmd: &str) -> Result<String, String> {
    #[cfg(windows)]
    let out = std::process::Command::new("cmd").args(["/C", cmd]).output();
    #[cfg(not(windows))]
    let out = std::process::Command::new("sh").args(["-c", cmd]).output();
    let out = out.map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if out.status.success() {
        Ok(stdout)
    } else {
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        Err(if err.trim().is_empty() { stdout } else { err })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modes_file() {
        let f: File = serde_json::from_str(
            r#"{"modes":[{"name":"Jogo","on":"a","off":"b","state":"true"}]}"#,
        )
        .unwrap();
        assert_eq!(f.modes.len(), 1);
        assert_eq!(f.modes[0].name, "Jogo");
        assert!(f.modes[0].hint.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn state_follows_exit_code() {
        assert!(shell("true").is_ok());
        assert_eq!(shell("echo oi >&2; false").unwrap_err().trim(), "oi");
    }
}
