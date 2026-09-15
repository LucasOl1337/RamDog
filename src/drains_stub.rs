//! Desperdício é Windows (Defender, SCM, Appx). No Linux/macOS a aba existe e explica.

use crate::config::Locale;
use crate::procs::ProcInfo;

pub enum DrainOut {
    Toast(String, bool),
    Kill(Vec<u32>),
}

pub struct Drains;

impl Drains {
    pub fn new() -> Self {
        Self
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, _procs: &[ProcInfo], _is_admin: bool, locale: Locale) -> Vec<DrainOut> {
        ui.add_space(16.0);
        ui.label(locale.text("Desperdício (Defender, serviços, Appx) é específico do Windows.", "Drains (Defender, services, Appx) are Windows-only."));
        ui.add_space(8.0);
        ui.label(locale.text("No Linux e no macOS o RamDog lista, categoriza e finaliza processos — essa aba não tem equivalente.", "On Linux and macOS, RamDog lists, categorizes, and terminates processes — this tab has no equivalent."));
        Vec::new()
    }
}
