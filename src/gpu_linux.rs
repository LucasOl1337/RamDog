//! NVIDIA telemetry is isolated in nvidia-smi; DRM/sysfs covers AMD and Intel.
use crate::metrics::GpuInfo;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default)]
pub struct Sample {
    pub cards: Vec<GpuInfo>,
    pub by_pid: HashMap<u32, f32>,
    pub process_supported: bool,
    pub memory_by_pid: HashMap<u32, u64>,
    pub error: Option<String>,
    pub taken: Option<Instant>,
}

pub struct Reader(Arc<Mutex<Sample>>);
impl Reader {
    pub fn new() -> Self {
        let latest = Arc::new(Mutex::new(Sample::default()));
        let weak = Arc::downgrade(&latest);
        std::thread::spawn(move || {
            let mut drm = DrmCounters::default();
            while let Some(latest) = weak.upgrade() {
                let sample = collect(&mut drm);
                if let Ok(mut target) = latest.lock() {
                    *target = sample;
                }
                drop(latest);
                std::thread::sleep(Duration::from_millis(800));
            }
        });
        Self(latest)
    }
    pub fn sample(&self) -> Sample {
        let s = self.0.lock().map(|s| s.clone()).unwrap_or_default();
        if s.taken
            .is_some_and(|t| t.elapsed() > Duration::from_secs(15))
        {
            Sample {
                error: Some("Leitura da GPU atrasada; aguardando o driver.".into()),
                ..Default::default()
            }
        } else {
            s
        }
    }
}

fn number(text: &str) -> Option<f32> {
    text.trim()
        .parse::<f32>()
        .ok()
        .filter(|n| n.is_finite() && *n >= 0.0)
}

pub fn parse_nvidia_csv(text: &str) -> Vec<GpuInfo> {
    text.lines()
        .filter_map(|line| {
            let f: Vec<_> = line.split(',').map(str::trim).collect();
            if f.len() != 8 {
                return None;
            }
            Some(GpuInfo {
                name: f[0].into(),
                util_pct: number(f[1]).map(|n| n.min(100.0)),
                mem_used: number(f[2])
                    .map(|n| (n as f64 * 1048576.0) as u64)
                    .unwrap_or(0),
                mem_total: number(f[3])
                    .map(|n| (n as f64 * 1048576.0) as u64)
                    .unwrap_or(0),
                temp_c: number(f[4]).map(|n| n as u32),
                power_w: number(f[5]),
                fan_pct: number(f[6]).map(|n| n as u32),
            })
        })
        .collect()
}

pub fn parse_pmon(text: &str) -> (HashMap<u32, f32>, HashMap<u32, u64>) {
    let mut load = HashMap::<u32, f32>::new();
    let mut memory = HashMap::new();
    let mut headers: Vec<&str> = Vec::new();
    for line in text.lines() {
        let f: Vec<_> = line.trim_start_matches('#').split_whitespace().collect();
        if line.trim_start().starts_with('#') {
            if f.first() == Some(&"gpu") && f.get(1) == Some(&"pid") {
                headers = f;
            }
            continue;
        }
        let Some(pid) = f.get(1).and_then(|p| p.parse::<u32>().ok()) else {
            continue;
        };
        // SM/encode/decode busiest engine; memory utilization is bandwidth, not compute.
        let pct = ["sm", "enc", "dec", "jpg", "ofa"]
            .iter()
            .filter_map(|key| {
                headers
                    .iter()
                    .position(|h| h == key)
                    .and_then(|i| f.get(i))
                    .and_then(|s| number(s))
            })
            .reduce(f32::max);
        if let Some(pct) = pct {
            load.entry(pid)
                .and_modify(|old| *old = old.max(pct))
                .or_insert(pct.min(100.0));
        }
        if let Some(mb) = headers
            .iter()
            .position(|h| *h == "fb")
            .and_then(|i| f.get(i))
            .and_then(|s| number(s))
        {
            *memory.entry(pid).or_insert(0) += (mb as f64 * 1048576.0) as u64;
        }
    }
    (load, memory)
}

