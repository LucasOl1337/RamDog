//! Visão Partida: tudo que o Windows dispara no boot e no logon.
//!
//! O Gerenciador de Tarefas só mostra o que está em `Run` + `StartupApproved` e atalhos
//! `.lnk` da pasta Iniciar. Esta lista vai além: `.vbs`/`.cmd` da pasta, RunOnce, Wow64,
//! tarefas com gatilho de boot/logon, serviços Auto, UWP, Winlogon, Active Setup.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use egui::{Color32, RichText, TextureHandle};
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

use crate::app::{ACCENT, ACCENT_BG, LINE, MUTED, SURFACE, SURFACE_HI};
use crate::config::{BootGroup, Config};
use crate::icons::IconBank;
use crate::procs::{self, ProcInfo};
use crate::sys::{self, SvcStart, SysResult};
use crate::usage;

pub enum BootOut {
    Toast(String, bool),
    Kill(Vec<u32>),
    /// A Partida mexeu na config (presets) — o `App` grava.
    SaveCfg,
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

/// Em que momento do arranque a entrada dispara — a hierarquia real do Windows.
///
/// O que está no topo sobe antes de existir tela de logon e não tem nada a ver com você;
/// o que está embaixo só aparece depois que a área de trabalho carregou e quase sempre é
/// escolha sua. Sem esta separação a lista mistura driver de chipset com Spotify, e a
/// pessoa não tem como saber o que é seguro desligar.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Phase {
    Kernel,
    Machine,
    Logon,
    Desktop,
}

impl Phase {
    /// Da superfície para o fundo: o que é escolha sua vem primeiro, o que é encanamento do
    /// Windows vem depois. Ordenar pela sequência real do boot enterraria os seus programas
    /// embaixo de cem serviços — justamente o que a pessoa abriu a tela para ver.
    fn ord(self) -> u8 {
        match self {
            Phase::Desktop => 0,
            Phase::Logon => 1,
            Phase::Machine => 2,
            Phase::Kernel => 3,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Phase::Kernel => "Antes do Windows",
            Phase::Machine => "Com a máquina",
            Phase::Logon => "Ao entrar na conta",
            Phase::Desktop => "Seus programas",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Phase::Kernel => "drivers e kernel: carregam antes de existir tela de logon",
            Phase::Machine => "serviços e tarefas de boot: sobem sozinhos, mesmo sem ninguém logado",
            Phase::Logon => "dispara no logon, antes da área de trabalho aparecer",
            Phase::Desktop => "abre depois da área de trabalho — é aqui que mora o atraso do seu login",
        }
    }

    fn color(self) -> Color32 {
        match self {
            Phase::Kernel => Color32::from_rgb(160, 170, 180),
            Phase::Machine => Color32::from_rgb(232, 178, 92),
            Phase::Logon => Color32::from_rgb(200, 160, 255),
            // Azul, não o verde do "sobe com o PC": faixa de fase dentro de faixa de estado
            // com a mesma cor viraria um bloco só.
            Phase::Desktop => Color32::from_rgb(96, 148, 214),
        }
    }
}

/// A pergunta principal do addon: isto sobe com o PC?
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Status {
    On,
    Off,
    Broken,
}

impl Status {
    fn of(e: &Entry) -> Status {
        if e.missing {
            Status::Broken
        } else if e.enabled {
            Status::On
        } else {
            Status::Off
        }
    }

    fn ord(self) -> u8 {
        match self {
            Status::On => 0,
            Status::Off => 1,
            Status::Broken => 2,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Status::On => "SOBE COM O PC",
            Status::Off => "NÃO SOBE",
            Status::Broken => "QUEBRADAS",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Status::On => "dispara sozinho toda vez que o Windows liga",
            Status::Off => "continua instalado, mas o Windows não dispara mais",
            Status::Broken => "a partida aponta para um arquivo que não existe mais",
        }
    }

    fn color(self) -> Color32 {
        match self {
            Status::On => Color32::from_rgb(120, 200, 140),
            Status::Off => Color32::from_rgb(150, 158, 170),
            Status::Broken => Color32::from_rgb(232, 120, 100),
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Status::On => "▲",
            Status::Off => "○",
            Status::Broken => "⚠",
        }
    }
}

/// Um nível de agrupamento já resolvido: serve tanto de chave quanto de cabeçalho.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Grp {
    St(Status),
    Ph(Phase),
    Kd(Kind),
}

impl Grp {
    fn ord(self) -> u8 {
        match self {
            Grp::St(s) => s.ord(),
            Grp::Ph(p) => p.ord(),
            Grp::Kd(k) => kind_ord(k),
        }
    }

    fn key(self) -> String {
        match self {
            Grp::St(s) => format!("s{}", s.ord()),
            Grp::Ph(p) => format!("p{}", p.ord()),
            Grp::Kd(k) => format!("k{}", kind_ord(k)),
        }
    }

    fn title(self) -> String {
        match self {
            Grp::St(s) => s.title().to_string(),
            Grp::Ph(p) => p.title().to_string(),
            Grp::Kd(k) => k.label().to_string(),
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Grp::St(s) => s.hint(),
            Grp::Ph(p) => p.hint(),
            Grp::Kd(_) => "",
        }
    }

    fn color(self) -> Color32 {
        match self {
            Grp::St(s) => s.color(),
            Grp::Ph(p) => p.color(),
            Grp::Kd(k) => k.color(),
        }
    }
}

/// Os dois níveis de grupo de uma entrada, segundo o modo escolhido.
fn grp_path(gb: BootGroup, e: &Entry) -> (Option<Grp>, Option<Grp>) {
    match gb {
        BootGroup::StatusPhase => (Some(Grp::St(Status::of(e))), Some(Grp::Ph(e.phase))),
        BootGroup::StatusKind => (Some(Grp::St(Status::of(e))), Some(Grp::Kd(e.kind))),
        BootGroup::Phase => (Some(Grp::Ph(e.phase)), None),
        BootGroup::Kind => (Some(Grp::Kd(e.kind)), None),
        BootGroup::Flat => (None, None),
    }
}

/// Cabeçalho de grupo já com os números que ele mostra.
struct Head {
    grp: Grp,
    depth: usize,
    key: String,
    total: usize,
    running: usize,
    /// Quantas do grupo sobem com o PC — só interessa quando o grupo não é o próprio estado.
    on: usize,
    collapsed: bool,
}

/// Uma linha da tabela: cabeçalho de grupo ou entrada.
enum Line {
    Head(usize),
    Item(usize),
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
    /// Momento do arranque em que dispara. Vem do coletor: só ele sabe se a tarefa tem
    /// gatilho de boot ou de logon, e se o serviço é de kernel ou automático comum.
    phase: Phase,
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
    /// Alinha várias entradas de uma vez ao estado guardado num preset.
    Preset(String, Vec<(Entry, bool)>),
    /// Cria um atalho na pasta Iniciar do usuário apontando para um executável.
    AddStartup { exe: String, label: String },
}

