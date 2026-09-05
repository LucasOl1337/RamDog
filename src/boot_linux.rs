use crate::{
    config::Config,
    linux::Job,
    procs::ProcInfo,
    startup_linux::{self, Inventory},
    usage,
};
use std::collections::BTreeMap;
pub enum BootOut {
    Toast(String, bool),
    Kill(Vec<u32>),
    SaveCfg,
}
#[derive(Default)]
pub struct Boot {
    scan: Job<Inventory>,
    action: Job<()>,
    search: String,
    preset: String,
    pending: Option<Vec<(startup_linux::Entry, bool)>>,
}
impl Boot {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        _procs: &[ProcInfo],
        _search: &str,
        _admin: bool,
        cfg: &mut Config,
        _tracker: &usage::Tracker,
    ) -> Vec<BootOut> {
        let mut out = Vec::new();
        self.scan.poll();
        if self.action.poll() {
            self.scan.start(startup_linux::scan);
        }
        if self.scan.due(30) {
            self.scan.start(startup_linux::scan);
        }
        ui.heading("Partida · Linux");
        ui.label("Serviços, temporizadores, sockets e aplicativos de login. Alterar a inicialização não encerra o serviço em execução.");
        ui.horizontal(|ui| {
            if ui.button("Atualizar").clicked() {
                self.scan.start(startup_linux::scan);
            }
            ui.label("Buscar");
            ui.text_edit_singleline(&mut self.search);
            ui.label(format!("{} entradas", self.scan.value.entries.len()));
        });
        self.scan.status(ui);
        self.action.status(ui);
        for warning in &self.scan.value.warnings {
            ui.colored_label(egui::Color32::YELLOW, warning);
        }
        ui.horizontal(|ui| {
            ui.label("Preset");
            ui.text_edit_singleline(&mut self.preset);
            if ui.button("Salvar estado atual").clicked() && !self.preset.trim().is_empty() {
                let states: BTreeMap<_, _> = self
                    .scan
                    .value
                    .entries
                    .iter()
                    .filter(|e| e.can_toggle)
                    .map(|e| (e.id.clone(), e.enabled))
                    .collect();
                cfg.boot_presets.insert(self.preset.trim().into(), states);
                out.push(BootOut::SaveCfg);
            }
            egui::ComboBox::from_id_salt("linux-boot-preset")
                .selected_text("Carregar preset")
                .show_ui(ui, |ui| {
                    for (name, states) in &cfg.boot_presets {
                        if ui.button(name).clicked() {
                            self.pending = Some(
                                self.scan
                                    .value
                                    .entries
                                    .iter()
                                    .filter_map(|e| {
                                        states
                                            .get(&e.id)
                                            .filter(|b| **b != e.enabled && e.can_toggle)
                                            .map(|b| (e.clone(), *b))
                                    })
                                    .collect(),
                            );
                        }
                    }
                });
        });
        if let Some(changes) = self.pending.clone() {
            ui.group(|ui| {
                ui.label(format!("{} alterações no preset:", changes.len()));
                for (e, on) in &changes {
                    ui.label(format!(
                        "{} → {}",
                        e.name,
                        if *on { "habilitar" } else { "desabilitar" }
                    ));
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!self.action.busy(), egui::Button::new("Aplicar alterações"))
                        .clicked()
                    {
                        self.action.start(move || {
                            for (e, on) in changes {
                                startup_linux::toggle(&e, on)?;
                            }
                            Ok(())
                        });
                        self.pending = None;
                    }
                    if ui.button("Cancelar").clicked() {
                        self.pending = None;
                    }
                });
            });
        }
        let query = self.search.to_lowercase();
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("linux-startup")
                .num_columns(6)
                .striped(true)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    for label in ["Iniciar", "Entrada", "Origem", "Estado", "RAM", "Ações"] {
                        ui.strong(label);
                    }
                    ui.end_row();
                    for e in &self.scan.value.entries {
                        if !format!("{} {} {}", e.name, e.description, e.kind)
                            .to_lowercase()
                            .contains(&query)
                        {
                            continue;
                        }
                        let mut on = e.enabled;
                        if ui
                            .add_enabled(
                                e.can_toggle && !self.action.busy(),
                                egui::Checkbox::without_text(&mut on),
                            )
                            .on_disabled_hover_text(
                                "Unidade essencial, estática ou gerenciada por dependências",
                            )
                            .changed()
                        {
                            let e = e.clone();
                            self.action.start(move || startup_linux::toggle(&e, on));
                        }
                        ui.label(&e.name).on_hover_text(&e.description);
                        ui.label(&e.kind);
                        ui.label(format!(
                            "{} · {}",
                            e.state,
                            if e.active { "em execução" } else { "parada" }
                        ));
                        ui.label(
                            e.memory
                                .map(|n| format!("{:.1} MiB", n as f64 / 1048576.0))
                                .unwrap_or_else(|| "—".into()),
                        );
                        if let startup_linux::Source::Unit { user, unit } = &e.source {
                            let action = if e.active { "stop" } else { "start" };
                            if ui
                                .add_enabled(
                                    !e.protected && !self.action.busy(),
                                    egui::Button::new(if e.active { "Parar" } else { "Iniciar" }),
                                )
                                .clicked()
                            {
                                let (user, unit) = (*user, unit.clone());
                                self.action
                                    .start(move || startup_linux::unit_action(user, action, &unit));
                            }
                        } else {
                            ui.label("No próximo login");
                        }
                        ui.end_row();
                    }
                });
        });
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs(1));
        out
    }
}
