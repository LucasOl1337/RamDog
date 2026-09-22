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

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        _procs: &[ProcInfo],
        cfg: &mut Config,
    ) -> Vec<ScreenOut> {
        ui.add_space(16.0);
        ui.label(cfg.locale.text(
            "A visão Telas arrasta janelas entre monitores, encaixa em grades e aplica cenários.",
            "The Screens view drags windows between monitors, snaps them to grids, and applies scenes.",
        ));
        ui.add_space(8.0);
        ui.label(
            cfg.locale.text(
                "Ela é escrita direto em Win32. No Linux o equivalente seria X11/_NET_WM ou o protocolo \
                 wlr-foreign-toplevel do Wayland; no macOS, a Accessibility API (permissão explícita). \
                 Ainda não está nesta aba.",
                "This view is implemented directly with Win32. On Linux the equivalent would be X11/_NET_WM or Wayland's \
                 wlr-foreign-toplevel protocol; on macOS, the Accessibility API (explicit permission). \
                 It is not available on this platform yet.",
            ),
        );
        Vec::new()
    }
}
