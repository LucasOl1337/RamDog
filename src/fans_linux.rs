//! Independent privileged fan controller. Owns PWM overrides and restores them on exit.
use crate::hwtemp::{FanRow, StabState};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct State {
    pub fans: Vec<FanRow>,
    pub stab: StabState,
    pub error: Option<String>,
}
#[derive(Clone)]
struct Fan {
    name: String,
    pwm: PathBuf,
    enable: PathBuf,
    rpm: PathBuf,
    original_pwm: u32,
    original_mode: u32,
    manual: Option<f32>,
    guard: bool,
}
fn number(path: &std::path::Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}
fn discover() -> Vec<Fan> {
    let mut fans = Vec::new();
    let Ok(entries) = std::fs::read_dir("/sys/class/hwmon") else {
        return fans;
    };
    for e in entries.flatten() {
        let dir = e.path();
        let chip = std::fs::read_to_string(dir.join("name")).unwrap_or_default();
        if ["amdgpu", "nvidia", "nouveau", "i915", "xe"].contains(&chip.trim()) {
            continue;
        }
        for i in 1..=16 {
            let pwm = dir.join(format!("pwm{i}"));
            let enable = dir.join(format!("pwm{i}_enable"));
            let (Some(value), Some(mode)) = (number(&pwm), number(&enable)) else {
                continue;
            };
            if ![0, 1, 2, 3, 4, 5, 99].contains(&mode) {
                continue;
            }
            let label = std::fs::read_to_string(dir.join(format!("fan{i}_label")))
                .unwrap_or_else(|_| format!("Fan {i}"));
            fans.push(Fan {
                name: format!(
                    "{} {} ({})",
                    chip.trim(),
                    label.trim(),
                    e.file_name().to_string_lossy()
                ),
                pwm,
                enable,
                rpm: dir.join(format!("fan{i}_input")),
                original_pwm: value,
                original_mode: mode,
                manual: None,
                guard: false,
            });
        }
    }
    fans
}
fn address(uid: u32, pid: u32) -> std::io::Result<SocketAddr> {
    SocketAddr::from_abstract_name(format!("ramdog-fans-{uid}-{pid}"))
}
fn request(command: &str) -> Result<State, String> {
    let addr = address(unsafe { libc::getuid() }, std::process::id()).map_err(|e| e.to_string())?;
    let mut stream = UnixStream::connect_addr(&addr).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_millis(500)))
        .map_err(|e| e.to_string())?;
    writeln!(stream, "{command}").map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(stream)
        .take(65536)
        .read_to_string(&mut line)
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&line).map_err(|e| e.to_string())
}
pub fn state() -> Option<State> {
    request("status").ok()
}
pub fn send(command: &str) {
    match request(command) {
        Err(e) => crate::linux::log(&format!("controle de fan: {e}")),
        Ok(s) => {
            if let Some(e) = s.error {
                crate::linux::log(&format!("controle de fan: {e}"));
            }
        }
    }
}
pub fn enable() {
    std::thread::spawn(|| {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let result = std::process::Command::new("pkexec")
            .arg(exe)
            .arg("--fan-helper")
            .arg(std::process::id().to_string())
            .status();
        if !matches!(result,Ok(s)if s.success()) {
            crate::linux::log("helper de fans encerrado ou autenticação cancelada");
        }
    });
}
pub fn supported() -> bool {
    !discover().is_empty()
}
fn cpu_temperature() -> Option<f32> {
    std::fs::read_dir("/sys/class/hwmon")
        .ok()?
        .flatten()
        .filter_map(|e| {
            let chip = std::fs::read_to_string(e.path().join("name")).ok()?;
            if ![
                "k10temp",
                "coretemp",
                "zenpower",
                "zenpower3",
                "cpu_thermal",
            ]
            .contains(&chip.trim())
            {
                return None;
            }
            (1..=32)
                .filter_map(|i| {
                    number(&e.path().join(format!("temp{i}_input"))).map(|n| n as f32 / 1000.0)
                })
                .filter(|n| (1.0..=150.0).contains(n))
                .reduce(f32::max)
        })
        .reduce(f32::max)
}
fn restore(fan: &mut Fan) -> std::io::Result<()> {
    if fan.manual.is_some() {
        // If the original owner used manual mode, restore its exact duty cycle.
        if fan.original_mode == 1 {
            std::fs::write(&fan.pwm, fan.original_pwm.to_string())?;
        }
        std::fs::write(&fan.enable, fan.original_mode.to_string())?;
        fan.manual = None;
        fan.guard = false;
    }
    Ok(())
}
pub fn target(temp: Option<f32>, manual: f32, stab: bool, guard: bool) -> (f32, bool) {
    let Some(temp) = temp else {
        return (100.0, true);
    };
    let guard = temp >= 80.0 || (guard && temp > 72.0);
    if stab {
        (
            (50.0 + (temp - 80.0).max(0.0) / 12.0 * 50.0).clamp(50.0, 100.0),
            false,
        )
    } else if guard {
        (100.0, true)
    } else {
        (manual.clamp(30.0, 100.0), false)
    }
}
static STOP: AtomicBool = AtomicBool::new(false);
extern "C" fn stop(_: i32) {
    STOP.store(true, Ordering::Relaxed);
}
fn process_identity(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)
        .map(str::to_string)
}
pub fn helper(pid: u32) -> Result<(), String> {
    if unsafe { libc::geteuid() } != 0 {
        return Err("O helper precisa de autenticação administrativa".into());
    }
    use std::os::unix::fs::MetadataExt;
    let uid = std::fs::metadata(format!("/proc/{pid}"))
        .map_err(|e| e.to_string())?
        .uid();
    let identity = process_identity(pid).ok_or("Processo pai indisponível")?;
    let listener = UnixListener::bind_addr(&address(uid, pid).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    unsafe {
        libc::signal(libc::SIGTERM, stop as *const () as usize);
        libc::signal(libc::SIGINT, stop as *const () as usize);
    }
    let mut fans = discover();
    if fans.is_empty() {
        return Err("Driver sem controles PWM graváveis".into());
    }
    let mut state = State::default();
    let mut stab = false;
    let mut held = 50.0f32;
    while !STOP.load(Ordering::Relaxed) && process_identity(pid).as_deref() == Some(&identity) {
        if let Ok((mut client, _)) = listener.accept() {
            // Abstract sockets have no filesystem permissions; authenticate the peer.
            let mut credentials = libc::ucred {
                pid: 0,
                uid: 0,
                gid: 0,
            };
            let mut size = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
            use std::os::fd::AsRawFd;
            let ok = unsafe {
                libc::getsockopt(
                    client.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_PEERCRED,
                    (&mut credentials as *mut libc::ucred).cast(),
                    &mut size,
                )
            } == 0;
            if !ok || credentials.uid != uid {
                continue;
            }
            let _ = client.set_read_timeout(Some(Duration::from_millis(200)));
            let _ = client.set_write_timeout(Some(Duration::from_millis(200)));
            let mut line = String::new();
            let _ = BufReader::new(&client).take(4096).read_line(&mut line);
            let line = line.trim();
            if line != "status" {
                state.error = None;
            }
            let operation = (|| -> Result<(), String> {
                if line == "status" {
                    return Ok(());
                }
                if line == "stab on" {
                    stab = true;
                    for fan in &mut fans {
                        fan.manual = Some(50.0);
                    }
                    return Ok(());
                }
                if line == "stab off" {
                    stab = false;
                    for fan in &mut fans {
                        restore(fan).map_err(|e| e.to_string())?;
                    }
                    return Ok(());
                }
                if let Some(name) = line.strip_prefix("auto ") {
                    let fan = fans
                        .iter_mut()
                        .find(|f| f.name == name)
                        .ok_or("Fan desconhecido")?;
                    return restore(fan).map_err(|e| e.to_string());
                }
                if let Some(rest) = line.strip_prefix("set ") {
                    let (pct, name) = rest.split_once(' ').ok_or("Comando incompleto")?;
                    let pct = pct.parse::<f32>().map_err(|e| e.to_string())?;
                    if !pct.is_finite() || !(30.0..=100.0).contains(&pct) {
                        return Err("A faixa manual é 30–100%".into());
                    }
                    let fan = fans
                        .iter_mut()
                        .find(|f| f.name == name)
                        .ok_or("Fan desconhecido")?;
                    fan.manual = Some(pct);
                    return Ok(());
                }
                Err("Comando desconhecido".into())
            })();
            if let Err(e) = operation {
                state.error = Some(e);
            }
            let _ = serde_json::to_writer(&mut client, &state);
            let _ = client.flush();
        }
        let temp = cpu_temperature();
        let desired = target(temp, 50.0, true, false).0;
        held = if temp.is_none_or(|t| t >= 95.0) {
            100.0
        } else {
            desired.clamp(held - 0.3, held + 0.3)
        };
        for fan in &mut fans {
            if let Some(manual) = fan.manual {
                let (pct, guard) = if stab {
                    (held, false)
                } else {
                    target(temp, manual, false, fan.guard)
                };
                fan.guard = guard;
                let result = (|| -> std::io::Result<()> {
                    std::fs::write(&fan.enable, "1")?;
                    std::fs::write(&fan.pwm, ((pct * 255.0 / 100.0).round() as u32).to_string())
                })();
                if let Err(e) = result {
                    state.error = Some(format!("{}: {e}", fan.name));
                    let _ = restore(fan);
                }
            }
        }
        state.fans = fans
            .iter()
            .map(|f| FanRow {
                name: f.name.clone(),
                pct: number(&f.pwm).map(|n| n as f32 / 255.0 * 100.0),
                rpm: number(&f.rpm).map(|n| n as f32),
                auto: f.manual.is_none(),
                guard: f.guard,
            })
            .collect();
        state.stab = StabState { on: stab, held };
        std::thread::sleep(Duration::from_millis(100));
    }
    let mut errors = Vec::new();
    for fan in &mut fans {
        if let Err(e) = restore(fan) {
            errors.push(format!("{}: {e}", fan.name));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn thermal_guard_has_hysteresis_and_missing_sensor_fails_high() {
        assert_eq!(super::target(None, 30.0, false, false), (100.0, true));
        assert_eq!(super::target(Some(81.0), 40.0, false, false), (100.0, true));
        assert_eq!(super::target(Some(75.0), 40.0, false, true), (100.0, true));
        assert_eq!(super::target(Some(70.0), 40.0, false, true), (40.0, false));
        assert_eq!(super::target(Some(92.0), 50.0, true, false), (100.0, false));
    }
}
