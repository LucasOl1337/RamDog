//! Ponte para o helper `.NET` `hwtemp.exe` (LibreHardwareMonitorLib): sensores e fans.
//!
//! Não existe API pública do Windows para Tctl/Tdie (CPU), sensores DIMM nem controle de fan
//! SuperIO — só dá para ler/escrever via acesso direto a hardware (MSR da CPU, Super I/O pelo
//! SMBus), que exige um driver de kernel assinado. Testado nesta máquina (MSI B650M + Ryzen 7
//! 9800X3D): sem admin, Tctl vem 0 e nenhum sensor DIMM aparece; com admin, os dois respondem.
//! Em vez de reimplementar esse driver em Rust, usamos a mesma LibreHardwareMonitorLib do
//! TempHUD num executável .NET pequeno — `hwtemp/`, ao lado deste crate — que imprime uma
//! linha JSON por leitura em stdout e aceita comandos de fan por stdin (ver protocolo no
//! `hwtemp/Program.cs`).
//!
//! A curva ESTABILIZAR mora no helper, não aqui: se o RamDog morrer, o helper detecta (pai
//! encerrado / EOF no stdin) e devolve os fans à BIOS antes de sair.
//!
//! Inicia sempre que `hwtemp.exe` está ao lado do `ramdog.exe`. Sem admin o helper
//! sobe (asInvoker) mas Tctl/DIMM/fans vêm vazios — a UI mostra "–", nunca inventa número.

use std::io::{BufRead, BufReader};
#[cfg(windows)]
use std::io::Write;
#[cfg(windows)]
use std::process::{Child, ChildStdin, Command, Stdio};
#[cfg(not(windows))]
use std::process::Child;
use std::sync::{Arc, Mutex};

use serde::Deserialize;

#[derive(Deserialize, Default, Clone)]
struct HwTempMsg {
    cpu_temp: Option<f32>,
    #[serde(default)]
    dimm: Vec<f32>,
    #[serde(default)]
    sensors: Vec<SensorRow>,
    #[serde(default)]
    fans: Vec<FanRow>,
    #[serde(default)]
    stab: StabState,
}

/// Uma leitura de sensor: `hw` é o grupo curto ("CPU", "GPU", "Placa-mãe", "RAM"),
/// `kind` é "temp" | "rpm" | "load".
#[derive(Deserialize, Clone)]
pub struct SensorRow {
    pub hw: String,
    pub name: String,
    pub kind: String,
    pub value: f32,
}

/// Um controle de fan SuperIO. `auto` = BIOS no comando; `guard` = proteção térmica
/// sobrepôs o % manual (CPU quente) e está segurando 100%.
#[derive(Deserialize, Clone)]
pub struct FanRow {
    pub name: String,
    pub pct: Option<f32>,
    pub rpm: Option<f32>,
    pub auto: bool,
    #[serde(default)]
    pub guard: bool,
}

#[derive(Deserialize, Clone, Copy, Default)]
pub struct StabState {
    pub on: bool,
    /// % que a curva está segurando agora (só significa algo com `on`).
    #[serde(default)]
    pub held: f32,
}

#[derive(Clone, Default)]
pub struct HwTemp {
    pub cpu_temp: Option<f32>,
    /// Um valor por pente de RAM populado; vazio = indisponível (sem admin, sem helper, ou
    /// placa-mãe sem Super I/O suportado pela lib).
    pub dimm_temps: Vec<f32>,
    /// Leituras completas (temps, loads, RPM) para a visão Térmico. Vazio = sem helper.
    pub sensors: Vec<SensorRow>,
    /// Controles de fan SuperIO. Vazio = sem admin ou placa sem Super I/O suportado.
    pub fans: Vec<FanRow>,
    pub stab: StabState,
}

impl HwTemp {
    pub fn ram_max(&self) -> Option<f32> {
        self.dimm_temps.iter().cloned().fold(None, |acc, v| Some(acc.map_or(v, |a: f32| a.max(v))))
    }
}

/// Canal de comando pro helper (linhas no stdin): `set <pct> <fan>`, `auto <fan>`,
/// `stab on`/`stab off`. Clonável — a UI guarda um e manda direto.
#[derive(Clone)]
pub struct HwCmd {
    #[cfg(windows)]
    stdin: Arc<Mutex<ChildStdin>>,
}

impl HwCmd {
    pub fn send(&self, line: &str) {
        #[cfg(windows)]
        if let Ok(mut w) = self.stdin.lock() {
            let _ = writeln!(w, "{line}");
            let _ = w.flush();
        }
        #[cfg(not(windows))]
        let _ = line;
    }
}

pub struct HwTempReader {
    latest: Arc<Mutex<HwTemp>>,
    #[cfg(windows)]
    stdin: Arc<Mutex<ChildStdin>>,
    child: Child,
}

impl HwTempReader {
    /// `None` só se `hwtemp.exe` não estiver ao lado do `ramdog.exe` (ou o spawn falhar).
    pub fn spawn() -> Option<Self> {
        #[cfg(not(windows))]
        {
            return None;
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            let exe = std::env::current_exe().ok()?;
            let helper = exe.parent()?.join("hwtemp.exe");
            if !helper.exists() {
                return None;
            }
            let pid = std::process::id();
            let mut cmd = Command::new(&helper);
            cmd.arg(pid.to_string())
                .stdout(Stdio::piped())
                .stdin(Stdio::piped())
                .stderr(Stdio::null());
            cmd.creation_flags(CREATE_NO_WINDOW);
            let mut child = cmd.spawn().ok()?;
            let stdout = child.stdout.take()?;
            let stdin = Arc::new(Mutex::new(child.stdin.take()?));
            let latest = Arc::new(Mutex::new(HwTemp::default()));
            let latest2 = latest.clone();
            let _ = std::thread::Builder::new().name("hwtemp-reader".into()).spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    if let Ok(msg) = serde_json::from_str::<HwTempMsg>(&line) {
                        if let Ok(mut g) = latest2.lock() {
                            g.cpu_temp = msg.cpu_temp;
                            g.dimm_temps = msg.dimm;
                            g.sensors = msg.sensors;
                            g.fans = msg.fans;
                            g.stab = msg.stab;
                        }
                    }
                }
            });
            Some(Self { latest, stdin, child })
        }
    }

    pub fn read(&self) -> HwTemp {
        self.latest.lock().map(|g| g.clone()).unwrap_or_default()
    }

    #[cfg(windows)]
    pub fn sender(&self) -> HwCmd {
        HwCmd { stdin: self.stdin.clone() }
    }
    #[cfg(not(windows))]
    pub fn sender(&self) -> HwCmd {
        HwCmd {}
    }
}

impl Drop for HwTempReader {
    fn drop(&mut self) {
        // "quit" deixa o helper devolver os fans à BIOS antes de sair; kill é só o plano B.
        // (Na saída normal do app este Drop nem roda — o helper percebe o pai morto e faz a
        // mesma limpeza sozinho.)
        #[cfg(windows)]
        {
            if let Ok(mut w) = self.stdin.lock() {
                let _ = writeln!(w, "quit");
                let _ = w.flush();
            }
            for _ in 0..20 {
                if matches!(self.child.try_wait(), Ok(Some(_))) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
        let _ = self.child.kill();
    }
}
