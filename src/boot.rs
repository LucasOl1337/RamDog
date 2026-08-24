//! Visão Partida: tudo que o Windows dispara no boot e no logon.
//!
//! O Gerenciador de Tarefas só mostra o que está em `Run` + `StartupApproved` e atalhos
//! `.lnk` da pasta Iniciar. Esta lista vai além: `.vbs`/`.cmd` da pasta, RunOnce, Wow64,
//! tarefas com gatilho de boot/logon, serviços Auto, UWP, Winlogon, Active Setup.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use egui::{Color32, RichText};
use egui_extras::{Column, TableBuilder};
use windows::core::{BSTR, Interface, IUnknown, PCWSTR};

use windows::Win32::Storage::FileSystem::WIN32_FIND_DATAW;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, REG_EXPAND_SZ, REG_SZ,
};
use windows::Win32::System::Services::{
    CloseServiceHandle, EnumServicesStatusExW, OpenSCManagerW, OpenServiceW, QueryServiceConfig2W,
    QueryServiceConfigW, ENUM_SERVICE_STATUS_PROCESSW, QUERY_SERVICE_CONFIGW, SC_ENUM_PROCESS_INFO,
    SC_HANDLE, SC_MANAGER_CONNECT, SC_MANAGER_ENUMERATE_SERVICE, SERVICE_AUTO_START, SERVICE_BOOT_START,
    SERVICE_CONFIG_DELAYED_AUTO_START_INFO, SERVICE_DELAYED_AUTO_START_INFO, SERVICE_DEMAND_START,
    SERVICE_DISABLED, SERVICE_DRIVER, SERVICE_QUERY_CONFIG, SERVICE_QUERY_STATUS, SERVICE_RUNNING,
    SERVICE_STATE_ALL, SERVICE_SYSTEM_START, SERVICE_WIN32,
};
use windows::Win32::System::TaskScheduler::{
    IRegisteredTaskCollection, ITaskFolder, ITaskService, TaskScheduler, TASK_ENUM_HIDDEN,
};
use windows::Win32::System::Variant::{VARIANT, VT_I4};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

use crate::app::{LINE, MUTED, SURFACE};
use crate::procs::{self, ProcInfo};
use crate::sys::{self, SvcStart, SysResult};

pub enum BootOut {
    Toast(String, bool),
    Kill(Vec<u32>),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    Run,
    Folder,
    Task,
    Service,
    Driver,
    Uwp,
    Winlogon,
    Other,
}

impl Kind {
    const ALL: [Kind; 8] = [
        Kind::Run,
        Kind::Folder,
        Kind::Task,
        Kind::Service,
        Kind::Driver,
        Kind::Uwp,
        Kind::Winlogon,
        Kind::Other,
    ];

    fn label(self) -> &'static str {
        match self {
            Kind::Run => "Registro",
            Kind::Folder => "Pasta Iniciar",
            Kind::Task => "Tarefa",
            Kind::Service => "Serviço",
            Kind::Driver => "Driver",
            Kind::Uwp => "App UWP",
            Kind::Winlogon => "Winlogon",
            Kind::Other => "Outro",
        }
    }

    fn chip(self) -> &'static str {
        match self {
            Kind::Run => "Registro",
            Kind::Folder => "Pasta",
            Kind::Task => "Tarefas",
            Kind::Service => "Serviços",
            Kind::Driver => "Drivers",
            Kind::Uwp => "UWP",
            Kind::Winlogon => "Winlogon",
            Kind::Other => "Outros",
        }
    }

    fn color(self) -> Color32 {
        match self {
            Kind::Run => Color32::from_rgb(96, 148, 214),
            Kind::Folder => Color32::from_rgb(120, 200, 140),
            Kind::Task => Color32::from_rgb(200, 160, 255),
            Kind::Service => Color32::from_rgb(232, 178, 92),
            Kind::Driver => Color32::from_rgb(160, 170, 180),
            Kind::Uwp => Color32::from_rgb(90, 190, 210),
            Kind::Winlogon => Color32::from_rgb(232, 120, 100),
            Kind::Other => Color32::from_rgb(180, 150, 110),
        }
    }
}

#[derive(Clone)]
enum Target {
    Run { machine: bool, wow64: bool, name: String },
    Folder { common: bool, file_name: String, path: PathBuf },
    Task { path: String },
    Service { name: String },
    Uwp { key: String },
    ReadOnly,
}

#[derive(Clone)]
struct Entry {
    id: String,
    name: String,
    command: String,
    kind: Kind,
    machine: bool,
    enabled: bool,
    missing: bool,
    can_toggle: bool,
    can_remove: bool,
    microsoft: bool,
    origin: String,
    target: Target,
    /// Só serviços/drivers: já vem do SCM.
    running_hint: bool,
}

enum Action {
    Toggle(Entry, bool),
    Remove(Entry),
}

struct Pending {
    title: String,
    lines: Vec<String>,
    action: Action,
}

pub struct Boot {
    entries: Vec<Entry>,
    last_refresh: Option<Instant>,
    kinds: HashSet<Kind>,
    hide_microsoft: bool,
    only_enabled: bool,
    only_running: bool,
    selected: Option<String>,
    tx: Sender<SysResult>,
    rx: Receiver<SysResult>,
    busy: usize,
    pending: Option<Pending>,
}

