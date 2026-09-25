//! Identidade da *tarefa* que o humano vê — não o binário do runtime.
//!
//! Wine, Python e Electron compartilham o executável entre apps diferentes. Agrupar
//! pelo path do `wine64` junta Overwatch com outro prefixo Proton. Aqui a chave é a
//! instância: Steam appid, prefixo Wine, projeto do venv, janela, família de agente.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use crate::config::Locale;

fn agent_slug(agent: &str) -> Option<&'static str> {
    match agent {
        "Claude Code" => Some("claude"),
        "Codex" => Some("codex"),
        "Grok CLI" => Some("grok"),
        "Cursor Agent" => Some("cursor"),
        "Gemini CLI" => Some("gemini"),
        "Hermes" => Some("hermes"),
        _ => None,
    }
}

fn family_from_basename(base: &str) -> Option<&'static str> {
    match base {
        "claude" | "claude-code" => Some("claude"),
        "codex" => Some("codex"),
        "grok" | "grok-bot" => Some("grok"),
        "chatgpt" => Some("chatgpt"),
        "maestri" | "maestri-app" => Some("maestri"),
        "cursor" => Some("cursor"),
        "hermes" | "hermes-agent" => Some("hermes"),
        "opencode" => Some("opencode"),
        "gemini" | "gemini-cli" => Some("gemini"),
        _ => None,
    }
}