struct Pending {
    title: String,
    lines: Vec<String>,
    action: Action,
}

/// Por qual coluna a lista está ordenada.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SortKey {
    /// Rodando → no boot → desligada → ausente. É o padrão: responde "o que está de pé".
    State,
    Name,
    Kind,
    Scope,
    Usage,
}

/// O que a Partida sabe do executável de uma entrada. Calculado uma vez por refresh —
/// `SHGetFileInfoW` e `Path::exists` por linha por frame seriam caros demais.
#[derive(Clone, Default)]
struct Resolved {
    /// Arquivo de onde tirar o ícone (o exe, ou o próprio .lnk quando o alvo sumiu).
    icon: Option<String>,
    /// Caminho do exe em minúsculo, chave do mapa de uso.
    exe: Option<String>,
}

/// Dado de uso já pronto para uma linha, para o corpo da tabela não tocar em `self`.
#[derive(Clone, Copy, Default)]
struct RowUsage {
    focus: u64,
    open: u64,
    last: i64,
}

impl RowUsage {
    fn total(self) -> u64 {
        self.focus + self.open
    }
}

pub struct Boot {
    entries: Vec<Entry>,
    /// id da entrada → executável/ícone resolvidos.
    resolved: HashMap<String, Resolved>,
    last_refresh: Option<Instant>,
    kinds: HashSet<Kind>,
    hide_microsoft: bool,
    /// Mostrar só um dos três estados. `None` = os três, cada um no seu bloco.
    status_filter: Option<Status>,
    only_running: bool,
    /// Chaves de grupo recolhidas — vale por sessão, não vai para o disco.
    collapsed: HashSet<String>,
    sort: SortKey,
    sort_desc: bool,
    selected: Option<String>,
    icons: IconBank,
    /// Ranking completo de uso (Scan). Também alimenta a coluna "Uso" da lista.
    rank: Vec<usage::Ranked>,
    /// caminho em minúsculo → índice em `rank`.
    rank_idx: HashMap<String, usize>,
    usage_at: Option<Instant>,
    scan_open: bool,
    /// Nome digitado para salvar um preset novo.
    preset_name: String,
    preset_sel: String,
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
            resolved: HashMap::new(),
            last_refresh: None,
            kinds: Kind::ALL.iter().copied().filter(|k| *k != Kind::Driver).collect(),
            hide_microsoft: false,
            status_filter: None,
            only_running: false,
            collapsed: HashSet::new(),
            sort: SortKey::State,
            sort_desc: false,
            selected: None,
            icons: IconBank::new(),
            rank: Vec::new(),
            rank_idx: HashMap::new(),
            usage_at: None,
            scan_open: false,
            preset_name: String::new(),
            preset_sel: String::new(),
            tx,
            rx,
            busy: 0,
            pending: None,
        }
    }

    fn refresh(&mut self) {
        self.entries = collect();
        self.resolved = self
            .entries
            .iter()
            .map(|e| (e.id.clone(), resolve_entry(e)))
            .collect();
        self.last_refresh = Some(Instant::now());
    }

    fn maybe_refresh(&mut self) {
        let due = self.last_refresh.map(|t| t.elapsed() > Duration::from_secs(8)).unwrap_or(true);
        if due {
            self.refresh();
        }
    }

    /// Relê UserAssist e funde com a contagem local. Custa uma varredura de registro —
    /// por isso vale por um minuto, não por frame.
    fn refresh_usage(&mut self, tracker: &usage::Tracker) {
        let ua = usage::user_assist();
        self.rank = usage::rank(tracker.apps(), &ua, 0);
        self.rank_idx = self
            .rank
            .iter()
            .enumerate()
            .map(|(i, r)| (r.path.to_lowercase(), i))
            .collect();
        self.usage_at = Some(Instant::now());
    }

    fn maybe_refresh_usage(&mut self, tracker: &usage::Tracker) {
        let due = self.usage_at.map(|t| t.elapsed() > Duration::from_secs(60)).unwrap_or(true);
        if due {
            self.refresh_usage(tracker);
        }
    }

    fn usage_of(&self, exe_lower: Option<&String>) -> RowUsage {
        let Some(key) = exe_lower else { return RowUsage::default() };
        match self.rank_idx.get(key) {
            Some(i) => {
                let r = &self.rank[*i];
                RowUsage { focus: r.focus_secs, open: r.open_secs, last: r.last_used }
            }
            None => RowUsage::default(),
        }
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        procs: &[ProcInfo],
        search: &str,
        is_admin: bool,
        cfg: &mut Config,
        tracker: &usage::Tracker,
    ) -> Vec<BootOut> {
        while let Ok(_r) = self.rx.try_recv() {
            self.busy = self.busy.saturating_sub(1);
            self.last_refresh = None;
        }
        self.maybe_refresh();
        self.maybe_refresh_usage(tracker);
        if self.icons.poll(ui.ctx()) {
            ui.ctx().request_repaint();
        }

        let mut out = Vec::new();
        let mut queued: Vec<Action> = Vec::new();
        let mut confirm: Option<Pending> = None;

        let running = running_exes(procs);
        let q = search.trim().to_lowercase();

        let mut filtered: Vec<Entry> = self
            .entries
            .iter()
            .filter(|e| self.kinds.contains(&e.kind))
            .filter(|e| !self.hide_microsoft || !e.microsoft)
            .filter(|e| self.status_filter.map(|s| Status::of(e) == s).unwrap_or(true))
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

        // A ordem dos grupos vem primeiro e a coluna escolhida só desempata dentro deles:
        // sem isso os membros de um grupo não ficariam contíguos e não haveria onde emitir
        // cabeçalho. Depois disso o desempate é sempre o nome, para a lista não trocar de
        // ordem sozinha entre dois refreshes.
        let gb = cfg.boot_group;
        {
            let key = self.sort;
            let desc = self.sort_desc;
            let mut with_meta: Vec<(Entry, u8, u64, String, u8, u8)> = filtered
                .drain(..)
                .map(|e| {
                    let is_run = e.running_hint || exe_running(&e.command, &running);
                    let st = state_rank(&e, is_run);
                    let exe = self.resolved.get(&e.id).and_then(|r| r.exe.clone());
                    let usage = self.usage_of(exe.as_ref()).total();
                    let name = e.name.to_lowercase();
                    let (g1, g2) = grp_path(gb, &e);
                    let o1 = g1.map(|g| g.ord()).unwrap_or(0);
                    let o2 = g2.map(|g| g.ord()).unwrap_or(0);
                    (e, st, usage, name, o1, o2)
                })
                .collect();
            with_meta.sort_by(|a, b| {
                let ord = match key {
                    SortKey::State => a.1.cmp(&b.1),
                    SortKey::Usage => a.2.cmp(&b.2),
                    SortKey::Name => a.3.cmp(&b.3),
                    SortKey::Kind => kind_ord(a.0.kind).cmp(&kind_ord(b.0.kind)),
                    SortKey::Scope => a.0.machine.cmp(&b.0.machine),
                };
                let ord = if desc { ord.reverse() } else { ord };
                // Dentro do mesmo grupo, o que você mais usa vem primeiro — é o que faz a
                // lista responder "o que importa aqui" em vez de despejar a ordem alfabética.
                a.4.cmp(&b.4)
                    .then_with(|| a.5.cmp(&b.5))
                    .then(ord)
                    .then_with(|| b.2.cmp(&a.2))
                    .then_with(|| a.3.cmp(&b.3))
            });
            filtered = with_meta.into_iter().map(|t| t.0).collect();
        }

        // Tudo que a tabela precisa saber por linha, resolvido antes do corpo — dentro do
        // closure `self` já está emprestado.
        let row_icons: Vec<Option<TextureHandle>> = filtered
            .iter()
            .map(|e| {
                let path = self.resolved.get(&e.id).and_then(|r| r.icon.clone());
                path.and_then(|p| self.icons.get(&p))
            })
            .collect();
        let row_usage: Vec<RowUsage> = filtered
            .iter()
            .map(|e| {
                let exe = self.resolved.get(&e.id).and_then(|r| r.exe.clone());
                self.usage_of(exe.as_ref())
            })
            .collect();

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Partida").strong().size(16.0));
            ui.label(
                RichText::new("— tudo que o Windows dispara no boot e no logon, sem o recorte do Gerenciador de Tarefas")
                    .color(MUTED),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Atualizar").on_hover_text("Relê registro, pasta Iniciar, tarefas e serviços").clicked() {
                    self.last_refresh = None;
                }
                let scan = egui::Button::new(RichText::new("⌕ Scan de uso").color(Color32::WHITE).strong())
                    .fill(ACCENT_BG)
                    .stroke(egui::Stroke::new(1.0_f32, ACCENT));
                if ui
                    .add(scan)
                    .on_hover_text("Mede quais programas você mais usa (tempo em foco + tempo aberto) e sugere o que vale colocar na partida")
                    .clicked()
                {
                    self.refresh_usage(tracker);
                    self.scan_open = true;
                }
                if self.busy > 0 {
                    ui.spinner();
                    ui.label(RichText::new(format!("{} ação(ões) aguardando UAC", self.busy)).color(MUTED).small());
                }
            });
        });
        ui.add_space(4.0);

        // A pergunta que este addon existe para responder, em três números clicáveis. Cada um
        // filtra a lista para o seu estado; clicar de novo volta a mostrar os três blocos.
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for st in [Status::On, Status::Off, Status::Broken] {
                let n = self.entries.iter().filter(|e| Status::of(e) == st).count();
                let sel = self.status_filter == Some(st);
                let c = st.color();
                let text = RichText::new(format!("{}  {}  {n}", st.glyph(), st.title()))
                    .size(12.0)
                    .strong()
                    .color(if n == 0 && !sel { MUTED.gamma_multiply(0.7) } else { c });
                let btn = egui::Button::new(text)
                    .fill(if sel { c.gamma_multiply(0.22) } else { SURFACE })
                    .stroke(egui::Stroke::new(1.0_f32, if sel { c } else { LINE }));
                if ui
                    .add(btn)
                    .on_hover_text(format!("{}

Clique para ver só estas.", st.hint()))
                    .clicked()
                {
                    self.status_filter = if sel { None } else { Some(st) };
                }
            }
            let total = self.entries.len();
            ui.label(
                RichText::new(format!("de {total} entradas · {} visíveis agora", filtered.len()))
                    .small()
                    .color(MUTED),
            );
        });
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
            ui.label(RichText::new("Separar por:").small().color(MUTED));
            let cur = cfg.boot_group;
            let mut pick: Option<BootGroup> = None;
            egui::ComboBox::from_id_salt("boot_group")
                .selected_text(RichText::new(cur.label()).size(12.0))
                .width(168.0)
                .show_ui(ui, |ui| {
                    for g in BootGroup::ALL {
                        if ui.selectable_label(cur == g, g.label()).on_hover_text(g.tip()).clicked() {
                            pick = Some(g);
                        }
                    }
                });
            if let Some(g) = pick {
                if g != cur {
                    cfg.boot_group = g;
                    self.collapsed.clear();
                    out.push(BootOut::SaveCfg);
                }
            }
            if cur != BootGroup::Flat {
                if ui.small_button("⊟").on_hover_text("Recolher todos os grupos").clicked() {
                    // Vale varrer tudo, não só o que está filtrado: chave sobrando no conjunto
                    // não atrapalha, chave faltando deixaria um grupo aberto sem motivo.
                    for e in &self.entries {
                        let (a, b) = grp_path(cur, e);
                        if let Some(a) = a {
                            let k = a.key();
                            if let Some(b) = b {
                                self.collapsed.insert(format!("{k}/{}", b.key()));
                            }
                            self.collapsed.insert(k);
                        }
                    }
                }
                if ui.small_button("⊞").on_hover_text("Abrir todos os grupos").clicked() {
                    self.collapsed.clear();
                }
            }
            ui.separator();
            ui.checkbox(&mut self.hide_microsoft, "esconder Microsoft");
            ui.checkbox(&mut self.only_running, "só as que estão rodando");
            ui.separator();
            self.presets_bar(ui, &mut confirm, cfg, &mut out);
        });
        ui.add_space(4.0);

        let mut toggle: Option<(Entry, bool)> = None;
        let mut remove: Option<Entry> = None;
        let mut kill: Option<Vec<u32>> = None;
        let mut select: Option<String> = None;
        let mut add: Option<(String, String)> = None;
        let mut sort_click: Option<SortKey> = None;

        if self.scan_open {
            self.scan_panel(ui, &mut add);
            ui.add_space(4.0);
        }

        let sort = self.sort;
        let sort_desc = self.sort_desc;
        let head = |ui: &mut egui::Ui, text: &str, key: SortKey, click: &mut Option<SortKey>| {
            let arrow = if sort == key {
                if sort_desc { " ▼" } else { " ▲" }
            } else {
                ""
            };
            let color = if sort == key { ACCENT } else { MUTED };
            let b = egui::Button::new(RichText::new(format!("{text}{arrow}")).small().color(color).strong()).frame(false);
            if ui.add(b).on_hover_text("Ordenar por esta coluna").clicked() {
                *click = Some(key);
            }
        };

        let row_running: Vec<bool> = filtered
            .iter()
            .map(|e| e.running_hint || exe_running(&e.command, &running))
            .collect();

        // Cabeçalhos de grupo intercalados com as entradas. `filtered` já saiu ordenado pela
        // hierarquia, então basta emitir um cabeçalho toda vez que a chave muda.
        let mut heads: Vec<Head> = Vec::new();
        let mut lines: Vec<Line> = Vec::new();
        if gb == BootGroup::Flat {
            lines = (0..filtered.len()).map(Line::Item).collect();
        } else {
            let path: Vec<(Option<Grp>, Option<Grp>)> =
                filtered.iter().map(|e| grp_path(gb, e)).collect();
            let tally = |r: std::ops::Range<usize>| -> (usize, usize) {
                let run = r.clone().filter(|&j| row_running[j]).count();
                let on = r.filter(|&j| filtered[j].enabled && !filtered[j].missing).count();
                (run, on)
            };
            let mut i = 0usize;
            while i < filtered.len() {
                let Some(g1) = path[i].0 else { break };
                let end1 = (i..filtered.len())
                    .find(|&j| path[j].0 != Some(g1))
                    .unwrap_or(filtered.len());
                let key1 = g1.key();
                let (run1, on1) = tally(i..end1);
                let col1 = self.collapsed.contains(&key1);
                heads.push(Head {
                    grp: g1,
                    depth: 0,
                    key: key1.clone(),
                    total: end1 - i,
                    running: run1,
                    on: on1,
                    collapsed: col1,
                });
                lines.push(Line::Head(heads.len() - 1));
                if col1 {
                    i = end1;
                    continue;
                }
                let mut j = i;
                while j < end1 {
                    match path[j].1 {
                        None => {
                            lines.push(Line::Item(j));
                            j += 1;
                        }
                        Some(g2) => {
                            let end2 = (j..end1).find(|&k| path[k].1 != Some(g2)).unwrap_or(end1);
                            let key2 = format!("{key1}/{}", g2.key());
                            let (run2, on2) = tally(j..end2);
                            let col2 = self.collapsed.contains(&key2);
                            heads.push(Head {
                                grp: g2,
                                depth: 1,
                                key: key2,
                                total: end2 - j,
                                running: run2,
                                on: on2,
                                collapsed: col2,
                            });
                            lines.push(Line::Head(heads.len() - 1));
                            if !col2 {
                                for k in j..end2 {
                                    lines.push(Line::Item(k));
                                }
                            }
                            j = end2;
                        }
                    }
                }
                i = end1;
            }
        }

        // Entrada dentro de grupo anda para a direita — é o que faz a faixa parecer um título
        // e não mais uma linha da lista.
        let indent = match gb {
            BootGroup::Flat => 0.0,
            BootGroup::Phase | BootGroup::Kind => 10.0,
            _ => 20.0,
        };

        if filtered.is_empty() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Nenhuma entrada bate com os filtros de agora").color(MUTED));
                ui.label(
                    RichText::new("Tire o filtro de estado lá em cima, ligue mais tipos ou limpe a busca.")
                        .small()
                        .color(MUTED),
                );
            });
        } else {
        // Onde a faixa de cabeçalho começa e termina. A faixa é pintada da última coluna,
        // que desenha depois de todas as outras e não tem recorte próprio.
        let band_x = ui.available_rect_before_wrap().x_range();
        let mut toggle_group: Option<String> = None;
        let heights: Vec<f32> = lines
            .iter()
            .map(|l| match l {
                Line::Head(h) => {
                    if heads[*h].depth == 0 {
                        30.0
                    } else {
                        24.0
                    }
                }
                Line::Item(_) => 26.0,
            })
            .collect();

        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(30.0))
            .column(Column::initial(250.0).at_least(140.0).clip(true))
            .column(Column::initial(110.0).at_least(80.0).clip(true))
            .column(Column::initial(80.0).at_least(64.0))
            .column(Column::remainder().at_least(140.0).clip(true))
            .column(Column::initial(84.0).at_least(70.0))
            .column(Column::initial(84.0).at_least(64.0))
            .column(Column::initial(132.0).at_least(90.0))
            .header(22.0, |mut h| {
                h.col(|ui| {
                    ui.label(RichText::new("▲").small().strong().color(Status::On.color()))
                        .on_hover_text("Marcado = sobe com o PC. Desmarcar tira da partida sem desinstalar nada.");
                });
                h.col(|ui| head(ui, "Nome", SortKey::Name, &mut sort_click));
                h.col(|ui| head(ui, "Tipo", SortKey::Kind, &mut sort_click));
                h.col(|ui| head(ui, "Escopo", SortKey::Scope, &mut sort_click));
                h.col(|ui| { ui.label(RichText::new("Comando").small().color(MUTED).strong()); });
                h.col(|ui| head(ui, "Agora", SortKey::State, &mut sort_click));
                h.col(|ui| head(ui, "Uso", SortKey::Usage, &mut sort_click));
                h.col(|ui| { ui.label(RichText::new("Ações").small().color(MUTED).strong()); });
            })
            .body(|body| {
                body.heterogeneous_rows(heights.into_iter(), |mut row| {
                    match lines[row.index()] {
                        Line::Head(hi) => {
                            let h = &heads[hi];
                            // As sete primeiras células ficam vazias: o cabeçalho é uma faixa
                            // só, atravessando as colunas.
                            for _ in 0..7 {
                                row.col(|_| {});
                            }
                            row.col(|ui| {
                                let sp = ui.spacing().item_spacing;
                                let deep = h.depth == 0;
                                let x0 = band_x.min + if deep { 0.0 } else { 14.0 };
                                let band = egui::Rect::from_x_y_ranges(
                                    egui::Rangef::new(x0, band_x.max),
                                    ui.max_rect().y_range(),
                                )
                                .expand2(egui::Vec2::new(0.0, 0.5 * sp.y));
                                let c = h.grp.color();
                                let p = ui.painter();
                                if deep {
                                    p.rect_filled(band, 0.0, c.gamma_multiply(0.20));
                                } else {
                                    p.rect_filled(band, 0.0, SURFACE_HI);
                                    p.rect_filled(band, 0.0, c.gamma_multiply(0.10));
                                }
                                p.rect_filled(
                                    egui::Rect::from_min_size(band.min, egui::Vec2::new(3.0, band.height())),
                                    0.0,
                                    if deep { c } else { c.gamma_multiply(0.7) },
                                );
                                let cy = band.center().y;
                                let x = band.left() + 12.0 + h.depth as f32 * 14.0;
                                p.text(
                                    egui::pos2(x, cy),
                                    egui::Align2::LEFT_CENTER,
                                    if h.collapsed { "▸" } else { "▾" },
                                    egui::FontId::proportional(11.0),
                                    c,
                                );
                                let t = p.text(
                                    egui::pos2(x + 16.0, cy),
                                    egui::Align2::LEFT_CENTER,
                                    h.grp.title(),
                                    egui::FontId::proportional(if deep { 13.0 } else { 12.0 }),
                                    c,
                                );
                                let hint = h.grp.hint();
                                if !hint.is_empty() {
                                    p.text(
                                        egui::pos2(t.right() + 10.0, cy),
                                        egui::Align2::LEFT_CENTER,
                                        format!("— {hint}"),
                                        egui::FontId::proportional(10.5),
                                        MUTED,
                                    );
                                }
                                let plural = if h.total == 1 { "entrada" } else { "entradas" };
                                let counts = if matches!(h.grp, Grp::St(_)) {
                                    format!("{} {plural} · {} rodando agora", h.total, h.running)
                                } else if deep {
                                    format!("{}/{} sobem com o PC · {} rodando", h.on, h.total, h.running)
                                } else {
                                    // Já está dentro de um bloco de estado: repetir "23/23 sobem"
                                    // seria dizer duas vezes a mesma coisa.
                                    format!("{} {plural} · {} rodando", h.total, h.running)
                                };
                                p.text(
                                    egui::pos2(band.right() - 10.0, cy),
                                    egui::Align2::RIGHT_CENTER,
                                    counts,
                                    egui::FontId::proportional(11.0),
                                    MUTED,
                                );
                            });
                            if row.response().clicked() {
                                toggle_group = Some(heads[hi].key.clone());
                            }
                        }
                        Line::Item(i) => {
                            let e = &filtered[i];
                            let is_run = row_running[i];
                            let selected = self.selected.as_deref() == Some(e.id.as_str());
                            row.set_selected(selected);

                            row.col(|ui| {
                                if e.can_toggle {
                                    let mut on = e.enabled;
                                    if ui
                                        .checkbox(&mut on, "")
                                        .on_hover_text(if e.enabled {
                                            "Sobe com o PC — desmarque para tirar da partida"
                                        } else {
                                            "Não sobe — marque para voltar a subir com o PC"
                                        })
                                        .changed()
                                    {
                                        toggle = Some((e.clone(), on));
                                    }
                                } else {
                                    let mut dummy = e.enabled;
                                    ui.add_enabled(false, egui::Checkbox::new(&mut dummy, ""))
                                        .on_hover_text("Esta origem não se liga nem desliga por aqui");
                                }
                            });
                            row.col(|ui| {
                                ui.add_space(indent);
                                match &row_icons[i] {
                                    Some(tex) => {
                                        ui.add(egui::Image::new((tex.id(), egui::Vec2::splat(16.0))));
                                    }
                                    None => {
                                        let (r, _) = ui.allocate_exact_size(egui::Vec2::splat(16.0), egui::Sense::hover());
                                        ui.painter().circle_filled(r.center(), 4.0, e.kind.color().gamma_multiply(0.6));
                                    }
                                }
                                ui.add_space(4.0);
                                let name_c = if e.missing {
                                    Status::Broken.color()
                                } else if e.enabled {
                                    Color32::from_gray(225)
                                } else {
                                    MUTED
                                };
                                ui.add(egui::Label::new(RichText::new(&e.name).strong().color(name_c)).truncate());
                            });
                            row.col(|ui| {
                                ui.label(RichText::new(e.kind.label()).small().color(e.kind.color()))
                                    .on_hover_text(&e.origin);
                            });
                            row.col(|ui| {
                                ui.label(
                                    RichText::new(if e.machine { "máquina" } else { "usuário" })
                                        .small()
                                        .color(MUTED),
                                )
                                .on_hover_text(if e.machine {
                                    "Vale para todas as contas do PC — mexer pede admin"
                                } else {
                                    "Só para a sua conta"
                                });
                            });
                            row.col(|ui| {
                                ui.add(egui::Label::new(RichText::new(&e.command).monospace().small().color(MUTED)).truncate())
                                    .on_hover_text(&e.command);
                            });
                            row.col(|ui| {
                                let (txt, c) = if e.missing {
                                    ("ausente", Status::Broken.color())
                                } else if is_run {
                                    ("rodando", Color32::from_rgb(120, 200, 140))
                                } else {
                                    ("parado", MUTED)
                                };
                                ui.label(RichText::new(txt).small().color(c)).on_hover_text(if e.missing {
                                    "O arquivo apontado não existe mais"
                                } else if is_run {
                                    "Tem processo deste executável rodando agora"
                                } else {
                                    "Nenhum processo deste executável rodando agora"
                                });
                            });
                            row.col(|ui| {
                                let u = row_usage[i];
                                if u.total() == 0 {
                                    ui.label(RichText::new("—").small().color(MUTED));
                                } else {
                                    let strong = u.total() >= 3600;
                                    let c = if strong { Color32::from_rgb(120, 200, 140) } else { MUTED };
                                    ui.label(RichText::new(usage::fmt_secs(u.total())).small().color(c)).on_hover_text(format!(
                                        "Em foco: {}\nAberto (medido pelo RamDog): {}\nÚltima vez: {}",
                                        usage::fmt_secs(u.focus),
                                        usage::fmt_secs(u.open),
                                        usage::fmt_ago(u.last)
                                    ));
                                }
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
                        }
                    }
                });
            });

        if let Some(k) = toggle_group {
            if !self.collapsed.remove(&k) {
                self.collapsed.insert(k);
            }
        }
        }

        if let Some(k) = sort_click {
            if self.sort == k {
                self.sort_desc = !self.sort_desc;
            } else {
                self.sort = k;
                // Uso só interessa do maior para o menor; as outras leem melhor em ordem direta.
                self.sort_desc = k == SortKey::Usage;
            }
        }
        if let Some(id) = select {
            self.selected = Some(id);
        }
        if let Some((e, on)) = toggle {
            queued.push(Action::Toggle(e, on));
        }
        if let Some((exe, label)) = add {
            queued.push(Action::AddStartup { exe, label });
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
                        let st = Status::of(e);
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(&e.name).strong());
                            ui.label(
                                RichText::new(format!("{} {}", st.glyph(), st.title()))
                                    .color(st.color())
                                    .small()
                                    .strong(),
                            );
                            ui.label(RichText::new(e.kind.label()).color(e.kind.color()).small());
                            ui.label(RichText::new(&e.origin).weak().small());
                        });
                        ui.label(
                            RichText::new(format!("{} — {}", e.phase.title(), e.phase.hint()))
                                .color(e.phase.color())
                                .small(),
                        );
                        ui.add(egui::Label::new(RichText::new(&e.command).monospace().small()).wrap());
                        if !e.can_toggle {
                            ui.label(
                                RichText::new(
                                    "Só leitura: mexer nesta origem daqui derrubaria o logon ou o boot.",
                                )
                                .color(MUTED)
                                .small(),
                            );
                        }
                    });
            }
        }

        out
    }

    /// Faixa de presets: guarda o ligado/desligado de tudo que dá para alternar e devolve
    /// a máquina a esse estado depois.
    fn presets_bar(
        &mut self,
        ui: &mut egui::Ui,
        confirm: &mut Option<Pending>,
        cfg: &mut Config,
        out: &mut Vec<BootOut>,
    ) {
        ui.label(RichText::new("Presets:").small().color(MUTED));
        let names: Vec<String> = cfg.boot_presets.keys().cloned().collect();
        let shown = if self.preset_sel.is_empty() { "—".to_string() } else { self.preset_sel.clone() };
        egui::ComboBox::from_id_salt("boot_preset")
            .selected_text(RichText::new(shown).size(12.0))
            .width(140.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.preset_sel, String::new(), "—");
                for n in &names {
                    ui.selectable_value(&mut self.preset_sel, n.clone(), n);
                }
            });

        let has_sel = cfg.boot_presets.contains_key(&self.preset_sel);
        if ui
            .add_enabled(has_sel, egui::Button::new(RichText::new("Aplicar").small()))
            .on_hover_text("Liga e desliga as entradas até a partida ficar igual ao preset")
            .clicked()
        {
            let want = cfg.boot_presets.get(&self.preset_sel).cloned().unwrap_or_default();
            let diff = preset_diff(&self.entries, &want);
            if diff.is_empty() {
                out.push(BootOut::Toast(format!("{}: a partida já está assim", self.preset_sel), false));
            } else {
                let machine = diff.iter().filter(|(e, _)| e.machine).count();
                let mut lines: Vec<String> = diff
                    .iter()
                    .take(12)
                    .map(|(e, w)| format!("{} {}", if *w { "ligar" } else { "desligar" }, e.name))
                    .collect();
                if diff.len() > 12 {
                    lines.push(format!("… e mais {}", diff.len() - 12));
                }
                if machine > 0 {
                    lines.push(format!("{machine} entrada(s) de máquina — vai pedir UAC uma vez só."));
                }
                *confirm = Some(Pending {
                    title: format!("Aplicar preset \"{}\" — {} mudança(s)?", self.preset_sel, diff.len()),
                    lines,
                    action: Action::Preset(self.preset_sel.clone(), diff),
                });
            }
        }
        if ui
            .add_enabled(has_sel, egui::Button::new(RichText::new("Excluir").small().color(Color32::from_rgb(232, 120, 100))))
            .clicked()
        {
            cfg.boot_presets.remove(&self.preset_sel);
            out.push(BootOut::Toast(format!("preset \"{}\" excluído", self.preset_sel), false));
            self.preset_sel.clear();
            out.push(BootOut::SaveCfg);
        }

        ui.add_space(6.0);
        ui.add(
            egui::TextEdit::singleline(&mut self.preset_name)
                .hint_text("nome do preset")
                .desired_width(120.0),
        );
        let can_save = !self.preset_name.trim().is_empty() && !self.entries.is_empty();
        if ui
            .add_enabled(can_save, egui::Button::new(RichText::new("Salvar atual").small()))
            .on_hover_text("Guarda o estado ligado/desligado de todas as entradas que dá para alternar")
            .clicked()
        {
            let name = self.preset_name.trim().to_string();
            let snap: std::collections::BTreeMap<String, bool> = self
                .entries
                .iter()
                .filter(|e| e.can_toggle)
                .map(|e| (e.id.clone(), e.enabled))
                .collect();
            let n = snap.len();
            cfg.boot_presets.insert(name.clone(), snap);
            self.preset_sel = name.clone();
            self.preset_name.clear();
            out.push(BootOut::Toast(format!("preset \"{name}\" salvo com {n} entradas"), false));
            out.push(BootOut::SaveCfg);
        }
    }

    /// Painel do Scan: os programas mais usados, com ícone, e um botão para pôr na partida.
    fn scan_panel(&mut self, ui: &mut egui::Ui, add: &mut Option<(String, String)>) {
        // Quem já está na partida não precisa de sugestão — só de um selo.
        let already: HashSet<String> = self
            .resolved
            .values()
            .filter_map(|r| r.exe.clone())
            .collect();
        // `add_sized` centraliza o conteúdo na caixa; aqui o que se quer é coluna alinhada
        // à esquerda, senão cada linha começa num lugar diferente.
        fn cell(ui: &mut egui::Ui, w: f32, text: RichText) -> egui::Response {
            ui.allocate_ui_with_layout(
                egui::Vec2::new(w, 18.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    // Sem o mínimo a caixa encolhe até o texto e as colunas somem.
                    ui.set_min_width(w);
                    ui.add(egui::Label::new(text).truncate())
                },
            )
            .response
        }

        let top: Vec<usage::Ranked> = self.rank.iter().take(30).cloned().collect();
        let icons: Vec<Option<TextureHandle>> = top.iter().map(|r| self.icons.get(&r.path.to_lowercase())).collect();
        let mut close = false;

        egui::Frame::new()
            .fill(SURFACE)
            .stroke(egui::Stroke::new(1.0_f32, ACCENT.gamma_multiply(0.5)))
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("O que você mais usa neste PC").strong().size(14.0));
                    ui.label(
                        RichText::new("— tempo em foco (histórico do Windows) + tempo aberto medido pelo RamDog")
                            .small()
                            .color(MUTED),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("Fechar").clicked() {
                            close = true;
                        }
                    });
                });
                ui.add_space(4.0);
                if top.is_empty() {
                    ui.label(
                        RichText::new(
                            "Nada medido ainda. O histórico do Windows (UserAssist) pode estar limpo — \
                             deixe o RamDog aberto e a contagem própria começa a preencher a lista.",
                        )
                        .color(MUTED),
                    );
                    return;
                }
                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                    for (i, r) in top.iter().enumerate() {
                        let on_startup = already.contains(&r.path.to_lowercase());
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("{:>2}.", i + 1)).small().color(MUTED).monospace());
                            match &icons[i] {
                                Some(tex) => {
                                    ui.add(egui::Image::new((tex.id(), egui::Vec2::splat(20.0))));
                                }
                                None => {
                                    let (rc, _) = ui.allocate_exact_size(egui::Vec2::splat(20.0), egui::Sense::hover());
                                    ui.painter().circle_filled(rc.center(), 5.0, MUTED.gamma_multiply(0.5));
                                }
                            }
                            cell(ui, 200.0, RichText::new(&r.name).strong()).on_hover_text(&r.path);
                            cell(
                                ui,
                                92.0,
                                RichText::new(usage::fmt_secs(r.focus_secs + r.open_secs)).color(Color32::from_rgb(120, 200, 140)),
                            )
                            .on_hover_text(format!(
                                "Em foco: {}\nAberto com janela (medido pelo RamDog): {}",
                                usage::fmt_secs(r.focus_secs),
                                usage::fmt_secs(r.open_secs)
                            ));
                            cell(ui, 60.0, RichText::new(format!("{}×", r.launches)).small().color(MUTED))
                                .on_hover_text("Vezes que o programa foi aberto");
                            cell(ui, 96.0, RichText::new(usage::fmt_ago(r.last_used)).small().color(MUTED))
                                .on_hover_text("Última vez usado");
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if on_startup {
                                    ui.label(
                                        RichText::new("já na partida")
                                            .small()
                                            .color(Color32::from_rgb(120, 200, 140)),
                                    );
                                } else if ui
                                    .add(egui::Button::new(RichText::new("+ Partida").small().color(Color32::WHITE)).fill(ACCENT_BG))
                                    .on_hover_text("Cria um atalho na pasta Iniciar do seu usuário — sem UAC")
                                    .clicked()
                                {
                                    let label = std::path::Path::new(&r.path)
                                        .file_stem()
                                        .map(|s| s.to_string_lossy().into_owned())
                                        .unwrap_or_else(|| r.name.clone());
                                    *add = Some((r.path.clone(), label));
                                }
                            });
                        });
                    }
                });
            });
        if close {
            self.scan_open = false;
        }
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
            Action::Preset(name, list) => {
                let mut ok = 0usize;
                let mut failed: Vec<String> = Vec::new();
                // Tudo que precisa de admin vira UM script elevado: 20 prompts de UAC
                // seguidos seriam pior que não ter preset nenhum.
                let mut elevate: Vec<String> = Vec::new();
                for (e, want) in list {
                    match apply_toggle(&e, want) {
                        Ok(()) => ok += 1,
                        Err(err) if e.machine && !is_admin && err.contains("acesso negado") => {
                            let ps = toggle_ps(&e, want);
                            if ps.is_empty() {
                                failed.push(e.name.clone());
                            } else {
                                elevate.push(ps);
                            }
                        }
                        Err(_) => failed.push(e.name.clone()),
                    }
                }
                if !elevate.is_empty() {
                    self.busy += 1;
                    sys::run_elevated_ps(format!("preset {name}"), elevate.join("; "), self.tx.clone());
                }
                self.last_refresh = None;
                let mut msg = format!("preset \"{name}\": {ok} aplicada(s)");
                if !elevate.is_empty() {
                    msg.push_str(&format!(", {} via UAC", elevate.len()));
                }
                if !failed.is_empty() {
                    msg.push_str(&format!(", {} falharam", failed.len()));
                }
                out.push(BootOut::Toast(msg, !failed.is_empty()));
            }
            Action::AddStartup { exe, label } => match create_startup_lnk(&exe, &label) {
                Ok(name) => {
                    out.push(BootOut::Toast(format!("{name} agora sobe com o PC"), false));
                    self.last_refresh = None;
                }
                Err(err) => out.push(BootOut::Toast(format!("{label}: {err}"), true)),
            },
        }
    }
}

