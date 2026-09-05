//! Linux platform services. Commands never pass through a shell and have a deadline.
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

pub fn command(program: &str, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("timeout")
        .args(["--signal=TERM", "--kill-after=1s", "10s", program])
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let why = String::from_utf8_lossy(&output.stderr);
        let why = if why.trim().is_empty() {
            String::from_utf8_lossy(&output.stdout)
        } else {
            why
        };
        Err(format!(
            "{program}: {} ({})",
            why.trim().chars().take(500).collect::<String>(),
            output.status
        ))
    }
}

pub fn spawn<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Receiver<Result<T, String>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx
}

pub struct Job<T> {
    pub value: T,
    pub error: Option<String>,
    rx: Option<Receiver<Result<T, String>>>,
    updated: Option<Instant>,
}
impl<T: Default + Send + 'static> Default for Job<T> {
    fn default() -> Self {
        Self {
            value: T::default(),
            error: None,
            rx: None,
            updated: None,
        }
    }
}
impl<T: Default + Send + 'static> Job<T> {
    pub fn busy(&self) -> bool {
        self.rx.is_some()
    }
    pub fn due(&self, seconds: u64) -> bool {
        !self.busy()
            && self
                .updated
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(seconds))
    }
    pub fn start(&mut self, f: impl FnOnce() -> Result<T, String> + Send + 'static) {
        if self.busy() {
            return;
        }
        self.rx = Some(spawn(f));
    }
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.rx else { return false };
        match rx.try_recv() {
            Ok(result) => {
                match result {
                    Ok(value) => {
                        self.value = value;
                        self.error = None;
                    }
                    Err(e) => self.error = Some(e),
                }
                self.rx = None;
                self.updated = Some(Instant::now());
                true
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.error =
                    Some("A operação em segundo plano foi interrompida; tente atualizar.".into());
                self.rx = None;
                self.updated = Some(Instant::now());
                true
            }
            Err(mpsc::TryRecvError::Empty) => false,
        }
    }
    pub fn status(&self, ui: &mut egui::Ui) {
        if self.busy() {
            ui.spinner();
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        if let Some(error) = &self.error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
    }
}

pub fn state_dir() -> std::path::PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                .join(".local/state")
        })
        .join("RamDog")
}

pub fn log(message: &str) {
    use std::io::Write;
    let dir = state_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("ramdog.log");
    if path.metadata().is_ok_and(|m| m.len() > 2 * 1024 * 1024) {
        let _ = std::fs::rename(&path, dir.join("ramdog.previous.log"));
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let _ = writeln!(file, "{time} pid={} {message}", std::process::id());
    }
}

/// Desktop Exec grammar subset: quotes/backslash, no expansion and no shell.
pub fn words(input: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escape = false;
    let mut started = false;
    for c in input.chars() {
        if escape {
            word.push(c);
            escape = false;
            started = true;
            continue;
        }
        if c == '\\' && quote != Some('\'') {
            escape = true;
            started = true;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                word.push(c);
            }
            started = true;
        } else if c == '"' || c == '\'' {
            quote = Some(c);
            started = true;
        } else if c.is_whitespace() {
            if started {
                out.push(std::mem::take(&mut word));
                started = false;
            }
        } else {
            word.push(c);
            started = true;
        }
    }
    if escape || quote.is_some() {
        return Err("Aspas ou escape incompletos no comando".into());
    }
    if started {
        out.push(word);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn command_words_keep_arguments_literal() {
        assert_eq!(
            super::words("app \"two words\" '' '$(touch /tmp/no)' ").unwrap(),
            ["app", "two words", "", "$(touch /tmp/no)"]
        );
        assert!(super::words("app \"bad").is_err());
    }
}
