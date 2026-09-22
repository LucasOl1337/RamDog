//! Amostragem de processos no macOS/Linux.
//!
//! Linux: `/proc` direto. sysinfo 0.33 indexa cada thread como processo e mantém o
//! `/proc/*/stat` aberto até metade do rlimit — na prática milhares de FDs e CPU
//! queimada no refresh. Aqui não se abre `/proc/*/task`, não se segura FD, e o que
//! não muda (exe, cmdline, environ) é lido uma vez por (pid, starttime).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::{launcher_from_env_lines, KillOutcome, MemStatus, ProcInfo};

#[cfg(not(target_os = "linux"))]
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

#[cfg(target_os = "linux")]
const CPU_EMA_TAU: f64 = 1.0;
#[cfg(target_os = "linux")]
const SMAPS_INTERVAL: Duration = Duration::from_secs(5);
#[cfg(target_os = "linux")]
// 96 × ~0,2 ms na thread de amostragem: varre os ~700 processos de um desktop
// carregado em poucos ticks. Com 24, a coluna PSS/Privado só tinha leitura
// fresca pra 24 linhas e o resto mostrava zero.
const SMAPS_PER_SAMPLE: usize = 96;
#[cfg(target_os = "linux")]
const FD_INTERVAL: Duration = Duration::from_secs(5);
#[cfg(target_os = "linux")]
const FD_PER_SAMPLE: usize = 80;

#[cfg(target_os = "linux")]
struct StaticInfo {
    exe_path: String,
    cmdline: String,
    launcher: super::Launcher,
}

#[cfg(target_os = "linux")]
struct Live {
    cpu_ticks: u64,
    /// `cutime + cstime`: CPU dos filhos que já encerraram e o pai recolheu com `wait`.
    child_ticks: u64,
    io_bytes: u64,
    ema: f32,
    at: Instant,
}

#[cfg(target_os = "linux")]
struct CachedRead<T> {
    value: Option<T>,
    at: Instant,
}

#[cfg(target_os = "linux")]
type ProcessKey = (u32, u64);

#[cfg(target_os = "linux")]
fn refresh_cached_reads<T>(
    cache: &mut HashMap<ProcessKey, CachedRead<T>>,
    candidates: &[(ProcessKey, u64)],
    now: Instant,
    interval: Duration,
    limit: usize,
    own: u32,
    mut read: impl FnMut(u32) -> Option<T>,
) {
    let mut due: Vec<_> = candidates.iter().copied().filter(|(key, _)| {
        cache.get(key).is_none_or(|c| now.duration_since(c.at) >= interval)
    }).collect();
    // Unattempted entries first, then the oldest attempt. RSS and our own PID
    // only break ties: a fast-expiring large process must not starve the rest.
    due.sort_by(|(a, ra), (b, rb)| {
        cache.get(a).map(|c| c.at).cmp(&cache.get(b).map(|c| c.at))
            .then((b.0 == own).cmp(&(a.0 == own)))
            .then(rb.cmp(ra))
            .then(a.cmp(b))
    });
    for (key, _) in due.into_iter().take(limit) {
        // Denied/disappeared processes get the same cooldown as successful reads.
        // A failed refresh clears the old value instead of reporting stale data.
        cache.insert(key, CachedRead { value: read(key.0), at: now });
    }
}

#[cfg(target_os = "linux")]
fn cached_value<T: Copy>(cache: &HashMap<ProcessKey, CachedRead<T>>, key: &ProcessKey) -> Option<T> {
    // Sem prazo de validade: a última leitura boa vale até a próxima tentativa.
    // Expirar em `interval` fazia PSS/Privado piscar pra zero em todo processo
    // fora da janela de releitura — dado atrasado uns segundos é melhor que dado
    // zerado. Leitura que falhou limpa o valor, e processo morto sai no retain.
    cache.get(key).and_then(|c| c.value)
}

pub struct Sampler {
    #[cfg(not(target_os = "linux"))]
    sys: System,
    ncpu: f32,
    #[cfg(not(target_os = "linux"))]
    prev_io: HashMap<u32, (u64, Instant, u64)>,
    #[cfg(target_os = "linux")]
    clk_tck: u64,
    #[cfg(target_os = "linux")]
    page_size: u64,
    #[cfg(target_os = "linux")]
    boot_time: u64,
    #[cfg(target_os = "linux")]
    statics: HashMap<(u32, u64), StaticInfo>,
    #[cfg(target_os = "linux")]
    live: HashMap<(u32, u64), Live>,
    #[cfg(target_os = "linux")]
    smaps: HashMap<ProcessKey, CachedRead<(u64, u64)>>,
    #[cfg(target_os = "linux")]
    fds: HashMap<ProcessKey, CachedRead<u32>>,
    /// Instante da amostra anterior: a janela que o medidor do topo também mede.
    #[cfg(target_os = "linux")]
    last_at: Option<Instant>,
}