/// O que precisa mudar para a partida ficar igual ao preset: só entradas que dá para
/// alternar, que o preset conhece, e cujo estado atual difere do guardado.
fn preset_diff(entries: &[Entry], want: &std::collections::BTreeMap<String, bool>) -> Vec<(Entry, bool)> {
    entries
        .iter()
        .filter(|e| e.can_toggle)
        .filter_map(|e| want.get(&e.id).map(|w| (e.clone(), *w)))
        .filter(|(e, w)| e.enabled != *w)
        .collect()
}

/// Ordem de "quem está de pé primeiro": rodando, no boot, desligada, ausente.
fn state_rank(e: &Entry, is_run: bool) -> u8 {
    if is_run {
        0
    } else if e.missing {
        3
    } else if e.enabled {
        1
    } else {
        2
    }
}

/// De onde tirar ícone e uso de uma entrada.
fn resolve_entry(e: &Entry) -> Resolved {
    let exe = exe_path_from_cmd(&e.command);
    match &e.target {
        // Atalho com alvo quebrado ainda tem ícone próprio — melhor que uma bolinha.
        Target::Folder { path, .. } => Resolved {
            icon: exe.clone().or_else(|| Some(path.to_string_lossy().into_owned())),
            exe: exe.map(|p| p.to_lowercase()),
        },
        Target::Uwp { .. } => Resolved::default(),
        _ => Resolved { icon: exe.clone(), exe: exe.map(|p| p.to_lowercase()) },
    }
}