fn family_label(slug: &str) -> String {
    match slug {
        "claude" => "Claude".into(),
        "codex" => "Codex".into(),
        "grok" => "Grok".into(),
        "chatgpt" => "ChatGPT".into(),
        "maestri" => "Maestri".into(),
        "cursor" => "Cursor".into(),
        "hermes" => "Hermes".into(),
        "opencode" => "OpenCode".into(),
        "gemini" => "Gemini".into(),
        other => {
            let mut c = other.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => other.to_string(),
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub key: String,
    pub label: String,
    pub origin: Option<String>,
    pub kind: Kind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Game,
    Project,
    Agent,
    Desktop,
    Emulator,
    Runtime,
}

/// Entrada pura, para testes não dependerem de `ProcInfo` completo.
#[derive(Clone, Debug, Default)]
pub struct Facts<'a> {
    pub name: &'a str,
    pub exe_path: &'a str,
    pub cmdline: &'a str,
    pub init_cwd: Option<&'a str>,
    pub agent: Option<&'a str>,
    pub steam_app_id: Option<u32>,
    pub wine_prefix: Option<&'a str>,
    pub window_title: Option<&'a str>,
    pub window_class: Option<&'a str>,
}

pub fn of(p: &crate::procs::ProcInfo) -> Identity {
    resolve(Facts {
        name: &p.name,
        exe_path: &p.exe_path,
        cmdline: &p.cmdline,
        init_cwd: p.launcher.init_cwd.as_deref(),
        agent: p.launcher.agent.as_deref(),
        steam_app_id: p.launcher.steam_app_id,
        wine_prefix: p.launcher.wine_prefix.as_deref(),
        window_title: p.window_title.as_deref(),
        window_class: p.window_class.as_deref(),
    })
}

pub fn resolve(facts: Facts<'_>) -> Identity {
    // O cliente Steam relançado por um atalho carrega `steam://rungameid/2357570` na linha de
    // comando e fica horas vivo depois do jogo fechar. Não é o jogo: é a Steam.
    if is_steam_client(facts.name, facts.exe_path) {
        return Identity {
            key: "app:steam".into(),
            label: "Steam".into(),
            origin: None,
            kind: Kind::Desktop,
        };
    }
    if let Some(id) = steam_id(&facts) {
        let label = windows_game_label(&facts)
            .or_else(|| steam_name(id))
            .unwrap_or_else(|| format!("Steam {id}"));
        return Identity {
            key: format!("steam:{id}"),
            origin: Some("Steam / Proton".into()),
            label,
            kind: Kind::Game,
        };
    }
    if let Some(prefix) = facts.wine_prefix.filter(|p| !p.is_empty()) {
        if let Some(label) = windows_game_label(&facts) {
            return Identity {
                key: format!("wine:{prefix}"),
                label,
                origin: Some("Wine".into()),
                kind: Kind::Game,
            };
        }
        if is_wine_runtime(facts.name, facts.exe_path) {
            return Identity {
                key: format!("wine:{prefix}"),
                label: windows_game_label(&facts).unwrap_or_else(|| "Wine".into()),
                origin: Some("Wine".into()),
                kind: Kind::Runtime,
            };
        }
    }
    if let Some(label) = windows_game_label(&facts) {
        if is_wine_runtime(facts.name, facts.exe_path) || looks_like_windows_game(facts.name) {
            let key = format!("game:{}", label.to_lowercase().replace(' ', "-"));
            return Identity {
                key,
                label,
                origin: Some("Proton / Wine".into()),
                kind: Kind::Game,
            };
        }
    }
    if let Some(avd) = qemu_avd(facts.cmdline) {
        return Identity {
            key: format!("qemu:{avd}"),
            label: format!("Emulador Android ({avd})"),
            origin: Some("Android emulator".into()),
            kind: Kind::Emulator,
        };
    }
    if let Some(agent) = facts.agent.and_then(agent_slug) {
        return Identity {
            key: format!("app:{agent}"),
            label: family_label(agent),
            origin: facts.agent.map(|s| s.to_string()),
            kind: Kind::Agent,
        };
    }
    if let Some(fam) = family_from_basename(&basename(facts.exe_path, facts.name)) {
        return Identity {
            key: format!("app:{fam}"),
            label: family_label(fam),
            origin: None,
            kind: Kind::Agent,
        };
    }
    if let Some(project) = project_name(&facts) {
        let pretty = pretty_project(&project);
        return Identity {
            key: format!("project:{project}"),
            label: pretty,
            origin: Some("projeto".into()),
            kind: Kind::Project,
        };
    }
    if let Some(title) = facts
        .window_title
        .map(str::trim)
        .filter(|s| !s.is_empty() && !is_generic_title(s))
    {
        if is_runtime_name(facts.name, facts.exe_path) {
            return Identity {
                key: exe_key(facts.exe_path, facts.name),
                label: shorten_title(title),
                origin: facts.window_class.map(|s| s.to_string()),
                kind: Kind::Desktop,
            };
        }
    }
    Identity {
        key: exe_key(facts.exe_path, facts.name),
        label: display_basename(facts.name, facts.exe_path),
        origin: None,
        kind: if is_runtime_name(facts.name, facts.exe_path) {
            Kind::Runtime
        } else {
            Kind::Desktop
        },
    }
}

pub fn richness(id: &Identity) -> u8 {
    let base: u8 = match id.kind {
        Kind::Game => 50,
        Kind::Project => 40,
        Kind::Emulator => 38,
        Kind::Agent => 30,
        Kind::Desktop => 20,
        Kind::Runtime => 5,
    };
    base.saturating_add(if id.origin.is_some() { 2u8 } else { 0u8 })
}

pub fn steam_app_id_from(
    cmdline: &str,
    exe_path: &str,
    wine_prefix: Option<&str>,
    env_id: Option<u32>,
) -> Option<u32> {
    if let Some(id) = env_id.filter(|n| *n > 0) {
        return Some(id);
    }
    for hay in [cmdline, exe_path, wine_prefix.unwrap_or("")] {
        if let Some(id) = parse_steam_id(hay) {
            return Some(id);
        }
    }
    None
}

pub fn parse_steam_id(text: &str) -> Option<u32> {
    for marker in [
        "compatdata/",
        "rungameid/",
        "SteamAppId=",
        "STEAM_COMPAT_APP_ID=",
    ] {
        if let Some(rest) = find_ci(text, marker) {
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(id) = digits.parse::<u32>() {
                if id > 0 {
                    return Some(id);
                }
            }
        }
    }
    None
}

pub fn parse_acf_name(text: &str) -> Option<String> {
    let rest = find_ci(text, "\"name\"")?;
    let start = rest.find('"')? + 1;
    let end = rest[start..].find('"')?;
    let name = rest[start..start + end].trim();
    if name.is_empty() {
        None
    } else {
        Some(name.replace('®', "").replace('™', "").trim().to_string())
    }
}

pub fn windows_exe_from_cmdline(cmdline: &str) -> Option<&str> {
    let mut last = None;
    let bytes = cmdline.as_bytes();
    let mut i = 0;
    while i + 4 <= bytes.len() {
        if bytes[i..i + 4].eq_ignore_ascii_case(b".exe") {
            let end = i + 4;
            let prefix = &cmdline[..end];
            let start = prefix
                .rfind(|c: char| c == '/' || c == '\\' || c.is_whitespace() || c == '"')
                .map(|s| s + 1)
                .unwrap_or(0);
            let token = &cmdline[start..end];
            if !token.is_empty() && !is_wine_helper(token) {
                last = Some(token);
            }
            i = end;
        } else {
            i += 1;
        }
    }
    last
}

pub fn qemu_avd(cmdline: &str) -> Option<&str> {
    let rest = find_ci(cmdline, "-avd ")?;
    let name = rest.split_whitespace().next()?;
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

pub fn project_from_path(path: &str) -> Option<String> {
    let parts: Vec<&str> = Path::new(path)
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    for i in 0..parts.len() {
        if parts[i].starts_with(".venv") && i > 0 {
            let parent = parts[i - 1];
            if parent != "home" && parent != "Users" && !parent.starts_with('.') {
                return Some(parent.to_string());
            }
        }
    }
    for marker in ["Projects", "projects", "work"] {
        if let Some(i) = parts.iter().position(|p| *p == marker) {
            if let Some(name) = parts.get(i + 1) {
                if !name.starts_with('.') && *name != "bin" && *name != "lib" {
                    return Some((*name).to_string());
                }
            }
        }
    }
    None
}

fn steam_id(facts: &Facts<'_>) -> Option<u32> {
    steam_app_id_from(
        facts.cmdline,
        facts.exe_path,
        facts.wine_prefix,
        facts.steam_app_id,
    )
}

fn windows_game_label(facts: &Facts<'_>) -> Option<String> {
    if looks_like_windows_game(facts.name) {
        return Some(pretty_win_exe(facts.name));
    }
    windows_exe_from_cmdline(facts.cmdline).map(pretty_win_exe)
}

fn looks_like_windows_game(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(".exe") && !is_wine_helper(&n)
}

fn pretty_win_exe(name: &str) -> String {
    let stem = Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(name);
    match stem.to_ascii_lowercase().as_str() {
        "overwatch" | "overwatch2" => "Overwatch 2".into(),
        "cyberpunk2077" => "Cyberpunk 2077".into(),
        other => title_case(other),
    }
}

fn is_wine_helper(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    let n = n.rsplit(['/', '\\']).next().unwrap_or(&n);
    matches!(
        n,
        "wine"
            | "wine64"
            | "wine64.exe"
            | "wine.exe"
            | "wineserver"
            | "wineserver.exe"
            | "start.exe"
            | "plugplay.exe"
            | "services.exe"
            | "explorer.exe"
            | "rpcss.exe"
            | "winedevice.exe"
            | "svchost.exe"
    )
}

/// Binários do próprio cliente Steam (não o runtime que embrulha o jogo).
fn is_steam_client(name: &str, exe: &str) -> bool {
    let n = basename(exe, name).to_ascii_lowercase();
    let e = exe.to_ascii_lowercase();
    matches!(
        n.as_str(),
        "steam"
            | "steam.sh"
            | "steamwebhelper"
            | "steamwebhelper_sniper_wrap.sh"
            | "steamservice"
            | "steam-runtime-launcher-service"
            | "steamerrorreporter"
            | "fossilize_replay"
    ) || ((e.contains("/steam/ubuntu12_32/") || e.contains("/steam/ubuntu12_64/"))
        && !n.ends_with(".exe"))
}

fn is_wine_runtime(name: &str, exe: &str) -> bool {
    is_wine_helper(name) || exe.to_ascii_lowercase().contains("/wine")
}

fn is_runtime_name(name: &str, exe: &str) -> bool {
    let n = basename(exe, name).to_ascii_lowercase();
    is_wine_helper(&n)
        || matches!(
            n.as_str(),
            "python"
                | "python3"
                | "python3.11"
                | "python3.12"
                | "python3.13"
                | "python3.14"
                | "node"
                | "nodejs"
                | "electron"
                | "java"
                | "qemu-system-x86_64"
                | "qemu-system-x86"
        )
}

fn project_name(facts: &Facts<'_>) -> Option<String> {
    for hay in [facts.exe_path, facts.cmdline, facts.init_cwd.unwrap_or("")] {
        if let Some(p) = project_from_path(hay) {
            return Some(p);
        }
    }
    None
}

fn pretty_project(name: &str) -> String {
    match name {
        "sussurro" => "Sussurro".into(),
        "sonora" => "Sonora".into(),
        "OmniVoice-Studio" | "omnivoice-studio" => "OmniVoice Studio".into(),
        "DailyWork" | "Daily Work app" => "Daily Work".into(),
        other => other.replace('-', " "),
    }
}

fn exe_key(exe: &str, name: &str) -> String {
    if !exe.is_empty() {
        let path = if cfg!(windows) {
            exe.to_lowercase()
        } else {
            exe.to_string()
        };
        format!("exe:{path}")
    } else {
        format!("name:{}", name.to_lowercase())
    }
}

/// O processo é o próprio CLI de um agente (claude, codex…), não algo que um agente abriu.
pub fn is_agent_cli(p: &crate::procs::ProcInfo) -> bool {
    family_from_basename(&basename(&p.exe_path, &p.name)).is_some()
}

fn basename(exe: &str, name: &str) -> String {
    Path::new(exe)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(name)
        .trim_end_matches(".exe")
        .to_string()
}

fn display_basename(name: &str, exe: &str) -> String {
    let raw = if name.is_empty() {
        basename(exe, name)
    } else {
        name.to_string()
    };
    if looks_like_windows_game(&raw) {
        pretty_win_exe(&raw)
    } else {
        raw
    }
}

fn shorten_title(title: &str) -> String {
    let t = title.split(" — ").next().unwrap_or(title);
    let t = t.split(" - ").next().unwrap_or(t).trim();
    if t.chars().count() > 48 {
        let s: String = t.chars().take(45).collect();
        format!("{s}…")
    } else {
        t.to_string()
    }
}

fn is_generic_title(title: &str) -> bool {
    let t = title.to_ascii_lowercase();
    t == "wine" || t == "wine64" || t.starts_with("wine ")
}

fn title_case(stem: &str) -> String {
    stem.split(|c: char| c == '-' || c == '_' || c == '.')
        .filter(|s| !s.is_empty())
        .map(|s| {
            let mut c = s.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn find_ci<'a>(hay: &'a str, needle: &str) -> Option<&'a str> {
    let h = hay.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || h.len() < n.len() {
        return None;
    }
    for i in 0..=h.len() - n.len() {
        if h[i..i + n.len()].eq_ignore_ascii_case(n) {
            return Some(&hay[i + n.len()..]);
        }
    }
    None
}

/// Por que esta linha parece leftover — só com evidência, nunca por “CPU baixa”.
pub fn leftover_reason(
    cmdline: &str,
    kernel_state: Option<char>,
    has_window: bool,
) -> Option<&'static str> {
    leftover_reason_for(cmdline, kernel_state, has_window, Locale::Portuguese)
}

pub fn leftover_reason_for(
    cmdline: &str,
    kernel_state: Option<char>,
    has_window: bool,
    locale: Locale,
) -> Option<&'static str> {
    if kernel_state == Some('Z') {
        return Some(locale.text(
            "zombie: o processo já morreu e o pai não recolheu o estado",
            "zombie: the process is dead and its parent has not reaped it",
        ));
    }
    if !has_window
        && qemu_avd(cmdline).is_some()
        && (cmdline.contains("-qt-hide-window") || cmdline.contains("-no-window"))
    {
        return Some(locale.text(
            "emulador Android sem janela (-qt-hide-window)",
            "Android emulator without a window (-qt-hide-window)",
        ));
    }
    None
}

pub fn state_label(
    kernel_state: Option<char>,
    focused: bool,
    has_window: bool,
    leftover: Option<&str>,
) -> &'static str {
    state_label_for(
        kernel_state,
        focused,
        has_window,
        leftover,
        Locale::Portuguese,
    )
}

