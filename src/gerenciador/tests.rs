use std::collections::HashMap;

use super::*;
use crate::procs::ProcInfo;

const MB: u64 = 1024 * 1024;

fn p(pid: u32, ppid: u32, exe: &str, ram_mb: u64, cpu: f32) -> ProcInfo {
    let name = std::path::Path::new(exe)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(exe)
        .to_string();
    ProcInfo {
        pid,
        ppid,
        raw_ppid: ppid,
        name_lower: name.to_lowercase(),
        name,
        exe_path: exe.into(),
        private_ws: ram_mb * MB,
        cpu_pct: cpu,
        create_time: pid as i64,
        ..ProcInfo::default()
    }
}

fn win(pid: u32, address: &str, class: &str, title: &str) -> Win {
    Win {
        address: address.into(),
        pid,
        title: title.into(),
        class: class.into(),
        focused: false,
    }
}

struct Case {
    procs: Vec<ProcInfo>,
    windows: Vec<Win>,
    cats: HashMap<u32, Category>,
    cwd: HashMap<u32, String>,
    locked: Vec<u32>,
}

impl Case {
    fn new(procs: Vec<ProcInfo>, windows: Vec<Win>) -> Self {
        Self {
            procs,
            windows,
            cats: HashMap::new(),
            cwd: HashMap::new(),
            locked: Vec::new(),
        }
    }

    fn build(&self) -> Vec<Entry> {
        let mem = |p: &ProcInfo| (p.private_ws > 0).then_some(p.private_ws);
        let protected = |p: &ProcInfo| self.locked.contains(&p.pid);
        let cwd = |pid: u32| self.cwd.get(&pid).cloned();
        let app_name = |exe: &str, class: &str| match (exe, class) {
            (e, _) if e.ends_with("/brave") => Some("Brave".to_string()),
            (e, _) if e.ends_with("/foot") => Some("Foot".to_string()),
            _ => None,
        };
        build(&Inputs {
            procs: &self.procs,
            windows: &self.windows,
            cats: &self.cats,
            mem: &mem,
            protected: &protected,
            cwd: &cwd,
            app_name: &app_name,
            home: Some("/home/lol"),
            system: &|p: &ProcInfo| p.exe_path.is_empty(),
            me: 999,
        })
    }
}

fn find<'a>(entries: &'a [Entry], name: &str) -> &'a Entry {
    entries.iter().find(|e| e.name == name).unwrap_or_else(|| {
        panic!(
            "sem linha {name}: {:?}",
            entries.iter().map(|e| &e.name).collect::<Vec<_>>()
        )
    })
}

#[test]
fn window_owner_takes_its_whole_subtree() {
    let case = Case::new(
        vec![
            p(10, 1, "/usr/lib/brave/brave", 400, 1.0),
            p(11, 10, "/usr/lib/brave/brave", 300, 2.0),
            p(12, 11, "/usr/lib/brave/brave", 200, 0.5),
            p(13, 10, "/usr/lib/brave/brave", 100, 0.0),
        ],
        vec![win(10, "0x1", "brave-browser", "WhatsApp - Brave")],
    );
    let entries = case.build();
    assert_eq!(entries.len(), 1);
    let brave = find(&entries, "Brave");
    assert_eq!(brave.section, Section::Apps);
    assert_eq!(brave.pids.len(), 4);
    assert_eq!(brave.ram, 1000 * MB);
    assert!((brave.cpu - 3.5).abs() < 1e-4);
    assert_eq!(brave.detail, "WhatsApp - Brave");
}

#[test]
fn terminal_with_agent_is_named_after_the_agent_and_project() {
    let mut case = Case::new(
        vec![
            p(20, 1, "/usr/bin/foot", 30, 0.0),
            p(21, 20, "/usr/bin/bash", 5, 0.0),
            p(22, 21, "/home/lol/.local/bin/claude", 400, 0.4),
            p(23, 22, "/usr/bin/node", 80, 0.1),
        ],
        vec![win(20, "0x2", "foot", "✳ Módulo Freelance")],
    );
    case.cwd.insert(22, "/home/lol/nexunio/nexsales".into());
    let entries = case.build();
    let claude = find(&entries, "Claude");
    assert!(claude.agent);
    assert_eq!(claude.cat, Category::Ai);
    assert_eq!(claude.detail, "nexsales · Módulo Freelance");
    assert_eq!(claude.pids.len(), 4);
    assert_eq!(claude.ram, 515 * MB);
}

