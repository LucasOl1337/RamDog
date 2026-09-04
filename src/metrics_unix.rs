//! CPU, memória e disco via sysinfo. Disco % no Linux vem de `/proc/diskstats`.
//! Sem NVML/PDH: GPU por processo fica vazia nos dois Unix.

use std::collections::HashMap;
#[cfg(target_os = "linux")]
use std::time::Instant;

use sysinfo::System;

use super::SysSample;

pub struct Metrics {
    sys: System,
    #[cfg(target_os = "linux")]
    disk: DiskCounters,
}

impl Metrics {
    pub fn new() -> Self {
        let mut sys = System::new();
        sys.refresh_cpu_usage();
        Self {
            sys,
            #[cfg(target_os = "linux")]
            disk: DiskCounters::new(),
        }
    }

    pub fn gpu_per_process_available(&self) -> bool {
        false
    }

    pub fn sample(&mut self) -> SysSample {
        self.sys.refresh_cpu_usage();
        #[cfg(target_os = "linux")]
        let (disk_pct, disk_bps) = self.disk.sample();
        #[cfg(not(target_os = "linux"))]
        let (disk_pct, disk_bps) = (None, None);
        SysSample {
            cpu_pct: Some(self.sys.global_cpu_usage().clamp(0.0, 100.0)),
            disk_pct,
            disk_bps,
            gpu: None,
            gpu_by_pid: HashMap::new(),
        }
    }
}

/// Soma dos discos inteiros em `/proc/diskstats` (não partições, não loop/ram/zram).
/// `%` = `io_ticks` / tempo de parede, como o `%util` do iostat — teto 100.
#[cfg(target_os = "linux")]
struct DiskCounters {
    prev_read: u64,
    prev_write: u64,
    prev_ticks: u64,
    prev_at: Instant,
}

#[cfg(target_os = "linux")]
impl DiskCounters {
    fn new() -> Self {
        let (r, w, t) = read_diskstats();
        Self {
            prev_read: r,
            prev_write: w,
            prev_ticks: t,
            prev_at: Instant::now(),
        }
    }

    fn sample(&mut self) -> (Option<f32>, Option<f64>) {
        let (r, w, ticks) = read_diskstats();
        let now = Instant::now();
        let dt = now.duration_since(self.prev_at).as_secs_f64();
        let out = if dt > 0.0 && (self.prev_read != 0 || self.prev_write != 0 || self.prev_ticks != 0) {
            let bytes = r.saturating_sub(self.prev_read).saturating_add(w.saturating_sub(self.prev_write)) as f64;
            let bps = bytes / dt;
            let dt_ms = dt * 1000.0;
            let util = (ticks.saturating_sub(self.prev_ticks) as f64 / dt_ms * 100.0).clamp(0.0, 100.0) as f32;
            (Some(util), Some(bps))
        } else {
            (None, None)
        };
        self.prev_read = r;
        self.prev_write = w;
        self.prev_ticks = ticks;
        self.prev_at = now;
        out
    }
}

#[cfg(target_os = "linux")]
fn is_whole_disk(name: &str) -> bool {
    if name.starts_with("loop")
        || name.starts_with("ram")
        || name.starts_with("zram")
        || name.starts_with("sr")
        || name.starts_with("dm-")
        || name.starts_with("md")
    {
        return false;
    }
    // nvme0n1 sim; nvme0n1p1 não.
    if name.starts_with("nvme") {
        return name.contains('n') && !name.contains('p');
    }
    // mmcblk0 sim; mmcblk0p1 não.
    if name.starts_with("mmcblk") {
        return !name.contains('p');
    }
    // sda, vda, xvda, hda — sem dígito de partição.
    (name.starts_with("sd") || name.starts_with("vd") || name.starts_with("hd") || name.starts_with("xvd"))
        && name.chars().all(|c| c.is_ascii_lowercase())
}

#[cfg(target_os = "linux")]
fn read_diskstats() -> (u64, u64, u64) {
    let Ok(s) = std::fs::read_to_string("/proc/diskstats") else {
        return (0, 0, 0);
    };
    let mut read_bytes = 0u64;
    let mut write_bytes = 0u64;
    let mut io_ticks = 0u64;
    for line in s.lines() {
        let mut it = line.split_whitespace();
        let Some(_) = it.next() else { continue }; // major
        let Some(_) = it.next() else { continue }; // minor
        let Some(name) = it.next() else { continue };
        if !is_whole_disk(name) {
            continue;
        }
        // 0 reads 1 merged 2 sectors_read 3 time_r 4 writes 5 wmerged 6 sectors_written
        // 7 time_w 8 inflight 9 io_ticks
        let mut fields = [0u64; 10];
        for slot in fields.iter_mut() {
            *slot = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
        }
        read_bytes = read_bytes.saturating_add(fields[2].saturating_mul(512));
        write_bytes = write_bytes.saturating_add(fields[6].saturating_mul(512));
        io_ticks = io_ticks.saturating_add(fields[9]);
    }
    (read_bytes, write_bytes, io_ticks)
}