impl Sampler {
    pub fn new() -> Self {
        #[cfg(target_os = "linux")]
        {
            let ncpu = sysconf_positive(libc::_SC_NPROCESSORS_ONLN) as f32;
            Self {
                ncpu: ncpu.max(1.0),
                clk_tck: sysconf_positive(libc::_SC_CLK_TCK),
                page_size: sysconf_positive(libc::_SC_PAGESIZE),
                boot_time: boot_time_secs(),
                statics: HashMap::new(),
                live: HashMap::new(),
                smaps: HashMap::new(),
                fds: HashMap::new(),
                last_at: None,
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let mut sys = System::new();
            sys.refresh_cpu_usage();
            sys.refresh_processes_specifics(ProcessesToUpdate::All, true, process_refresh());
            let ncpu = sys.cpus().len().max(1) as f32;
            Self {
                sys,
                ncpu,
                prev_io: HashMap::new(),
            }
        }
    }

    pub fn sample(&mut self) -> Vec<ProcInfo> {
        #[cfg(target_os = "linux")]
        {
            self.sample_linux()
        }
        #[cfg(not(target_os = "linux"))]
        {
            self.sample_sysinfo()
        }
    }

    #[cfg(target_os = "linux")]
    fn sample_linux(&mut self) -> Vec<ProcInfo> {
        // /proc covers the machine, independent of this process's affinity/cgroup.
        // Refresh for CPU hotplug as well as the initial sample.
        self.ncpu = sysconf_positive(libc::_SC_NPROCESSORS_ONLN) as f32;
        let now = Instant::now();
        // Janela desta amostra, a mesma que o medidor do topo cobre. `None` só na primeira.
        let window = self.last_at.map(|t| now.duration_since(t).as_secs_f64());
        self.last_at = Some(now);
        let uptime = read_uptime_secs();
        let own = std::process::id();
        let Ok(dir) = std::fs::read_dir("/proc") else {
            return Vec::new();
        };
        let mut parsed = Vec::new();
        for entry in dir.flatten() {
            let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
                continue;
            };
            let path = entry.path();
            let Ok(stat_text) = std::fs::read_to_string(path.join("stat")) else {
                continue;
            };
            let Some(stat) = parse_stat(&stat_text) else {
                continue;
            };
            if stat.pid != pid {
                continue;
            }
            let (virt, rss, shared) = match std::fs::read_to_string(path.join("statm"))
                .ok()
                .and_then(|t| parse_statm(&t, self.page_size))
            {
                Some(m) => m,
                None => (stat.vsize, stat.rss_pages.saturating_mul(self.page_size), 0),
            };
            parsed.push(RawProc {
                pid,
                stat,
                virt,
                rss,
                shared,
                io_bytes: read_io_bytes(pid).unwrap_or(0),
            });
        }

        let candidates: Vec<_> = parsed.iter()
            .map(|p| ((p.pid, p.stat.starttime), p.rss)).collect();
        refresh_cached_reads(
            &mut self.smaps, &candidates, now, SMAPS_INTERVAL, SMAPS_PER_SAMPLE, own,
            |pid| std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup"))
                .ok().and_then(|t| parse_smaps_rollup(&t)),
        );
        refresh_cached_reads(
            &mut self.fds, &candidates, now, FD_INTERVAL, FD_PER_SAMPLE, own, count_fds,
        );

        let mut seen = HashMap::new();
        let mut out = Vec::with_capacity(parsed.len());
        for p in parsed {
            let key = (p.pid, p.stat.starttime);
            seen.insert(key, ());
            let st = self.statics.entry(key).or_insert_with(|| {
                let cmdline = read_cmdline(p.pid);
                let exe = read_exe(p.pid);
                let lines = read_environ(p.pid);
                let mut launcher = launcher_from_env_lines(&lines);
                launcher.unit = read_unit(p.pid);
                if launcher.steam_app_id.is_none() {
                    for hay in [cmdline.as_str(), exe.as_str()] {
                        if let Some(id) = hay
                            .split("compatdata/")
                            .nth(1)
                            .or_else(|| hay.split("rungameid/").nth(1))
                            .and_then(|rest| {
                                rest.chars()
                                    .take_while(|c| c.is_ascii_digit())
                                    .collect::<String>()
                                    .parse()
                                    .ok()
                            })
                            .filter(|n| *n > 0)
                        {
                            launcher.steam_app_id = Some(id);
                            break;
                        }
                    }
                }
                StaticInfo {
                    exe_path: exe,
                    cmdline,
                    launcher,
                }
            });
            let ticks = p.stat.utime.saturating_add(p.stat.stime);
            let child_ticks = p.stat.cutime.saturating_add(p.stat.cstime);
            let (cpu_raw, cpu_pct, cpu_children, disk_bps) = match self.live.get(&key) {
                Some(prev) => {
                    let dt = now.duration_since(prev.at).as_secs_f64();
                    let raw = cpu_machine_pct(
                        ticks.saturating_sub(prev.cpu_ticks),
                        dt,
                        self.clk_tck,
                        self.ncpu,
                    );
                    let children = cpu_machine_pct(
                        child_ticks.saturating_sub(prev.child_ticks),
                        dt,
                        self.clk_tck,
                        self.ncpu,
                    );
                    let ema = smooth(prev.ema, raw, dt);
                    let disk = if dt > 0.0 {
                        p.io_bytes.saturating_sub(prev.io_bytes) as f64 / dt
                    } else {
                        0.0
                    };
                    (raw, ema, children, disk)
                }
                // Processo que nasceu depois da amostra anterior. Antes entrava com 0% e só
                // ganhava número na amostra seguinte, se ainda estivesse vivo — um `cargo`
                // ou `rg` de 4 s a 100% de um núcleo nunca aparecia, e o medidor do topo
                // ficava sem explicação. O kernel dá o instante de criação, então o total
                // acumulado já é uma taxa: dividido pela janela se nasceu dentro dela, pela
                // idade se é mais velho (só acontece se a leitura anterior falhou).
                None => match window {
                    Some(window) => {
                        let age = uptime - p.stat.starttime as f64 / self.clk_tck.max(1) as f64;
                        let dt = first_sample_window(window, age);
                        let raw = cpu_machine_pct(ticks, dt, self.clk_tck, self.ncpu);
                        let children = cpu_machine_pct(child_ticks, dt, self.clk_tck, self.ncpu);
                        (raw, raw, children, 0.0)
                    }
                    None => (0.0, 0.0, 0.0, 0.0),
                },
            };
            self.live.insert(
                key,
                Live {
                    cpu_ticks: ticks,
                    child_ticks,
                    io_bytes: p.io_bytes,
                    ema: cpu_pct,
                    at: now,
                },
            );
            let linux_memory = cached_value(&self.smaps, &key);
            let private_ws = linux_memory
                .map(|m| m.0)
                .unwrap_or_else(|| p.rss.saturating_sub(p.shared));
            let name = if p.stat.name.is_empty() {
                format!("[pid {}]", p.pid)
            } else {
                p.stat.name.clone()
            };
            let unix = self
                .boot_time
                .saturating_add(p.stat.starttime / self.clk_tck.max(1));
            let create_time = unix_to_filetime(unix);
            out.push(ProcInfo {
                pid: p.pid,
                ppid: 0,
                raw_ppid: p.stat.ppid,
                name_lower: name.to_lowercase(),
                name,
                exe_path: st.exe_path.clone(),
                cmdline: st.cmdline.clone(),
                linux_memory,
                private_ws,
                working_set: p.rss,
                commit: p.virt,
                threads: p.stat.num_threads,
                handles: cached_value(&self.fds, &key),
                session: p.stat.session,
                create_time,
                cpu_pct,
                cpu_raw_pct: cpu_raw,
                cpu_children_pct: cpu_children,
                disk_bps,
                gpu_pct: 0.0,
                gpu_load: None,
                gpu_vram: None,
                kernel_state: p.stat.state,
                has_window: false,
                focused: false,
                window_title: None,
                window_class: None,
                launcher: st.launcher.clone(),
            });
        }
        self.statics.retain(|k, _| seen.contains_key(k));
        self.live.retain(|k, _| seen.contains_key(k));
        self.smaps.retain(|k, _| seen.contains_key(k));
        self.fds.retain(|k, _| seen.contains_key(k));

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

    #[cfg(not(target_os = "linux"))]
    fn sample_sysinfo(&mut self) -> Vec<ProcInfo> {
        self.sys
            .refresh_processes_specifics(ProcessesToUpdate::All, true, process_refresh());
        let now = Instant::now();
        let mut out = Vec::with_capacity(self.sys.processes().len());
        let mut seen = HashMap::new();

        for (pid, p) in self.sys.processes() {
            if matches!(p.thread_kind(), Some(sysinfo::ThreadKind::Userland)) {
                continue;
            }
            let pid_u = pid.as_u32();
            seen.insert(pid_u, ());
            let name = os(p.name());
            let exe = p
                .exe()
                .map(|x| x.to_string_lossy().into_owned())
                .unwrap_or_default();
            let cmdline = p.cmd().iter().map(os).collect::<Vec<_>>().join(" ");
            let lines: Vec<String> = p.environ().iter().map(os).collect();
            let launcher = launcher_from_env_lines(&lines);
            let io = p.disk_usage();
            let io_total = io.total_read_bytes.saturating_add(io.total_written_bytes);
            let disk_bps = match self.prev_io.get(&pid_u) {
                Some((prev, t0, start)) if *start == p.start_time() => {
                    let dt = now.duration_since(*t0).as_secs_f64();
                    if dt > 0.0 {
                        io_total.saturating_sub(*prev) as f64 / dt
                    } else {
                        0.0
                    }
                }
                _ => 0.0,
            };
            self.prev_io.insert(pid_u, (io_total, now, p.start_time()));

            let start = p.start_time();
            let create_time = unix_to_filetime(start);
            let rss = p.memory();
            let virt = p.virtual_memory();
            let cpu_pct = (p.cpu_usage() / self.ncpu).clamp(0.0, 100.0);
            let ppid = p.parent().map(|x| x.as_u32()).unwrap_or(0);
            let session = p.session_id().map(|s| s.as_u32()).unwrap_or(0);
            let threads = p.tasks().map(|t| t.len() as u32 + 1).unwrap_or(0);
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
                threads,
                handles: None,
                session,
                create_time,
                cpu_pct,
                cpu_raw_pct: cpu_pct,
                cpu_children_pct: 0.0,
                disk_bps,
                gpu_pct: 0.0,
                gpu_load: None,
                gpu_vram: None,
                kernel_state: None,
                has_window: false,
                focused: false,
                window_title: None,
                window_class: None,
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

#[cfg(not(target_os = "linux"))]
fn process_refresh() -> ProcessRefreshKind {
    ProcessRefreshKind::nothing()
        .with_memory()
        .with_cpu()
        .with_disk_usage()
        .with_exe(UpdateKind::OnlyIfNotSet)
        .with_cmd(UpdateKind::OnlyIfNotSet)
        .with_environ(UpdateKind::OnlyIfNotSet)
}

#[cfg(not(target_os = "linux"))]
fn os(s: impl AsRef<std::ffi::OsStr>) -> String {
    s.as_ref().to_string_lossy().into_owned()
}

fn unix_to_filetime(unix_secs: u64) -> i64 {
    (unix_secs as i64).saturating_mul(10_000_000) + 116_444_736_000_000_000
}

pub fn mem_status() -> MemStatus {
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
        let total = kb_field(&text, "MemTotal:").unwrap_or(0);
        let avail = kb_field(&text, "MemAvailable:").unwrap_or(0);
        let swap_total = kb_field(&text, "SwapTotal:").unwrap_or(0);
        let swap_free = kb_field(&text, "SwapFree:").unwrap_or(0);
        let linux_commit = match (
            kb_field(&text, "Committed_AS:"),
            kb_field(&text, "CommitLimit:"),
        ) {
            (Some(a), Some(b)) => Some((a, b)),
            _ => None,
        };
        return MemStatus {
            linux_commit,
            total_phys: total,
            avail_phys: avail,
            total_commit: linux_commit.map(|m| m.1).unwrap_or(0),
            avail_commit: linux_commit.map(|m| m.1.saturating_sub(m.0)).unwrap_or(0),
            swap_used: swap_total.saturating_sub(swap_free),
            swap_total,
        };
    }
    #[cfg(not(target_os = "linux"))]
    {
        let mut sys = System::new();
        sys.refresh_memory();
        MemStatus {
            total_phys: sys.total_memory(),
            avail_phys: sys.available_memory(),
            total_commit: sys.total_memory().saturating_add(sys.total_swap()),
            avail_commit: sys.available_memory().saturating_add(sys.free_swap()),
            swap_used: sys.total_swap().saturating_sub(sys.free_swap()),
            swap_total: sys.total_swap(),
        }
    }
}

pub fn kill(pid: u32) -> KillOutcome {
    signal_pid(pid, libc::SIGKILL)
}

pub fn terminate(pid: u32) -> KillOutcome {
    signal_pid(pid, libc::SIGTERM)
}

/// Zombie não morre com sinal: já morreu. O que resolve é o pai chamar `wait`, e o
/// SIGCHLD é o toque que a maioria dos pais bem escritos atende.
pub fn nudge_parent(ppid: u32) -> KillOutcome {
    signal_pid(ppid, libc::SIGCHLD)
}

/// Estado do kernel agora (`Z` = zombie), sem esperar a próxima amostra.
pub fn kernel_state(pid: u32) -> Option<char> {
    live_state(pid).map(|(state, _)| state)
}

/// Estado e pai agora, direto do kernel: `(estado, ppid)`. `None` = o PID já sumiu.
///
/// O PPID da amostra pode estar velho: quando o pai morre, o kernel reparenta o zumbi
/// para o init ou para o subreaper mais próximo, e é esse novo pai que recolhe (ou não).
pub fn live_state(pid: u32) -> Option<(char, u32)> {
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let rest = &text[text.rfind(')')? + 1..];
        let mut it = rest.split_whitespace();
        let state = it.next()?.chars().next()?;
        let ppid = it.next()?.parse().ok()?;
        Some((state, ppid))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

/// Nome curto (`comm`) de um PID agora, mesmo que ele nunca tenha entrado numa amostra.
pub fn comm_of(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
        let name = text.trim();
        (!name.is_empty()).then(|| name.to_string())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

fn signal_pid(pid: u32, sig: i32) -> KillOutcome {
    // POSIX treats 0 and negative PIDs as process groups, not individual tasks.
    if pid == 0 || pid > i32::MAX as u32 {
        return KillOutcome::Invalid;
    }
    let rc = unsafe { libc::kill(pid as i32, sig) };
    if rc == 0 {
        KillOutcome::Signaled
    } else {
        match std::io::Error::last_os_error().raw_os_error() {
            Some(1) => KillOutcome::Denied,
            Some(3) => KillOutcome::AlreadyGone,
            Some(code) => KillOutcome::Failed(format!("errno {code}")),
            None => KillOutcome::Failed("erro desconhecido".into()),
        }
    }
}

pub fn is_admin() -> bool {
    unsafe { libc::geteuid() == 0 }
}

pub fn enable_debug_privilege() {}

#[cfg(not(target_os = "linux"))]
#[allow(dead_code)]
fn _pid_ty(_: Pid) {}

#[cfg(target_os = "linux")]
struct RawProc {
    pid: u32,
    stat: Stat,
    virt: u64,
    rss: u64,
    shared: u64,
    io_bytes: u64,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct Stat {
    pid: u32,
    name: String,
    state: Option<char>,
    ppid: u32,
    session: u32,
    utime: u64,
    stime: u64,
    cutime: u64,
    cstime: u64,
    num_threads: u32,
    starttime: u64,
    vsize: u64,
    rss_pages: u64,
}

#[cfg(target_os = "linux")]
fn parse_stat(text: &str) -> Option<Stat> {
    let start = text.find('(')?;
    let end = text.rfind(')')?;
    if end <= start {
        return None;
    }
    let pid = text[..start].trim().parse().ok()?;
    let name = text[start + 1..end].to_string();
    let rest: Vec<&str> = text[end + 1..].split_whitespace().collect();
    // After comm: state ppid pgrp session tty tpgid flags minflt cminflt majflt cmajflt
    // utime stime cutime cstime priority nice num_threads itrealvalue starttime vsize rss
    if rest.len() < 22 {
        return None;
    }
    Some(Stat {
        pid,
        name,
        state: rest[0].chars().next(),
        ppid: rest[1].parse().ok()?,
        session: rest[3].parse().ok()?,
        utime: rest[11].parse().ok()?,
        stime: rest[12].parse().ok()?,
        cutime: rest[13].parse().ok()?,
        cstime: rest[14].parse().ok()?,
        num_threads: rest[17].parse().ok()?,
        starttime: rest[19].parse().ok()?,
        vsize: rest[20].parse().ok()?,
        rss_pages: rest[21].parse().ok()?,
    })
}

#[cfg(target_os = "linux")]
fn parse_statm(text: &str, page_size: u64) -> Option<(u64, u64, u64)> {
    let mut it = text.split_whitespace();
    let size: u64 = it.next()?.parse().ok()?;
    let resident: u64 = it.next()?.parse().ok()?;
    let shared: u64 = it.next()?.parse().ok()?;
    Some((
        size.saturating_mul(page_size),
        resident.saturating_mul(page_size),
        shared.saturating_mul(page_size),
    ))
}

#[cfg(target_os = "linux")]
fn sysconf_positive(name: libc::c_int) -> u64 {
    let n = unsafe { libc::sysconf(name) };
    if n > 0 {
        n as u64
    } else {
        1
    }
}

#[cfg(target_os = "linux")]
fn boot_time_secs() -> u64 {
    let Ok(text) = std::fs::read_to_string("/proc/stat") else {
        return 0;
    };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("btime ") {
            return rest.trim().parse().unwrap_or(0);
        }
    }
    0
}

#[cfg(target_os = "linux")]
fn read_cmdline(pid: u32) -> String {
    let Ok(bytes) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
        return String::new();
    };
    let parts: Vec<&str> = bytes
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| std::str::from_utf8(p).unwrap_or(""))
        .filter(|s| !s.is_empty())
        .collect();
    parts.join(" ")
}

#[cfg(target_os = "linux")]
fn read_exe(pid: u32) -> String {
    std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn read_environ(pid: u32) -> Vec<String> {
    let Ok(bytes) = std::fs::read(format!("/proc/{pid}/environ")) else {
        return Vec::new();
    };
    bytes
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect()
}

/// Folha do cgroup v2 (`0::/user.slice/.../hermes-gateway.service` → `hermes-gateway.service`).
/// Lida uma vez por (pid, starttime), junto do environ: o processo não muda de unidade.
#[cfg(target_os = "linux")]
fn read_unit(pid: u32) -> Option<String> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    parse_unit(&text)
}

#[cfg(target_os = "linux")]
pub(crate) fn parse_unit(cgroup: &str) -> Option<String> {
    let path = cgroup
        .lines()
        .find_map(|l| l.strip_prefix("0::"))
        .or_else(|| cgroup.lines().next()?.rsplit(':').next())?;
    let leaf = path.trim().trim_end_matches('/').rsplit('/').next()?;
    (leaf.ends_with(".service") || leaf.ends_with(".scope")).then(|| leaf.to_string())
}

#[cfg(target_os = "linux")]
fn read_io_bytes(pid: u32) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/io")).ok()?;
    let mut read: Option<u64> = None;
    let mut write: Option<u64> = None;
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("read_bytes:") {
            read = v.trim().parse().ok();
        } else if let Some(v) = line.strip_prefix("write_bytes:") {
            write = v.trim().parse().ok();
        }
    }
    Some(read.unwrap_or(0).saturating_add(write.unwrap_or(0)))
}

#[cfg(target_os = "linux")]
fn count_fds(pid: u32) -> Option<u32> {
    let n = std::fs::read_dir(format!("/proc/{pid}/fd")).ok()?.count();
    Some(n as u32)
}

#[cfg(target_os = "linux")]
fn cpu_machine_pct(delta_ticks: u64, dt: f64, clk_tck: u64, ncpu: f32) -> f32 {
    if dt <= 0.0 || clk_tck == 0 || ncpu <= 0.0 {
        return 0.0;
    }
    let seconds = delta_ticks as f64 / clk_tck as f64;
    ((seconds / dt / ncpu as f64) * 100.0).clamp(0.0, 100.0) as f32
}

/// Janela sobre a qual dividir o CPU acumulado de um processo visto pela primeira vez.
/// Nasceu dentro da janela: tudo que acumulou aconteceu nela, divide pela janela e o
/// número é comparável ao medidor do topo. Mais velho que a janela: só dá pra falar da
/// média de vida. Idade inválida (relógio andou pra trás) cai na janela.
#[cfg(target_os = "linux")]
fn first_sample_window(window: f64, age: f64) -> f64 {
    if age.is_finite() && age > window {
        age
    } else {
        window
    }
}

#[cfg(target_os = "linux")]
fn read_uptime_secs() -> f64 {
    std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|t| t.split_whitespace().next()?.parse().ok())
        .unwrap_or(0.0)
}

