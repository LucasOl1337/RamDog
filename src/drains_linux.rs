use egui::{Align, Color32, Layout, RichText, Vec2};
use serde_json::json;

use crate::app::MUTED;
use crate::{
    config::Locale,
    linux::Job,
    procs::ProcInfo,
    startup_linux::{self, Inventory, Source},
};
pub enum DrainOut {
    Toast(String, bool),
    Kill(Vec<u32>),
}
#[derive(Default)]
pub struct Drains {
    scan: Job<Inventory>,
    action: Job<()>,
    only_active: bool,
}
impl Drains {
    pub fn new() -> Self {
        Self {
            only_active: true,
            ..Default::default()
        }
    }

    pub fn snapshot_json(&mut self) -> serde_json::Value {
        let inventory = startup_linux::scan().unwrap_or_default();
        let services = inventory
            .entries
            .into_iter()
            .filter_map(|entry| {
                let Source::Unit { .. } = entry.source else {
                    return None;
                };
                Some(json!({
                    "name": entry.name,
                    "label": entry.description,
                    "why": entry.kind,
                    "proc_hint": entry.pid,
                    "stop_only": false,
                    "state": if entry.active { "running" } else { "stopped" },
                    "start": if entry.enabled { "automatic" } else { "disabled" },
                }))
            })
            .collect::<Vec<_>>();
        json!({
            "supported": true,
            "services": services,
            "protected_services": [],
            "defender": serde_json::Value::Null,
            "appx_families": [],
        })
    }
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        _procs: &[ProcInfo],
        _admin: bool,
        locale: Locale,
    ) -> Vec<DrainOut> {
        use crate::kit;
        self.scan.poll();
        if self.action.poll() {
            self.scan.start(move || startup_linux::scan_for(locale));
        }
        if self.scan.due(10) {
            self.scan.start(move || startup_linux::scan_for(locale));
        }
        kit::intro(ui, locale.text("Serviços em segundo plano, do maior pro menor em RAM. Revise o consumo e a finalidade antes de parar um; os essenciais estão protegidos.", "Background services, largest to smallest by RAM. Review their purpose before stopping one; essential services are protected."));
        ui.add_space(6.0);
        kit::toolbar(ui, |ui| {
            ui.checkbox(
                &mut self.only_active,
                RichText::new(locale.text("Somente em execução", "Running only")).size(12.5),
            );
            if ui
                .add(kit::button(locale.text("Atualizar", "Refresh")))
                .clicked()
            {
                self.scan.start(move || startup_linux::scan_for(locale));
            }
            self.scan.status(ui);
            self.action.status(ui);
        });
        for warning in &self.scan.value.warnings {
            ui.colored_label(egui::Color32::YELLOW, warning);
        }
        let mut entries: Vec<_> = self
            .scan
            .value
            .entries
            .iter()
            .filter(|e| matches!(e.source, Source::Unit { .. }) && (!self.only_active || e.active))
            .collect();
        entries.sort_by_key(|e| std::cmp::Reverse(e.memory.unwrap_or(0)));
        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                for e in entries {
                    kit::row(ui, |ui| {
                        ui.horizontal(|ui| {
                            let mut sub = e.kind_for(locale);
                            if e.pid > 0 {
                                sub.push_str(&format!(" · PID {}", e.pid));
                            }
                            if !e.description.is_empty() {
                                sub.push_str(" · ");
                                sub.push_str(&e.description);
                            }
                            ui.scope(|ui| {
                                ui.set_max_width((ui.available_width() - 520.0).max(160.0));
                                kit::two_lines(ui, RichText::new(&e.name), &sub);
                            });
                            // Colunas de largura fixa, da direita pra esquerda: boot, ação,
                            // estado e RAM. Serviço protegido não mostra botão nenhum, em vez
                            // de quatro botões apagados que não fazem nada.
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let busy = self.action.busy();
                                let h = 24.0;
                                ui.allocate_ui_with_layout(
                                    Vec2::new(150.0, h),
                                    Layout::right_to_left(Align::Center),
                                    |ui| {
                                        ui.set_min_width(150.0);
                                        if e.protected {
                                            kit::badge(ui, locale.text("protegido", "protected"), MUTED)
                                                .on_hover_text(locale.text(
                                                    "Essencial para a sessão ou o sistema: o RamDog não para nem desliga.",
                                                    "Essential to the session or system: RamDog will not stop or disable it.",
                                                ));
                                        } else if e.can_toggle {
                                            let label = if e.enabled {
                                                locale.text("Não subir no boot", "Don't start at boot")
                                            } else {
                                                locale.text("Subir no boot", "Start at boot")
                                            };
                                            if ui
                                                .add_enabled_ui(!busy, |ui| {
                                                    ui.add_sized([150.0, h], kit::button(label))
                                                })
                                                .inner
                                                .clicked()
                                            {
                                                let e = e.clone();
                                                self.action.start(move || {
                                                    startup_linux::toggle(&e, !e.enabled)
                                                });
                                            }
                                        }
                                    },
                                );
                                ui.allocate_ui_with_layout(
                                    Vec2::new(170.0, h),
                                    Layout::right_to_left(Align::Center),
                                    |ui| {
                                        ui.set_min_width(170.0);
                                        let Source::Unit { user, unit } = &e.source else {
                                            return;
                                        };
                                        if e.protected {
                                            return;
                                        }
                                        let actions: &[(&str, &str)] = if e.active {
                                            &[
                                                (locale.text("Reiniciar", "Restart"), "restart"),
                                                (locale.text("Parar", "Stop"), "stop"),
                                            ]
                                        } else {
                                            &[(locale.text("Iniciar", "Start"), "start")]
                                        };
                                        for (label, action) in actions {
                                            if ui
                                                .add_enabled_ui(!busy, |ui| {
                                                    ui.add_sized([80.0, h], kit::button(label))
                                                })
                                                .inner
                                                .clicked()
                                            {
                                                let (user, unit, action) =
                                                    (*user, unit.clone(), *action);
                                                self.action.start(move || {
                                                    startup_linux::unit_action(user, action, &unit)
                                                });
                                            }
                                        }
                                    },
                                );
                                ui.allocate_ui_with_layout(
                                    Vec2::new(90.0, h),
                                    Layout::right_to_left(Align::Center),
                                    |ui| {
                                        ui.set_min_width(90.0);
                                        if e.active {
                                            kit::badge(
                                                ui,
                                                locale.text("em execução", "running"),
                                                Color32::from_rgb(120, 200, 140),
                                            );
                                        }
                                    },
                                );
                                ui.allocate_ui_with_layout(
                                    Vec2::new(90.0, h),
                                    Layout::right_to_left(Align::Center),
                                    |ui| {
                                        ui.set_min_width(90.0);
                                        ui.label(
                                            RichText::new(
                                                e.memory
                                                    .map(crate::app::fmt_bytes)
                                                    .unwrap_or_else(|| "—".into()),
                                            )
                                            .monospace()
                                            .size(12.5),
                                        );
                                    },
                                );
                            });
                        });
                    });
                }
            });
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs(1));
        Vec::new()
    }
}
