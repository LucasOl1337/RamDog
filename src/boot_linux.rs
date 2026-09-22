use crate::{
    config::Config,
    linux::Job,
    procs::ProcInfo,
    startup_linux::{self, Inventory},
    usage,
};
use serde_json::json;
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

    pub fn snapshot_json(&mut self) -> serde_json::Value {
        let inventory = startup_linux::scan().unwrap_or_default();
        json!({
            "supported": true,
            "warnings": inventory.warnings,
            "entries": inventory.entries.into_iter().map(|entry| json!({
                "id": entry.id,
                "name": entry.name,
                "command": entry.description,
                "kind": entry.kind,
                "enabled": entry.enabled,
                "missing": false,
                "can_toggle": entry.can_toggle,
                "can_remove": false,
                "microsoft": false,
                "origin": entry.kind,
                "running_hint": entry.active,
                "status": entry.state,
            })).collect::<Vec<_>>(),
        })
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
            let locale = cfg.locale;
            self.scan.start(move || startup_linux::scan_for(locale));
        }
        if self.scan.due(30) {
            let locale = cfg.locale;
            self.scan.start(move || startup_linux::scan_for(locale));
        }
        let locale = cfg.locale;
        crate::kit::intro(ui, locale.text("Serviços, temporizadores, sockets e aplicativos de login. Alterar a inicialização não encerra o que já está rodando.", "Services, timers, sockets, and login applications. Changing startup does not stop anything already running."));
        ui.add_space(6.0);
        crate::kit::toolbar(ui, |ui| {
            if ui
                .add(crate::kit::button(locale.text("Atualizar", "Refresh")))
                .clicked()
            {
                self.scan.start(move || startup_linux::scan_for(locale));
            }
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text(locale.text("Buscar", "Search"))
                    .desired_width(200.0),
            );
            ui.label(crate::kit::muted(&format!(
                "{} {}",
                self.scan.value.entries.len(),
                locale.text("entradas", "entries")
            )));
        });
        self.scan.status(ui);
        self.action.status(ui);
        for warning in &self.scan.value.warnings {
            ui.colored_label(egui::Color32::YELLOW, warning);
        }
        crate::kit::toolbar(ui, |ui| {
            ui.label(crate::kit::muted(locale.text("Preset", "Preset")));
            ui.add(
                egui::TextEdit::singleline(&mut self.preset)
                    .hint_text(locale.text("nome", "name"))
                    .desired_width(160.0),
            );
            if ui
                .add(crate::kit::button(
                    locale.text("Salvar estado atual", "Save current state"),
                ))
                .clicked()
                && !self.preset.trim().is_empty()
            {
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
                .selected_text(locale.text("Carregar preset", "Load preset"))
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
            crate::kit::row(ui, |ui| {
                ui.label(format!(
                    "{} {}:",
                    changes.len(),
                    locale.text("alterações no preset", "preset changes")
                ));
                for (e, on) in &changes {
                    ui.label(format!(
                        "{} → {}",
                        e.name,
                        if *on {
                            locale.text("habilitar", "enable")
                        } else {
                            locale.text("desabilitar", "disable")
                        }
                    ));
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !self.action.busy(),
                            crate::kit::primary(locale.text("Aplicar alterações", "Apply changes")),
                        )
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
                    if ui
                        .add(crate::kit::button(locale.text("Cancelar", "Cancel")))
                        .clicked()
                    {
                        self.pending = None;
                    }
                });
            });
        }
        let query = self.search.to_lowercase();
        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("linux-startup")
                    .num_columns(6)
                    .striped(true)
                    .spacing([14.0, 8.0])
                    .min_row_height(26.0)
                    .show(ui, |ui| {
                        for label in [
                            locale.text("Iniciar", "Start"),
                            locale.text("Entrada", "Entry"),
                            locale.text("Origem", "Origin"),
                            locale.text("Estado", "State"),
                            "RAM",
                            locale.text("Ações", "Actions"),
                        ] {
                            ui.label(crate::kit::muted(label));
                        }
                        ui.end_row();
                        for e in &self.scan.value.entries {
                            let kind = e.kind_for(locale);
                            if !format!("{} {} {}", e.name, e.description, kind)
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
                                .on_disabled_hover_text(locale.text(
                                    "Unidade essencial, estática ou gerenciada por dependências",
                                    "Essential, static, or dependency-managed unit",
                                ))
                                .changed()
                            {
                                let e = e.clone();
                                self.action.start(move || startup_linux::toggle(&e, on));
                            }
                            ui.scope(|ui| {
                                ui.set_min_width(240.0);
                                ui.set_max_width(240.0);
                                ui.add(egui::Label::new(&e.name).truncate())
                                    .on_hover_text(&e.description);
                            });
                            ui.label(crate::kit::muted(&kind));
                            if e.active {
                                crate::kit::badge(
                                    ui,
                                    locale.text("em execução", "running"),
                                    egui::Color32::from_rgb(120, 200, 140),
                                );
                            } else {
                                ui.label(crate::kit::muted(&format!(
                                    "{} · {}",
                                    e.state_for(locale),
                                    locale.text("parada", "stopped")
                                )));
                            }
                            ui.label(
                                egui::RichText::new(
                                    e.memory
                                        .map(|n| format!("{:.1} MiB", n as f64 / 1048576.0))
                                        .unwrap_or_else(|| "—".into()),
                                )
                                .monospace()
                                .size(12.0),
                            );
                            if let startup_linux::Source::Unit { user, unit } = &e.source {
                                let action = if e.active { "stop" } else { "start" };
                                if ui
                                    .add_enabled(
                                        !e.protected && !self.action.busy(),
                                        crate::kit::button(if e.active {
                                            locale.text("Parar", "Stop")
                                        } else {
                                            locale.text("Iniciar", "Start")
                                        }),
                                    )
                                    .clicked()
                                {
                                    let (user, unit) = (*user, unit.clone());
                                    self.action.start(move || {
                                        startup_linux::unit_action(user, action, &unit)
                                    });
                                }
                            } else {
                                ui.label(crate::kit::muted(
                                    locale.text("no próximo login", "at next login"),
                                ));
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
