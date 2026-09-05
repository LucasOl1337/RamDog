use crate::{
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
    pub fn ui(&mut self, ui: &mut egui::Ui, _procs: &[ProcInfo], _admin: bool) -> Vec<DrainOut> {
        self.scan.poll();
        if self.action.poll() {
            self.scan.start(startup_linux::scan);
        }
        if self.scan.due(10) {
            self.scan.start(startup_linux::scan);
        }
        ui.heading("Desperdício · serviços em segundo plano");
        ui.label("Revise o consumo e a finalidade antes de parar um serviço. Serviços essenciais estão protegidos.");
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.only_active, "Somente em execução");
            if ui.button("Atualizar").clicked() {
                self.scan.start(startup_linux::scan);
            }
        });
        self.scan.status(ui);
        self.action.status(ui);
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
        egui::ScrollArea::vertical().show(ui, |ui| {
            for e in entries {
                ui.group(|ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(&e.name);
                        ui.label(&e.kind);
                        ui.label(
                            e.memory
                                .map(|m| format!("{:.1} MiB", m as f64 / 1048576.0))
                                .unwrap_or_else(|| "RAM indisponível".into()),
                        );
                        if e.pid > 0 {
                            ui.label(format!("PID {}", e.pid));
                        }
                        if e.protected {
                            ui.label("Protegido");
                        }
                        if let Source::Unit { user, unit } = &e.source {
                            for (label, action, enabled) in [
                                ("Parar", "stop", e.active),
                                ("Reiniciar", "restart", e.active),
                                ("Iniciar", "start", !e.active),
                            ] {
                                if ui
                                    .add_enabled(
                                        enabled && !e.protected && !self.action.busy(),
                                        egui::Button::new(label),
                                    )
                                    .clicked()
                                {
                                    let (user, unit) = (*user, unit.clone());
                                    self.action.start(move || {
                                        startup_linux::unit_action(user, action, &unit)
                                    });
                                }
                            }
                            if ui
                                .add_enabled(
                                    e.can_toggle && !self.action.busy(),
                                    egui::Button::new(if e.enabled {
                                        "Não iniciar no boot"
                                    } else {
                                        "Iniciar no boot"
                                    }),
                                )
                                .clicked()
                            {
                                let e = e.clone();
                                self.action
                                    .start(move || startup_linux::toggle(&e, !e.enabled));
                            }
                        }
                    });
                    ui.label(&e.description);
                });
            }
        });
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs(1));
        Vec::new()
    }
}
