//! Thread de amostragem: coleta processos + memória em intervalo configurável e envia snapshots.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::hwtemp::{HwTemp, HwTempReader};
use crate::icons::{icon_for_exe, RgbaIcon};
use crate::metrics::{Metrics, SysSample};
use crate::procs::{mem_status, MemStatus, ProcInfo, Sampler};

pub struct Snapshot {
    pub procs: Vec<ProcInfo>,
    pub mem: MemStatus,
    /// (caminho do exe em minúsculo, ícone) — só os ainda não enviados.
    pub new_icons: Vec<(String, Option<RgbaIcon>)>,
    pub taken: Instant,
    pub sample_ms: f32,
    /// CPU total, GPU total/temperatura. GPU por processo já vem aplicada em `procs`.
    pub sys: SysSample,
    /// `false` quando os contadores PDH de GPU não abriram — a UI explica em vez de mostrar 0.
    pub gpu_per_proc: bool,
    /// Temperatura de CPU/RAM via helper `hwtemp.exe` — vazio quando não elevado, sem o
    /// helper ao lado do exe, ou placa-mãe sem Super I/O suportado.
    pub hwtemp: HwTemp,
}

pub struct SamplerHandle {
    pub rx: Receiver<Snapshot>,
    pub interval_ms: Arc<AtomicU64>,
    pub paused: Arc<AtomicBool>,
    pub force: Arc<AtomicBool>,
}

pub fn spawn(ctx: egui::Context, interval_ms: u64) -> SamplerHandle {
    let (tx, rx): (Sender<Snapshot>, Receiver<Snapshot>) = channel();
    let interval = Arc::new(AtomicU64::new(interval_ms));
    let paused = Arc::new(AtomicBool::new(false));
    let force = Arc::new(AtomicBool::new(true));
    let h = SamplerHandle {
        rx,
        interval_ms: interval.clone(),
        paused: paused.clone(),
        force: force.clone(),
    };
    thread::Builder::new()
        .name("ramdog-sampler".into())
        .spawn(move || {
            let mut sampler = Sampler::new();
            let mut metrics = Metrics::new();
            let gpu_per_proc = metrics.gpu_per_process_available();
            let hwtemp_reader = HwTempReader::spawn();
            let mut icons_sent: HashSet<String> = HashSet::new();
            let mut last = Instant::now() - Duration::from_secs(3600);
            loop {
                let iv = Duration::from_millis(interval.load(Ordering::Relaxed).max(200));
                let due = last.elapsed() >= iv;
                let forced = force.swap(false, Ordering::Relaxed);
                if (due && !paused.load(Ordering::Relaxed)) || forced {
                    let t0 = Instant::now();
                    let mut procs = sampler.sample();
                    let mem = mem_status();
                    let sys = metrics.sample();
                    for p in procs.iter_mut() {
                        p.gpu_pct = sys.gpu_by_pid.get(&p.pid).copied().unwrap_or(0.0);
                    }
                    let mut new_icons = Vec::new();
                    for p in &procs {
                        if p.exe_path.is_empty() {
                            continue;
                        }
                        let key = p.exe_path.to_lowercase();
                        if icons_sent.insert(key.clone()) {
                            new_icons.push((key, icon_for_exe(&p.exe_path)));
                        }
                    }
                    if icons_sent.len() > 4000 {
                        icons_sent.clear();
                    }
                    let hwtemp = hwtemp_reader.as_ref().map(|r| r.read()).unwrap_or_default();
                    let snap = Snapshot {
                        procs,
                        mem,
                        new_icons,
                        taken: t0,
                        sample_ms: t0.elapsed().as_secs_f32() * 1000.0,
                        sys,
                        gpu_per_proc,
                        hwtemp,
                    };
                    last = Instant::now();
                    if tx.send(snap).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
                thread::sleep(Duration::from_millis(50));
            }
        })
        .expect("spawn sampler thread");
    h
}