/// Caminho completo do executável de uma linha de comando, já com variáveis expandidas.
/// Devolve `None` quando o arquivo não existe — sem arquivo não há ícone nem uso.
fn exe_path_from_cmd(cmd: &str) -> Option<String> {
    let c = cmd.trim();
    if c.is_empty() {
        return None;
    }
    let raw = if let Some(rest) = c.strip_prefix('"') {
        rest.split('"').next().unwrap_or("")
    } else {
        // Serviço costuma vir como `\??\C:\...\x.sys` ou `C:\...\svc.exe -k algo`.
        c.split_whitespace().next().unwrap_or("")
    };
    let raw = raw.trim_start_matches(r"\??\");
    let full = expand_env(raw);
    let p = Path::new(&full);
    if p.is_file() {
        return Some(full);
    }
    // Sem extensão o Windows completa com .exe.
    if p.extension().is_none() {
        let with_exe = format!("{full}.exe");
        if Path::new(&with_exe).is_file() {
            return Some(with_exe);
        }
    }
    None
}

/// Expande `%VAR%` no meio de um caminho do registro.
fn expand_env(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) => {
                let name = &after[..end];
                match std::env::var(name) {
                    Ok(v) => out.push_str(&v),
                    Err(_) => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push('%');
                out.push_str(after);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Cria o atalho na pasta Iniciar do usuário. Sem registro, sem UAC: é só um arquivo.
fn create_startup_lnk(exe: &str, label: &str) -> Result<String, String> {
    ensure_com();
    let dir = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| "não achei %APPDATA%".to_string())?
        .join(r"Microsoft\Windows\Start Menu\Programs\Startup");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let safe: String = label
        .chars()
        .map(|c| if r#"\/:*?"<>|"#.contains(c) { '_' } else { c })
        .collect();
    let safe = safe.trim().to_string();
    if safe.is_empty() {
        return Err("nome de atalho vazio".into());
    }
    let file = dir.join(format!("{safe}.lnk"));
    if file.exists() {
        return Err(format!("{safe}.lnk já existe na pasta Iniciar"));
    }
    unsafe {
        let link: IShellLinkW =
            CoCreateInstance::<Option<&IUnknown>, IShellLinkW>(&ShellLink, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| e.message())?;
        let w = sys::wide(exe);
        link.SetPath(PCWSTR(w.as_ptr())).map_err(|e| e.message())?;
        if let Some(parent) = Path::new(exe).parent() {
            let wd = sys::wide(&parent.to_string_lossy());
            let _ = link.SetWorkingDirectory(PCWSTR(wd.as_ptr()));
        }
        let d = sys::wide("Adicionado à partida pelo RamDog");
        let _ = link.SetDescription(PCWSTR(d.as_ptr()));
        let persist: IPersistFile = link.cast().map_err(|e| e.message())?;
        let wf = sys::wide(&file.to_string_lossy());
        persist.Save(PCWSTR(wf.as_ptr()), true).map_err(|e| e.message())?;
    }
    // Sobra de StartupApproved com o mesmo nome deixaria o atalho novo nascer desligado.
    let _ = sys::reg_delete_value(HKEY_CURRENT_USER, APPROVED_FOLDER, &format!("{safe}.lnk"));
    Ok(safe)
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
                phase: Phase::Desktop,
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
                phase: Phase::Desktop,
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
                phase: Phase::Desktop,
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
            phase: Phase::Desktop,
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
            phase: if boot { Phase::Machine } else { Phase::Logon },
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
                phase: if kind == Kind::Driver { Phase::Kernel } else { Phase::Machine },
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
                phase: Phase::Desktop,
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
                phase: Phase::Logon,
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
                phase: Phase::Logon,
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
                phase: Phase::Kernel,
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
                phase: Phase::Logon,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expande_variavel_de_ambiente_no_meio_do_caminho() {
        let win = std::env::var("WINDIR").unwrap();
        assert_eq!(expand_env(r"%WINDIR%\explorer.exe"), format!(r"{win}\explorer.exe"));
        assert_eq!(expand_env(r"C:\sem\variavel.exe"), r"C:\sem\variavel.exe");
        // Variável que não existe fica como estava, em vez de virar caminho quebrado.
        assert_eq!(expand_env(r"%NAO_EXISTE_XYZ%\a.exe"), r"%NAO_EXISTE_XYZ%\a.exe");
    }

    #[test]
    fn tira_o_exe_da_linha_de_comando() {
        let win = std::env::var("WINDIR").unwrap();
        let explorer = format!(r"{win}\explorer.exe");
        assert_eq!(exe_path_from_cmd(&format!("\"{explorer}\" -algo")), Some(explorer.clone()));
        assert_eq!(exe_path_from_cmd(&format!(r"\??\{explorer}")), Some(explorer.clone()));
        assert_eq!(exe_path_from_cmd(r"%WINDIR%\explorer.exe /n"), Some(explorer));
        assert_eq!(exe_path_from_cmd(r"C:\nao\existe\nada.exe"), None);
        assert_eq!(exe_path_from_cmd(""), None);
    }

    /// Cria um atalho de verdade na pasta Iniciar, confere o alvo e apaga.
    /// `cargo test -- --ignored --nocapture atalho`
    #[test]
    #[ignore]
    fn atalho_na_pasta_iniciar_ida_e_volta() {
        let win = std::env::var("WINDIR").unwrap();
        let exe = format!(r"{win}\System32\notepad.exe");
        let label = "RamDogTesteAtalho";
        let name = create_startup_lnk(&exe, label).expect("criar atalho");
        let file = PathBuf::from(std::env::var("APPDATA").unwrap())
            .join(r"Microsoft\Windows\Start Menu\Programs\Startup")
            .join(format!("{name}.lnk"));
        assert!(file.exists(), "atalho não apareceu em {}", file.display());
        let alvo = resolve_lnk(&file).expect("ler alvo do atalho");
        println!("atalho {} -> {alvo}", file.display());
        assert!(alvo.to_lowercase().contains("notepad.exe"), "alvo errado: {alvo}");
        // Duas vezes o mesmo nome tem de falhar, não sobrescrever atalho do usuário.
        assert!(create_startup_lnk(&exe, label).is_err());
        std::fs::remove_file(&file).expect("limpar atalho de teste");
        assert!(!file.exists());
    }

    fn ent(id: &str, enabled: bool, can_toggle: bool) -> Entry {
        Entry {
            id: id.into(),
            name: id.into(),
            command: String::new(),
            kind: Kind::Run,
            phase: Phase::Desktop,
            machine: false,
            enabled,
            missing: false,
            can_toggle,
            can_remove: false,
            microsoft: false,
            origin: String::new(),
            target: Target::ReadOnly,
            running_hint: false,
        }
    }

    #[test]
    fn preset_so_lista_o_que_precisa_mudar() {
        let entries = vec![
            ent("igual-ligada", true, true),
            ent("precisa-desligar", true, true),
            ent("precisa-ligar", false, true),
            ent("travada", false, false),
            ent("fora-do-preset", false, true),
        ];
        let want: std::collections::BTreeMap<String, bool> = [
            ("igual-ligada", true),
            ("precisa-desligar", false),
            ("precisa-ligar", true),
            // Entrada que não dá para alternar não pode entrar no diff nem que o preset peça.
            ("travada", true),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();

        let d = preset_diff(&entries, &want);
        let mut got: Vec<(String, bool)> = d.iter().map(|(e, w)| (e.id.clone(), *w)).collect();
        got.sort();
        assert_eq!(got, vec![("precisa-desligar".to_string(), false), ("precisa-ligar".to_string(), true)]);

        // Preset igual à partida atual: nada a fazer — é o caso do aviso "já está assim".
        let mesmo: std::collections::BTreeMap<String, bool> =
            entries.iter().filter(|e| e.can_toggle).map(|e| (e.id.clone(), e.enabled)).collect();
        assert!(preset_diff(&entries, &mesmo).is_empty());
    }

    /// Contra a partida real desta máquina: tira um retrato, vira uma entrada de verdade
    /// no retrato e confere que o diff aponta exatamente ela.
    /// `cargo test -- --ignored --nocapture preset_real`
    #[test]
    #[ignore]
    fn preset_real_desta_maquina() {
        let entries = collect();
        let mut snap: std::collections::BTreeMap<String, bool> =
            entries.iter().filter(|e| e.can_toggle).map(|e| (e.id.clone(), e.enabled)).collect();
        println!("{} entradas, {} alternáveis", entries.len(), snap.len());
        assert!(snap.len() > 5, "partida real veio vazia demais para o teste valer");
        assert!(preset_diff(&entries, &snap).is_empty(), "retrato do agora tem de dar diff zero");

        let alvo = entries.iter().find(|e| e.can_toggle).unwrap().id.clone();
        let atual = snap[&alvo];
        snap.insert(alvo.clone(), !atual);
        let d = preset_diff(&entries, &snap);
        assert_eq!(d.len(), 1, "diff devia ter só a entrada virada");
        assert_eq!(d[0].0.id, alvo);
        assert_eq!(d[0].1, !atual);
        println!("diff = {} -> {}", d[0].0.name, if d[0].1 { "ligar" } else { "desligar" });
    }
}