#[cfg(target_os = "linux")]
fn smooth(prev: f32, raw: f32, dt: f64) -> f32 {
    if dt <= 0.0 {
        return prev;
    }
    let a = 1.0 - (-dt / CPU_EMA_TAU).exp();
    (a as f32) * raw + (1.0 - a as f32) * prev
}

#[cfg(target_os = "linux")]
fn kb_field(text: &str, key: &str) -> Option<u64> {
    let mut fields = text
        .lines()
        .find(|line| line.split_whitespace().next() == Some(key))?
        .split_whitespace();
    fields.next()?;
    let value = fields.next()?.parse::<u64>().ok()?;
    if fields.next()? != "kB" {
        return None;
    }
    value.checked_mul(1024)
}

#[cfg(target_os = "linux")]
fn parse_smaps_rollup(text: &str) -> Option<(u64, u64)> {
    let private =
        kb_field(text, "Private_Clean:")?.checked_add(kb_field(text, "Private_Dirty:")?)?;
    Some((private, kb_field(text, "Pss:")?))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn cgroup_leaf_is_the_systemd_unit() {
        assert_eq!(
            parse_unit("0::/user.slice/user-1000.slice/user@1000.service/app.slice/hermes-gateway.service\n").as_deref(),
            Some("hermes-gateway.service")
        );
        assert_eq!(
            parse_unit("0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-agent\\x2dbench.slice/agent-bench@dailywork-campanhas.service").as_deref(),
            Some("agent-bench@dailywork-campanhas.service")
        );
        assert_eq!(parse_unit("0::/\n"), None);
        assert_eq!(parse_unit("0::/user.slice/user-1000.slice/user@1000.service/app.slice\n"), None);
    }

    #[test]
    fn private_and_proportional_memory_are_not_rss() {
        let text = "Rss: 900 kB\nPss: 450 kB\nPrivate_Clean: 100 kB\nPrivate_Dirty: 200 kB\n";
        assert_eq!(parse_smaps_rollup(text), Some((300 * 1024, 450 * 1024)));
        assert_eq!(parse_smaps_rollup("Rss: 900 kB"), None);
        assert_eq!(kb_field("Pss: invalid kB", "Pss:"), None);
        assert_eq!(kb_field("Pss: 450 MB", "Pss:"), None);
    }

    #[test]
    fn parse_stat_keeps_comm_with_spaces_and_parens() {
        let line = "1234 (Web Content) S 1200 1200 1200 0 -1 4194304 0 0 0 0 100 50 0 0 20 0 42 0 999 1099511627776 2048 18446744073709551615 0 0 0 0 0 0 0 0 0 0 0 0 17 0 0 0 0 0 0\n";
        let s = parse_stat(line).unwrap();
        assert_eq!(s.pid, 1234);
        assert_eq!(s.name, "Web Content");
        assert_eq!(s.state, Some('S'));
        assert_eq!(s.ppid, 1200);
        assert_eq!(s.session, 1200);
        assert_eq!(s.utime, 100);
        assert_eq!(s.stime, 50);
        assert_eq!(s.num_threads, 42);
        assert_eq!(s.starttime, 999);
        assert_eq!(s.vsize, 1099511627776);
        assert_eq!(s.rss_pages, 2048);
    }

    #[test]
    fn parse_statm_scales_pages() {
        assert_eq!(
            parse_statm("100 50 10 1 0 20 0\n", 4096),
            Some((409600, 204800, 40960))
        );
        assert_eq!(parse_statm("bad", 4096), None);
    }

    #[test]
    fn first_sample_uses_the_window_unless_the_process_is_older() {
        assert_eq!(first_sample_window(5.0, 1.2), 5.0);
        assert_eq!(first_sample_window(5.0, 5.0), 5.0);
        assert_eq!(first_sample_window(5.0, 40.0), 40.0);
        assert_eq!(first_sample_window(5.0, -0.5), 5.0);
        assert_eq!(first_sample_window(5.0, f64::NAN), 5.0);
        // Nasceu há 2 s dentro de uma janela de 5 s e queimou 2 s de um núcleo em 4:
        // 2 s / 5 s / 4 núcleos = 10% da máquina, o mesmo que o medidor do topo credita.
        assert_eq!(cpu_machine_pct(200, first_sample_window(5.0, 2.0), 100, 4.0), 10.0);
    }

    #[test]
    fn cpu_percent_is_share_of_the_machine() {
        // 100 ticks @ 100 Hz = 1s of CPU in a 1s window on 4 cores → 25%.
        assert_eq!(cpu_machine_pct(100, 1.0, 100, 4.0), 25.0);
        assert_eq!(cpu_machine_pct(400, 1.0, 100, 4.0), 100.0);
        assert_eq!(cpu_machine_pct(0, 1.0, 100, 4.0), 0.0);
        assert_eq!(cpu_machine_pct(100, 0.0, 100, 4.0), 0.0);
    }

    #[test]
    fn sampling_does_not_turn_threads_into_processes() {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let worker = std::thread::spawn(move || {
            let _ = rx.recv();
        });
        let mut sampler = Sampler::new();
        let rows = sampler.sample();
        let own = std::process::id();
        let process = rows.iter().find(|p| p.pid == own).unwrap();
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        let threads: u32 = status
            .lines()
            .find(|s| s.starts_with("Threads:"))
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(process.threads, threads);
        assert!(process.private_ws <= process.working_set || process.working_set == 0);
        tx.send(()).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn global_commit_can_exceed_limit() {
        let mem = MemStatus {
            linux_commit: Some((300, 200)),
            total_commit: 200,
            ..Default::default()
        };
        assert_eq!(mem.used_commit(), 300);
        assert!(mem_status().linux_commit.is_some());
    }

    #[test]
    fn kill_rejects_process_group_ids() {
        assert_eq!(kill(0), KillOutcome::Invalid);
        assert_eq!(kill(u32::MAX), KillOutcome::Invalid);
    }

    #[test]
    fn kill_missing_pid_is_already_gone() {
        assert_eq!(kill(i32::MAX as u32 - 7), KillOutcome::AlreadyGone);
    }

    fn open_stat_fds() -> usize {
        let dir = format!("/proc/{}/fd", std::process::id());
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter(|e| {
                std::fs::read_link(e.path()).ok().is_some_and(|p| {
                    let s = p.to_string_lossy();
                    s.ends_with("/stat") || s.contains("/task/")
                })
            })
            .count()
    }

    #[test]
    fn sampling_does_not_pin_proc_stat_file_descriptors() {
        let before = open_stat_fds();
        let mut sampler = Sampler::new();
        for _ in 0..5 {
            let _ = sampler.sample();
        }
        let after = open_stat_fds();
        assert!(
            after.saturating_sub(before) < 32,
            "kept /proc stat FDs: before={before} after={after}"
        );
    }

    #[test]
    fn self_reports_open_file_descriptors() {
        let mut sampler = Sampler::new();
        let rows = sampler.sample();
        let own = rows.iter().find(|p| p.pid == std::process::id()).unwrap();
        assert!(own.handles.is_some_and(|n| n >= 3), "handles={:?}", own.handles);
    }

    #[test]
    fn thread_tids_are_not_listed_as_processes() {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let worker = std::thread::spawn(move || {
            let _ = rx.recv();
        });
        let mut sampler = Sampler::new();
        let rows = sampler.sample();
        let own = std::process::id();
        let task_dir = format!("/proc/{own}/task");
        for e in std::fs::read_dir(task_dir).unwrap().flatten() {
            let tid: u32 = e.file_name().to_string_lossy().parse().unwrap();
            if tid != own {
                assert!(
                    !rows.iter().any(|p| p.pid == tid),
                    "thread {tid} leaked into process list"
                );
            }
        }
        tx.send(()).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn review_denied_reads_do_not_starve_later_processes() {
        for (limit, interval) in [(FD_PER_SAMPLE, FD_INTERVAL), (SMAPS_PER_SAMPLE, SMAPS_INTERVAL)] {
            let now = Instant::now();
            let candidates: Vec<_> = (1..=limit as u32 + 1)
                .map(|pid| ((pid, 1), 1000 - pid as u64)).collect();
            let target = (limit as u32 + 1, 1);
            let mut cache = HashMap::new();
            for _ in 0..2 {
                refresh_cached_reads(&mut cache, &candidates, now, interval, limit, 0,
                    |pid| (pid == target.0).then_some(3));
            }
            assert_eq!(cached_value(&cache, &target), Some(3));
            assert_eq!(cached_value(&cache, &(1, 1)), None);
        }
    }

    #[test]
    fn review_expiring_cache_rotates_past_busy_processes() {
        let now = Instant::now();
        let candidates = [((1, 1), 300), ((2, 1), 200), ((3, 1), 100)];
        let mut cache = HashMap::new();
        let mut read_pids = Vec::new();
        for i in 0..3 {
            refresh_cached_reads(&mut cache, &candidates, now + Duration::from_secs(i * 6),
                SMAPS_INTERVAL, 1, 1, |pid| { read_pids.push(pid); Some(1) });
        }
        assert_eq!(read_pids, vec![1, 2, 3]);
    }

    #[test]
    fn review_failed_refresh_is_unavailable_and_waits_for_retry() {
        let now = Instant::now();
        let key = (1, 1);
        let candidates = [(key, 1)];
        let mut cache = HashMap::new();
        refresh_cached_reads(&mut cache, &candidates, now, FD_INTERVAL, 1, 1, |_| Some(3));
        assert_eq!(cached_value(&cache, &key), Some(3));
        let expired = now + FD_INTERVAL;
        // Passado o intervalo o valor antigo continua valendo (não pisca pra None)…
        assert_eq!(cached_value(&cache, &key), Some(3));
        let mut retries = 0;
        for _ in 0..2 {
            refresh_cached_reads(&mut cache, &candidates, expired, FD_INTERVAL, 1, 1,
                |_| { retries += 1; None });
        }
        // …até uma releitura falhar de verdade: aí limpa, em vez de mentir dado velho.
        assert_eq!(cached_value(&cache, &key), None);
        assert_eq!(retries, 1);
    }

    #[test]
    fn review_cpu_counts_machine_even_with_thread_affinity() {
        let expected = sysconf_positive(libc::_SC_NPROCESSORS_ONLN) as f32;
        std::thread::spawn(move || {
            unsafe {
                let mut mask: libc::cpu_set_t = std::mem::zeroed();
                assert_eq!(libc::sched_getaffinity(0, std::mem::size_of_val(&mask), &mut mask), 0);
                let first = (0..libc::CPU_SETSIZE as usize)
                    .find(|&n| libc::CPU_ISSET(n, &mask)).unwrap();
                libc::CPU_ZERO(&mut mask);
                libc::CPU_SET(first, &mut mask);
                assert_eq!(libc::sched_setaffinity(0, std::mem::size_of_val(&mask), &mask), 0);
            }
            let sampler = Sampler::new();
            assert_eq!(sampler.ncpu, expected);
            assert_eq!(cpu_machine_pct(100, 1.0, 100, sampler.ncpu), 100.0 / expected);
        }).join().unwrap();
    }

    #[test]
    fn first_sample_stays_cheap() {
        let mut sampler = Sampler::new();
        let t0 = Instant::now();
        let rows = sampler.sample();
        let elapsed = t0.elapsed();
        assert!(!rows.is_empty());
        assert!(
            elapsed < Duration::from_millis(750),
            "sample took {elapsed:?} for {} processes",
            rows.len()
        );
    }
}
