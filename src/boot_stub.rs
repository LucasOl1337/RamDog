//! Partida (tudo que sobe com o PC) é Windows. No macOS a aba existe e explica.

use crate::procs::ProcInfo;

pub enum BootOut {
    Toast(String, bool),
    Kill(Vec<u32>),
}

pub struct Boot;

impl Boot {
    pub fn new() -> Self {
        Self
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, _procs: &[ProcInfo], _search: &str, _is_admin: bool) -> Vec<BootOut> {
        ui.add_space(16.0);
        ui.label("A visão Partida lista o que o Windows dispara no boot e no logon.");
        ui.add_space(8.0);
        ui.label("No macOS o equivalente são LaunchAgents/LaunchDaemons — ainda não está nesta aba.");
        Vec::new()
    }
}
