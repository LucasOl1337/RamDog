//! Modelo do Gerenciador: junta os processos por app, sem egui e sem ações.
//!
//! A lista completa responde "quais processos existem". O Gerenciador responde "quais
//! apps eu tenho abertos": cada janela vira um app com os filhos dela somados, e o que
//! não tem janela vai para Segundo plano ou Sistema, agrupado por família.

use std::collections::{HashMap, HashSet};

use crate::categories::Category;
use crate::identity;
use crate::procs::ProcInfo;

/// Uma janela do compositor, só com o que o modelo usa.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Win {
    pub address: String,
    pub pid: u32,
    pub title: String,
    pub class: String,
    pub focused: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Section {
    Apps,
    Background,
    System,
}

#[derive(Clone, Debug)]
pub struct Entry {
    /// Estável entre amostras: é o que mantém a seleção e a ordem congelada.
    pub key: String,
    pub name: String,
    /// Segunda linha: projeto e título da janela, ou quantas janelas o app tem.
    pub detail: String,
    /// Caminho do executável (minúsculo) de onde sai o ícone.
    pub icon_key: String,
    pub cat: Category,
    pub section: Section,
    /// O primeiro é a raiz: dono da janela ou o maior processo do grupo.
    pub pids: Vec<u32>,
    /// (pid, instante de criação) de cada membro, para não sinalizar um PID reciclado.
    pub identities: Vec<(u32, i64)>,
    pub windows: Vec<Win>,
    pub cpu: f32,
    pub ram: u64,
    /// Algum membro sem leitura de memória: a soma é um piso.
    pub ram_partial: bool,
    /// Todos os membros protegidos (sessão, lock ou o próprio RamDog).
    pub protected: bool,
    /// A janela é um terminal com um agente (Claude, Codex…) dentro.
    pub agent: bool,
    /// Quantos membros são o próprio CLI de um agente: "Claude, 12 sessões sem janela".
    pub sessions: usize,
}

pub struct Inputs<'a> {
    pub procs: &'a [ProcInfo],
    pub windows: &'a [Win],
    pub cats: &'a HashMap<u32, Category>,
    /// Memória na métrica escolhida; `None` quando o processo não tem leitura.
    pub mem: &'a dyn Fn(&ProcInfo) -> Option<u64>,
    pub protected: &'a dyn Fn(&ProcInfo) -> bool,
    /// Diretório de trabalho do processo, para dizer em que projeto o agente está.
    pub cwd: &'a dyn Fn(u32) -> Option<String>,
    /// Nome amigável do app pelo `.desktop` (`Brave`, `TigerVNC Viewer`).
    pub app_name: &'a dyn Fn(&str, &str) -> Option<String>,
    pub home: Option<&'a str>,
    /// Processo de outro usuário (root, serviços): vai para Sistema.
    pub system: &'a dyn Fn(&ProcInfo) -> bool,
    /// PID do próprio Gerenciador: não aparece na lista.
    pub me: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Tasks,
    Cpu,
    Ram,
}

/// Emuladores de terminal: a janela é deles, mas quem dá nome à linha é o que roda dentro.
fn is_terminal(p: &ProcInfo) -> bool {
    let base = std::path::Path::new(&p.exe_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&p.name_lower)
        .to_lowercase();
    matches!(
        base.as_str(),
        "foot"
            | "footclient"
            | "kitty"
            | "alacritty"
            | "ghostty"
            | "wezterm-gui"
            | "konsole"
            | "gnome-terminal-server"
            | "kgx"
            | "ptyxis"
            | "xterm"
            | "urxvt"
            | "st"
            | "tilix"
            | "terminator"
            | "xfce4-terminal"
            | "warp"
    )
}

/// Encanamento da sessão gráfica: portais, D-Bus, áudio, teclado. É do sistema para quem
/// lê, mesmo rodando com o usuário.
fn is_session_plumbing(p: &ProcInfo) -> bool {
    plumbing_name(&exe_base(p)) || plumbing_name(&p.name_lower)
}

fn plumbing_name(base: &str) -> bool {
    base.starts_with("xdg-")
        || base.starts_with("gvfs")
        || base.starts_with("at-spi")
        || base.starts_with("dbus")
        || base.starts_with("systemd")
        || base.starts_with("pipewire")
        || base.starts_with("localsearch")
        || base.starts_with("tracker")
        || matches!(
            base,
            "wireplumber"
                | "gnome-keyring-daemon"
                | "dconf-service"
                | "fcitx5"
                | "ibus-daemon"
                | "uwsm"
                | "hyprmoncfgd"
                | "hypridle"
                | "hyprsunset"
                | "hyprpolkitagent"
                | "polkit-gnome-authentication-agent-1"
                | "mako"
                | "swayosd-server"
                | "elephant"
                | "walker"
                | "upowerd"
                | "sddm-helper"
                | "login"
                | "sshd"
                | "(sd-pam)"
        )
}

