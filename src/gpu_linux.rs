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
            // Pico recente por PID. O pmon é uma foto instantânea: um render em rajada
            // aparece num tick e some no seguinte, e a coluna virava loteria. Segurar o
            // pico com meia-vida de ~5 s dá um número estável pra ler, ordenar e filtrar.
            let mut held: HashMap<u32, (f32, Instant)> = HashMap::new();
            while let Some(latest) = weak.upgrade() {
                let mut sample = collect(&mut drm);
                let now = Instant::now();
                for (pid, v) in sample.by_pid.iter_mut() {
                    let decayed = held
                        .get(pid)
                        .map(|(old, at)| {
                            old * 0.5f32.powf(now.duration_since(*at).as_secs_f32() / 5.0)
                        })
                        .unwrap_or(0.0);
                    let merged = v.max(decayed);
                    held.insert(*pid, (merged, now));
                    *v = if merged < 0.5 { 0.0 } else { merged };
                }
                // Quem saiu da GPU sai do pico junto — contexto encerrado não assombra.
                held.retain(|pid, _| sample.by_pid.contains_key(pid));
                if let Ok(mut target) = latest.lock() {
                    *target = sample;
                }
                drop(latest);
                std::thread::sleep(Duration::from_millis(1500));
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
        // A row full of "-" still means the process HAS a GPU context and was idle at
        // this instant — that is a known 0, not "driver didn't say". Without it the GPU
        // column showed "–" for almost everything and a min-GPU filter hid the world.
        let pct = ["sm", "enc", "dec", "jpg", "ofa"]
            .iter()
            .filter_map(|key| {
                headers
                    .iter()
                    .position(|h| h == key)
                    .and_then(|i| f.get(i))
                    .and_then(|s| number(s))
            })
            .reduce(f32::max)
            .unwrap_or(0.0);
        load.entry(pid)
            .and_modify(|old| *old = old.max(pct))
            .or_insert(pct.min(100.0));
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
    let need_drm_proc = sample
        .cards
        .iter()
        .any(|c| !c.name.to_ascii_lowercase().contains("nvidia"));
    let (load, memory) = if need_drm_proc {
        drm.sample()
    } else {
        (HashMap::new(), HashMap::new())
    };
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
pub fn is_drm_link(link: &str) -> bool {
    let link = link.trim();
    (link.contains("/dri/") || link.starts_with("/dev/dri")) && !link.contains("nvidia")
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
            let fd_dir = process.path().join("fd");
            let Ok(fds) = std::fs::read_dir(&fd_dir) else {
                continue;
            };
            for fd in fds.flatten() {
                let name = fd.file_name();
                let Ok(link) = std::fs::read_link(fd_dir.join(&name)) else {
                    continue;
                };
                if !is_drm_link(&link.to_string_lossy()) {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(process.path().join("fdinfo").join(&name))
                else {
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
    fn process_metrics_follow_headers_and_idle_context_is_zero() {
        let (load, memory) = parse_pmon("# gpu pid type sm mem enc dec jpg ofa fb ccpm command\n0 42 G 10 80 - 20 - - 415 0 app\n0 43 G - - - - - - 5 0 idle\n");
        assert_eq!(load.get(&42), Some(&20.0));
        // Linha toda "-" = contexto na GPU parado neste instante: 0% conhecido,
        // não "sem leitura". PID fora do pmon continua fora do mapa.
        assert_eq!(load.get(&43), Some(&0.0));
        assert!(!load.contains_key(&99));
        assert_eq!(memory[&43], 5 * 1048576);
    }

    #[test]
    fn drm_links_are_dri_not_regular_or_nvidia() {
        assert!(is_drm_link("/dev/dri/renderD128"));
        assert!(is_drm_link("/dev/dri/card1"));
        assert!(!is_drm_link("/dev/nvidia0"));
        assert!(!is_drm_link("/dev/nvidiactl"));
        assert!(!is_drm_link("/dev/null"));
        assert!(!is_drm_link("socket:[123]"));
        assert!(!is_drm_link("/usr/lib/libc.so.6"));
    }
}
