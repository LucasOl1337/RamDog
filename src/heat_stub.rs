//! Calor fora do Linux: a visão existe, mas explica que ainda não chegou.
use crate::categories::Category;
use crate::config::Locale;
use crate::hwtemp::HwTemp;
use crate::procs::ProcInfo;

pub enum HeatOut {
    Kill(Vec<u32>),
    Toast(String, bool),
}

#[derive(Default)]
pub struct Heat;

impl Heat {
    pub fn new() -> Self {
        Self
    }
    pub fn observe(
        &mut self,
        _procs: &[ProcInfo],
        _locked: &dyn Fn(&ProcInfo) -> bool,
        _cat: &dyn Fn(u32) -> Category,
        _auto: bool,
        _locale: Locale,
    ) -> Vec<HeatOut> {
        Vec::new()
    }
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        locale: Locale,
        _hw: &HwTemp,
        _auto: &mut bool,
    ) -> Vec<HeatOut> {
        ui.label(locale.text(
            "A visão Calor ainda é só Linux: CPU por processo, largados e o modo silencioso.",
            "Heat is currently Linux-only: per-process CPU, left-behind processes, and quiet mode.",
        ));
        Vec::new()
    }
}