fn exe_base(p: &ProcInfo) -> String {
    std::path::Path::new(&p.exe_path)
        .file_name()
        .and_then(|s| s.to_str())
        .map(str::to_lowercase)
        .unwrap_or_else(|| p.name_lower.clone())
}

/// Nome legível quando o `comm` não ajuda: o kernel corta em 15 letras
/// (`xdg-desktop-por`) e runtimes renomeiam a thread principal (`MainThread`).
fn readable(label: String, p: &ProcInfo) -> String {
    let base = std::path::Path::new(&p.exe_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if base.is_empty() || label != p.name {
        return label;
    }
    let truncated = label.chars().count() == 15 && base.starts_with(&label) && base != label;
    let thread =
        label == "MainThread" || label.ends_with("-MainThread") || label.starts_with("tokio-");
    if truncated || thread {
        base.to_string()
    } else {
        label
    }
}

/// O script que um interpretador está rodando: `python3 ~/.agents/bin/agent-bench-mcp`
/// → `agent-bench-mcp`, `python3 -m http.server` → `http.server`. `None` para `-c`.
pub fn script_of(cmdline: &str) -> Option<String> {
    let mut it = cmdline.split_whitespace().skip(1);
    while let Some(tok) = it.next() {
        match tok {
            "-m" => return it.next().map(str::to_string),
            "-c" | "-e" | "--eval" | "-p" | "--print" => return None,
            t if t.starts_with('-') => continue,
            t => {
                let path = std::path::Path::new(t);
                return path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
            }
        }
    }
    None
}

/// Kernel thread ou processo sem nada para mostrar: não entra em seção nenhuma.
fn is_noise(p: &ProcInfo, mem: Option<u64>) -> bool {
    p.exe_path.is_empty() && mem.unwrap_or(0) == 0
}

/// `✳ Módulo Freelance` → `Módulo Freelance`. O Claude marca o título com um símbolo
/// que não diz nada a quem lê a lista.
pub fn clean_title(title: &str) -> String {
    let t = title
        .trim_start_matches(|c: char| !c.is_alphanumeric() && c != '(' && c != '[')
        .trim();
    if t.chars().count() > 60 {
        let s: String = t.chars().take(57).collect();
        format!("{s}…")
    } else {
        t.to_string()
    }
}

fn project_of(cwd: Option<String>, home: Option<&str>) -> Option<String> {
    let cwd = cwd?;
    let cwd = cwd.trim_end_matches('/');
    if cwd.is_empty() || Some(cwd) == home.map(|h| h.trim_end_matches('/')) || cwd == "/" {
        return None;
    }
    std::path::Path::new(cwd)
        .file_name()
        .and_then(|s| s.to_str())
        .map(str::to_string)
}

fn join_detail(parts: &[Option<String>]) -> String {
    let mut seen: Vec<&str> = Vec::new();
    for p in parts.iter().flatten() {
        let p = p.as_str();
        if !p.is_empty() && !seen.iter().any(|s| s.eq_ignore_ascii_case(p)) {
            seen.push(p);
        }
    }
    seen.join(" · ")
}

struct Acc {
    key: String,
    name: String,
    detail: String,
    icon_key: String,
    section: Section,
    pids: Vec<u32>,
    windows: Vec<Win>,
    agent: bool,
}

pub fn build(inp: &Inputs<'_>) -> Vec<Entry> {
    let idx: HashMap<u32, usize> = inp
        .procs
        .iter()
        .enumerate()
        .map(|(i, p)| (p.pid, i))
        .collect();
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for p in inp.procs {
        if p.ppid != 0 && p.ppid != p.pid {
            children.entry(p.ppid).or_default().push(p.pid);
        }
    }
    for kids in children.values_mut() {
        kids.sort_unstable();
    }

    // Donos de janela, na ordem das janelas. Um processo com três janelas é um dono só.
    let mut owners: Vec<u32> = Vec::new();
    let mut wins_of: HashMap<u32, Vec<Win>> = HashMap::new();
    for w in inp.windows {
        if w.pid == inp.me || !idx.contains_key(&w.pid) {
            continue;
        }
        if !wins_of.contains_key(&w.pid) {
            owners.push(w.pid);
        }
        wins_of.entry(w.pid).or_default().push(w.clone());
    }
    let owner_set: HashSet<u32> = owners.iter().copied().collect();

    // Cada dono leva a própria subárvore, sem entrar na de outro dono.
    let mut claimed: HashSet<u32> = HashSet::new();
    let mut subtree_of: HashMap<u32, Vec<u32>> = HashMap::new();
    for &w in &owners {
        let mut members = vec![w];
        claimed.insert(w);
        let mut queue = std::collections::VecDeque::from([w]);
        let mut guard = 0;
        while let Some(x) = queue.pop_front() {
            guard += 1;
            if guard > 100_000 {
                break;
            }
            for &k in children.get(&x).map(Vec::as_slice).unwrap_or(&[]) {
                if owner_set.contains(&k) || k == inp.me || !claimed.insert(k) {
                    continue;
                }
                members.push(k);
                queue.push_back(k);
            }
        }
        subtree_of.insert(w, members);
    }

    let mut accs: Vec<Acc> = Vec::new();
    let mut by_key: HashMap<String, usize> = HashMap::new();
    for &w in &owners {
        let owner = &inp.procs[idx[&w]];
        let members = subtree_of.remove(&w).unwrap_or_default();
        let wins = wins_of.remove(&w).unwrap_or_default();
        let title = wins
            .iter()
            .map(|x| clean_title(&x.title))
            .find(|t| !t.is_empty());
        let class = wins.first().map(|x| x.class.clone()).unwrap_or_default();
        let agent = if is_terminal(owner) {
            members
                .iter()
                .filter_map(|pid| idx.get(pid).map(|&i| &inp.procs[i]))
                .find(|p| identity::is_agent_cli(p))
        } else {
            None
        };
        let acc = if let Some(a) = agent {
            // Cada terminal com agente é uma sessão: linha própria, mesmo com o mesmo CLI.
            let project = project_of((inp.cwd)(a.pid), inp.home).or_else(|| {
                a.launcher
                    .init_cwd
                    .clone()
                    .and_then(|c| project_of(Some(c), None))
            });
            Acc {
                key: format!("win:{w}"),
                name: identity::of(a).label,
                detail: join_detail(&[project, title]),
                icon_key: owner.exe_path.to_lowercase(),
                section: Section::Apps,
                pids: members,
                windows: wins,
                agent: true,
            }
        } else if is_terminal(owner) {
            let app = (inp.app_name)(&owner.exe_path, &class)
                .unwrap_or_else(|| identity::of(owner).label);
            Acc {
                key: format!("win:{w}"),
                name: title.clone().unwrap_or_else(|| app.clone()),
                detail: if title.is_some() { app } else { String::new() },
                icon_key: owner.exe_path.to_lowercase(),
                section: Section::Apps,
                pids: members,
                windows: wins,
                agent: false,
            }
        } else {
            let id = identity::of(owner);
            // Um app que o agente abriu (o Dino que o Claude compilou) herda o ambiente do
            // agente, mas a janela é do app: o nome vem dela.
            let inherited = id.kind == identity::Kind::Agent && !identity::is_agent_cli(owner);
            let name = match id.kind {
                identity::Kind::Desktop | identity::Kind::Runtime => {
                    (inp.app_name)(&owner.exe_path, &class)
                        .unwrap_or_else(|| readable(id.label.clone(), owner))
                }
                _ if inherited => (inp.app_name)(&owner.exe_path, &class)
                    .or_else(|| (!class.is_empty()).then(|| class.clone()))
                    .unwrap_or_else(|| readable(owner.name.clone(), owner)),
                _ => id.label.clone(),
            };
            let key = if inherited {
                format!("app:exe:{}", owner.exe_path)
            } else {
                format!("app:{}", id.key)
            };
            let detail = title.filter(|t| !t.eq_ignore_ascii_case(&name));
            Acc {
                key,
                name,
                detail: detail.unwrap_or_default(),
                icon_key: owner.exe_path.to_lowercase(),
                section: Section::Apps,
                pids: members,
                windows: wins,
                agent: false,
            }
        };
        match by_key.get(&acc.key) {
            // Mesmo app em dois processos com janela (duas instâncias do Foot, do mpv):
            // uma linha, as janelas somadas.
            Some(&i) => {
                let old = &mut accs[i];
                old.pids.extend(acc.pids);
                old.windows.extend(acc.windows);
            }
            None => {
                by_key.insert(acc.key.clone(), accs.len());
                accs.push(acc);
            }
        }
    }
    // Agentes sem janela (Claude headless, Codex do Maestri) levam a própria subárvore,
    // como uma janela leva a dela. O que o agente abriu e se desgarrou (setsid, systemd)
    // aparece com o próprio nome: herdar o ambiente não é pertencer.
    let mut rest: HashMap<String, Vec<u32>> = HashMap::new();
    for p in inp.procs {
        if claimed.contains(&p.pid) || p.pid == inp.me || !identity::is_agent_cli(p) {
            continue;
        }
        let key = format!("bg:{}", identity::of(p).key);
        let mut members = vec![p.pid];
        claimed.insert(p.pid);
        let mut queue = std::collections::VecDeque::from([p.pid]);
        let mut guard = 0;
        while let Some(x) = queue.pop_front() {
            guard += 1;
            if guard > 100_000 {
                break;
            }
            for &k in children.get(&x).map(Vec::as_slice).unwrap_or(&[]) {
                if k == inp.me || !claimed.insert(k) {
                    continue;
                }
                members.push(k);
                queue.push_back(k);
            }
        }
        rest.entry(key).or_default().extend(members);
    }

    // O resto, por família: `chromium (66)` da bancada, o gateway do Hermes, o pipewire.
    for p in inp.procs {
        if claimed.contains(&p.pid) || p.pid == inp.me || is_noise(p, (inp.mem)(p)) {
            continue;
        }
        let id = identity::of_own(p);
        // Interpretador genérico (python, node, bash): o executável não diz de quem é.
        // Dois scripts no mesmo python3 são apps diferentes, e o Fechar de um não pode
        // levar o outro junto (o uwsm também roda em python).
        let key = if id.kind == identity::Kind::Runtime {
            let who = script_of(&p.cmdline).unwrap_or_else(|| p.name_lower.clone());
            format!("bg:{}:{who}", id.key)
        } else {
            format!("bg:{}", id.key)
        };
        rest.entry(key).or_default().push(p.pid);
    }
    let mut rest: Vec<(String, Vec<u32>)> = rest.into_iter().collect();
    rest.sort_by(|a, b| a.0.cmp(&b.0));
    for (key, mut pids) in rest {
        // O maior processo dá nome, ícone e categoria ao grupo.
        pids.sort_by_key(|pid| {
            std::cmp::Reverse(
                idx.get(pid)
                    .and_then(|&i| (inp.mem)(&inp.procs[i]))
                    .unwrap_or(0),
            )
        });
        let top = &inp.procs[idx[&pids[0]]];
        let own = |p: &ProcInfo| {
            if key.starts_with("bg:app:") {
                identity::of(p)
            } else {
                identity::of_own(p)
            }
        };
        let mut best = own(top);
        for pid in &pids {
            let id = own(&inp.procs[idx[pid]]);
            if identity::richness(&id) > identity::richness(&best) {
                best = id;
            }
        }
        // Grupo de agente: o nome é o do agente dono da árvore (Maestri), não o do maior
        // filho (o Claude que o Maestri abriu).
        if let Some(family) = key.strip_prefix("bg:").filter(|k| k.starts_with("app:")) {
            if let Some(id) = pids
                .iter()
                .map(|pid| identity::of(&inp.procs[idx[pid]]))
                .find(|id| id.key == family)
            {
                best = id;
            }
        }
        let cat = inp.cats.get(&top.pid).copied().unwrap_or(Category::Other);
        let script = (best.kind == identity::Kind::Runtime)
            .then(|| script_of(&top.cmdline))
            .flatten();
        let name = match best.kind {
            identity::Kind::Runtime if script.is_some() => script.clone().unwrap_or_default(),
            identity::Kind::Desktop | identity::Kind::Runtime => (inp.app_name)(&top.exe_path, "")
                .unwrap_or_else(|| readable(best.label.clone(), top)),
            _ => best.label,
        };
        let system = cat == Category::System
            || (inp.system)(top)
            || (best.kind != identity::Kind::Agent && is_session_plumbing(top));
        accs.push(Acc {
            key,
            name,
            detail: String::new(),
            icon_key: top.exe_path.to_lowercase(),
            section: if system {
                Section::System
            } else {
                Section::Background
            },
            pids,
            windows: Vec::new(),
            agent: false,
        });
    }

    // Dois "Claude" em Segundo plano (o CLI e o Claude Desktop) precisam de algo que os
    // diferencie: o executável, ou a pasta dele quando o nome já é o executável.
    let mut names: HashMap<(Section, String), usize> = HashMap::new();
    for a in &accs {
        *names.entry((a.section, a.name.to_lowercase())).or_default() += 1;
    }
    for a in accs.iter_mut() {
        // Família de agente já se explica pelas sessões ("12 sessões sem janela").
        if !a.detail.is_empty()
            || a.key.starts_with("bg:app:")
            || names[&(a.section, a.name.to_lowercase())] < 2
        {
            continue;
        }
        let Some(p) = a
            .pids
            .first()
            .and_then(|pid| idx.get(pid))
            .map(|&i| &inp.procs[i])
        else {
            continue;
        };
        let path = std::path::Path::new(&p.exe_path);
        let base = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        a.detail = if !base.is_empty() && !base.eq_ignore_ascii_case(&a.name) {
            base.to_string()
        } else {
            path.parent()
                .and_then(|d| d.to_str())
                .map(|d| match inp.home {
                    Some(h) if d.starts_with(h) => d.replacen(h, "~", 1),
                    _ => d.to_string(),
                })
                .unwrap_or_default()
        };
    }

    accs.into_iter()
        .map(|a| {
            let mut cpu = 0.0f32;
            let mut ram = 0u64;
            let mut partial = false;
            let mut all_protected = true;
            let mut identities = Vec::with_capacity(a.pids.len());
            let mut sessions = 0;
            for pid in &a.pids {
                let Some(&i) = idx.get(pid) else { continue };
                let p = &inp.procs[i];
                if identity::is_agent_cli(p) {
                    sessions += 1;
                }
                cpu += p.cpu_pct;
                match (inp.mem)(p) {
                    Some(m) => ram += m,
                    None if !p.exe_path.is_empty() => partial = true,
                    None => {}
                }
                all_protected &= (inp.protected)(p);
                identities.push((p.pid, p.create_time));
            }
            let cat = a
                .pids
                .first()
                .and_then(|pid| inp.cats.get(pid))
                .copied()
                .unwrap_or(Category::Other);
            Entry {
                key: a.key,
                name: a.name,
                detail: a.detail,
                icon_key: a.icon_key,
                cat: if a.agent { Category::Ai } else { cat },
                section: a.section,
                pids: a.pids,
                identities,
                windows: a.windows,
                cpu,
                ram,
                ram_partial: partial,
                protected: all_protected,
                agent: a.agent,
                sessions,
            }
        })
        .collect()
}

/// Ordena dentro de cada seção. Empate cai no nome e depois na chave, para a lista não
/// trocar de lugar entre amostras iguais.
pub fn sort(entries: &mut [Entry], key: SortKey, desc: bool) {
    entries.sort_by(|a, b| {
        let ord = match key {
            SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            SortKey::Tasks => a.pids.len().cmp(&b.pids.len()),
            SortKey::Cpu => a.cpu.total_cmp(&b.cpu),
            SortKey::Ram => a.ram.cmp(&b.ram),
        };
        let ord = if desc { ord.reverse() } else { ord };
        a.section
            .cmp(&b.section)
            .then(ord)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.key.cmp(&b.key))
    });
}

