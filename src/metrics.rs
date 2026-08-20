//! Métricas de sistema além de RAM: CPU total, disco, GPU.
//!
//! Windows: PDH + NVML + GetSystemTimes (ver `metrics_win`).
//! macOS/Linux: `sysinfo`. GPU por processo e NVML ficam `None`.

use std::collections::HashMap;

/// Estado da GPU num instante. Campo `None` = o driver não respondeu; a UI mostra "–".
#[derive(Clone, Debug, Default)]
pub struct GpuInfo {
    pub name: String,
    pub util_pct: Option<f32>,
    pub temp_c: Option<u32>,
    pub mem_used: u64,
    pub mem_total: u64,
    pub power_w: Option<f32>,
    pub fan_pct: Option<u32>,
}

/// Uma amostra de tudo que não é RAM.
#[derive(Clone, Debug, Default)]
pub struct SysSample {
    pub cpu_pct: Option<f32>,
    pub disk_pct: Option<f32>,
    pub disk_bps: Option<f64>,
    pub gpu: Option<GpuInfo>,
    pub gpu_by_pid: HashMap<u32, f32>,
}

#[cfg(windows)]
#[path = "metrics_win.rs"]
mod metrics_win;
#[cfg(windows)]
pub use metrics_win::Metrics;

#[cfg(not(windows))]
#[path = "metrics_unix.rs"]
mod metrics_unix;
#[cfg(not(windows))]
pub use metrics_unix::Metrics;
