//! Visão "Desperdício": Defender, serviços dispensáveis e apps de sistema.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use egui::{Color32, RichText};

use crate::app::{fmt_bytes, fmt_bytes_short, LINE, MUTED, SURFACE, SURFACE_HI};
use crate::procs::ProcInfo;
use crate::sys::{self, DefenderStatus, SvcStart, SvcState, SvcStatus, SysResult};

/// Eventos que a visão devolve para o App tratar.
pub enum DrainOut {
    Toast(String, bool),
    Kill(Vec<u32>),
}

enum Action {
    SvcStop(&'static str),
    SvcDisable(&'static str),
    SvcEnable(&'static str),
    DefenderExclude(Vec<String>),
    DefenderCpu(u32),
    DefenderRealtime(bool),
    AppxRemove(&'static str),
}

struct Pending {
    title: String,
    lines: Vec<String>,
    action: Action,
}

pub struct Drains {
    svc: Vec<SvcStatus>,
    protected: Vec<SvcStatus>,
    defender: DefenderStatus,
    appx: HashSet<String>,
    last_refresh: Option<Instant>,
    tx: Sender<SysResult>,
    rx: Receiver<SysResult>,
    busy: usize,
    pending: Option<Pending>,
    exclusions_text: String,
    exclusions_seeded: bool,
}

impl Drains {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        Self {
            svc: Vec::new(),
            protected: Vec::new(),
            defender: DefenderStatus::default(),
            appx: HashSet::new(),
            last_refresh: None,
            tx,
            rx,
            busy: 0,
            pending: None,
            exclusions_text: String::new(),
            exclusions_seeded: false,
        }
    }

    pub fn refresh(&mut self) {
        self.svc = sys::SERVICES.iter().map(|e| sys::query_service(e.name)).collect();
        self.protected = sys::PROTECTED_SERVICES.iter().map(|(n, _)| sys::query_service(n)).collect();
        self.defender = sys::defender_status();
        self.appx = sys::installed_appx_families().into_iter().collect();
        self.last_refresh = Some(Instant::now());
    }

    fn maybe_refresh(&mut self) {
        let due = self.last_refresh.map(|t| t.elapsed() > Duration::from_secs(5)).unwrap_or(true);
        if due {
            self.refresh();
        }
    }

    /// Sugere pastas de projeto / agentes vistas nos processos atuais.
    fn seed_exclusions(&mut self, procs: &[ProcInfo]) {
        if self.exclusions_seeded {
            return;
        }
        self.exclusions_seeded = true;
        let home = std::env::var("USERPROFILE").unwrap_or_default();
        let mut set: BTreeSet<String> = BTreeSet::new();
        for p in procs {
            if let Some(cwd) = &p.launcher.init_cwd {
                if !cwd.is_empty() {
                    set.insert(cwd.clone());
                }
            }
        }
        for d in [".claude", ".codex", ".cargo", ".rustup", ".grok", "AppData\\Roaming\\npm", "AppData\\Local\\hermes", ".buzz"] {
            let path = format!("{home}\\{d}");
            if std::path::Path::new(&path).is_dir() {
                set.insert(path);
            }
        }
        self.exclusions_text = set.into_iter().collect::<Vec<_>>().join("\n");
    }

    fn run(&mut self, action: Action, is_admin: bool, out: &mut Vec<DrainOut>) {
        // Ações diretas quando dá (sem UAC); senão PowerShell elevado.
        let elevated = |label: &str, script: String, this: &mut Self| {
            this.busy += 1;
            sys::run_elevated_ps(label.to_string(), script, this.tx.clone());
        };
        match action {
            Action::SvcStop(name) => {
                if is_admin {
                    match sys::stop_service(name) {
                        Ok(()) => out.push(DrainOut::Toast(format!("{name}: parado"), false)),
                        Err(e) => out.push(DrainOut::Toast(format!("{name}: {e}"), true)),
                    }
                    self.last_refresh = None;
                } else {
                    elevated(&format!("{name}: parar"), format!("Stop-Service -Name {} -Force", sys::ps_quote(name)), self);
                }
            }
            Action::SvcDisable(name) => {
                if is_admin {
                    let r = sys::set_start_type(name, SvcStart::Disabled).and_then(|_| match sys::stop_service(name) {
                        Err(e) if e != "já estava parado" => Err(e),
                        _ => Ok(()),
                    });
                    match r {
                        Ok(()) => out.push(DrainOut::Toast(format!("{name}: desativado (não inicia mais)"), false)),
                        Err(e) => out.push(DrainOut::Toast(format!("{name}: {e}"), true)),
                    }
                    self.last_refresh = None;
                } else {
                    elevated(
                        &format!("{name}: desativar"),
                        format!("Set-Service -Name {0} -StartupType Disabled; Stop-Service -Name {0} -Force -ErrorAction SilentlyContinue", sys::ps_quote(name)),
                        self,
                    );
                }
            }
            Action::SvcEnable(name) => {
                if is_admin {
                    let r = sys::set_start_type(name, SvcStart::Auto).and_then(|_| sys::start_service(name));
                    match r {
                        Ok(()) => out.push(DrainOut::Toast(format!("{name}: reativado"), false)),
                        Err(e) => out.push(DrainOut::Toast(format!("{name}: {e}"), true)),
                    }
                    self.last_refresh = None;
                } else {
                    elevated(
                        &format!("{name}: reativar"),
                        format!("Set-Service -Name {0} -StartupType Automatic; Start-Service -Name {0}", sys::ps_quote(name)),
                        self,
                    );
                }
            }
            Action::DefenderExclude(paths) => {
                let list = paths.iter().map(|p| sys::ps_quote(p)).collect::<Vec<_>>().join(",");
                elevated("Defender: exclusões", format!("Add-MpPreference -ExclusionPath {list}"), self);
            }
            Action::DefenderCpu(f) => {
                elevated("Defender: CPU de varredura", format!("Set-MpPreference -ScanAvgCPULoadFactor {f}"), self);
            }
            Action::DefenderRealtime(disable) => {
                elevated(
                    if disable { "Defender: pausar tempo real" } else { "Defender: reativar tempo real" },
                    format!("Set-MpPreference -DisableRealtimeMonitoring ${}", if disable { "true" } else { "false" }),
                    self,
                );
            }
            Action::AppxRemove(pkg) => {
                // Remove-AppxPackage do usuário atual não exige admin, mas rodamos elevado para
                // cobrir pacotes provisionados (-AllUsers) e ter um único caminho de erro.
                elevated(
                    &format!("desinstalar {pkg}"),
                    format!("Get-AppxPackage -Name {0} -AllUsers | Remove-AppxPackage -AllUsers", sys::ps_quote(pkg)),
                    self,
                );
            }
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, procs: &[ProcInfo], is_admin: bool) -> Vec<DrainOut> {
        let mut out = Vec::new();
        self.maybe_refresh();
        self.seed_exclusions(procs);
        while let Ok(r) = self.rx.try_recv() {
            self.busy = self.busy.saturating_sub(1);
            match r.result {
                Ok(()) => out.push(DrainOut::Toast(format!("{}: ok", r.label), false)),
                Err(e) => out.push(DrainOut::Toast(format!("{}: {e}", r.label), true)),
            }
            self.last_refresh = None;
        }

        // índice por nome de processo → (RAM, CPU, pids)
        let mut by_name: HashMap<String, (u64, f32, Vec<u32>)> = HashMap::new();
        for p in procs {
            let e = by_name.entry(p.name_lower.clone()).or_default();
            e.0 += p.private_ws;
            e.1 += p.cpu_pct;
            e.2.push(p.pid);
        }
        let mut svchost_hint = HashMap::new();
        for p in procs {
            if p.name_lower == "svchost.exe" {
                let cl = p.cmdline.to_lowercase();
                for e in sys::SERVICES {
                    if cl.contains(&format!("-s {}", e.name.to_lowercase())) {
                        let x = svchost_hint.entry(e.name).or_insert((0u64, 0f32, Vec::new()));
                        x.0 += p.private_ws;
                        x.1 += p.cpu_pct;
                        x.2.push(p.pid);
                    }
                }
            }
        }

        let muted = MUTED;
        let accent = Color32::from_rgb(232, 178, 92);
        let ok_c = Color32::from_rgb(120, 200, 140);
        let warn_c = Color32::from_rgb(232, 120, 100);
        let mut queued: Vec<Action> = Vec::new();
        let mut confirm: Option<Pending> = None;

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Desperdício do Windows").strong().size(16.0));
                ui.label(RichText::new("— o que consome RAM/CPU sem você pedir, e o que dá para fazer a respeito").color(muted));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Atualizar").clicked() {
                        self.last_refresh = None;
                    }
                    if self.busy > 0 {
                        ui.spinner();
                        ui.label(RichText::new(format!("{} ação(ões) aguardando UAC/PowerShell", self.busy)).color(muted).small());
                    }
                    if !is_admin {
                        ui.label(RichText::new("sem admin: cada ação abre um UAC").color(muted).small());
                    }
                });
            });
            ui.add_space(8.0);