/// Mantém a ordem anterior enquanto o ponteiro está sobre a lista: quem some sai, quem
/// chega entra no fim da seção dele. Os números continuam atualizando.
pub fn keep_order(entries: Vec<Entry>, previous: &[String]) -> Vec<Entry> {
    let pos: HashMap<&str, usize> = previous
        .iter()
        .enumerate()
        .map(|(i, k)| (k.as_str(), i))
        .collect();
    let mut out = entries;
    out.sort_by(|a, b| {
        a.section
            .cmp(&b.section)
            .then(match (pos.get(a.key.as_str()), pos.get(b.key.as_str())) {
                (Some(x), Some(y)) => x.cmp(y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a.key.cmp(&b.key),
            })
    });
    out
}

/// Filtro da busca: nome, detalhe, PID ou nome de processo de qualquer membro.
pub fn matches(e: &Entry, procs: &[ProcInfo], idx: &HashMap<u32, usize>, needle: &str) -> bool {
    let n = needle.trim().to_lowercase();
    if n.is_empty() {
        return true;
    }
    if e.name.to_lowercase().contains(&n) || e.detail.to_lowercase().contains(&n) {
        return true;
    }
    if let Ok(pid) = n.parse::<u32>() {
        if e.pids.contains(&pid) {
            return true;
        }
    }
    e.pids.iter().any(|pid| {
        idx.get(pid)
            .map(|&i| procs[i].name_lower.contains(&n))
            .unwrap_or(false)
    })
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
