//! Ralos são Windows (Defender, SCM, Appx). No macOS a aba existe e explica.

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

    pub fn ui(&mut self, ui: &mut egui::Ui, _procs: &[ProcInfo], _is_admin: bool) -> Vec<DrainOut> {
        ui.add_space(16.0);
        ui.label("Ralos (Defender, serviços, Appx, inicialização) são específicos do Windows.");
        ui.add_space(8.0);
        ui.label("No macOS o RamDog lista, categoriza e finaliza processos — essa aba não tem equivalente.");
        Vec::new()
    }
}
