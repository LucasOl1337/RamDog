//! Partida (tudo que sobe com o PC) é Windows. No Linux/macOS a aba existe e explica.

use crate::config::Config;
use crate::procs::ProcInfo;
use crate::usage;

pub enum BootOut {
    Toast(String, bool),
    Kill(Vec<u32>),
    SaveCfg,
}

pub struct Boot;

impl Boot {
    pub fn new() -> Self {
        Self
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        _procs: &[ProcInfo],
        _search: &str,
        _is_admin: bool,
        _cfg: &mut Config,
        _tracker: &usage::Tracker,
    ) -> Vec<BootOut> {
        ui.add_space(16.0);
        ui.label("A visão Partida lista o que o Windows dispara no boot e no logon.");
        ui.add_space(8.0);
        ui.label(
            "No Linux o equivalente seria systemd (system/user units) e ~/.config/autostart — ainda não está nesta aba. \
             No macOS: LaunchAgents/LaunchDaemons.",
        );
        Vec::new()
    }
}
