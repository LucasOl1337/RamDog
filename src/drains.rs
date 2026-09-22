//! Visão "Desperdício": Defender, serviços dispensáveis e apps de sistema.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use egui::{Color32, RichText};
use serde_json::json;

use crate::app::{fmt_bytes, fmt_bytes_short, LINE, MUTED, SURFACE, SURFACE_HI};
use crate::config::Locale;
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
        self.svc = sys::SERVICES
            .iter()
            .map(|e| sys::query_service(e.name))
            .collect();
        self.protected = sys::PROTECTED_SERVICES
            .iter()
            .map(|(n, _)| sys::query_service(n))
            .collect();
        self.defender = sys::defender_status();
        self.appx = sys::installed_appx_families().into_iter().collect();
        self.last_refresh = Some(Instant::now());
    }

    pub fn snapshot_json(&mut self) -> serde_json::Value {
        self.refresh();
        let state = |value: SvcState| match value {
            SvcState::Running => "running",
            SvcState::Stopped => "stopped",
            SvcState::Pending => "pending",
            SvcState::Missing => "missing",
        };
        let start = |value: SvcStart| match value {
            SvcStart::Auto => "automatic",
            SvcStart::Manual => "manual",
            SvcStart::Disabled => "disabled",
            SvcStart::Unknown => "unknown",
        };
        let services = sys::SERVICES
            .iter()
            .zip(self.svc.iter())
            .map(|(entry, status)| {
                json!({
                    "name": entry.name,
                    "label": entry.label,
                    "why": entry.why,
                    "proc_hint": entry.proc_hint,
                    "stop_only": entry.stop_only,
                    "state": state(status.state),
                    "start": start(status.start),
                })
            })
            .collect::<Vec<_>>();
        let protected_services = sys::PROTECTED_SERVICES
            .iter()
            .zip(self.protected.iter())
            .map(|((name, label), status)| {
                json!({
                    "name": name,
                    "label": label,
                    "state": state(status.state),
                    "start": start(status.start),
                })
            })
            .collect::<Vec<_>>();
        json!({
            "supported": true,
            "services": services,
            "protected_services": protected_services,
            "defender": {
                "realtime_disabled": self.defender.realtime_disabled,
                "tamper_protection": self.defender.tamper_protection,
                "scan_cpu_factor": self.defender.scan_cpu_factor,
            },
            "appx_families": self.appx.iter().collect::<Vec<_>>(),
        })
    }

    fn maybe_refresh(&mut self) {
        let due = self
            .last_refresh
            .map(|t| t.elapsed() > Duration::from_secs(5))
            .unwrap_or(true);
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
        for d in [
            ".claude",
            ".codex",
            ".cargo",
            ".rustup",
            ".grok",
            "AppData\\Roaming\\npm",
            "AppData\\Local\\hermes",
            ".buzz",
        ] {
            let path = format!("{home}\\{d}");
            if std::path::Path::new(&path).is_dir() {
                set.insert(path);
            }
        }
        self.exclusions_text = set.into_iter().collect::<Vec<_>>().join("\n");
    }

    fn run(&mut self, action: Action, is_admin: bool, locale: Locale, out: &mut Vec<DrainOut>) {
        // Ações diretas quando dá (sem UAC); senão PowerShell elevado.
        let elevated = |label: &str, script: String, this: &mut Self| {
            this.busy += 1;
            sys::run_elevated_ps(label.to_string(), script, this.tx.clone());
        };
        match action {
            Action::SvcStop(name) => {
                if is_admin {
                    match sys::stop_service(name) {
                        Ok(()) => out.push(DrainOut::Toast(
                            format!("{name}: {}", locale.text("parado", "stopped")),
                            false,
                        )),
                        Err(e) => out.push(DrainOut::Toast(format!("{name}: {e}"), true)),
                    }
                    self.last_refresh = None;
                } else {
                    elevated(
                        &format!("{name}: {}", locale.text("parar", "stop")),
                        format!("Stop-Service -Name {} -Force", sys::ps_quote(name)),
                        self,
                    );
                }
            }
            Action::SvcDisable(name) => {
                if is_admin {
                    let r = sys::set_start_type(name, SvcStart::Disabled).and_then(|_| {
                        match sys::stop_service(name) {
                            Err(e) if e != "já estava parado" => Err(e),
                            _ => Ok(()),
                        }
                    });
                    match r {
                        Ok(()) => out.push(DrainOut::Toast(
                            format!(
                                "{name}: {}",
                                locale.text(
                                    "desativado (não inicia mais)",
                                    "disabled (will not start again)"
                                )
                            ),
                            false,
                        )),
                        Err(e) => out.push(DrainOut::Toast(format!("{name}: {e}"), true)),
                    }
                    self.last_refresh = None;
                } else {
                    elevated(
                        &format!("{name}: {}", locale.text("desativar", "disable")),
                        format!("Set-Service -Name {0} -StartupType Disabled; Stop-Service -Name {0} -Force -ErrorAction SilentlyContinue", sys::ps_quote(name)),
                        self,
                    );
                }
            }
            Action::SvcEnable(name) => {
                if is_admin {
                    let r = sys::set_start_type(name, SvcStart::Auto)
                        .and_then(|_| sys::start_service(name));
                    match r {
                        Ok(()) => out.push(DrainOut::Toast(
                            format!("{name}: {}", locale.text("reativado", "re-enabled")),
                            false,
                        )),
                        Err(e) => out.push(DrainOut::Toast(format!("{name}: {e}"), true)),
                    }
                    self.last_refresh = None;
                } else {
                    elevated(
                        &format!("{name}: {}", locale.text("reativar", "re-enable")),
                        format!(
                            "Set-Service -Name {0} -StartupType Automatic; Start-Service -Name {0}",
                            sys::ps_quote(name)
                        ),
                        self,
                    );
                }
            }
            Action::DefenderExclude(paths) => {
                let list = paths
                    .iter()
                    .map(|p| sys::ps_quote(p))
                    .collect::<Vec<_>>()
                    .join(",");
                elevated(
                    &format!("Defender: {}", locale.text("exclusões", "exclusions")),
                    format!("Add-MpPreference -ExclusionPath {list}"),
                    self,
                );
            }
            Action::DefenderCpu(f) => {
                elevated(
                    &format!("Defender: {}", locale.text("CPU de varredura", "scan CPU")),
                    format!("Set-MpPreference -ScanAvgCPULoadFactor {f}"),
                    self,
                );
            }
            Action::DefenderRealtime(disable) => {
                elevated(
                    if disable {
                        locale.text("Defender: pausar tempo real", "Defender: pause real-time")
                    } else {
                        locale.text(
                            "Defender: reativar tempo real",
                            "Defender: re-enable real-time",
                        )
                    },
                    format!(
                        "Set-MpPreference -DisableRealtimeMonitoring ${}",
                        if disable { "true" } else { "false" }
                    ),
                    self,
                );
            }
            Action::AppxRemove(pkg) => {
                // Remove-AppxPackage do usuário atual não exige admin, mas rodamos elevado para
                // cobrir pacotes provisionados (-AllUsers) e ter um único caminho de erro.
                elevated(
                    &format!("{} {pkg}", locale.text("desinstalar", "uninstall")),
                    format!(
                        "Get-AppxPackage -Name {0} -AllUsers | Remove-AppxPackage -AllUsers",
                        sys::ps_quote(pkg)
                    ),
                    self,
                );
            }
        }
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        procs: &[ProcInfo],
        is_admin: bool,
        locale: Locale,
    ) -> Vec<DrainOut> {
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
                        let x = svchost_hint
                            .entry(e.name)
                            .or_insert((0u64, 0f32, Vec::new()));
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
                ui.label(RichText::new(locale.text("Desperdício do Windows", "Windows overhead")).strong().size(16.0));
                ui.label(RichText::new(locale.text("— o que consome RAM/CPU sem você pedir, e o que dá para fazer a respeito", "— what uses RAM/CPU without asking, and what you can do about it")).color(muted));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button(locale.text("Atualizar", "Refresh")).clicked() {
                        self.last_refresh = None;
                    }
                    if self.busy > 0 {
                        ui.spinner();
                        ui.label(RichText::new(format!("{} {}", self.busy, locale.text("ação(ões) aguardando UAC/PowerShell", "action(s) waiting for UAC/PowerShell"))).color(muted).small());
                    }
                    if !is_admin {
                        ui.label(RichText::new(locale.text("sem admin: cada ação abre um UAC", "not elevated: each action opens UAC")).color(muted).small());
                    }
                });
            });
            ui.add_space(8.0);

            // ---------- Defender ----------
            let (mp_ram, mp_cpu, _) = by_name.get("msmpeng.exe").cloned().unwrap_or((0, 0.0, Vec::new()));
            section(ui, locale.text("Microsoft Defender", "Microsoft Defender"), &format!("MsMpEng.exe {} · CPU {:.1}%", fmt_bytes(mp_ram), mp_cpu), |ui| {
                ui.label(RichText::new(locale.text("Processo protegido pelo kernel: nem admin consegue finalizá-lo, e o serviço WinDefend não aceita parar. O que funciona é reduzir o trabalho dele:", "Kernel-protected process: even admin cannot terminate it, and WinDefend will not stop. What works is reducing its workload:")).color(muted));
                ui.add_space(4.0);
                let d = self.defender.clone();
                ui.horizontal(|ui| {
                    pill(ui, locale.text("tempo real", "real-time"), match d.realtime_disabled { Some(true) => (locale.text("pausado", "paused"), warn_c), Some(false) => (locale.text("ativo", "active"), ok_c), None => ("?", muted) });
                    pill(ui, locale.text("proteção contra adulteração", "tamper protection"), match d.tamper_protection { Some(true) => (locale.text("ligado", "on"), accent), Some(false) => (locale.text("desligado", "off"), muted), None => ("?", muted) });
                    let default_factor = locale.text("50 (padrão)", "50 (default)");
                    pill(ui, locale.text("CPU varredura agendada", "scheduled scan CPU"), (&format!("{}%", d.scan_cpu_factor.map(|v| v.to_string()).unwrap_or_else(|| default_factor.into())), muted));
                });
                ui.add_space(6.0);

                ui.label(RichText::new(locale.text("1. Excluir pastas de projeto/agentes da varredura em tempo real", "1. Exclude project/agent folders from real-time scanning")).strong());
                ui.label(RichText::new(locale.text("É onde o Defender gasta CPU/RAM: cada arquivo que node/cargo/git tocam é escaneado. Uma pasta por linha; edite à vontade.", "This is where Defender spends CPU/RAM: every file touched by node/cargo/git is scanned. One folder per line; edit freely.")).color(muted).small());
                ui.add(egui::TextEdit::multiline(&mut self.exclusions_text).desired_rows(4).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace));
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(RichText::new(locale.text("Adicionar exclusões", "Add exclusions")).strong())).clicked() {
                        let paths: Vec<String> = self.exclusions_text.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
                        if paths.is_empty() {
                            out.push(DrainOut::Toast(locale.text("Nenhuma pasta informada", "No folder provided").into(), true));
                        } else {
                            confirm = Some(Pending {
                                title: locale.text("Excluir pastas da varredura do Defender", "Exclude folders from Defender scanning").into(),
                                lines: paths.clone(),
                                action: Action::DefenderExclude(paths),
                            });
                        }
                    }
                    ui.label(RichText::new(locale.text("Add-MpPreference -ExclusionPath · arquivos nessas pastas deixam de ser verificados", "Add-MpPreference -ExclusionPath · files in these folders are not scanned")).color(muted).small());
                });
                ui.add_space(6.0);

                ui.label(RichText::new(locale.text("2. Limitar CPU da varredura agendada", "2. Limit scheduled-scan CPU")).strong());
                ui.horizontal(|ui| {
                    for f in [5u32, 10, 20] {
                        if ui.button(format!("{f}%")).clicked() {
                            queued.push(Action::DefenderCpu(f));
                        }
                    }
                    ui.label(RichText::new(locale.text("Set-MpPreference -ScanAvgCPULoadFactor · vale para as varreduras completas/agendadas", "Set-MpPreference -ScanAvgCPULoadFactor · applies to full/scheduled scans")).color(muted).small());
                });
                ui.add_space(6.0);

                ui.label(RichText::new(locale.text("3. Pausar a proteção em tempo real", "3. Pause real-time protection")).strong());
                ui.horizontal(|ui| {
                    let tp_on = d.tamper_protection == Some(true);
                    let paused = d.realtime_disabled == Some(true);
                    if paused {
                        if ui.button(locale.text("Reativar tempo real", "Re-enable real-time")).clicked() {
                            queued.push(Action::DefenderRealtime(false));
                        }
                    } else {
                        let b = ui.add_enabled(!tp_on, egui::Button::new(RichText::new(locale.text("Pausar tempo real", "Pause real-time")).color(warn_c)));
                        if b.clicked() {
                            confirm = Some(Pending {
                                title: locale.text("Pausar proteção em tempo real do Defender", "Pause Defender real-time protection").into(),
                                lines: vec![locale.text("Sem verificação de arquivos/downloads até você reativar (o Windows costuma religar sozinho depois de um tempo ou no reboot).", "Files/downloads will not be scanned until you re-enable it (Windows often turns it back on after a while or on reboot).").into()],
                                action: Action::DefenderRealtime(true),
                            });
                        }
                    }
                    if tp_on {
                        ui.label(RichText::new(locale.text("bloqueado pelo Tamper Protection — desligue-o em Segurança do Windows › Proteção contra vírus › Gerenciar configurações", "Blocked by Tamper Protection — turn it off in Windows Security › Virus & threat protection › Manage settings")).color(muted).small());
                        if ui.small_button(locale.text("Abrir Segurança do Windows", "Open Windows Security")).clicked() {
                            crate::app::open_url("windowsdefender://threatsettings");
                        }
                    }
                });
            });

            // ---------- Serviços ----------
            section(ui, locale.text("Serviços dispensáveis", "Optional services"), locale.text("parar agora ou desativar de vez (não iniciam mais)", "stop now or disable permanently (they will not start again)"), |ui| {
                egui::Grid::new("svc_grid").num_columns(5).spacing([14.0, 6.0]).striped(true).show(ui, |ui| {
                    ui.label(RichText::new(locale.text("Serviço", "Service")).strong());
                    ui.label(RichText::new(locale.text("O que é", "Purpose")).strong());
                    ui.label(RichText::new(locale.text("Estado", "State")).strong());
                    ui.label(RichText::new("RAM").strong());
                    ui.label(RichText::new(locale.text("Ações", "Actions")).strong());
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
                                SvcState::Running => (locale.text("em execução", "running"), accent),
                                SvcState::Stopped => (locale.text("parado", "stopped"), muted),
                                SvcState::Pending => (locale.text("mudando…", "changing…"), muted),
                                SvcState::Missing => ("—", muted),
                            };
                            ui.label(RichText::new(s).color(c));
                            let start = match st.start {
                                SvcStart::Auto => locale.text("início automático", "automatic start"),
                                SvcStart::Manual => locale.text("início manual", "manual start"),
                                SvcStart::Disabled => locale.text("desativado", "disabled"),
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
                            if st.state == SvcState::Running && ui.small_button(locale.text("Parar", "Stop")).on_hover_text(locale.text("Para agora; volta no próximo boot (ou quando algo pedir)", "Stops now; returns on the next boot (or when requested)")).clicked() {
                                queued.push(Action::SvcStop(e.name));
                            }
                            if !e.stop_only {
                                if st.start != SvcStart::Disabled {
                                    if ui.add(egui::Button::new(RichText::new(locale.text("Desativar", "Disable")).color(warn_c)).small()).on_hover_text(locale.text("Para e impede de iniciar de novo", "Stops it and prevents it from starting again")).clicked() {
                                        confirm = Some(Pending {
                                            title: if locale == Locale::Portuguese { format!("Desativar {}", e.label) } else { format!("Disable {}", e.label) },
                                            lines: vec![e.why.to_string(), if locale == Locale::Portuguese { format!("Serviço {} → StartupType Disabled. Reversível aqui mesmo (Reativar).", e.name) } else { format!("Service {} → StartupType Disabled. Reversible here (Re-enable).", e.name) }],
                                            action: Action::SvcDisable(e.name),
                                        });
                                    }
                                } else if ui.small_button(locale.text("Reativar", "Re-enable")).clicked() {
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
                            ui.add(egui::Label::new(RichText::new(locale.text("Protegido pelo Windows — não pode ser parado nem finalizado. Use as ações do Defender acima.", "Protected by Windows — cannot be stopped or terminated. Use the Defender actions above.")).color(muted).small()).wrap());
                        });
                        ui.label(RichText::new(locale.text("em execução", "running")).color(muted));
                        let pn = match *name { "WinDefend" => "msmpeng.exe", "WdNisSvc" => "nissrv.exe", _ => "mpdefendercoreservice.exe" };
                        ui.label(RichText::new(by_name.get(pn).map(|x| fmt_bytes_short(x.0)).unwrap_or_else(|| "–".into())).monospace());
                        ui.label(RichText::new("🔒").color(muted));
                        ui.end_row();
                    }
                });
            });

            // ---------- Apps de sistema ----------
            section(ui, locale.text("Apps de sistema dispensáveis", "Optional system apps"), locale.text("instalados neste usuário — finalizar agora ou desinstalar", "installed for this user — terminate now or uninstall"), |ui| {
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
                        ui.label(RichText::new(if ram > 0 { format!("{} · {} {}", fmt_bytes_short(ram), pids.len(), locale.text("proc.", "proc.")) } else { locale.text("não está rodando", "not running").into() }).color(if ram > 0 { accent } else { muted }).monospace());
                        ui.horizontal(|ui| {
                            if !pids.is_empty() && ui.small_button(locale.text("Finalizar", "Terminate")).on_hover_text(locale.text("Encerra os processos agora (o app pode voltar sozinho)", "Terminates the processes now (the app may restart itself)")).clicked() {
                                out.push(DrainOut::Kill(pids.clone()));
                            }
                            if ui.add(egui::Button::new(RichText::new(locale.text("Desinstalar", "Uninstall")).color(warn_c)).small()).clicked() {
                                confirm = Some(Pending {
                                    title: if locale == Locale::Portuguese { format!("Desinstalar {}", a.label) } else { format!("Uninstall {}", a.label) },
                                    lines: vec![a.why.to_string(), if locale == Locale::Portuguese { format!("Get-AppxPackage {} | Remove-AppxPackage. Dá para reinstalar pela Microsoft Store.", a.pkg_name) } else { format!("Get-AppxPackage {} | Remove-AppxPackage. Reinstall from the Microsoft Store if needed.", a.pkg_name) }],
                                    action: Action::AppxRemove(a.pkg_name),
                                });
                            }
                        });
                        ui.end_row();
                    }
                });
                if !any {
                    ui.label(RichText::new(locale.text("Nenhum dos apps catalogados está instalado.", "None of the catalogued apps is installed.")).color(muted));
                }
            });

            ui.add_space(8.0);
            ui.label(RichText::new(locale.text("O que sobe com o PC (registro, pasta Iniciar, tarefas, serviços) está na visão Partida — não o recorte do Gerenciador de Tarefas.", "What starts with the PC (registry, Startup folder, tasks, services) is in the Startup view — beyond Task Manager's limited list.")).color(muted).small());
            ui.add_space(12.0);
        });

        if let Some(p) = confirm {
            self.pending = Some(p);
        }
        for a in queued {
            self.run(a, is_admin, locale, &mut out);
        }
        // modal de confirmação
        if self.pending.is_some() {
            let mut go = false;
            let mut cancel = false;
            let title = self
                .pending
                .as_ref()
                .map(|p| p.title.clone())
                .unwrap_or_default();
            let lines = self
                .pending
                .as_ref()
                .map(|p| p.lines.clone())
                .unwrap_or_default();
            let modal = egui::Modal::new(egui::Id::new("drain_confirm")).show(ui.ctx(), |ui| {
                ui.set_width(520.0);
                ui.heading(&title);
                ui.add_space(6.0);
                for l in &lines {
                    ui.add(egui::Label::new(RichText::new(l).color(muted)).wrap());
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(locale.text("Confirmar", "Confirm")).strong(),
                            )
                            .fill(Color32::from_rgb(160, 60, 55)),
                        )
                        .clicked()
                    {
                        go = true;
                    }
                    if ui.button(locale.text("Cancelar", "Cancel")).clicked() {
                        cancel = true;
                    }
                    ui.label(
                        RichText::new(locale.text(
                            "Enter confirma · Esc cancela",
                            "Enter confirms · Esc cancels",
                        ))
                        .weak()
                        .small(),
                    );
                });
            });
            let (enter, esc) = ui.ctx().input(|i| {
                (
                    i.key_pressed(egui::Key::Enter),
                    i.key_pressed(egui::Key::Escape),
                )
            });
            if enter {
                go = true;
            }
            if esc || modal.should_close() {
                cancel = true;
            }
            if go {
                if let Some(p) = self.pending.take() {
                    self.run(p.action, is_admin, locale, &mut out);
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