#[test]
fn two_agent_terminals_stay_separate_rows() {
    let case = Case::new(
        vec![
            p(30, 1, "/usr/bin/foot", 30, 0.0),
            p(31, 30, "/home/lol/.local/bin/claude", 400, 0.0),
            p(40, 1, "/usr/bin/foot", 30, 0.0),
            p(41, 40, "/home/lol/.local/bin/claude", 300, 0.0),
        ],
        vec![
            win(30, "0x3", "foot", "✳ Atlas"),
            win(40, "0x4", "foot", "✳ Freelance"),
        ],
    );
    let entries = case.build();
    let claudes: Vec<_> = entries.iter().filter(|e| e.name == "Claude").collect();
    assert_eq!(claudes.len(), 2);
    assert_ne!(claudes[0].key, claudes[1].key);
}

#[test]
fn plain_terminal_uses_the_window_title() {
    let case = Case::new(
        vec![
            p(50, 1, "/usr/bin/foot", 30, 0.0),
            p(51, 50, "/usr/bin/bash", 5, 0.0),
        ],
        vec![win(50, "0x5", "foot", "lol@omarchy:~")],
    );
    let entries = case.build();
    let t = find(&entries, "lol@omarchy:~");
    assert_eq!(t.detail, "Foot");
    assert!(!t.agent);
}

#[test]
fn app_that_is_an_agent_family_keeps_its_own_name() {
    // O Maestri tem janela e terminais de agente dentro; a linha é o Maestri, não o
    // primeiro Claude que ele abriu.
    let case = Case::new(
        vec![
            p(60, 1, "/opt/maestri/maestri-app", 500, 0.0),
            p(61, 60, "/home/lol/.local/bin/claude", 400, 0.0),
        ],
        vec![win(60, "0x6", "maestri-app", "DailyWork - Maestri")],
    );
    let entries = case.build();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "Maestri");
    assert!(!entries[0].agent);
    assert_eq!(entries[0].pids.len(), 2);
}

#[test]
fn processes_without_window_go_to_background_or_system_by_family() {
    let mut case = Case::new(
        vec![
            p(70, 1, "/usr/lib/chromium/chromium", 300, 0.0),
            p(71, 70, "/usr/lib/chromium/chromium", 200, 0.0),
            p(72, 1, "/usr/bin/pipewire", 20, 0.0),
            // kernel thread: sem exe e sem memória
            p(73, 2, "", 0, 0.0),
        ],
        vec![],
    );
    case.procs[3].name = "kworker/0:1".into();
    case.procs[3].name_lower = "kworker/0:1".into();
    case.cats.insert(72, Category::System);
    let entries = case.build();
    assert_eq!(entries.len(), 2);
    let chromium = find(&entries, "chromium");
    assert_eq!(chromium.section, Section::Background);
    assert_eq!(chromium.pids.len(), 2);
    assert_eq!(chromium.ram, 500 * MB);
    assert_eq!(find(&entries, "pipewire").section, Section::System);
}

#[test]
fn nested_window_owner_is_not_swallowed_by_its_parent() {
    // A Steam abre o jogo: cada um com a sua janela, cada um com a sua linha.
    let case = Case::new(
        vec![
            p(80, 1, "/usr/bin/steam", 300, 0.0),
            p(81, 80, "/games/game.bin", 2000, 30.0),
            p(82, 81, "/games/helper", 100, 0.0),
        ],
        vec![
            win(80, "0x8", "steam", "Steam"),
            win(81, "0x9", "game", "Game"),
        ],
    );
    let entries = case.build();
    assert_eq!(entries.len(), 2);
    let game = entries.iter().find(|e| e.pids[0] == 81).unwrap();
    assert_eq!(game.pids, vec![81, 82]);
    let steam = entries.iter().find(|e| e.pids[0] == 80).unwrap();
    assert_eq!(steam.pids, vec![80]);
}

#[test]
fn own_process_and_protected_rows() {
    let mut case = Case::new(
        vec![
            p(999, 1, "/home/lol/.local/bin/ramdog", 80, 0.0),
            p(90, 1, "/usr/bin/Hyprland", 300, 1.0),
        ],
        vec![win(999, "0xa", "ramdog-gerenciador", "Gerenciador")],
    );
    case.locked.push(90);
    let entries = case.build();
    assert_eq!(entries.len(), 1, "o próprio Gerenciador não aparece");
    assert!(entries[0].protected);
}