impl Boot {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        Self {
            entries: Vec::new(),
            last_refresh: None,
            kinds: Kind::ALL.iter().copied().filter(|k| *k != Kind::Driver).collect(),
            hide_microsoft: false,
            only_enabled: false,
            only_running: false,
            selected: None,
            tx,
            rx,
            busy: 0,
            pending: None,
        }
    }

    fn refresh(&mut self) {
        self.entries = collect();
        self.last_refresh = Some(Instant::now());
    }

    fn maybe_refresh(&mut self) {
        let due = self.last_refresh.map(|t| t.elapsed() > Duration::from_secs(8)).unwrap_or(true);
        if due {
            self.refresh();
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, procs: &[ProcInfo], search: &str, is_admin: bool) -> Vec<BootOut> {
        while let Ok(_r) = self.rx.try_recv() {
            self.busy = self.busy.saturating_sub(1);
            self.last_refresh = None;
        }
        self.maybe_refresh();

        let mut out = Vec::new();
        let mut queued: Vec<Action> = Vec::new();
        let mut confirm: Option<Pending> = None;

        let running = running_exes(procs);
        let q = search.trim().to_lowercase();

        let filtered: Vec<Entry> = self
            .entries
            .iter()
            .filter(|e| self.kinds.contains(&e.kind))
            .filter(|e| !self.hide_microsoft || !e.microsoft)
            .filter(|e| !self.only_enabled || e.enabled)
            .filter(|e| {
                if !self.only_running {
                    return true;
                }
                e.running_hint || exe_running(&e.command, &running)
            })
            .filter(|e| {
                if q.is_empty() {
                    return true;
                }
                e.name.to_lowercase().contains(&q)
                    || e.command.to_lowercase().contains(&q)
                    || e.origin.to_lowercase().contains(&q)
                    || e.kind.label().to_lowercase().contains(&q)
            })
            .cloned()
            .collect();

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Partida").strong().size(16.0));
            ui.label(
                RichText::new("— tudo que o Windows dispara no boot e no logon, sem o recorte do Gerenciador de Tarefas")
                    .color(MUTED),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Atualizar").clicked() {
                    self.last_refresh = None;
                }
                if self.busy > 0 {
                    ui.spinner();
                    ui.label(RichText::new(format!("{} ação(ões) aguardando UAC", self.busy)).color(MUTED).small());
                }
            });
        });
        ui.label(
            RichText::new(format!(
                "{} entradas · {} ativas · {} visíveis agora",
                self.entries.len(),
                self.entries.iter().filter(|e| e.enabled).count(),
                filtered.len()
            ))
            .small()
            .color(MUTED),
        );
        ui.add_space(6.0);

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for k in Kind::ALL {
                let n = self.entries.iter().filter(|e| e.kind == k).count();
                let on = self.kinds.contains(&k);
                let label = format!("{} {n}", k.chip());
                let text = RichText::new(label).size(12.0).color(if on { k.color() } else { MUTED });
                let btn = if on {
                    egui::Button::new(text).stroke(egui::Stroke::new(1.0_f32, k.color().gamma_multiply(0.55)))
                } else {
                    egui::Button::new(text).fill(Color32::TRANSPARENT)
                };
                if ui.add(btn).on_hover_text(k.label()).clicked() {
                    if on && self.kinds.len() > 1 {
                        self.kinds.remove(&k);
                    } else {
                        self.kinds.insert(k);
                    }
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.hide_microsoft, "esconder Microsoft");
            ui.checkbox(&mut self.only_enabled, "só ativas");
            ui.checkbox(&mut self.only_running, "só as que estão rodando");
        });
        ui.add_space(4.0);

        let mut toggle: Option<(Entry, bool)> = None;
        let mut remove: Option<Entry> = None;
        let mut kill: Option<Vec<u32>> = None;
        let mut select: Option<String> = None;

        let row_h = 26.0;
        let n = filtered.len();
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(28.0))
            .column(Column::initial(220.0).at_least(120.0).clip(true))
            .column(Column::initial(110.0).at_least(80.0).clip(true))
            .column(Column::initial(88.0).at_least(70.0))
            .column(Column::remainder().at_least(160.0).clip(true))
            .column(Column::initial(88.0).at_least(70.0))
            .column(Column::initial(132.0).at_least(90.0))
            .header(22.0, |mut h| {
                h.col(|_| {});
                h.col(|ui| { ui.label(RichText::new("Nome").small().color(MUTED).strong()); });
                h.col(|ui| { ui.label(RichText::new("Origem").small().color(MUTED).strong()); });
                h.col(|ui| { ui.label(RichText::new("Escopo").small().color(MUTED).strong()); });
                h.col(|ui| { ui.label(RichText::new("Comando").small().color(MUTED).strong()); });
                h.col(|ui| { ui.label(RichText::new("Estado").small().color(MUTED).strong()); });
                h.col(|ui| { ui.label(RichText::new("Ações").small().color(MUTED).strong()); });
            })
            .body(|body| {
                body.rows(row_h, n, |mut row| {
                    let i = row.index();
                    let e = &filtered[i];
                    let is_run = e.running_hint || exe_running(&e.command, &running);
                    let selected = self.selected.as_deref() == Some(e.id.as_str());
                    row.set_selected(selected);

                    row.col(|ui| {
                        if e.can_toggle {
                            let mut on = e.enabled;
                            if ui.checkbox(&mut on, "").changed() {
                                toggle = Some((e.clone(), on));
                            }
                        } else {
                            let mut dummy = e.enabled;
                            ui.add_enabled(false, egui::Checkbox::new(&mut dummy, ""));
                        }
                    });
                    row.col(|ui| {
                        let name_c = if e.missing {
                            MUTED
                        } else if e.enabled {
                            Color32::from_gray(225)
                        } else {
                            MUTED
                        };
                        ui.label(RichText::new(&e.name).strong().color(name_c));
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(e.kind.label()).small().color(e.kind.color()));
                    });
                    row.col(|ui| {
                        ui.label(
                            RichText::new(if e.machine { "máquina" } else { "usuário" })
                                .small()
                                .color(MUTED),
                        );
                    });
                    row.col(|ui| {
                        ui.add(egui::Label::new(RichText::new(&e.command).monospace().small().color(MUTED)).truncate())
                            .on_hover_text(&e.command);
                    });
                    row.col(|ui| {
                        let (txt, c) = if e.missing {
                            ("ausente", Color32::from_rgb(232, 120, 100))
                        } else if is_run {
                            ("rodando", Color32::from_rgb(120, 200, 140))
                        } else if e.enabled {
                            ("no boot", Color32::from_rgb(232, 178, 92))
                        } else {
                            ("desligada", MUTED)
                        };
                        ui.label(RichText::new(txt).small().color(c));
                    });
                    row.col(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        if is_run {
                            if let Some(name) = exe_name_from_cmd(&e.command) {
                                let pids: Vec<u32> = procs.iter().filter(|p| p.name_lower == name).map(|p| p.pid).collect();
                                if !pids.is_empty() && ui.small_button("Finalizar").clicked() {
                                    kill = Some(pids);
                                }
                            }
                        }
                        if e.can_remove && ui.add(egui::Button::new(RichText::new("Remover").color(Color32::from_rgb(232, 120, 100))).small()).clicked() {
                            remove = Some(e.clone());
                        }
                    });

                    if row.response().clicked() {
                        select = Some(e.id.clone());
                    }
                });
            });

        if let Some(id) = select {
            self.selected = Some(id);
        }
        if let Some((e, on)) = toggle {
            queued.push(Action::Toggle(e, on));
        }
        if let Some(e) = remove {
            confirm = Some(Pending {
                title: format!("Remover {} da partida?", e.name),
                lines: vec![e.origin.clone(), e.command.clone(), "O programa continua instalado. Só deixa de subir com o PC.".into()],
                action: Action::Remove(e),
            });
        }
        if let Some(pids) = kill {
            out.push(BootOut::Kill(pids));
        }

        if let Some(p) = confirm {
            self.pending = Some(p);
        }
        for a in queued {
            self.run(a, is_admin, &mut out);
        }

        if self.pending.is_some() {
            let mut go = false;
            let mut cancel = false;
            let title = self.pending.as_ref().map(|p| p.title.clone()).unwrap_or_default();
            let lines = self.pending.as_ref().map(|p| p.lines.clone()).unwrap_or_default();
            egui::Modal::new(egui::Id::new("boot_confirm")).show(ui.ctx(), |ui| {
                ui.set_width(520.0);
                ui.heading(&title);
                ui.add_space(6.0);
                for l in &lines {
                    ui.add(egui::Label::new(RichText::new(l).color(MUTED)).wrap());
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(RichText::new("Confirmar").strong()).fill(Color32::from_rgb(160, 60, 55))).clicked() {
                        go = true;
                    }
                    if ui.button("Cancelar").clicked() {
                        cancel = true;
                    }
                });
            });
            if cancel {
                self.pending = None;
            }
            if go {
                if let Some(p) = self.pending.take() {
                    self.run(p.action, is_admin, &mut out);
                }
            }
        }

        if let Some(id) = &self.selected {
            if let Some(e) = self.entries.iter().find(|x| x.id == *id) {
                ui.add_space(6.0);
                egui::Frame::new()
                    .fill(SURFACE)
                    .stroke(egui::Stroke::new(1.0_f32, LINE))
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(&e.name).strong());
                            ui.label(RichText::new(e.kind.label()).color(e.kind.color()).small());
                            ui.label(RichText::new(&e.origin).weak().small());
                        });
                        ui.add(egui::Label::new(RichText::new(&e.command).monospace().small()).wrap());
                    });
            }
        }

        out
    }

    fn run(&mut self, action: Action, is_admin: bool, out: &mut Vec<BootOut>) {
        match action {
            Action::Toggle(e, enabled) => {
                let label = format!("{}: {}", e.name, if enabled { "ativar" } else { "desativar" });
                match apply_toggle(&e, enabled) {
                    Ok(()) => {
                        out.push(BootOut::Toast(format!("{}: {}", e.name, if enabled { "ativa na partida" } else { "fora da partida" }), false));
                        self.last_refresh = None;
                    }
                    Err(err) if e.machine && !is_admin && err.contains("acesso negado") => {
                        self.busy += 1;
                        sys::run_elevated_ps(label, toggle_ps(&e, enabled), self.tx.clone());
                    }
                    Err(err) => out.push(BootOut::Toast(format!("{}: {err}", e.name), true)),
                }
            }
            Action::Remove(e) => {
                let label = format!("{}: remover", e.name);
                match apply_remove(&e) {
                    Ok(()) => {
                        out.push(BootOut::Toast(format!("{}: removido da partida", e.name), false));
                        self.last_refresh = None;
                    }
                    Err(err) if e.machine && !is_admin && err.contains("acesso negado") => {
                        self.busy += 1;
                        sys::run_elevated_ps(label, remove_ps(&e), self.tx.clone());
                    }
                    Err(err) => out.push(BootOut::Toast(format!("{}: {err}", e.name), true)),
                }
            }
        }
    }
}

