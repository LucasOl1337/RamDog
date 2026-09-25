//! Visão "Faxina": acha o que está aberto sem uso, separa em "pode fechar", "talvez" e
//! "em uso", e fecha de uma vez o que o usuário deixar marcado.
//!
//! A unidade é a **instância**: o processo raiz e os descendentes com a mesma identidade
//! (o Brave com seus renderers, um Codex com os helpers do Electron). Cada sessão de agente
//! vira uma linha própria, porque o pai dela é um shell ou terminal com outra identidade.
//!
//! O uso sai de observação, não de palpite: a cada amostra o RamDog anota quando cada PID
//! gastou CPU, disco ou GPU e quando teve foco. "Parado há 3h" quer dizer que o RamDog viu
//! 3h de silêncio; com o app recém-aberto, nada tem tempo de virar candidato.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use egui::{Align, Color32, Layout, RichText};
use serde::{Deserialize, Serialize};

use crate::app::{fmt_age, fmt_bytes, fmt_bytes_short, MUTED};
use crate::categories::Category;
use crate::config::Locale;
use crate::identity::{self, Kind};
use crate::procs::ProcInfo;

pub enum SweepOut {
    Kill(Vec<u32>),
    Toast(String),
}

/// CPU (todos os núcleos = 100%) acima disso conta como uso. 0,5% de 16 núcleos é ~8% de
/// um núcleo: um Node parado esperando entrada fica abaixo, um build ou um vídeo fica acima.
const BUSY_CPU: f32 = 0.5;
const BUSY_DISK: f64 = 512.0 * 1024.0;
const BUSY_GPU: f32 = 2.0;
/// Uso ou foco mais recente que isso: em uso, fora da faxina.
const RECENT_BUSY: u64 = 10 * 60;
const RECENT_FOCUS: u64 = 20 * 60;
/// Sem janela e sem atividade por esse tempo: talvez.
const IDLE_HIDDEN: u64 = 30 * 60;
/// Janela aberta sem foco por esse tempo: talvez.
const UNFOCUSED: u64 = 2 * 3600;
/// Lançado por um agente que já saiu e parado desde então: pode fechar.
const ORPHAN_IDLE: u64 = 10 * 60;
/// Segundos entre gravações do sweep.json.
const SAVE_SECS: u64 = 60;
/// Restart mais longo que isso descarta o que foi visto: com o RamDog fechado ninguém
/// olhou, e contar a lacuna como silêncio inventaria "parado há".
const MAX_GAP: i64 = 15 * 60;

/// Intermediários que não contam como "o app pai": o shell entre o terminal e o agente.
const THIN: &[&str] = &[
    "bash",
    "zsh",
    "fish",
    "sh",
    "dash",
    "sudo",
    "env",
    "nohup",
    "setsid",
    "uwsm-app",
    "systemd-run",
    "script",
    "timeout",
    "npm",
    "npx",
    "pnpm",
    "uv",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Close,
    Maybe,
    Keep,
}

/// Por que a linha caiu onde caiu. Formatado só na hora de desenhar, no idioma da vez.
#[derive(Clone, Debug, PartialEq)]
pub enum Why {
    Leftover(&'static str, &'static str),
    AgentGone { agent: String, idle: u64 },
    IdleHidden(u64),
    Unfocused(u64),
    Busy(u64),
    Focused(u64),
    HelperOf(String),
    Service(String),
    Watching(u64),
    Pinned,
}

impl Why {
    fn text(&self, locale: Locale) -> String {
        let en = locale == Locale::English;
        match self {
            Why::Leftover(pt, e) => (if en { e } else { pt }).to_string(),
            Why::AgentGone { agent, idle } => {
                if en {
                    format!(
                        "started by {agent}, which already exited; idle for {}",
                        fmt_age(*idle)
                    )
                } else {
                    format!(
                        "aberto por um {agent} que já saiu; parado há {}",
                        fmt_age(*idle)
                    )
                }
            }
            Why::IdleHidden(s) => {
                if en {
                    format!("no window and idle for {}", fmt_age(*s))
                } else {
                    format!("sem janela e parado há {}", fmt_age(*s))
                }
            }
            Why::Unfocused(s) => {
                if en {
                    format!("window not focused for {}", fmt_age(*s))
                } else {
                    format!("janela sem foco há {}", fmt_age(*s))
                }
            }
            Why::Busy(s) if *s < 60 => locale.text("trabalhando agora", "working now").into(),
            Why::Busy(s) => {
                if en {
                    format!("last activity {} ago", fmt_age(*s))
                } else {
                    format!("última atividade há {}", fmt_age(*s))
                }
            }
            Why::Focused(s) if *s < 60 => locale.text("em foco", "focused").into(),
            Why::Focused(s) => {
                if en {
                    format!("focused {} ago", fmt_age(*s))
                } else {
                    format!("em foco há {}", fmt_age(*s))
                }
            }
            Why::HelperOf(parent) => {
                if en {
                    format!("helper of {parent}, which is in use")
                } else {
                    format!("auxiliar de {parent}, que está em uso")
                }
            }
            Why::Service(unit) => {
                if en {
                    format!("systemd service ({unit}); stop it in Drains")
                } else {
                    format!("serviço do systemd ({unit}); pare pelo Desperdício")
                }
            }
            Why::Pinned => locale
                .text(
                    "você marcou pra manter sempre",
                    "you chose to always keep it",
                )
                .into(),
            Why::Watching(s) => {
                if en {
                    format!("observed for only {}", fmt_age(*s))
                } else {
                    format!("observado só há {}", fmt_age(*s))
                }
            }
        }
    }
}

