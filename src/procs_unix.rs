//! Amostragem de processos no macOS/Linux via sysinfo.

use std::collections::HashMap;
use std::time::Instant;

use sysinfo::{Pid, System};

use super::{launcher_from_env_lines, Launcher, MemStatus, ProcInfo};

pub struct Sampler {
    sys: System,
    ncpu: f32,
    prev_io: HashMap<u32, (u64, Instant)>,
}

impl Sampler {
    pub fn new() -> Self {
        let mut sys = System::new();
        sys.refresh_all();
        let ncpu = sys.cpus().len().max(1) as f32;
        Self {
            sys,
            ncpu,
            prev_io: HashMap::new(),
        }
    }

    pub fn sample(&mut self) -> Vec<ProcInfo> {
        self.sys.refresh_all();
        let now = Instant::now();
        let mut out = Vec::with_capacity(self.sys.processes().len());
        let mut seen = HashMap::new();

        for (pid, p) in self.sys.processes() {
            let pid_u = pid.as_u32();
            seen.insert(pid_u, ());
            let name = os(p.name());
            let exe = p.exe().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
            let cmdline = p
                .cmd()
                .iter()
                .map(os)
                .collect::<Vec<_>>()
                .join(" ");
            let lines: Vec<String> = p.environ().iter().map(os).collect();
            let launcher = launcher_from_env_lines(&lines);
            let io = p.disk_usage();
            let io_total = io.total_read_bytes.saturating_add(io.total_written_bytes);
            let disk_bps = match self.prev_io.get(&pid_u) {
                Some((prev, t0)) => {
                    let dt = now.duration_since(*t0).as_secs_f64();
                    if dt > 0.0 {
                        io_total.saturating_sub(*prev) as f64 / dt
                    } else {
                        0.0
                    }
                }
                None => 0.0,
            };
            self.prev_io.insert(pid_u, (io_total, now));

            let start = p.start_time();
            let create_time = (start as i64).saturating_mul(10_000_000) + 116_444_736_000_000_000;
            let rss = p.memory();
            let virt = p.virtual_memory();
            // sysinfo: 0–100*ncpu; aqui 0–100 da máquina, igual ao Windows.
            let cpu_pct = (p.cpu_usage() / self.ncpu).clamp(0.0, 100.0);
            let ppid = p.parent().map(|x| x.as_u32()).unwrap_or(0);
            let name_lower = name.to_lowercase();
            out.push(ProcInfo {
                pid: pid_u,
                ppid: 0,
                raw_ppid: ppid,
                name,
                name_lower,
                exe_path: exe,
                cmdline,
                private_ws: rss,
                working_set: rss,
                commit: virt,
                threads: 0,
                handles: 0,
                session: 0,
                create_time,
                cpu_pct,
                disk_bps,
                gpu_pct: 0.0,
                launcher,
            });
        }
        self.prev_io.retain(|k, _| seen.contains_key(k));

        let by_pid: HashMap<u32, i64> = out.iter().map(|p| (p.pid, p.create_time)).collect();
        for p in out.iter_mut() {
            if p.raw_ppid != 0 && p.raw_ppid != p.pid {
                if let Some(&pct) = by_pid.get(&p.raw_ppid) {
                    if pct <= p.create_time {
                        p.ppid = p.raw_ppid;
                    }
                }
            }
        }
        out
    }
}

fn os(s: impl AsRef<std::ffi::OsStr>) -> String {
    s.as_ref().to_string_lossy().into_owned()
}

pub fn mem_status() -> MemStatus {
    let mut sys = System::new();
    sys.refresh_memory();
    MemStatus {
        total_phys: sys.total_memory(),
        avail_phys: sys.available_memory(),
        total_commit: sys.total_memory().saturating_add(sys.total_swap()),
        avail_commit: sys.available_memory().saturating_add(sys.free_swap()),
    }
}

pub fn kill(pid: u32) -> Result<(), String> {
    let pid_i = pid as i32;
    let rc = unsafe { libc::kill(pid_i, libc::SIGKILL) };
    if rc == 0 {
        Ok(())
    } else {
        let err = std::io::Error::last_os_error();
        match err.raw_os_error() {
            Some(1) => Err("acesso negado (sudo?)".into()),
            Some(3) => Err("processo já encerrado".into()),
            _ => Err(err.to_string()),
        }
    }
}

pub fn is_admin() -> bool {
    unsafe { libc::geteuid() == 0 }
}

pub fn enable_debug_privilege() {}

// silencia aviso se o `kill` import acima parecer unused no glob
#[allow(dead_code)]
fn _pid_ty(_: Pid) {}