#[test]
fn same_app_in_two_windowed_processes_is_one_row_with_both_windows() {
    let case = Case::new(
        vec![
            p(100, 1, "/usr/bin/mpv", 100, 0.0),
            p(101, 1, "/usr/bin/mpv", 120, 0.0),
        ],
        vec![
            win(100, "0xb", "mpv", "a.mkv"),
            win(101, "0xc", "mpv", "b.mkv"),
        ],
    );
    let entries = case.build();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].windows.len(), 2);
    assert_eq!(entries[0].ram, 220 * MB);
}

#[test]
fn sort_groups_sections_and_breaks_ties_by_name() {
    let mut case = Case::new(
        vec![
            p(1, 0, "/usr/bin/pipewire", 900, 0.0),
            p(2, 0, "/usr/bin/zed", 100, 0.0),
            p(3, 0, "/usr/bin/alpha", 100, 0.0),
            p(4, 0, "/usr/bin/daemon", 50, 0.0),
        ],
        vec![win(2, "0x1", "zed", "zed"), win(3, "0x2", "alpha", "alpha")],
    );
    case.cats.insert(1, Category::System);
    let mut entries = case.build();
    sort(&mut entries, SortKey::Ram, true);
    let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["alpha", "zed", "daemon", "pipewire"]);
}

#[test]
fn keep_order_preserves_previous_positions_and_appends_newcomers() {
    let case = Case::new(
        vec![
            p(1, 0, "/usr/bin/a", 100, 0.0),
            p(2, 0, "/usr/bin/b", 900, 0.0),
            p(3, 0, "/usr/bin/c", 500, 0.0),
        ],
        vec![
            win(1, "0x1", "a", "a"),
            win(2, "0x2", "b", "b"),
            win(3, "0x3", "c", "c"),
        ],
    );
    let entries = case.build();
    let key = |n: &str| entries.iter().find(|e| e.name == n).unwrap().key.clone();
    let previous = vec![key("a"), key("b")];
    let kept = keep_order(entries.clone(), &previous);
    let names: Vec<_> = kept.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["a", "b", "c"]);
}

#[test]
fn search_matches_name_detail_pid_and_member_process() {
    let mut case = Case::new(
        vec![
            p(20, 1, "/usr/bin/foot", 30, 0.0),
            p(22, 20, "/home/lol/.local/bin/claude", 400, 0.4),
            p(23, 22, "/usr/bin/node", 80, 0.1),
        ],
        vec![win(20, "0x2", "foot", "✳ Atlas")],
    );
    case.cwd.insert(22, "/home/lol/Projects/robo".into());
    let entries = case.build();
    let idx: HashMap<u32, usize> = case
        .procs
        .iter()
        .enumerate()
        .map(|(i, p)| (p.pid, i))
        .collect();
    let e = &entries[0];
    for needle in ["claude", "robo", "atlas", "23", "node", ""] {
        assert!(matches(e, &case.procs, &idx, needle), "{needle}");
    }
    assert!(!matches(e, &case.procs, &idx, "brave"));
}

#[test]
fn clean_title_drops_the_leading_marker() {
    assert_eq!(clean_title("✳ Módulo Freelance"), "Módulo Freelance");
    assert_eq!(clean_title("  WhatsApp - Brave"), "WhatsApp - Brave");
    assert_eq!(clean_title("(1) Inbox"), "(1) Inbox");
}

#[test]
fn app_opened_by_an_agent_is_named_after_its_window() {
    let mut case = Case::new(
        vec![p(
            110,
            1,
            "/home/lol/Projects/dino/target/debug/dino",
            300,
            1.0,
        )],
        vec![win(110, "0xd", "Dino", "Dino (DEBUG)")],
    );
    case.procs[0].launcher.agent = Some("Claude Code".into());
    let entries = case.build();
    assert_eq!(entries[0].name, "Dino");
    assert!(!entries[0].agent);
}

#[test]
fn truncated_and_thread_names_fall_back_to_the_executable() {
    let mut case = Case::new(
        vec![
            p(120, 1, "/usr/lib/xdg-desktop-portal-gtk", 30, 0.0),
            p(121, 1, "/usr/lib/chatgpt/node", 90, 0.0),
        ],
        vec![],
    );
    case.procs[0].name = "xdg-desktop-por".into();
    case.procs[0].name_lower = "xdg-desktop-por".into();
    case.procs[1].name = "MainThread".into();
    case.procs[1].name_lower = "mainthread".into();
    let entries = case.build();
    let portal = find(&entries, "xdg-desktop-portal-gtk");
    assert_eq!(portal.section, Section::System, "encanamento da sessão");
    assert_eq!(find(&entries, "node").section, Section::Background);
}

