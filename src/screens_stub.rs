//! Organizar monitores e janelas depende do Win32 (EnumWindows, SetWindowPos, DWM).
//! No Linux/macOS a aba existe e explica por quê.

use crate::config::Config;
use crate::procs::ProcInfo;

pub enum ScreenOut {
    Toast(String, bool),
    SaveCfg,
}

pub struct Screens;

impl Screens {
    pub fn new() -> Self {
        Self
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, _procs: &[ProcInfo], _cfg: &mut Config) -> Vec<ScreenOut> {
        ui.add_space(16.0);
        ui.label("A visão Telas arrasta janelas entre monitores, encaixa em grades e aplica cenários.");
        ui.add_space(8.0);
        ui.label(
            "Ela é escrita direto em Win32. No Linux o equivalente seria X11/_NET_WM ou o protocolo \
             wlr-foreign-toplevel do Wayland; no macOS, a Accessibility API (permissão explícita). \
             Ainda não está nesta aba.",
        );
        Vec::new()
    }
}
