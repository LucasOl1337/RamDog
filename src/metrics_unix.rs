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
    gpu: crate::gpu_linux::Reader,
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
            gpu: crate::gpu_linux::Reader::new(),
            #[cfg(target_os = "linux")]
            disk: DiskCounters::new(),
        }
    }

    pub fn gpu_per_process_available(&self) -> bool {
        #[cfg(target_os = "linux")]
        { self.gpu.sample().process_supported }
        #[cfg(not(target_os = "linux"))]
        { false }
    }

    pub fn sample(&mut self) -> SysSample {
        self.sys.refresh_cpu_usage();
        #[cfg(target_os = "linux")]
        let (disk_pct, disk_bps) = self.disk.sample();
        #[cfg(not(target_os = "linux"))]
        let (disk_pct, disk_bps) = (None, None);
        #[cfg(target_os = "linux")]
        let gpu = self.gpu.sample();
        SysSample {
            cpu_pct: Some(self.sys.global_cpu_usage().clamp(0.0, 100.0)),
            disk_pct,
            disk_bps,
            #[cfg(target_os = "linux")]
            gpu: gpu.cards.first().cloned(),
            #[cfg(target_os = "linux")]
            gpu_by_pid: gpu.by_pid.clone(),
            #[cfg(target_os = "linux")]
            gpu_linux: gpu,
            #[cfg(not(target_os = "linux"))]
            gpu: None,
            #[cfg(not(target_os = "linux"))]
            gpu_by_pid: HashMap::new(),
        }
    }
}

/// Throughput sums physical disks; utilization is the busiest individual disk.
/// Keep counters per device so hotplug/reset cannot corrupt the aggregate delta.
#[cfg(target_os = "linux")]
struct DiskCounters {
    previous: Option<HashMap<String, [u64; 3]>>,
    prev_at: Instant,
}

#[cfg(target_os = "linux")]
impl DiskCounters {
    fn new() -> Self {
        Self { previous: read_diskstats(), prev_at: Instant::now() }
    }

    fn sample(&mut self) -> (Option<f32>, Option<f64>) {
        let current = read_diskstats();
        let now = Instant::now();
        let dt = now.duration_since(self.prev_at).as_secs_f64();
        let result = match (&self.previous, &current) {
            (Some(previous), Some(current)) => disk_delta(previous, current, dt),
            _ => (None, None),
        };
        self.previous = current;
        self.prev_at = now;
        result
    }
}

#[cfg(target_os = "linux")]
fn disk_delta(previous: &HashMap<String, [u64; 3]>, current: &HashMap<String, [u64; 3]>, dt: f64) -> (Option<f32>, Option<f64>) {
    if dt <= 0.0 { return (None, None); }
    let mut bytes = 0.0f64;
    let mut busy = 0.0f64;
    let mut measured = false;
    for (name, now) in current {
        let Some(old) = previous.get(name) else { continue };
        if now.iter().zip(old).any(|(n, o)| n < o) { continue; }
        measured = true;
        bytes += (now[0] - old[0]) as f64 + (now[1] - old[1]) as f64;
        busy = busy.max((now[2] - old[2]) as f64 / (dt * 1000.0) * 100.0);
    }
    if measured { (Some(busy.clamp(0.0, 100.0) as f32), Some(bytes / dt)) }
    else { (None, None) }
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
fn read_diskstats() -> Option<HashMap<String, [u64; 3]>> {
    let text = std::fs::read_to_string("/proc/diskstats").ok()?;
    let mut disks = HashMap::new();
    for line in text.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() < 13 || !is_whole_disk(fields[2]) { continue; }
        let (Ok(read), Ok(write), Ok(ticks)) = (fields[5].parse::<u64>(), fields[9].parse::<u64>(), fields[12].parse::<u64>()) else { continue; };
        disks.insert(fields[2].to_owned(), [read.saturating_mul(512), write.saturating_mul(512), ticks]);
    }
    if disks.is_empty() { None } else { Some(disks) }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn concurrent_disks_show_busiest_not_sum() {
        let previous = HashMap::from([("sda".into(), [0, 0, 0]), ("sdb".into(), [0, 0, 0])]);
        let current = HashMap::from([("sda".into(), [1000, 2000, 600]), ("sdb".into(), [3000, 4000, 700])]);
        assert_eq!(disk_delta(&previous, &current, 1.0), (Some(70.0), Some(10000.0)));
    }

    #[test]
    fn missing_new_and_reset_disks_do_not_create_spikes() {
        let previous = HashMap::from([("sda".into(), [100, 100, 100])]);
        let current = HashMap::from([("sdb".into(), [100000, 100000, 100000])]);
        assert_eq!(disk_delta(&previous, &current, 1.0), (None, None));
        let reset = HashMap::from([("sda".into(), [0, 0, 0])]);
        assert_eq!(disk_delta(&previous, &reset, 1.0), (None, None));
        assert_eq!(disk_delta(&previous, &previous, 0.0), (None, None));
        assert_eq!(disk_delta(&previous, &previous, 1.0), (Some(0.0), Some(0.0)));
    }

    #[test]
    fn excludes_partitions_and_virtual_duplicates() {
        for name in ["nvme0n1", "sda", "vda", "mmcblk0"] { assert!(is_whole_disk(name)); }
        for name in ["nvme0n1p1", "sda1", "dm-0", "zram0", "loop0", "mmcblk0p1"] { assert!(!is_whole_disk(name)); }
    }
}