#[test]
fn duplicate_names_get_a_distinguishing_detail_and_sessions_are_counted() {
    let mut case = Case::new(
        vec![
            p(130, 1, "/home/lol/.local/bin/claude", 400, 0.0),
            p(131, 1, "/home/lol/.local/bin/claude", 300, 0.0),
            p(132, 131, "/usr/bin/node", 50, 0.0),
            p(140, 1, "/usr/lib/claude-desktop/claude-desktop", 500, 0.0),
        ],
        vec![],
    );
    case.procs[2].launcher.agent = Some("Claude Code".into());
    let entries = case.build();
    let cli = entries.iter().find(|e| e.key == "bg:app:claude").unwrap();
    // o node filho do Claude entra no grupo pela árvore
    assert_eq!(cli.sessions, 2);
    assert_eq!(cli.pids.len(), 3);
    let desktop = entries.iter().find(|e| e.pids[0] == 140).unwrap();
    assert_eq!(desktop.sessions, 0);
}

#[test]
fn scripts_in_the_same_interpreter_are_separate_rows() {
    let mut case = Case::new(
        vec![
            p(150, 1, "/usr/bin/python3.14", 60, 0.0),
            p(151, 1, "/usr/bin/python3.14", 80, 0.0),
        ],
        vec![],
    );
    case.procs[0].name = "uwsm".into();
    case.procs[0].name_lower = "uwsm".into();
    case.procs[1].name = "teimoso".into();
    case.procs[1].name_lower = "teimoso".into();
    let entries = case.build();
    assert_eq!(entries.len(), 2);
    assert_eq!(find(&entries, "teimoso").pids, vec![151]);
    assert_eq!(find(&entries, "uwsm").section, Section::System);
}

#[test]
fn interpreter_rows_are_named_after_the_script() {
    let mut case = Case::new(
        vec![
            p(160, 1, "/usr/bin/python3.14", 60, 0.0),
            p(161, 1, "/usr/bin/python3.14", 40, 0.0),
            p(162, 1, "/usr/bin/python3.14", 70, 0.0),
        ],
        vec![],
    );
    for (i, cmd) in [
        "python3 /home/lol/.agents/bin/agent-bench-mcp",
        "python3 /home/lol/.agents/bin/agent-bench-mcp",
        "/usr/bin/python3 /opt/bridge/omnivoice_pc_bridge.py",
    ]
    .iter()
    .enumerate()
    {
        case.procs[i].name = "python3".into();
        case.procs[i].name_lower = "python3".into();
        case.procs[i].cmdline = cmd.to_string();
    }
    let entries = case.build();
    assert_eq!(entries.len(), 2);
    assert_eq!(find(&entries, "agent-bench-mcp").pids.len(), 2);
    assert_eq!(find(&entries, "omnivoice_pc_bridge").pids, vec![162]);
    assert_eq!(
        script_of("python3 -m http.server 8000").as_deref(),
        Some("http.server")
    );
    assert_eq!(script_of("python3 -c 'print(1)'"), None);
}

#[test]
fn detached_process_started_by_an_agent_stands_on_its_own() {
    let mut case = Case::new(
        vec![
            p(170, 1, "/home/lol/.local/bin/claude", 400, 0.0),
            p(171, 170, "/usr/bin/node", 60, 0.0),
            // lançado pelo Claude com setsid: o pai virou o systemd
            p(172, 1, "/usr/bin/python3.14", 90, 0.0),
        ],
        vec![],
    );
    for i in 1..3 {
        case.procs[i].launcher.agent = Some("Claude Code".into());
    }
    case.procs[2].name = "teimoso".into();
    case.procs[2].name_lower = "teimoso".into();
    case.procs[2].cmdline = "python3 /tmp/teimoso.py".into();
    let entries = case.build();
    let claude = entries.iter().find(|e| e.key == "bg:app:claude").unwrap();
    assert_eq!(claude.pids, vec![170, 171]);
    let t = find(&entries, "teimoso");
    assert_eq!(t.pids, vec![172]);
    assert_eq!(t.section, Section::Background);
}
