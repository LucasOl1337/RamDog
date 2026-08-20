//! CPU, memória e disco via sysinfo. Sem NVML/PDH no macOS.

use std::collections::HashMap;

use sysinfo::System;

use super::SysSample;

pub struct Metrics {
    sys: System,
}

impl Metrics {
    pub fn new() -> Self {
        let mut sys = System::new();
        sys.refresh_cpu_usage();
        Self { sys }
    }

    pub fn gpu_per_process_available(&self) -> bool {
        false
    }

    pub fn sample(&mut self) -> SysSample {
        self.sys.refresh_cpu_usage();
        SysSample {
            cpu_pct: Some(self.sys.global_cpu_usage().clamp(0.0, 100.0)),
            disk_pct: None,
            disk_bps: None,
            gpu: None,
            gpu_by_pid: HashMap::new(),
        }
    }
}