/// O que o RamDog viu de um PID até agora, em segundos.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Act {
    /// Desde a última vez que gastou CPU, disco ou GPU (ou desde que foi visto).
    pub idle: u64,
    /// Desde o último foco; `None` = nunca teve foco enquanto o RamDog olhava.
    pub unfocused: Option<u64>,
    /// Há quanto tempo o RamDog o vê.
    pub observed: u64,
}

#[derive(Clone, Debug)]
pub struct Row {
    pub key: String,
    /// Identidade do app (`project:sussurro`, `app:claude`): o que "manter sempre" guarda.
    pub ident: String,
    #[cfg_attr(not(test), allow(dead_code))]
    pub pid: u32,
    pub label: String,
    pub detail: String,
    pub tier: Tier,
    pub why: Why,
    /// RAM do que sai se fechar: a instância e tudo abaixo dela.
    pub ram: u64,
    /// PIDs que o fechamento sinaliza (sem os protegidos), com a RAM de cada um.
    pub kill: Vec<(u32, u64)>,
    /// Quantos deles estão abaixo da instância, com outra identidade.
    pub children: usize,
    pub age: u64,
}

/// Em segundos Unix, pra sobreviver a um restart do RamDog (atualização, crash).
#[derive(Clone, Serialize, Deserialize)]
struct Seen {
    created: i64,
    first: i64,
    busy: i64,
    focus: Option<i64>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Store {
    saved_at: i64,
    since: i64,
    seen: HashMap<u32, Seen>,
    /// Identidades que o usuário mandou manter sempre. Não expira com a lacuna.
    keep: BTreeSet<String>,
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn store_path() -> std::path::PathBuf {
    crate::config::config_path().with_file_name("sweep.json")
}

pub struct Sweep {
    seen: HashMap<u32, Seen>,
    keep: BTreeSet<String>,
    /// As primeiras amostras chegam antes do cache de janelas do compositor: tudo pareceria
    /// sem janela. Espera um pouco antes de classificar.
    born: Instant,
    /// Desde quando o RamDog observa, em segundos Unix.
    since: i64,
    last_save: Instant,
    rows: Vec<Row>,
    protected: usize,
    /// Marcações feitas à mão, por chave de linha. Sem entrada = o padrão do grupo
    /// ("pode fechar" já vem marcado).
    picked: HashMap<String, bool>,
    min_ram: u64,
    search: String,
    confirm: bool,
}

impl Default for Sweep {
    fn default() -> Self {
        Self::new()
    }
}

fn is_busy(p: &ProcInfo) -> bool {
    p.cpu_pct + p.cpu_children_pct >= BUSY_CPU || p.disk_bps >= BUSY_DISK || p.gpu_pct >= BUSY_GPU
}

fn base_name(p: &ProcInfo) -> &str {
    p.name_lower.strip_suffix(".exe").unwrap_or(&p.name_lower)
}

fn is_service(p: &ProcInfo) -> Option<String> {
    let unit = p.launcher.unit.as_deref()?;
    if unit.ends_with(".service") && !unit.starts_with("app-") {
        Some(p.launcher.unit_label().unwrap_or_else(|| unit.to_string()))
    } else {
        None
    }
}

/// Pasta de trabalho, pra dizer qual sessão de agente é qual.
fn cwd_of(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let path = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        let shown = match home.as_deref().and_then(|h| path.strip_prefix(h).ok()) {
            Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
            Some(rest) => format!("~/{}", rest.display()),
            None => path.display().to_string(),
        };
        Some(shown)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

/// Nome do programa: o script quando o binário é um runtime (`node …/chrome-devtools-axi/…`).
fn program_label(p: &ProcInfo) -> String {
    let base = |s: &str| s.rsplit('/').next().unwrap_or(s).to_string();
    let exe = base(&p.exe_path);
    let exe = if exe.is_empty() { p.name.clone() } else { exe };
    let runtime = ["node", "bun", "deno", "python", "python3", "java"]
        .iter()
        .any(|r| exe == *r || exe.starts_with(&format!("{r}.")));
    if runtime {
        let script = p
            .cmdline
            .split_whitespace()
            .skip(1)
            .find(|a| !a.starts_with('-'));
        if let Some(script) = script {
            if let Some((_, rest)) = script.rsplit_once("node_modules/") {
                let pkg = if rest.starts_with('@') {
                    rest.splitn(3, '/').take(2).collect::<Vec<_>>().join("/")
                } else {
                    rest.split('/').next().unwrap_or(rest).to_string()
                };
                return pkg;
            }
            return format!("{exe} {}", base(script));
        }
    }
    exe
}

struct Inst {
    root: usize,
    members: Vec<usize>,
    kind: Kind,
    label: String,
    protected: bool,
    thin: bool,
    /// Instâncias filhas (índices em `insts`).
    kids: Vec<usize>,
    parent: Option<usize>,
    idle: u64,
    unfocused: Option<u64>,
    observed: u64,
    window: bool,
}

/// Classifica as instâncias. Função pura: o `Sweep` só fornece a atividade observada.
pub fn classify(
    procs: &[ProcInfo],
    act: &dyn Fn(&ProcInfo) -> Act,
    mem: &dyn Fn(&ProcInfo) -> u64,
    locked: &dyn Fn(&ProcInfo) -> bool,
    cat: &dyn Fn(u32) -> Category,
    keep: &BTreeSet<String>,
    now_ft: i64,
) -> (Vec<Row>, usize) {
    let by_pid: HashMap<u32, usize> = procs.iter().enumerate().map(|(i, p)| (p.pid, i)).collect();
    let ids: Vec<identity::Identity> = procs.iter().map(identity::of).collect();

    // Raiz de cada processo: sobe enquanto o pai tem a mesma identidade.
    let root_of = |mut i: usize| -> usize {
        for _ in 0..64 {
            let Some(&pi) = by_pid.get(&procs[i].ppid) else {
                break;
            };
            if procs[i].ppid == 0 || ids[pi].key != ids[i].key {
                break;
            }
            i = pi;
        }
        i
    };
    let mut inst_of: Vec<usize> = vec![usize::MAX; procs.len()];
    let mut insts: Vec<Inst> = Vec::new();
    let mut by_root: HashMap<usize, usize> = HashMap::new();
    for i in 0..procs.len() {
        let r = root_of(i);
        let n = *by_root.entry(r).or_insert_with(|| {
            let rp = &procs[r];
            insts.push(Inst {
                root: r,
                members: Vec::new(),
                kind: ids[r].kind,
                label: ids[r].label.clone(),
                protected: rp.pid <= 2 || cat(rp.pid) == Category::System,
                thin: THIN.contains(&base_name(rp)),
                kids: Vec::new(),
                parent: None,
                idle: u64::MAX,
                unfocused: None,
                observed: 0,
                window: false,
            });
            insts.len() - 1
        });
        inst_of[i] = n;
        let p = &procs[i];
        let a = act(p);
        let it = &mut insts[n];
        it.members.push(i);
        it.protected |= locked(p);
        it.idle = it.idle.min(a.idle);
        it.unfocused = match (it.unfocused, a.unfocused) {
            (Some(x), Some(y)) => Some(x.min(y)),
            (x, y) => x.or(y),
        };
        it.observed = it.observed.max(a.observed);
        it.window |= p.has_window;
    }
    for n in 0..insts.len() {
        let rp = &procs[insts[n].root];
        if let Some(&pi) = by_pid.get(&rp.ppid).filter(|_| rp.ppid != 0) {
            let parent = inst_of[pi];
            if parent != n {
                insts[n].parent = Some(parent);
                insts[parent].kids.push(n);
            }
        }
    }

    // Uso da subárvore: um terminal com um build rodando dentro está em uso.
    let mut sub_idle = vec![u64::MAX; insts.len()];
    let mut sub_unfocused: Vec<Option<u64>> = vec![None; insts.len()];
    fn walk(
        n: usize,
        insts: &[Inst],
        idle: &mut [u64],
        unf: &mut [Option<u64>],
        depth: usize,
    ) -> (u64, Option<u64>) {
        if idle[n] != u64::MAX {
            return (idle[n], unf[n]);
        }
        let mut i = insts[n].idle;
        let mut u = insts[n].unfocused;
        if depth < 64 {
            for &k in &insts[n].kids {
                let (ki, ku) = walk(k, insts, idle, unf, depth + 1);
                i = i.min(ki);
                u = match (u, ku) {
                    (Some(x), Some(y)) => Some(x.min(y)),
                    (x, y) => x.or(y),
                };
            }
        }
        idle[n] = i;
        unf[n] = u;
        (i, u)
    }
    for n in 0..insts.len() {
        walk(n, &insts, &mut sub_idle, &mut sub_unfocused, 0);
    }

    // O app de verdade acima da instância, pulando shells.
    let real_parent = |n: usize| -> Option<usize> {
        let mut cur = insts[n].parent;
        for _ in 0..32 {
            let c = cur?;
            if insts[c].protected {
                return None;
            }
            if !insts[c].thin {
                return Some(c);
            }
            cur = insts[c].parent;
        }
        None
    };
    let recently_used = |n: usize| {
        insts[n].idle < RECENT_BUSY || insts[n].unfocused.is_some_and(|u| u < RECENT_FOCUS)
    };

    let mut verdict: Vec<Option<(Tier, Why)>> = vec![None; insts.len()];
    let mut protected = 0;
    for n in 0..insts.len() {
        let it = &insts[n];
        let rp = &procs[it.root];
        if it.protected {
            protected += 1;
            continue;
        }
        let members = || it.members.iter().map(|&i| &procs[i]);
        if members().all(|p| p.kernel_state == Some('Z')) {
            continue;
        }
        let leftover = members().find_map(|p| {
            if p.kernel_state == Some('Z') {
                return None;
            }
            let pt = identity::leftover_reason_for(
                &p.cmdline,
                p.kernel_state,
                p.has_window,
                Locale::Portuguese,
            )?;
            let en = identity::leftover_reason_for(
                &p.cmdline,
                p.kernel_state,
                p.has_window,
                Locale::English,
            )?;
            Some((pt, en))
        });
        let unfocused = sub_unfocused[n].unwrap_or(it.observed);
        let v = if keep.contains(&ids[it.root].key) {
            (Tier::Keep, Why::Pinned)
        } else if let Some((pt, en)) = leftover {
            (Tier::Close, Why::Leftover(pt, en))
        } else if sub_idle[n] < RECENT_BUSY {
            (Tier::Keep, Why::Busy(sub_idle[n]))
        } else if unfocused < RECENT_FOCUS && sub_unfocused[n].is_some() {
            (Tier::Keep, Why::Focused(unfocused))
        } else if let Some(p) =
            real_parent(n).filter(|&p| it.kind != Kind::Agent && recently_used(p))
        {
            (Tier::Keep, Why::HelperOf(insts[p].label.clone()))
        } else if let Some(unit) = is_service(rp) {
            (Tier::Keep, Why::Service(unit))
        } else if let Some(agent) = rp
            .launcher
            .agent
            .clone()
            .filter(|_| {
                rp.launcher
                    .agent_pid
                    .is_some_and(|a| a != rp.pid && !by_pid.contains_key(&a))
            })
            // Sessão de agente aberta de dentro de outra herda o CLAUDE_PID dela; o pai sair
            // não faz dela uma sobra.
            .filter(|_| !it.window && !identity::is_agent_cli(rp) && sub_idle[n] >= ORPHAN_IDLE)
        {
            (
                Tier::Close,
                Why::AgentGone {
                    agent,
                    idle: sub_idle[n],
                },
            )
        } else if it.window {
            if unfocused >= UNFOCUSED {
                (Tier::Maybe, Why::Unfocused(unfocused))
            } else if it.observed < UNFOCUSED {
                (Tier::Keep, Why::Watching(it.observed))
            } else {
                (Tier::Keep, Why::Focused(unfocused))
            }
        } else if sub_idle[n] >= IDLE_HIDDEN {
            (Tier::Maybe, Why::IdleHidden(sub_idle[n]))
        } else if it.observed < IDLE_HIDDEN {
            (Tier::Keep, Why::Watching(it.observed))
        } else {
            (Tier::Keep, Why::Busy(sub_idle[n]))
        };
        verdict[n] = Some(v);
    }

    // Candidato dentro de candidato vai junto com o de cima, em vez de virar linha própria:
    // fechar uma sessão parada já leva os MCPs dela.
    let is_cand = |n: usize| matches!(verdict[n], Some((Tier::Close | Tier::Maybe, _)));
    let mut folded = vec![false; insts.len()];
    for n in 0..insts.len() {
        if !is_cand(n) {
            continue;
        }
        let mut cur = insts[n].parent;
        for _ in 0..64 {
            let Some(c) = cur else { break };
            if insts[c].protected {
                break;
            }
            if is_cand(c) {
                folded[n] = true;
                break;
            }
            cur = insts[c].parent;
        }
    }

    let mut rows = Vec::new();
    for n in 0..insts.len() {
        let Some((tier, why)) = verdict[n].clone() else {
            continue;
        };
        let it = &insts[n];
        if folded[n] || (it.thin && tier != Tier::Close) {
            continue;
        }
        // Tudo abaixo da instância, sem os protegidos.
        let mut kill = Vec::new();
        let mut stack = vec![n];
        let mut guard = 0;
        while let Some(c) = stack.pop() {
            guard += 1;
            if guard > 4096 {
                break;
            }
            if insts[c].protected {
                continue;
            }
            kill.extend(
                insts[c]
                    .members
                    .iter()
                    .map(|&i| (procs[i].pid, mem(&procs[i]))),
            );
            stack.extend(insts[c].kids.iter().copied());
        }
        let ram: u64 = kill.iter().map(|(_, m)| m).sum();
        let rp = &procs[it.root];
        let own = it.members.len();
        let mut detail = format!("PID {}", rp.pid);
        if own > 1 {
            detail.push_str(&format!(" +{}", own - 1));
        }
        let title = it
            .members
            .iter()
            .find_map(|&i| procs[i].window_title.as_deref())
            .filter(|t| !t.is_empty() && *t != it.label);
        if let Some(t) = title {
            detail.push_str(&format!(" · {t}"));
        } else if tier != Tier::Keep && it.kind == Kind::Agent {
            if let Some(dir) = cwd_of(rp.pid) {
                detail.push_str(&format!(" · {dir}"));
            }
        }
        let who = rp.launcher.short();
        if !who.is_empty() {
            detail.push_str(&format!(" · {who}"));
        }
        // O que um agente abriu herda a identidade dele ("Claude"); na faxina o que importa
        // é o programa (o daemon do adb, um servidor MCP).
        let spawned = rp.launcher.agent_pid.is_some_and(|a| a != rp.pid);
        let label = if it.kind == Kind::Agent && spawned && !identity::is_agent_cli(rp) {
            program_label(rp)
        } else {
            it.label.clone()
        };
        rows.push(Row {
            key: format!("{}:{}", rp.pid, rp.create_time),
            ident: ids[it.root].key.clone(),
            pid: rp.pid,
            label,
            detail,
            tier,
            why,
            ram,
            children: kill.len().saturating_sub(own),
            kill,
            age: ((now_ft - rp.create_time).max(0) / 10_000_000) as u64,
        });
    }
    rows.sort_by_key(|r| std::cmp::Reverse(r.ram));
    (rows, protected)
}

impl Sweep {
    pub fn new() -> Self {
        let now = unix_now();
        let mut store = std::fs::read_to_string(store_path())
            .ok()
            .and_then(|s| serde_json::from_str::<Store>(&s).ok())
            .unwrap_or_default();
        if !(store.since > 0 && (0..=MAX_GAP).contains(&(now - store.saved_at))) {
            store.seen.clear();
            store.since = now;
        }
        Self {
            seen: store.seen,
            keep: store.keep,
            born: Instant::now(),
            since: store.since,
            last_save: Instant::now(),
            rows: Vec::new(),
            protected: 0,
            picked: HashMap::new(),
            min_ram: 50 << 20,
            search: String::new(),
            confirm: false,
        }
    }

    /// Chamado a cada amostra, com a aba aberta ou não: é o que mede "parado há".
    pub fn observe(
        &mut self,
        procs: &[ProcInfo],
        mem: &dyn Fn(&ProcInfo) -> u64,
        locked: &dyn Fn(&ProcInfo) -> bool,
        cat: &dyn Fn(u32) -> Category,
    ) {
        let now = unix_now();
        let live: HashSet<u32> = procs.iter().map(|p| p.pid).collect();
        self.seen.retain(|pid, _| live.contains(pid));
        for p in procs {
            let s = self.seen.entry(p.pid).or_insert(Seen {
                created: p.create_time,
                first: now,
                busy: now,
                focus: None,
            });
            if s.created != p.create_time {
                *s = Seen {
                    created: p.create_time,
                    first: now,
                    busy: now,
                    focus: None,
                };
            }
            if is_busy(p) {
                s.busy = now;
            }
            if p.focused {
                s.focus = Some(now);
            }
        }
        let seen = &self.seen;
        let now_ft = crate::procs::now_filetime();
        let act = |p: &ProcInfo| -> Act {
            // Nada fica parado há mais tempo do que existe.
            let age = ((now_ft - p.create_time).max(0) / 10_000_000) as u64;
            let ago = |t: i64| ((now - t).max(0) as u64).min(age);
            match seen.get(&p.pid) {
                Some(s) => Act {
                    idle: ago(s.busy),
                    unfocused: s.focus.map(ago),
                    observed: ago(s.first),
                },
                None => Act::default(),
            }
        };
        if self.born.elapsed().as_secs() < 6 {
            return;
        }
        let (rows, protected) = classify(procs, &act, mem, locked, cat, &self.keep, now_ft);
        self.rows = rows;
        self.protected = protected;
        let alive: HashSet<&str> = self.rows.iter().map(|r| r.key.as_str()).collect();
        self.picked.retain(|k, _| alive.contains(k.as_str()));
        if self.last_save.elapsed().as_secs() >= SAVE_SECS {
            self.last_save = Instant::now();
            self.save(now);
        }
    }

    fn save(&self, now: i64) {
        let path = store_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let store = Store {
            saved_at: now,
            since: self.since,
            seen: self.seen.clone(),
            keep: self.keep.clone(),
        };
        if let Ok(s) = serde_json::to_string(&store) {
            let _ = std::fs::write(&path, s);
        }
    }

    fn is_picked(&self, r: &Row) -> bool {
        self.picked
            .get(&r.key)
            .copied()
            .unwrap_or(r.tier == Tier::Close)
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, locale: Locale) -> Vec<SweepOut> {
        let mut out = Vec::new();
        let en = locale == Locale::English;
        crate::kit::intro(ui, locale.text(
            "O RamDog observa CPU, disco, GPU e foco de cada app e separa o que dá pra fechar. \"Pode fechar\" já vem marcado; desmarque o que quiser manter, marque o que quiser a mais e feche tudo de uma vez.",
            "RamDog watches each app's CPU, disk, GPU, and focus and sorts out what can be closed. \"Can close\" comes pre-selected; uncheck what you want to keep, check anything else, and close it all at once.",
        ));
        ui.add_space(6.0);

        let needle = self.search.trim().to_lowercase();
        let min_ram = self.min_ram;
        let visible = move |r: &Row| {
            (r.tier == Tier::Close || r.ram >= min_ram)
                && (needle.is_empty()
                    || r.label.to_lowercase().contains(&needle)
                    || r.detail.to_lowercase().contains(&needle))
        };
        // Linhas de "em uso" podem se sobrepor (terminal e o agente dentro dele): soma por
        // PID, não por linha.
        let mut picked: Vec<String> = Vec::new();
        let mut kill: Vec<u32> = Vec::new();
        let mut ram = 0u64;
        let mut seen = HashSet::new();
        for r in self.rows.iter().filter(|r| visible(r) && self.is_picked(r)) {
            picked.push(r.key.clone());
            for &(pid, m) in &r.kill {
                if seen.insert(pid) {
                    kill.push(pid);
                    ram += m;
                }
            }
        }
        let n_apps = picked.len();

        crate::kit::toolbar(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text(locale.text("Filtrar…", "Filter…"))
                    .desired_width(180.0),
            );
            ui.label(
                RichText::new(locale.text("acima de", "above"))
                    .size(12.5)
                    .color(MUTED),
            );
            let mut mb = (self.min_ram >> 20) as u32;
            if ui
                .add(
                    egui::DragValue::new(&mut mb)
                        .range(0..=8192)
                        .suffix(" MB")
                        .speed(10),
                )
                .changed()
            {
                self.min_ram = (mb as u64) << 20;
            }
            {
                let watched = (unix_now() - self.since).max(0) as u64;
                ui.label(crate::kit::muted(&if en {
                    format!("watching for {}", fmt_age(watched))
                } else {
                    format!("observando há {}", fmt_age(watched))
                }))
                .on_hover_text(locale.text(
                    "\"Parado há\" e \"sem foco há\" contam desde que o RamDog começou a olhar. Com ele aberto há pouco, quase tudo fica em uso.",
                    "\"Idle for\" and \"not focused for\" count from when RamDog started watching. Shortly after launch, almost everything stays in use.",
                ));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.confirm && !kill.is_empty() {
                    if ui.add(crate::kit::button(locale.text("Cancelar", "Cancel"))).clicked() {
                        self.confirm = false;
                    }
                    let label = if en {
                        format!("Confirm: close {n_apps} app(s)")
                    } else {
                        format!("Confirmar: fechar {n_apps} app(s)")
                    };
                    if ui.add(crate::kit::danger(&label)).clicked() {
                        out.push(SweepOut::Kill(kill.clone()));
                        for k in &picked {
                            self.picked.remove(k);
                        }
                        self.confirm = false;
                    }
                } else {
                    self.confirm = false;
                    let label = if kill.is_empty() {
                        locale.text("Nada marcado", "Nothing selected").to_string()
                    } else if en {
                        format!("Close {n_apps} app(s) · {} processes · {}", kill.len(), fmt_bytes(ram))
                    } else {
                        format!("Fechar {n_apps} app(s) · {} processos · {}", kill.len(), fmt_bytes(ram))
                    };
                    if ui
                        .add_enabled(!kill.is_empty(), crate::kit::primary(&label))
                        .on_hover_text(locale.text(
                            "Encerra cada app marcado com tudo que roda abaixo dele. Protegidos e o sistema ficam de fora.",
                            "Terminates each selected app with everything running below it. Protected and system processes are skipped.",
                        ))
                        .clicked()
                    {
                        self.confirm = true;
                    }
                }
            });
        });
        ui.add_space(6.0);

        if self.born.elapsed().as_secs() < 6 {
            ui.label(crate::kit::muted(locale.text(
                "lendo processos e janelas…",
                "reading processes and windows…",
            )));
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs(1));
            return out;
        }
        let rows: Vec<Row> = self.rows.iter().filter(|r| visible(r)).cloned().collect();
        let mut toast: Option<String> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for tier in [Tier::Close, Tier::Maybe, Tier::Keep] {
                    let group: Vec<&Row> = rows.iter().filter(|r| r.tier == tier).collect();
                    let (title, color, hint) = match tier {
                        Tier::Close => (
                            locale.text("Pode fechar", "Can close"),
                            Color32::from_rgb(230, 120, 120),
                            locale.text("Sobras com evidência: emulador escondido, processo de um agente que já saiu.", "Leftovers with evidence: hidden emulator, process from an agent that already exited."),
                        ),
                        Tier::Maybe => (
                            locale.text("Talvez", "Maybe"),
                            Color32::from_rgb(230, 170, 90),
                            locale.text("Parados há tempo ou com janela esquecida. Não vêm marcados.", "Idle for a while or with a forgotten window. Not pre-selected."),
                        ),
                        Tier::Keep => (
                            locale.text("Em uso", "In use"),
                            Color32::from_rgb(120, 200, 140),
                            locale.text("Atividade ou foco recente, serviço do systemd ou auxiliar de app em uso.", "Recent activity or focus, a systemd service, or a helper of an app in use."),
                        ),
                    };
                    let mut counted = HashSet::new();
                    let group_ram: u64 = group
                        .iter()
                        .flat_map(|r| r.kill.iter())
                        .filter(|(pid, _)| counted.insert(*pid))
                        .map(|(_, m)| m)
                        .sum();
                    let head = format!("{title} · {} · {}", group.len(), fmt_bytes_short(group_ram));
                    let mut body = |ui: &mut egui::Ui, this: &mut Sweep| {
                        ui.horizontal(|ui| {
                            ui.label(crate::kit::muted(hint));
                            if !group.is_empty() {
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if ui.small_button(locale.text("desmarcar todos", "clear all")).clicked() {
                                        for r in &group {
                                            this.picked.insert(r.key.clone(), false);
                                        }
                                    }
                                    if ui.small_button(locale.text("marcar todos", "select all")).clicked() {
                                        for r in &group {
                                            this.picked.insert(r.key.clone(), true);
                                        }
                                    }
                                });
                            }
                        });
                        ui.add_space(4.0);
                        if group.is_empty() {
                            ui.label(crate::kit::muted(locale.text("nada aqui agora", "nothing here right now")));
                        }
                        ui.spacing_mut().item_spacing.y = 4.0;
                        for r in &group {
                            this.ui_row(ui, r, color, locale, &mut toast);
                        }
                    };
                    if tier == Tier::Keep {
                        egui::CollapsingHeader::new(RichText::new(head).strong().color(color))
                            .id_salt("sweep-keep")
                            .default_open(false)
                            .show(ui, |ui| body(ui, self));
                    } else {
                        ui.label(RichText::new(head).strong().size(14.0).color(color));
                        body(ui, self);
                    }
                    ui.add_space(12.0);
                }
                if self.protected > 0 {
                    ui.label(crate::kit::muted(&if en {
                        format!("{} protected or system instances are never listed.", self.protected)
                    } else {
                        format!("{} instâncias protegidas ou do sistema nunca entram na lista.", self.protected)
                    }));
                }
            });
        if let Some(t) = toast {
            out.push(SweepOut::Toast(t));
        }
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs(1));
        out
    }

    fn ui_row(
        &mut self,
        ui: &mut egui::Ui,
        r: &Row,
        color: Color32,
        locale: Locale,
        toast: &mut Option<String>,
    ) {
        let mut on = self.is_picked(r);
        crate::kit::row(ui, |ui| {
            ui.horizontal(|ui| {
                ui.scope(|ui| {
                    ui.set_max_width((ui.available_width() - 190.0).max(200.0));
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        let label = RichText::new(&r.label).size(13.0).strong();
                        if ui.checkbox(&mut on, label).changed() {
                            self.picked.insert(r.key.clone(), on);
                        }
                        ui.horizontal(|ui| {
                            ui.add_space(26.0);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(r.why.text(locale)).size(11.5).color(color),
                                )
                                .truncate(),
                            );
                        });
                        let detail = match (r.children, locale) {
                            (0, _) => r.detail.clone(),
                            (n, Locale::English) => {
                                format!("{} · takes {n} child process(es)", r.detail)
                            }
                            (n, _) => format!("{} · leva {n} filho(s) junto", r.detail),
                        };
                        ui.horizontal(|ui| {
                            ui.add_space(26.0);
                            ui.add(egui::Label::new(crate::kit::muted(&detail)).truncate())
                                .on_hover_text(&detail);
                        });
                    });
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_sized(
                        [80.0, 18.0],
                        egui::Label::new(crate::kit::muted(&if locale == Locale::English {
                            format!("up {}", fmt_age(r.age))
                        } else {
                            format!("aberto {}", fmt_age(r.age))
                        })),
                    );
                    ui.add_sized(
                        [80.0, 18.0],
                        egui::Label::new(
                            RichText::new(fmt_bytes_short(r.ram)).monospace().size(12.5),
                        ),
                    )
                    .on_hover_text(locale.text(
                        "RAM do app e de tudo que roda abaixo dele: o que sai se fechar.",
                        "RAM of the app and everything below it: what goes away if you close it.",
                    ));
                    let pinned = r.why == Why::Pinned;
                    let (icon, tip) = if pinned {
                        (
                            "📌",
                            locale.text(
                                "Manter sempre: ligado. Clique pra voltar a classificar este app.",
                                "Always keep: on. Click to classify this app again.",
                            ),
                        )
                    } else {
                        (
                            "📍",
                            locale.text(
                                "Manter sempre: este app nunca mais vira candidato.",
                                "Always keep: this app never becomes a candidate again.",
                            ),
                        )
                    };
                    if ui.small_button(icon).on_hover_text(tip).clicked() {
                        if pinned {
                            self.keep.remove(&r.ident);
                        } else {
                            self.keep.insert(r.ident.clone());
                            self.picked.remove(&r.key);
                            // Some da lista de candidatos já, sem esperar a próxima amostra.
                            for row in self.rows.iter_mut().filter(|x| x.ident == r.ident) {
                                row.tier = Tier::Keep;
                                row.why = Why::Pinned;
                            }
                            *toast = Some(if locale == Locale::English {
                                format!("{} will always be kept", r.label)
                            } else {
                                format!("{} fica sempre", r.label)
                            });
                        }
                        self.save(unix_now());
                    }
                });
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::procs::Launcher;

    fn proc(pid: u32, ppid: u32, name: &str) -> ProcInfo {
        ProcInfo {
            pid,
            ppid,
            raw_ppid: ppid,
            name: name.into(),
            name_lower: name.to_lowercase(),
            exe_path: format!("/usr/bin/{name}"),
            cmdline: name.into(),
            create_time: pid as i64,
            ..Default::default()
        }
    }

    fn run(procs: &[ProcInfo], act: &HashMap<u32, Act>) -> Vec<Row> {
        let a = |p: &ProcInfo| {
            act.get(&p.pid).copied().unwrap_or(Act {
                idle: 0,
                unfocused: None,
                observed: 86400,
            })
        };
        classify(
            procs,
            &a,
            &|_| 100 << 20,
            &|p| p.pid == 1,
            &|_| Category::Other,
            &BTreeSet::new(),
            0,
        )
        .0
    }

    fn idle(s: u64) -> Act {
        Act {
            idle: s,
            unfocused: None,
            observed: 86400,
        }
    }

    #[test]
    fn idle_background_app_is_maybe_and_busy_one_is_kept() {
        let procs = [
            proc(1, 0, "systemd"),
            proc(10, 1, "slack"),
            proc(20, 1, "obs"),
        ];
        let act = HashMap::from([(10, idle(3 * 3600)), (20, idle(5))]);
        let rows = run(&procs, &act);
        let slack = rows.iter().find(|r| r.pid == 10).unwrap();
        assert_eq!(slack.tier, Tier::Maybe);
        let obs = rows.iter().find(|r| r.pid == 20).unwrap();
        assert_eq!(obs.tier, Tier::Keep);
        assert!(rows.iter().all(|r| r.pid != 1), "protegido não aparece");
    }

    #[test]
    fn same_identity_children_join_the_root_and_busy_child_keeps_the_parent() {
        let mut term = proc(10, 1, "foot");
        term.has_window = true;
        let procs = [
            proc(1, 0, "systemd"),
            term,
            proc(11, 10, "bash"),
            proc(12, 11, "make"),
        ];
        let act = HashMap::from([
            (
                10,
                Act {
                    idle: 9000,
                    unfocused: Some(9000),
                    observed: 86400,
                },
            ),
            (11, idle(9000)),
            (12, idle(3)),
        ]);
        let rows = run(&procs, &act);
        let foot = rows.iter().find(|r| r.pid == 10).unwrap();
        assert_eq!(
            foot.tier,
            Tier::Keep,
            "build rodando dentro mantém o terminal"
        );
        assert!(
            rows.iter().all(|r| r.pid != 11),
            "shell não vira linha própria"
        );
    }

    #[test]
    fn orphan_of_exited_agent_is_close_and_takes_its_children() {
        let mut mcp = proc(30, 1, "node");
        mcp.launcher = Launcher {
            agent: Some("Claude Code".into()),
            agent_pid: Some(999),
            ..Default::default()
        };
        let procs = [proc(1, 0, "systemd"), mcp, proc(31, 30, "python3")];
        let act = HashMap::from([(30, idle(3600)), (31, idle(3600))]);
        let rows = run(&procs, &act);
        assert_eq!(rows.len(), 1, "o filho parado vai junto, sem linha própria");
        assert_eq!(rows[0].tier, Tier::Close);
        let pids: Vec<u32> = rows[0].kill.iter().map(|(p, _)| *p).collect();
        assert_eq!(pids, vec![30, 31]);
    }

    #[test]
    fn agent_session_nested_in_an_exited_agent_is_not_a_leftover() {
        let mut claude = proc(30, 1, "claude");
        claude.launcher = Launcher {
            agent: Some("Claude Code".into()),
            agent_pid: Some(999),
            ..Default::default()
        };
        let procs = [proc(1, 0, "systemd"), claude];
        let rows = run(&procs, &HashMap::from([(30, idle(3600))]));
        assert_eq!(rows[0].tier, Tier::Maybe);
    }

    #[test]
    fn helper_of_a_focused_app_is_kept_and_services_are_kept() {
        let mut app = proc(10, 1, "code");
        app.has_window = true;
        let mut svc = proc(40, 1, "hermes");
        svc.launcher.unit = Some("hermes-gateway.service".into());
        let procs = [
            proc(1, 0, "systemd"),
            app,
            proc(11, 10, "rust-analyzer"),
            svc,
        ];
        let act = HashMap::from([
            (
                10,
                Act {
                    idle: 9000,
                    unfocused: Some(30),
                    observed: 86400,
                },
            ),
            (11, idle(9000)),
            (40, idle(9000)),
        ]);
        let rows = run(&procs, &act);
        assert!(matches!(
            rows.iter().find(|r| r.pid == 11).unwrap().why,
            Why::HelperOf(_)
        ));
        assert!(matches!(
            rows.iter().find(|r| r.pid == 40).unwrap().why,
            Why::Service(_)
        ));
    }

    #[test]
    fn nothing_is_a_candidate_right_after_launch() {
        let procs = [proc(1, 0, "systemd"), proc(10, 1, "slack")];
        let act = HashMap::from([(
            10,
            Act {
                idle: 40,
                unfocused: None,
                observed: 40,
            },
        )]);
        let rows = run(&procs, &act);
        assert_eq!(rows[0].tier, Tier::Keep);
    }
}