pub fn collect(drm: &mut DrmCounters) -> Sample {
    let mut sample = Sample::default();
    let has_nvidia = Path::new("/proc/driver/nvidia/gpus").exists();
    if has_nvidia {
        match crate::linux::command("nvidia-smi", &["--query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw,fan.speed,index", "--format=csv,noheader,nounits"]) {
            Ok(text) => sample.cards = parse_nvidia_csv(&text),
            Err(error) => sample.error = Some(error),
        }
        if let Ok(text) = crate::linux::command("nvidia-smi", &["pmon", "-c", "1", "-s", "um"]) {
            sample.process_supported = text
                .lines()
                .any(|line| line.starts_with("#") && line.contains("pid") && line.contains("sm"));
            (sample.by_pid, sample.memory_by_pid) = parse_pmon(&text);
        }
    }
    if let Ok(entries) = std::fs::read_dir("/sys/class/drm") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name
                .strip_prefix("card")
                .is_some_and(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
            {
                continue;
            }
            let device = entry.path().join("device");
            let vendor = std::fs::read_to_string(device.join("vendor")).unwrap_or_default();
            if vendor.trim() == "0x10de" && !sample.cards.is_empty() {
                continue;
            }
            let driver = std::fs::read_link(device.join("driver"))
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_else(|| "DRM".into());
            let mut card = GpuInfo {
                name: format!("{driver} ({name})"),
                util_pct: read_num(&device.join("gpu_busy_percent")),
                mem_used: read_u64(&device.join("mem_info_vram_used")).unwrap_or(0),
                mem_total: read_u64(&device.join("mem_info_vram_total")).unwrap_or(0),
                ..Default::default()
            };
            if let Ok(monitors) = std::fs::read_dir(device.join("hwmon")) {
                for hw in monitors.flatten() {
                    card.temp_c =
                        read_num(&hw.path().join("temp1_input")).map(|n| (n / 1000.0) as u32);
                    card.power_w = read_num(&hw.path().join("power1_average"))
                        .or_else(|| read_num(&hw.path().join("power1_input")))
                        .map(|n| n / 1_000_000.0);
                }
            }
            sample.cards.push(card);
        }
    }
    let (load, memory) = drm.sample();
    sample.process_supported |= !drm.previous.is_empty();
    for (pid, pct) in load {
        sample.by_pid.entry(pid).or_insert(pct);
    }
    for (pid, bytes) in memory {
        sample.memory_by_pid.entry(pid).or_insert(bytes);
    }
    sample.taken = Some(Instant::now());
    sample
}
fn read_num(path: &Path) -> Option<f32> {
    number(&std::fs::read_to_string(path).ok()?)
}
fn read_u64(path: &Path) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[derive(Default)]
pub struct DrmCounters {
    previous: HashMap<String, (u64, Instant)>,
}
impl DrmCounters {
    fn sample(&mut self) -> (HashMap<u32, f32>, HashMap<u32, u64>) {
        let mut seen = HashSet::new();
        let mut live = HashSet::new();
        let mut loads = HashMap::<u32, f32>::new();
        let mut memory = HashMap::new();
        let now = Instant::now();
        let Ok(processes) = std::fs::read_dir("/proc") else {
            return (loads, memory);
        };
        for process in processes.flatten() {
            let Ok(pid) = process.file_name().to_string_lossy().parse::<u32>() else {
                continue;
            };
            let Ok(fds) = std::fs::read_dir(process.path().join("fdinfo")) else {
                continue;
            };
            for fd in fds.flatten() {
                let Ok(text) = std::fs::read_to_string(fd.path()) else {
                    continue;
                };
                let fields: HashMap<_, _> = text
                    .lines()
                    .filter_map(|l| l.split_once(':'))
                    .map(|(k, v)| (k.trim(), v.trim()))
                    .collect();
                let Some(driver) = fields.get("drm-driver") else {
                    continue;
                };
                if *driver == "nvidia" {
                    continue;
                }
                let Some(client) = fields.get("drm-client-id") else {
                    continue;
                };
                let dev = fields.get("drm-pdev").copied().unwrap_or(driver);
                let id = format!("{dev}:{client}");
                if !seen.insert(id.clone()) {
                    continue;
                } // shared FDs counted once globally
                for (key, value) in &fields {
                    if key.starts_with("drm-engine-") && !key.starts_with("drm-engine-capacity-") {
                        let Some(ns) = value
                            .split_whitespace()
                            .next()
                            .and_then(|n| n.parse::<u64>().ok())
                        else {
                            continue;
                        };
                        let engine = key.trim_start_matches("drm-engine-");
                        let capacity = fields
                            .get(format!("drm-engine-capacity-{engine}").as_str())
                            .and_then(|s| s.parse::<f64>().ok())
                            .unwrap_or(1.0)
                            .max(1.0);
                        let k = format!("{id}:{engine}");
                        live.insert(k.clone());
                        if let Some((prev, at)) = self.previous.get(&k) {
                            if ns >= *prev {
                                let pct = ((ns - prev) as f64
                                    / now.duration_since(*at).as_nanos().max(1) as f64
                                    / capacity
                                    * 100.0)
                                    .clamp(0.0, 100.0)
                                    as f32;
                                loads
                                    .entry(pid)
                                    .and_modify(|p| *p = p.max(pct))
                                    .or_insert(pct);
                            } else {
                                continue;
                            }
                        }
                        self.previous.insert(k, (ns, now));
                    }
                }
                if let Some(value) = fields
                    .get("drm-resident-vram")
                    .or_else(|| fields.get("drm-memory-vram"))
                {
                    let f: Vec<_> = value.split_whitespace().collect();
                    if let Some(bytes) = f.first().and_then(|s| s.parse::<u64>().ok()) {
                        let multiplier = match f.get(1).copied() {
                            Some("KiB") => 1024,
                            Some("MiB") => 1048576,
                            _ => 1,
                        };
                        *memory.entry(pid).or_insert(0) += bytes.saturating_mul(multiplier);
                    }
                }
            }
        }
        self.previous.retain(|key, _| live.contains(key));
        (loads, memory)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nvidia_units_and_missing_values() {
        let cards = parse_nvidia_csv(
            "RTX, 40, 1024, 16384, 50, 77.5, 30, 0\nGPU2, N/A, 0, 2048, N/A, N/A, N/A, 1\n",
        );
        assert_eq!(cards[0].mem_used, 1073741824);
        assert_eq!(cards[0].util_pct, Some(40.0));
        assert_eq!(cards[1].temp_c, None);
    }
    #[test]
    fn process_metrics_follow_headers_and_keep_unknown() {
        let (load, memory) = parse_pmon("# gpu pid type sm mem enc dec jpg ofa fb ccpm command\n0 42 G 10 80 - 20 - - 415 0 app\n0 43 G - - - - - - 5 0 idle\n");
        assert_eq!(load.get(&42), Some(&20.0));
        assert!(!load.contains_key(&43));
        assert_eq!(memory[&43], 5 * 1048576);
    }
}
