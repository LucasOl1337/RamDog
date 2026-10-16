//! Limpeza fora do Linux: a visão existe, mas explica que ainda não chegou.
use crate::config::Locale;
use crate::procs::ProcInfo;

pub enum CleanOut {
    Toast(String, bool),
    Kill(Vec<u32>),
}

#[derive(Default)]
pub struct Clean;

impl Clean {
    pub fn new() -> Self {
        Self
    }
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        _procs: &[ProcInfo],
        _mem: &dyn Fn(&ProcInfo) -> u64,
        _locked: &dyn Fn(&ProcInfo) -> bool,
        locale: Locale,
    ) -> Vec<CleanOut> {
        ui.heading(locale.text("RAM e disco", "Memory & disk"));
        ui.label(locale.text("RAM e disco ainda é só Linux: cache do usuário, lixeira, pacman, journal, coredumps e sobras na RAM.", "Memory & disk is currently Linux-only: user cache, trash, pacman, journals, coredumps, and leftover RAM."));
        Vec::new()
    }
}