            // ---------- Defender ----------
            let (mp_ram, mp_cpu, _) = by_name.get("msmpeng.exe").cloned().unwrap_or((0, 0.0, Vec::new()));
            section(ui, "Microsoft Defender", &format!("MsMpEng.exe {} · CPU {:.1}%", fmt_bytes(mp_ram), mp_cpu), |ui| {
                ui.label(RichText::new("Processo protegido pelo kernel: nem admin consegue finalizá-lo, e o serviço WinDefend não aceita parar. O que funciona é reduzir o trabalho dele:").color(muted));
                ui.add_space(4.0);
                let d = self.defender.clone();
                ui.horizontal(|ui| {
                    pill(ui, "tempo real", match d.realtime_disabled { Some(true) => ("pausado", warn_c), Some(false) => ("ativo", ok_c), None => ("?", muted) });
                    pill(ui, "tamper protection", match d.tamper_protection { Some(true) => ("ligado", accent), Some(false) => ("desligado", muted), None => ("?", muted) });
                    pill(ui, "CPU varredura agendada", (&format!("{}%", d.scan_cpu_factor.map(|v| v.to_string()).unwrap_or_else(|| "50 (padrão)".into())), muted));
                });
                ui.add_space(6.0);

                ui.label(RichText::new("1. Excluir pastas de projeto/agentes da varredura em tempo real").strong());
                ui.label(RichText::new("É onde o Defender gasta CPU/RAM: cada arquivo que node/cargo/git tocam é escaneado. Uma pasta por linha; edite à vontade.").color(muted).small());
                ui.add(egui::TextEdit::multiline(&mut self.exclusions_text).desired_rows(4).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace));
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(RichText::new("Adicionar exclusões").strong())).clicked() {
                        let paths: Vec<String> = self.exclusions_text.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
                        if paths.is_empty() {
                            out.push(DrainOut::Toast("Nenhuma pasta informada".into(), true));
                        } else {
                            confirm = Some(Pending {
                                title: "Excluir pastas da varredura do Defender".into(),
                                lines: paths.clone(),
                                action: Action::DefenderExclude(paths),
                            });
                        }
                    }
                    ui.label(RichText::new("Add-MpPreference -ExclusionPath · arquivos nessas pastas deixam de ser verificados").color(muted).small());
                });
                ui.add_space(6.0);

                ui.label(RichText::new("2. Limitar CPU da varredura agendada").strong());
                ui.horizontal(|ui| {
                    for f in [5u32, 10, 20] {
                        if ui.button(format!("{f}%")).clicked() {
                            queued.push(Action::DefenderCpu(f));
                        }
                    }
                    ui.label(RichText::new("Set-MpPreference -ScanAvgCPULoadFactor · vale para as varreduras completas/agendadas").color(muted).small());
                });
                ui.add_space(6.0);

                ui.label(RichText::new("3. Pausar a proteção em tempo real").strong());
                ui.horizontal(|ui| {
                    let tp_on = d.tamper_protection == Some(true);
                    let paused = d.realtime_disabled == Some(true);
                    if paused {
                        if ui.button("Reativar tempo real").clicked() {
                            queued.push(Action::DefenderRealtime(false));
                        }
                    } else {
                        let b = ui.add_enabled(!tp_on, egui::Button::new(RichText::new("Pausar tempo real").color(warn_c)));
                        if b.clicked() {
                            confirm = Some(Pending {
                                title: "Pausar proteção em tempo real do Defender".into(),
                                lines: vec!["Sem verificação de arquivos/downloads até você reativar (o Windows costuma religar sozinho depois de um tempo ou no reboot).".into()],
                                action: Action::DefenderRealtime(true),
                            });
                        }
                    }
                    if tp_on {
                        ui.label(RichText::new("bloqueado pelo Tamper Protection — desligue-o em Segurança do Windows › Proteção contra vírus › Gerenciar configurações").color(muted).small());
                        if ui.small_button("Abrir Segurança do Windows").clicked() {
                            crate::app::open_url("windowsdefender://threatsettings");
                        }
                    }
                });
            });

            // ---------- Serviços ----------
            section(ui, "Serviços dispensáveis", "parar agora ou desativar de vez (não iniciam mais)", |ui| {
                egui::Grid::new("svc_grid").num_columns(5).spacing([14.0, 6.0]).striped(true).show(ui, |ui| {
                    ui.label(RichText::new("Serviço").strong());
                    ui.label(RichText::new("O que é").strong());
                    ui.label(RichText::new("Estado").strong());
                    ui.label(RichText::new("RAM").strong());
                    ui.label(RichText::new("Ações").strong());
                    ui.end_row();
                    for (i, e) in sys::SERVICES.iter().enumerate() {
                        let st = self.svc.get(i).cloned().unwrap_or(SvcStatus { state: SvcState::Missing, start: SvcStart::Unknown });
                        if st.state == SvcState::Missing {
                            continue;
                        }
                        ui.vertical(|ui| {
                            ui.set_width(230.0);
                            ui.label(RichText::new(e.label).strong());
                            ui.label(RichText::new(e.name).monospace().small().color(muted));
                        });
                        ui.vertical(|ui| {
                            ui.set_max_width(440.0);
                            ui.add(egui::Label::new(RichText::new(e.why).color(muted).small()).wrap());
                        });
                        ui.vertical(|ui| {
                            let (s, c) = match st.state {
                                SvcState::Running => ("em execução", accent),
                                SvcState::Stopped => ("parado", muted),
                                SvcState::Pending => ("mudando…", muted),
                                SvcState::Missing => ("—", muted),
                            };
                            ui.label(RichText::new(s).color(c));
                            let start = match st.start {
                                SvcStart::Auto => "início automático",
                                SvcStart::Manual => "início manual",
                                SvcStart::Disabled => "desativado",
                                SvcStart::Unknown => "",
                            };
                            ui.label(RichText::new(start).small().color(if st.start == SvcStart::Disabled { ok_c } else { muted }));
                        });
                        let ram = if st.state != SvcState::Running {
                            0
                        } else if let Some(x) = svchost_hint.get(e.name) {
                            x.0
                        } else if e.proc_hint != "svchost.exe" {
                            by_name.get(e.proc_hint).map(|x| x.0).unwrap_or(0)
                        } else {
                            0
                        };
                        ui.label(RichText::new(if ram > 0 { fmt_bytes_short(ram) } else { "–".into() }).monospace());
                        ui.horizontal(|ui| {
                            if st.state == SvcState::Running && ui.small_button("Parar").on_hover_text("Para agora; volta no próximo boot (ou quando algo pedir)").clicked() {
                                queued.push(Action::SvcStop(e.name));
                            }
                            if !e.stop_only {
                                if st.start != SvcStart::Disabled {
                                    if ui.add(egui::Button::new(RichText::new("Desativar").color(warn_c)).small()).on_hover_text("Para e impede de iniciar de novo").clicked() {
                                        confirm = Some(Pending {
                                            title: format!("Desativar {}", e.label),
                                            lines: vec![e.why.to_string(), format!("Serviço {} → StartupType Disabled. Reversível aqui mesmo (Reativar).", e.name)],
                                            action: Action::SvcDisable(e.name),
                                        });
                                    }
                                } else if ui.small_button("Reativar").clicked() {
                                    queued.push(Action::SvcEnable(e.name));
                                }
                            }
                        });
                        ui.end_row();
                    }
                    for (i, (name, label)) in sys::PROTECTED_SERVICES.iter().enumerate() {
                        let st = self.protected.get(i).cloned();
                        if st.as_ref().map(|s| s.state == SvcState::Missing).unwrap_or(true) {
                            continue;
                        }
                        ui.vertical(|ui| {
                            ui.set_width(230.0);
                            ui.label(RichText::new(*label).color(muted));
                            ui.label(RichText::new(*name).monospace().small().color(muted));
                        });
                        ui.vertical(|ui| {
                            ui.set_max_width(440.0);
                            ui.add(egui::Label::new(RichText::new("Protegido pelo Windows — não pode ser parado nem finalizado. Use as ações do Defender acima.").color(muted).small()).wrap());
                        });
                        ui.label(RichText::new("em execução").color(muted));
                        let pn = match *name { "WinDefend" => "msmpeng.exe", "WdNisSvc" => "nissrv.exe", _ => "mpdefendercoreservice.exe" };
                        ui.label(RichText::new(by_name.get(pn).map(|x| fmt_bytes_short(x.0)).unwrap_or_else(|| "–".into())).monospace());
                        ui.label(RichText::new("🔒").color(muted));
                        ui.end_row();
                    }
                });
            });

            // ---------- Apps de sistema ----------
            section(ui, "Apps de sistema dispensáveis", "instalados neste usuário — finalizar agora ou desinstalar", |ui| {
                let mut any = false;
                egui::Grid::new("appx_grid").num_columns(4).spacing([14.0, 6.0]).striped(true).show(ui, |ui| {
                    for a in sys::APPX {
                        let installed = self.appx.iter().any(|f| f.starts_with(a.family_prefix));
                        if !installed {
                            continue;
                        }
                        any = true;
                        let mut ram = 0u64;
                        let mut pids = Vec::new();
                        for pn in a.procs {
                            if let Some((r, _, ps)) = by_name.get(*pn) {
                                ram += r;
                                pids.extend(ps.iter().copied());
                            }
                        }
                        ui.vertical(|ui| { ui.set_width(230.0); ui.label(RichText::new(a.label).strong()); });
                        ui.vertical(|ui| { ui.set_max_width(440.0); ui.add(egui::Label::new(RichText::new(a.why).color(muted).small()).wrap()); });
                        ui.label(RichText::new(if ram > 0 { format!("{} · {} proc.", fmt_bytes_short(ram), pids.len()) } else { "não está rodando".into() }).color(if ram > 0 { accent } else { muted }).monospace());
                        ui.horizontal(|ui| {
                            if !pids.is_empty() && ui.small_button("Finalizar").on_hover_text("Encerra os processos agora (o app pode voltar sozinho)").clicked() {
                                out.push(DrainOut::Kill(pids.clone()));
                            }
                            if ui.add(egui::Button::new(RichText::new("Desinstalar").color(warn_c)).small()).clicked() {
                                confirm = Some(Pending {
                                    title: format!("Desinstalar {}", a.label),
                                    lines: vec![a.why.to_string(), format!("Get-AppxPackage {} | Remove-AppxPackage. Dá para reinstalar pela Microsoft Store.", a.pkg_name)],
                                    action: Action::AppxRemove(a.pkg_name),
                                });
                            }
                        });
                        ui.end_row();
                    }
                });
                if !any {
                    ui.label(RichText::new("Nenhum dos apps catalogados está instalado.").color(muted));
                }
            });

            ui.add_space(8.0);
            ui.label(RichText::new("O que sobe com o PC (registro, pasta Iniciar, tarefas, serviços) está na visão Partida — não o recorte do Gerenciador de Tarefas.").color(muted).small());
            ui.add_space(12.0);
        });

        if let Some(p) = confirm {
            self.pending = Some(p);
        }
        for a in queued {
            self.run(a, is_admin, &mut out);
        }
        // modal de confirmação
        if self.pending.is_some() {
            let mut go = false;
            let mut cancel = false;
            let title = self.pending.as_ref().map(|p| p.title.clone()).unwrap_or_default();
            let lines = self.pending.as_ref().map(|p| p.lines.clone()).unwrap_or_default();
            let modal = egui::Modal::new(egui::Id::new("drain_confirm")).show(ui.ctx(), |ui| {
                ui.set_width(520.0);
                ui.heading(&title);
                ui.add_space(6.0);
                for l in &lines {
                    ui.add(egui::Label::new(RichText::new(l).color(muted)).wrap());
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(RichText::new("Confirmar").strong()).fill(Color32::from_rgb(160, 60, 55))).clicked() {
                        go = true;
                    }
                    if ui.button("Cancelar").clicked() {
                        cancel = true;
                    }
                    ui.label(RichText::new("Enter confirma · Esc cancela").weak().small());
                });
            });
            let (enter, esc) = ui.ctx().input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
            if enter {
                go = true;
            }
            if esc || modal.should_close() {
                cancel = true;
            }
            if go {
                if let Some(p) = self.pending.take() {
                    self.run(p.action, is_admin, &mut out);
                }
            } else if cancel {
                self.pending = None;
            }
        }
        out
    }
}

fn section(ui: &mut egui::Ui, title: &str, sub: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(egui::Stroke::new(1.0_f32, LINE))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).strong().size(14.5));
                ui.label(RichText::new(sub).color(MUTED).small());
            });
            ui.add_space(6.0);
            add(ui);
        });
    ui.add_space(10.0);
}

fn pill(ui: &mut egui::Ui, label: &str, (value, color): (&str, Color32)) {
    egui::Frame::new()
        .fill(SURFACE_HI)
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(label).small().color(MUTED));
                ui.label(RichText::new(value).small().strong().color(color));
            });
        });
}