pub fn state_label_for(
    kernel_state: Option<char>,
    focused: bool,
    has_window: bool,
    leftover: Option<&str>,
    locale: Locale,
) -> &'static str {
    if kernel_state == Some('Z') {
        return "zombie";
    }
    if leftover.is_some() {
        return "leftover";
    }
    if focused {
        return locale.text("em foco", "focused");
    }
    if has_window {
        return locale.text("janela", "window");
    }
    locale.text("fundo", "background")
}

fn steam_name(id: u32) -> Option<String> {
    static CACHE: OnceLock<Mutex<HashMap<u32, Option<String>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(map) = cache.lock() {
        if let Some(hit) = map.get(&id) {
            return hit.clone();
        }
    }
    let name = read_steam_name(id);
    if let Ok(mut map) = cache.lock() {
        map.insert(id, name.clone());
    }
    name
}

fn read_steam_name(id: u32) -> Option<String> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from)?;
    let file = format!("appmanifest_{id}.acf");
    let candidates = [
        home.join(".local/share/Steam/steamapps").join(&file),
        home.join(".steam/steam/steamapps").join(&file),
        home.join(".steam/root/steamapps").join(&file),
    ];
    for path in candidates {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Some(name) = parse_acf_name(&text) {
                return Some(name);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts<'a>(name: &'a str, exe: &'a str, cmd: &'a str) -> Facts<'a> {
        Facts {
            name,
            exe_path: exe,
            cmdline: cmd,
            ..Facts::default()
        }
    }

    #[test]
    fn steam_client_relaunched_with_rungameid_is_steam_not_the_game() {
        let id = resolve(facts(
            "steam",
            "/home/lol/.local/share/Steam/ubuntu12_32/steam",
            "/home/lol/.local/share/Steam/ubuntu12_32/steam -srt-logger-opened -language brazilian steam://rungameid/2357570//--tank",
        ));
        assert_eq!(id.key, "app:steam");
        assert_eq!(id.label, "Steam");
        assert_eq!(id.kind, Kind::Desktop);
        let helper = resolve(facts(
            "steamwebhelper",
            "/home/lol/.local/share/Steam/ubuntu12_64/steamwebhelper",
            "steamwebhelper -steampid=3313888 -lang=pt_BR",
        ));
        assert_eq!(helper.key, "app:steam");
    }

    #[test]
    fn proton_overwatch_is_not_wine64() {
        let wine =
            "/home/lol/.local/share/Steam/steamapps/common/Proton - Experimental/files/bin/wine64";
        let cmd = format!(
            "{wine} /home/lol/.local/share/Steam/steamapps/compatdata/2357570/pfx/drive_c/Program Files (x86)/Overwatch/_retail_/Overwatch.exe"
        );
        let id = resolve(Facts {
            name: "wine64",
            exe_path: wine,
            cmdline: &cmd,
            steam_app_id: Some(2357570),
            wine_prefix: Some("/home/lol/.local/share/Steam/steamapps/compatdata/2357570/pfx"),
            ..Facts::default()
        });
        assert_eq!(id.key, "steam:2357570");
        assert_eq!(id.label, "Overwatch 2");
        assert_eq!(id.kind, Kind::Game);
    }

    #[test]
    fn two_proton_games_do_not_share_a_group() {
        let wine = "/opt/proton/bin/wine64";
        let a = resolve(Facts {
            name: "wine64",
            exe_path: wine,
            cmdline: "wine64 C:\\Overwatch.exe",
            steam_app_id: Some(2357570),
            ..Facts::default()
        });
        let b = resolve(Facts {
            name: "wine64",
            exe_path: wine,
            cmdline: "wine64 C:\\Cyberpunk2077.exe",
            steam_app_id: Some(1091500),
            ..Facts::default()
        });
        assert_ne!(a.key, b.key);
        assert_eq!(a.label, "Overwatch 2");
        assert_eq!(b.label, "Cyberpunk 2077");
    }

    #[test]
    fn python_venvs_are_the_project_not_python() {
        let a = resolve(facts(
            "python",
            "/home/lol/Projects/sussurro/.venv-tk/bin/python",
            "/home/lol/Projects/sussurro/.venv-tk/bin/python app.py",
        ));
        let b = resolve(facts(
            "python",
            "/home/lol/Projects/sonora/.venv/bin/python",
            "/home/lol/Projects/sonora/.venv/bin/python app.py",
        ));
        assert_eq!(a.key, "project:sussurro");
        assert_eq!(a.label, "Sussurro");
        assert_eq!(b.key, "project:sonora");
        assert_ne!(a.key, b.key);
    }

    #[test]
    fn same_system_python_splits_by_project_path_in_cmdline() {
        let a = resolve(facts(
            "python3",
            "/usr/bin/python3",
            "/usr/bin/python3 /home/lol/Projects/sussurro/app.py",
        ));
        let b = resolve(facts(
            "python3",
            "/usr/bin/python3",
            "/usr/bin/python3 /home/lol/Projects/sonora/app.py",
        ));
        assert_ne!(a.key, b.key);
        assert_eq!(a.label, "Sussurro");
    }

    #[test]
    fn qemu_avd_becomes_named_emulator() {
        let id = resolve(facts(
            "qemu-system-x86_64",
            "/sdk/emulator/qemu-system-x86_64",
            "qemu-system-x86_64 -avd sfr-portfolio -qt-hide-window -no-boot-anim",
        ));
        assert_eq!(id.key, "qemu:sfr-portfolio");
        assert_eq!(id.label, "Emulador Android (sfr-portfolio)");
    }

    #[test]
    fn steam_id_from_compatdata_path() {
        assert_eq!(
            parse_steam_id("/steamapps/compatdata/2357570/pfx"),
            Some(2357570)
        );
        assert_eq!(
            parse_steam_id("steam://rungameid/2357570//--tank"),
            Some(2357570)
        );
    }

    #[test]
    fn acf_name_strips_trademark() {
        let text = "\"appid\"\t\t\"2357570\"\n\t\"name\"\t\t\"Overwatch® 2\"\n";
        assert_eq!(parse_acf_name(text).as_deref(), Some("Overwatch 2"));
    }

    #[test]
    fn cmdline_hides_wine_keeps_game_exe() {
        let cmd = "/opt/proton/bin/wine64 /pfx/drive_c/Program Files/Overwatch.exe --foo";
        assert_eq!(windows_exe_from_cmdline(cmd), Some("Overwatch.exe"));
    }

    #[test]
    fn leftover_needs_evidence() {
        assert_eq!(
            leftover_reason("qemu-system-x86_64 -avd sfr -qt-hide-window", None, false),
            Some("emulador Android sem janela (-qt-hide-window)")
        );
        assert_eq!(leftover_reason("sussurro", None, false), None);
        assert_eq!(
            leftover_reason("wine64", Some('Z'), true),
            Some("zombie: o processo já morreu e o pai não recolheu o estado")
        );
        assert_eq!(state_label(None, true, true, None), "em foco");
        assert_eq!(state_label(None, false, true, None), "janela");
        assert_eq!(state_label(None, false, false, None), "fundo");
    }

    #[test]
    fn unrelated_workers_still_split_by_exe() {
        let a = resolve(facts("worker", "/opt/a/worker", ""));
        let b = resolve(facts("worker", "/opt/b/worker", ""));
        assert_ne!(a.key, b.key);
    }
}
