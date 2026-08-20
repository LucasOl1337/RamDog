//! Ponte para o helper `.NET` `hwtemp.exe` (LibreHardwareMonitorLib): temperatura de CPU e RAM.
//!
//! Não existe API pública do Windows para Tctl/Tdie (CPU) nem para os sensores DIMM da
//! placa-mãe — só dá para ler via acesso direto a hardware (MSR da CPU, Super I/O pelo SMBus),
//! que exige um driver de kernel assinado. Testado nesta máquina (MSI B650M + Ryzen 7 9800X3D):
//! sem admin, Tctl vem 0 e nenhum sensor DIMM aparece; com admin, os dois respondem. Em vez de
//! reimplementar esse driver em Rust, usamos a mesma LibreHardwareMonitorLib do TempHUD
//! (já validada nesta máquina) num executável .NET pequeno — `hwtemp/`, ao lado deste crate —
//! que imprime uma linha JSON por leitura em stdout.
//!
//! Inicia sempre que `hwtemp.exe` está ao lado do `ramdog.exe`. Sem admin o helper
//! sobe (asInvoker) mas Tctl/DIMM vêm vazios — a UI mostra "–", nunca inventa número.

use std::io::{BufRead, BufReader};
#[cfg(windows)]
use std::process::{Child, Command, Stdio};
#[cfg(not(windows))]
use std::process::Child;
use std::sync::{Arc, Mutex};

use serde::Deserialize;

#[derive(Deserialize, Default, Clone)]
struct HwTempMsg {
    cpu_temp: Option<f32>,
    #[serde(default)]
    dimm: Vec<f32>,
}

#[derive(Clone, Default)]
pub struct HwTemp {
    pub cpu_temp: Option<f32>,
    /// Um valor por pente de RAM populado; vazio = indisponível (sem admin, sem helper, ou
    /// placa-mãe sem Super I/O suportado pela lib).
    pub dimm_temps: Vec<f32>,
}

impl HwTemp {
    pub fn ram_max(&self) -> Option<f32> {
        self.dimm_temps.iter().cloned().fold(None, |acc, v| Some(acc.map_or(v, |a: f32| a.max(v))))
    }
}

pub struct HwTempReader {
    latest: Arc<Mutex<HwTemp>>,
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
                .stdin(Stdio::null())
                .stderr(Stdio::null());
            cmd.creation_flags(CREATE_NO_WINDOW);
            let mut child = cmd.spawn().ok()?;
            let stdout = child.stdout.take()?;
            let latest = Arc::new(Mutex::new(HwTemp::default()));
            let latest2 = latest.clone();
            let _ = std::thread::Builder::new().name("hwtemp-reader".into()).spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    if let Ok(msg) = serde_json::from_str::<HwTempMsg>(&line) {
                        if let Ok(mut g) = latest2.lock() {
                            g.cpu_temp = msg.cpu_temp;
                            g.dimm_temps = msg.dimm;
                        }
                    }
                }
            });
            Some(Self { latest, child })
        }
    }

    pub fn read(&self) -> HwTemp {
        self.latest.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

impl Drop for HwTempReader {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}