// ---------- coleta ----------

fn collect() -> Vec<Entry> {
    ensure_com();
    let mut out = Vec::new();
    collect_run(&mut out);
    collect_folder(&mut out);
    collect_tasks(&mut out);
    collect_services(&mut out);
    collect_uwp(&mut out);
    collect_winlogon(&mut out);
    collect_other(&mut out);
    out.sort_by(|a, b| {
        let ka = kind_ord(a.kind);
        let kb = kind_ord(b.kind);
        ka.cmp(&kb)
            .then_with(|| a.missing.cmp(&b.missing))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    out
}

fn kind_ord(k: Kind) -> u8 {
    match k {
        Kind::Folder => 0,
        Kind::Run => 1,
        Kind::Uwp => 2,
        Kind::Task => 3,
        Kind::Service => 4,
        Kind::Winlogon => 5,
        Kind::Other => 6,
        Kind::Driver => 7,
    }
}

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_ONCE: &str = r"Software\Microsoft\Windows\CurrentVersion\RunOnce";
const RUN_WOW: &str = r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Run";
const RUN_ONCE_WOW: &str = r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\RunOnce";
const APPROVED_RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
const APPROVED_RUN32: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run32";
const APPROVED_FOLDER: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\StartupFolder";
const POLICIES_RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Policies\Explorer\Run";

fn collect_run(out: &mut Vec<Entry>) {
    let sets = [
        (HKEY_CURRENT_USER, RUN, APPROVED_RUN, false, false, "HKCU Run"),
        (HKEY_LOCAL_MACHINE, RUN, APPROVED_RUN, true, false, "HKLM Run"),
        (HKEY_LOCAL_MACHINE, RUN_WOW, APPROVED_RUN32, true, true, "HKLM Run (32-bit)"),
        (HKEY_CURRENT_USER, RUN_ONCE, APPROVED_RUN, false, false, "HKCU RunOnce"),
        (HKEY_LOCAL_MACHINE, RUN_ONCE, APPROVED_RUN, true, false, "HKLM RunOnce"),
        (HKEY_LOCAL_MACHINE, RUN_ONCE_WOW, APPROVED_RUN32, true, true, "HKLM RunOnce (32-bit)"),
        (HKEY_CURRENT_USER, POLICIES_RUN, APPROVED_RUN, false, false, "HKCU Policy Run"),
        (HKEY_LOCAL_MACHINE, POLICIES_RUN, APPROVED_RUN, true, false, "HKLM Policy Run"),
    ];
    for (root, run_path, approved_path, machine, wow64, origin) in sets {
        let once = run_path.contains("RunOnce");
        let approved = approved_map(root, approved_path);
        let Some(k) = sys::reg_open(root, run_path, false) else { continue };
        let mut seen = HashSet::new();
        for (name, ty, data) in sys::reg_values(&k) {
            if ty != REG_SZ && ty != REG_EXPAND_SZ {
                continue;
            }
            seen.insert(name.to_lowercase());
            let command = sys::utf16_bytes_to_string(&data);
            let enabled = approved.get(&name.to_lowercase()).map(|a| a.1).unwrap_or(true);
            out.push(Entry {
                id: format!("run:{}:{}:{name}", if machine { "hklm" } else { "hkcu" }, if wow64 { "32" } else { "64" }),
                name: name.clone(),
                command,
                kind: Kind::Run,
                machine,
                enabled,
                missing: false,
                can_toggle: !once,
                can_remove: true,
                microsoft: is_microsoft_cmd(&name, ""),
                origin: origin.to_string(),
                target: Target::Run { machine, wow64, name },
                running_hint: false,
            });
        }
        if once || run_path.contains("Policies") {
            continue;
        }
        for (key, (orig, enabled)) in &approved {
            if seen.contains(key) {
                continue;
            }
            out.push(Entry {
                id: format!("run-orphan:{}:{}:{key}", if machine { "hklm" } else { "hkcu" }, if wow64 { "32" } else { "64" }),
                name: orig.clone(),
                command: String::new(),
                kind: Kind::Run,
                machine,
                enabled: *enabled,
                missing: true,
                can_toggle: false,
                can_remove: true,
                microsoft: false,
                origin: format!("{origin} (órfã)"),
                target: Target::Run { machine, wow64, name: orig.clone() },
                running_hint: false,
            });
        }
    }
}

fn collect_folder(out: &mut Vec<Entry>) {
    let user = std::env::var_os("APPDATA").map(PathBuf::from).map(|p| p.join(r"Microsoft\Windows\Start Menu\Programs\Startup"));
    let common = std::env::var_os("ProgramData").map(PathBuf::from).map(|p| p.join(r"Microsoft\Windows\Start Menu\Programs\StartUp"));
    if let Some(p) = user {
        collect_one_folder(out, &p, false);
    }
    if let Some(p) = common {
        collect_one_folder(out, &p, true);
    }
}

fn collect_one_folder(out: &mut Vec<Entry>, dir: &Path, common: bool) {
    let root = if common { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER };
    let approved = approved_map(root, APPROVED_FOLDER);
    let mut seen = HashSet::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for ent in rd.flatten() {
            let path = ent.path();
            let file_name = ent.file_name().to_string_lossy().to_string();
            if file_name.eq_ignore_ascii_case("desktop.ini") {
                continue;
            }
            seen.insert(file_name.to_lowercase());
            let enabled = approved.get(&file_name.to_lowercase()).map(|a| a.1).unwrap_or(true);
            let command = resolve_startup_file(&path);
            out.push(Entry {
                id: format!("folder:{}:{file_name}", if common { "common" } else { "user" }),
                name: file_name.clone(),
                command,
                kind: Kind::Folder,
                machine: common,
                enabled,
                missing: false,
                can_toggle: true,
                can_remove: true,
                microsoft: false,
                origin: if common { "pasta Iniciar (todos)" } else { "pasta Iniciar (usuário)" }.into(),
                target: Target::Folder { common, file_name, path },
                running_hint: false,
            });
        }
    }
    for (key, (orig, enabled)) in &approved {
        if seen.contains(key) {
            continue;
        }
        out.push(Entry {
            id: format!("folder-orphan:{}:{key}", if common { "common" } else { "user" }),
            name: orig.clone(),
            command: String::new(),
            kind: Kind::Folder,
            machine: common,
            enabled: *enabled,
            missing: true,
            can_toggle: true,
            can_remove: true,
            microsoft: false,
            origin: if common { "pasta Iniciar (órfã, todos)" } else { "pasta Iniciar (órfã)" }.into(),
            target: Target::Folder { common, file_name: orig.clone(), path: dir.join(orig) },
            running_hint: false,
        });
    }
}

fn collect_tasks(out: &mut Vec<Entry>) {
    unsafe {
        let Ok(svc) = CoCreateInstance::<Option<&IUnknown>, ITaskService>(&TaskScheduler, None, CLSCTX_INPROC_SERVER) else { return };
        let empty = VARIANT::default();
        if svc.Connect(&empty, &empty, &empty, &empty).is_err() {
            return;
        }
        let Ok(root) = svc.GetFolder(&BSTR::from("\\")) else { return };
        walk_task_folder(&root, out);
    }
}

fn walk_task_folder(folder: &ITaskFolder, out: &mut Vec<Entry>) {
    unsafe {
        if let Ok(tasks) = folder.GetTasks(TASK_ENUM_HIDDEN.0) {
            if let Ok(n) = tasks.Count() {
                for i in 1..=n {
                    if let Some(e) = task_entry(&tasks, i) {
                        out.push(e);
                    }
                }
            }
        }
        if let Ok(subs) = folder.GetFolders(0) {
            if let Ok(n) = subs.Count() {
                for i in 1..=n {
                    let idx = variant_i4(i);
                    if let Ok(sub) = subs.get_Item(&idx) {
                        walk_task_folder(&sub, out);
                    }
                }
            }
        }
    }
}

fn task_entry(tasks: &IRegisteredTaskCollection, index: i32) -> Option<Entry> {
    unsafe {
        let idx = variant_i4(index);
        let t = tasks.get_Item(&idx).ok()?;
        let xml = t.Xml().ok().map(|b| b.to_string()).unwrap_or_default();
        let boot = xml.contains("BootTrigger");
        let logon = xml.contains("LogonTrigger");
        if !boot && !logon {
            return None;
        }
        let path = t.Path().ok()?.to_string();
        let name = t.Name().ok()?.to_string();
        let enabled = t.Enabled().ok().map(|v| v.0 != 0).unwrap_or(true);
        let command = xml_tag(&xml, "Command")
            .map(|c| {
                if let Some(a) = xml_tag(&xml, "Arguments") {
                    format!("{c} {a}")
                } else {
                    c
                }
            })
            .or_else(|| xml_tag(&xml, "ClassId"))
            .unwrap_or_default();
        let trigger = match (boot, logon) {
            (true, true) => "boot+logon",
            (true, false) => "boot",
            _ => "logon",
        };
        Some(Entry {
            id: format!("task:{path}"),
            name,
            command,
            kind: Kind::Task,
            machine: !path_is_user_task(&path),
            enabled,
            missing: false,
            can_toggle: true,
            can_remove: false,
            microsoft: path.starts_with(r"\Microsoft\"),
            origin: format!("tarefa ({trigger}) {path}"),
            target: Target::Task { path },
            running_hint: false,
        })
    }
}

fn collect_services(out: &mut Vec<Entry>) {
    collect_scm(out, SERVICE_WIN32, Kind::Service);
    collect_scm(out, SERVICE_DRIVER, Kind::Driver);
}

fn collect_scm(out: &mut Vec<Entry>, ty: windows::Win32::System::Services::ENUM_SERVICE_TYPE, kind: Kind) {
    unsafe {
        let scm = match OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_ENUMERATE_SERVICE | SC_MANAGER_CONNECT) {
            Ok(h) => Sc(h),
            Err(_) => return,
        };
        let mut needed = 0u32;
        let mut returned = 0u32;
        let _ = EnumServicesStatusExW(scm.0, SC_ENUM_PROCESS_INFO, ty, SERVICE_STATE_ALL, None, &mut needed, &mut returned, None, PCWSTR::null());
        if needed == 0 {
            return;
        }
        let mut buf = vec![0u8; needed as usize];
        if EnumServicesStatusExW(scm.0, SC_ENUM_PROCESS_INFO, ty, SERVICE_STATE_ALL, Some(&mut buf), &mut needed, &mut returned, None, PCWSTR::null()).is_err() {
            return;
        }
        let items = buf.as_ptr() as *const ENUM_SERVICE_STATUS_PROCESSW;
        for i in 0..returned as usize {
            let e = &*items.add(i);
            let name = sys::from_wide(e.lpServiceName.0);
            let display = sys::from_wide(e.lpDisplayName.0);
            let running = e.ServiceStatusProcess.dwCurrentState == SERVICE_RUNNING;
            let w = sys::wide(&name);
            let Ok(svc) = OpenServiceW(scm.0, PCWSTR(w.as_ptr()), SERVICE_QUERY_CONFIG | SERVICE_QUERY_STATUS) else { continue };
            let sh = Sc(svc);
            let mut need = 0u32;
            let _ = QueryServiceConfigW(sh.0, None, 0, &mut need);
            if need == 0 {
                continue;
            }
            let mut cfg_buf = vec![0u8; need as usize];
            let cfg = cfg_buf.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW;
            if QueryServiceConfigW(sh.0, Some(cfg), need, &mut need).is_err() {
                continue;
            }
            let start = (*cfg).dwStartType;
            let delayed = start == SERVICE_AUTO_START && service_delayed(sh.0);
            let bootish = start == SERVICE_AUTO_START || start == SERVICE_BOOT_START || start == SERVICE_SYSTEM_START;
            if !bootish && start != SERVICE_DISABLED {
                continue;
            }
            // Manual não sobe sozinho — fora desta lista. Disabled entra para dar para religar.
            if start == SERVICE_DEMAND_START {
                continue;
            }
            if kind == Kind::Service && start == SERVICE_BOOT_START {
                continue; // kernel boot: vai em Driver
            }
            let bin = sys::from_wide((*cfg).lpBinaryPathName.0);
            let enabled = start != SERVICE_DISABLED;
            let dangerous = start == SERVICE_BOOT_START || start == SERVICE_SYSTEM_START;
            let origin = if delayed {
                "serviço automático (atrasado)".into()
            } else if start == SERVICE_BOOT_START {
                "driver no boot".into()
            } else if start == SERVICE_SYSTEM_START {
                "driver no start do kernel".into()
            } else if start == SERVICE_DISABLED {
                "serviço desativado".into()
            } else {
                "serviço automático".into()
            };
            out.push(Entry {
                id: format!("svc:{name}"),
                name: if display.is_empty() { name.clone() } else { display },
                command: bin.clone(),
                kind,
                machine: true,
                enabled,
                missing: false,
                can_toggle: !dangerous,
                can_remove: false,
                microsoft: is_microsoft_cmd(&name, &bin),
                origin,
                target: Target::Service { name },
                running_hint: running,
            });
        }
    }
}

fn service_delayed(svc: SC_HANDLE) -> bool {
    unsafe {
        let mut need = 0u32;
        let _ = QueryServiceConfig2W(svc, SERVICE_CONFIG_DELAYED_AUTO_START_INFO, None, &mut need);
        if need == 0 {
            return false;
        }
        let mut buf = vec![0u8; need.max(8) as usize];
        if QueryServiceConfig2W(svc, SERVICE_CONFIG_DELAYED_AUTO_START_INFO, Some(&mut buf), &mut need).is_err() {
            return false;
        }
        let info = buf.as_ptr() as *const SERVICE_DELAYED_AUTO_START_INFO;
        (*info).fDelayedAutostart.as_bool()
    }
}

fn collect_uwp(out: &mut Vec<Entry>) {
    let base = r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\SystemAppData";
    let Some(root) = sys::reg_open(HKEY_CURRENT_USER, base, false) else { return };
    for pkg in sys::reg_subkeys(&root) {
        let st_path = format!("{base}\\{pkg}\\StartupTask");
        let Some(st) = sys::reg_open(HKEY_CURRENT_USER, &st_path, false) else { continue };
        for task in sys::reg_subkeys(&st) {
            let key = format!("{st_path}\\{task}");
            let state = sys::reg_dword(HKEY_CURRENT_USER, &key, "State").unwrap_or(0);
            let enabled = state == 1;
            out.push(Entry {
                id: format!("uwp:{pkg}:{task}"),
                name: task.clone(),
                command: pkg.clone(),
                kind: Kind::Uwp,
                machine: false,
                enabled,
                missing: false,
                can_toggle: true,
                can_remove: false,
                microsoft: pkg.starts_with("Microsoft.") || pkg.starts_with("Windows."),
                origin: "tarefa de app UWP".into(),
                target: Target::Uwp { key },
                running_hint: false,
            });
        }
    }
}

fn collect_winlogon(out: &mut Vec<Entry>) {
    let path = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon";
    if let Some(k) = sys::reg_open(HKEY_LOCAL_MACHINE, path, false) {
        for (name, ty, data) in sys::reg_values(&k) {
            if name != "Userinit" && name != "Shell" && name != "Taskman" && name != "VmApplet" {
                continue;
            }
            if ty != REG_SZ && ty != REG_EXPAND_SZ {
                continue;
            }
            let command = sys::utf16_bytes_to_string(&data);
            if command.trim().is_empty() {
                continue;
            }
            out.push(Entry {
                id: format!("winlogon:{name}"),
                name: name.clone(),
                command,
                kind: Kind::Winlogon,
                machine: true,
                enabled: true,
                missing: false,
                can_toggle: false,
                can_remove: false,
                microsoft: true,
                origin: "Winlogon".into(),
                target: Target::ReadOnly,
                running_hint: false,
            });
        }
    }
}

fn collect_other(out: &mut Vec<Entry>) {
    if let Some(k) = sys::reg_open(HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Windows", false) {
        for (name, ty, data) in sys::reg_values(&k) {
            if name != "AppInit_DLLs" {
                continue;
            }
            if ty != REG_SZ && ty != REG_EXPAND_SZ {
                continue;
            }
            let command = sys::utf16_bytes_to_string(&data);
            if command.trim().is_empty() {
                continue;
            }
            out.push(Entry {
                id: "appinit".into(),
                name: "AppInit_DLLs".into(),
                command,
                kind: Kind::Other,
                machine: true,
                enabled: true,
                missing: false,
                can_toggle: false,
                can_remove: false,
                microsoft: false,
                origin: "DLLs injetadas em todo processo (AppInit)".into(),
                target: Target::ReadOnly,
                running_hint: false,
            });
        }
    }
    if let Some(k) = sys::reg_open(HKEY_LOCAL_MACHINE, r"SYSTEM\CurrentControlSet\Control\Session Manager", false) {
        for (name, ty, data) in sys::reg_values(&k) {
            if name != "BootExecute" {
                continue;
            }
            let command = sys::utf16_bytes_to_string(&data);
            if command.trim().is_empty() {
                continue;
            }
            let _ = ty;
            out.push(Entry {
                id: "bootexecute".into(),
                name: "BootExecute".into(),
                command,
                kind: Kind::Other,
                machine: true,
                enabled: true,
                missing: false,
                can_toggle: false,
                can_remove: false,
                microsoft: true,
                origin: "Session Manager (antes do Winlogon)".into(),
                target: Target::ReadOnly,
                running_hint: false,
            });
        }
    }
    let as_path = r"SOFTWARE\Microsoft\Active Setup\Installed Components";
    if let Some(root) = sys::reg_open(HKEY_LOCAL_MACHINE, as_path, false) {
        for guid in sys::reg_subkeys(&root) {
            let key = format!("{as_path}\\{guid}");
            let Some(k) = sys::reg_open(HKEY_LOCAL_MACHINE, &key, false) else { continue };
            let mut stub = String::new();
            let mut label = guid.clone();
            for (name, ty, data) in sys::reg_values(&k) {
                if ty != REG_SZ && ty != REG_EXPAND_SZ {
                    continue;
                }
                let s = sys::utf16_bytes_to_string(&data);
                if name == "StubPath" {
                    stub = s;
                } else if name.is_empty() && !s.is_empty() {
                    label = s;
                }
            }
            if stub.is_empty() {
                continue;
            }
            let pending = sys::reg_open(HKEY_CURRENT_USER, &format!(r"SOFTWARE\Microsoft\Active Setup\Installed Components\{guid}"), false).is_none();
            out.push(Entry {
                id: format!("activesetup:{guid}"),
                name: label,
                command: stub,
                kind: Kind::Other,
                machine: true,
                enabled: pending,
                missing: false,
                can_toggle: false,
                can_remove: false,
                microsoft: true,
                origin: if pending { "Active Setup (pendente neste usuário)" } else { "Active Setup (já rodou neste usuário)" }.into(),
                target: Target::ReadOnly,
                running_hint: false,
            });
        }
    }
}

// ---------- ações ----------

fn apply_toggle(e: &Entry, enabled: bool) -> Result<(), String> {
    match &e.target {
        Target::Run { machine, wow64, name } => {
            let root = if *machine { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER };
            let approved = if *wow64 { APPROVED_RUN32 } else { APPROVED_RUN };
            set_approved(root, approved, name, enabled)
        }
        Target::Folder { common, file_name, .. } => {
            let root = if *common { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER };
            set_approved(root, APPROVED_FOLDER, file_name, enabled)
        }
        Target::Task { path } => set_task_enabled(path, enabled),
        Target::Service { name } => {
            sys::set_start_type(name, if enabled { SvcStart::Auto } else { SvcStart::Disabled })
        }
        Target::Uwp { key } => sys::reg_set_dword(HKEY_CURRENT_USER, key, "State", if enabled { 1 } else { 2 }),
        Target::ReadOnly => Err("esta entrada não pode ser alternada por aqui".into()),
    }
}

fn apply_remove(e: &Entry) -> Result<(), String> {
    match &e.target {
        Target::Run { machine, wow64, name } => {
            let root = if *machine { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER };
            let run = if *wow64 { RUN_WOW } else { RUN };
            let approved = if *wow64 { APPROVED_RUN32 } else { APPROVED_RUN };
            let r1 = sys::reg_delete_value(root, run, name);
            let _ = sys::reg_delete_value(root, approved, name);
            if e.missing { Ok(()) } else { r1 }
        }
        Target::Folder { path, common, file_name } => {
            if path.exists() {
                std::fs::remove_file(path).map_err(|e| e.to_string())?;
            }
            let root = if *common { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER };
            let _ = sys::reg_delete_value(root, APPROVED_FOLDER, file_name);
            Ok(())
        }
        _ => Err("esta origem não se remove daqui — desative".into()),
    }
}

fn set_approved(root: windows::Win32::System::Registry::HKEY, path: &str, name: &str, enabled: bool) -> Result<(), String> {
    let mut data = [0u8; 12];
    data[0] = if enabled { 2 } else { 3 };
    if !enabled {
        data[4..12].copy_from_slice(&procs::now_filetime().to_le_bytes());
    }
    sys::reg_set_binary(root, path, name, &data)
}

fn set_task_enabled(path: &str, enabled: bool) -> Result<(), String> {
    ensure_com();
    unsafe {
        let svc: ITaskService = CoCreateInstance::<Option<&IUnknown>, ITaskService>(&TaskScheduler, None, CLSCTX_INPROC_SERVER).map_err(|e| e.message())?;
        let empty = VARIANT::default();
        svc.Connect(&empty, &empty, &empty, &empty).map_err(|e| e.message())?;
        let folder = svc.GetFolder(&BSTR::from("\\")).map_err(|e| e.message())?;
        let task = folder.GetTask(&BSTR::from(path)).map_err(|e| e.message())?;
        let flag = if enabled {
            windows::Win32::Foundation::VARIANT_TRUE
        } else {
            windows::Win32::Foundation::VARIANT_FALSE
        };
        task.SetEnabled(flag).map_err(|e| e.message())
    }
}

fn toggle_ps(e: &Entry, enabled: bool) -> String {
    match &e.target {
        Target::Run { wow64, name, .. } => {
            let key = if *wow64 { APPROVED_RUN32 } else { APPROVED_RUN };
            let bytes = if enabled { "2,0,0,0,0,0,0,0,0,0,0,0" } else { "3,0,0,0,0,0,0,0,0,0,0,0" };
            format!(
                "Set-ItemProperty -Path 'HKLM:\\{key}' -Name {} -Value ([byte[]]({bytes}))",
                sys::ps_quote(name)
            )
        }
        Target::Folder { file_name, .. } => {
            let bytes = if enabled { "2,0,0,0,0,0,0,0,0,0,0,0" } else { "3,0,0,0,0,0,0,0,0,0,0,0" };
            format!(
                "Set-ItemProperty -Path 'HKLM:\\{APPROVED_FOLDER}' -Name {} -Value ([byte[]]({bytes}))",
                sys::ps_quote(file_name)
            )
        }
        Target::Task { path } => {
            let flag = if enabled { "/ENABLE" } else { "/DISABLE" };
            format!("schtasks /Change /TN {} {flag}", sys::ps_quote(path))
        }
        Target::Service { name } => {
            let ty = if enabled { "Automatic" } else { "Disabled" };
            format!("Set-Service -Name {} -StartupType {ty}", sys::ps_quote(name))
        }
        _ => String::new(),
    }
}

fn remove_ps(e: &Entry) -> String {
    match &e.target {
        Target::Run { wow64, name, .. } => {
            let run = if *wow64 { RUN_WOW } else { RUN };
            let approved = if *wow64 { APPROVED_RUN32 } else { APPROVED_RUN };
            format!(
                "Remove-ItemProperty -Path 'HKLM:\\{run}' -Name {0} -ErrorAction SilentlyContinue; Remove-ItemProperty -Path 'HKLM:\\{approved}' -Name {0} -ErrorAction SilentlyContinue",
                sys::ps_quote(name)
            )
        }
        Target::Folder { path, file_name, .. } => {
            format!(
                "Remove-Item -LiteralPath {} -Force -ErrorAction SilentlyContinue; Remove-ItemProperty -Path 'HKLM:\\{APPROVED_FOLDER}' -Name {} -ErrorAction SilentlyContinue",
                sys::ps_quote(&path.to_string_lossy()),
                sys::ps_quote(file_name)
            )
        }
        _ => String::new(),
    }
}

// ---------- helpers ----------

fn approved_map(root: windows::Win32::System::Registry::HKEY, path: &str) -> HashMap<String, (String, bool)> {
    sys::reg_open(root, path, false)
        .map(|k| {
            sys::reg_values(&k)
                .into_iter()
                .map(|(n, _, d)| {
                    let enabled = d.first().map(|b| *b != 3).unwrap_or(true);
                    let key = n.to_lowercase();
                    (key, (n, enabled))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn resolve_startup_file(path: &Path) -> String {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if ext == "lnk" {
        if let Some(s) = resolve_lnk(path) {
            return s;
        }
    }
    path.to_string_lossy().into_owned()
}

fn resolve_lnk(path: &Path) -> Option<String> {
    ensure_com();
    unsafe {
        let link: IShellLinkW = CoCreateInstance::<Option<&IUnknown>, IShellLinkW>(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let persist: IPersistFile = link.cast().ok()?;
        let w = sys::wide(&path.to_string_lossy());
        persist.Load(PCWSTR(w.as_ptr()), STGM_READ).ok()?;
        let mut file = vec![0u16; 32768];
        let mut fd = WIN32_FIND_DATAW::default();
        link.GetPath(&mut file, &mut fd, 0).ok()?;
        let n = file.iter().position(|&c| c == 0).unwrap_or(file.len());
        let mut cmd = String::from_utf16_lossy(&file[..n]);
        let mut args = vec![0u16; 4096];
        if link.GetArguments(&mut args).is_ok() {
            let an = args.iter().position(|&c| c == 0).unwrap_or(0);
            let a = String::from_utf16_lossy(&args[..an]);
            if !a.is_empty() {
                cmd.push(' ');
                cmd.push_str(&a);
            }
        }
        Some(cmd)
    }
}

fn ensure_com() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    });
}

fn variant_i4(v: i32) -> VARIANT {
    let mut var = VARIANT::default();
    unsafe {
        let inner = &mut var.Anonymous.Anonymous;
        inner.vt = VT_I4;
        inner.Anonymous.lVal = v;
    }
    var
}

fn xml_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    let s = xml[start..end].trim();
    if s.is_empty() { None } else { Some(s.to_string()) }
}

fn path_is_user_task(path: &str) -> bool {
    path.contains("S-1-5-21-") || path.contains("\\User")
}

fn is_microsoft_cmd(name: &str, cmd: &str) -> bool {
    let n = name.to_ascii_lowercase();
    let c = cmd.to_ascii_lowercase();
    n.starts_with("microsoft")
        || n == "securityhealth"
        || c.contains(r"\windows\system32\")
        || c.contains(r"\windows\syswow64\")
        || c.contains(r"\microsoft\")
}

fn exe_name_from_cmd(cmd: &str) -> Option<String> {
    let c = cmd.trim();
    if c.is_empty() {
        return None;
    }
    let path = if let Some(rest) = c.strip_prefix('"') {
        rest.split('"').next().unwrap_or("")
    } else {
        c.split_whitespace().next().unwrap_or("")
    };
    let name = path.rsplit(['\\', '/']).next()?.to_lowercase();
    if name.is_empty() || !name.contains('.') {
        None
    } else {
        Some(name)
    }
}

fn running_exes(procs: &[ProcInfo]) -> HashSet<String> {
    procs.iter().map(|p| p.name_lower.clone()).collect()
}

fn exe_running(cmd: &str, running: &HashSet<String>) -> bool {
    exe_name_from_cmd(cmd).map(|n| running.contains(&n)).unwrap_or(false)
}

struct Sc(SC_HANDLE);
impl Drop for Sc {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseServiceHandle(self.0);
        }
    }
}
