//! Classificação de processos em categorias (regras + herança do pai + override do usuário).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::procs::ProcInfo;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub enum Category {
    Ai,
    Dev,
    Browser,
    Games,
    Personal,
    System,
    Other,
}

impl Category {
    pub const ALL: [Category; 7] = [
        Category::Ai,
        Category::Dev,
        Category::Browser,
        Category::Games,
        Category::Personal,
        Category::System,
        Category::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Ai => "IA / Agentes",
            Category::Dev => "Dev",
            Category::Browser => "Navegador",
            Category::Games => "Jogos",
            Category::Personal => "Pessoal",
            Category::System => "Sistema",
            Category::Other => "Outros",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Category::Ai => "IA",
            Category::Dev => "Dev",
            Category::Browser => "Web",
            Category::Games => "Jogos",
            Category::Personal => "Pessoal",
            Category::System => "Sistema",
            Category::Other => "Outros",
        }
    }

    pub fn color(self) -> egui::Color32 {
        match self {
            Category::Ai => egui::Color32::from_rgb(168, 120, 255),
            Category::Dev => egui::Color32::from_rgb(90, 190, 255),
            Category::Browser => egui::Color32::from_rgb(255, 170, 60),
            Category::Games => egui::Color32::from_rgb(90, 220, 130),
            Category::Personal => egui::Color32::from_rgb(255, 120, 170),
            Category::System => egui::Color32::from_rgb(150, 150, 160),
            Category::Other => egui::Color32::from_rgb(200, 200, 120),
        }
    }
}

/// Nomes de executáveis (sem .exe, minúsculo) por categoria — regra específica, alta prioridade.
const AI_NAMES: &[&str] = &[
    "codex", "claude", "claude code", "grok", "grok bot", "cursor", "windsurf", "ollama", "ollama app",
    "lm studio", "lmstudio", "maestri", "wispr flow", "wisprflow", "antigravity", "chatgpt", "copilot",
    "comet", "perplexity", "gemini", "opencode", "hermes", "orca", "jan", "gpt4all", "msty", "cline",
    "kiro", "trae", "zed", "aider", "9router", "openclaw", "zcode", "continue",
];
const AI_CMD_HINTS: &[&str] = &[
    "\\.claude\\", "/.claude/", "claude-code", "@anthropic", "anthropic", "\\.codex\\", "/.codex/", "codex",
    "openai", "\\.grok\\", "grok", "mcp-server", "mcp_server", "modelcontextprotocol", "\\mcp\\", "ollama",
    "\\.cursor\\", "cursor-server", "windsurf", "gemini-cli", "@google/gemini", "opencode", "hermes",
    "9router", "maestri", "langchain", "llama", "\\.pi\\", "openclaw", "browser-use", "playwright-mcp",
    "\\claude", "\\codex", "\\grok", "claude extensions", "-mcp", "mcp-", "\\.hermes\\", "hermes-agent",
    "\\orca\\", "orca-terminal", "cua-driver", "computer-use", "windows-mcp",
];
const DEV_NAMES: &[&str] = &[
    "code", "code - insiders", "code-oss", "codium", "devenv", "rider64", "idea64", "pycharm64", "webstorm64", "clion64",
    "goland64", "datagrip64", "studio64", "git", "git-bash", "cargo", "rustc", "rust-analyzer",
    "dotnet", "msbuild", "docker", "docker desktop", "com.docker.backend", "com.docker.build", "dockerd", "containerd",
    "podman", "wsl", "wslservice", "wslhost", "vmmem", "vmmemwsl", "windowsterminal", "openconsole", "alacritty",
    "wezterm-gui", "wezterm", "kitty", "foot", "ghostty", "gnome-terminal", "gnome-terminal-server", "konsole",
    "tilix", "xfce4-terminal", "mintty", "gitkraken", "postman", "insomnia", "tabby", "hyper", "qemu-system-x86_64",
    "emulator", "adb", "gradle", "kotlin", "tsc", "esbuild", "vite", "deno", "bun", "go", "gopls",
    "clangd", "cmake", "ninja", "make", "mingw32-make", "nvim", "vim", "emacs", "ollama-runner", "sqlite3", "redis-server",
    "postgres", "pg_ctl", "mysqld", "mongod", "nginx", "ngrok", "cloudflared", "vagrant", "virtualbox",
    "vboxheadless", "vmware-vmx", "vmplayer", "notepad++", "sublime_text", "fleet", "warp", "orbstack",
];
const BROWSER_NAMES: &[&str] = &[
    "chrome", "brave", "brave browser", "msedge", "firefox", "firefox-bin", "opera", "opera_gx", "vivaldi", "arc",
    "chromium", "chromium-browser", "google-chrome", "google-chrome-stable", "waterfox", "librewolf", "tor",
    "iexplore", "zen",
];
const GAMES_NAMES: &[&str] = &[
    "steam", "steamwebhelper", "steamservice", "epicgameslauncher", "epicwebhelper", "riotclientservices",
    "riotclientux", "leagueclient", "league of legends", "valorant", "valorant-win64-shipping",
    "battle.net", "battle.net helper", "agent", "gog galaxy", "galaxyclient", "eadesktop", "ealauncher",
    "origin", "upc", "ubisoft connect", "ubisoftconnect", "minecraft", "minecraftlauncher", "roblox",
    "robloxplayerbeta", "xboxapp", "xboxpcapp", "gamingservices", "gamebar", "gamebarpresencewriter",
    "gamingservicesnet", "rockstarservice", "launcher", "cs2", "dota2", "fortniteclient-win64-shipping",
    "genshinimpact", "starrail", "wutheringwaves", "playnite", "curseforge", "overwolf", "medal",
];
const PERSONAL_NAMES: &[&str] = &[
    "spotify", "discord", "whatsapp", "telegram", "slack", "teams", "ms-teams", "zoom", "vlc", "obs64",
    "notion", "obsidian", "onenote", "winword", "excel", "powerpnt", "outlook", "olk", "thunderbird",
    "1password", "bitwarden", "todoist", "signal", "skype", "messenger", "netflix", "amazon music",
    "itunes", "applemusic", "musicbee", "foobar2000", "mpc-hc64", "potplayermini64", "steamvr", "kindle",
    "calibre", "acrobat", "acrord32", "sumatrapdf", "foxitpdfreader", "figma", "canva", "photoshop",
    "illustrator", "premiere", "afterfx", "lightroom", "davinci resolve", "resolve", "blender", "krita",
    "gimp-2.10", "paint.net", "paintdotnet", "audacity", "capcut", "wisprflow-tray", "onedrive",
    "dropbox", "googledrivefs", "google drive", "icloud", "megasync", "clipchamp", "snagit32", "sharex",
    "greenshot", "flameshot", "lightshot", "screenpresso", "loom", "streamlabs obs",
];
/// Hosts genéricos: herdam a categoria do pai (node lançado pelo Codex é IA; pelo VS Code é Dev).
const GENERIC_HOSTS: &[&str] = &[
    "node", "nodejs", "python", "python3", "pythonw", "py", "uv", "uvx", "npm", "npx", "pnpm", "yarn",
    "bun", "deno", "conhost", "cmd", "powershell", "pwsh", "bash", "sh", "dash", "zsh", "fish", "login",
    "sudo", "su", "env", "systemd-run", "wsl", "wslhost",
    "msedgewebview2", "java", "javaw", "ruby", "perl", "php", "electron", "webview2", "dotnet",
    "cscript", "wscript", "mshta", "rundll32", "esbuild", "tsserver", "typescript", "cargo", "rustc",
    "link", "cl", "gcc", "g++", "clang", "clang++", "make", "cmake", "ninja", "git", "ssh", "sshd",
    "ssh-agent", "curl", "wget", "tar", "7z", "7zg", "ffmpeg", "ffprobe", "chrome", "chromium",
    "playwright", "chromedriver", "msedgedriver", "geckodriver", "crashpad_handler", "watchdog",
];
const SYSTEM_NAMES: &[&str] = &[
    "hyprland", "hyprland-wrapp", "uwsm", "quickshell", "hypridle", "hyprlock", "waybar",
    "system", "registry", "memory compression", "secure system", "smss", "csrss", "wininit", "winlogon",
    "services", "lsass", "svchost", "fontdrvhost", "dwm", "sihost", "ctfmon", "explorer", "runtimebroker",
    "searchhost", "searchindexer", "searchprotocolhost", "searchfilterhost", "startmenuexperiencehost",
    "shellexperiencehost", "textinputhost", "widgets", "widgetservice", "securityhealthservice",
    "securityhealthsystray", "msmpeng", "nissrv", "mpdefendercoreservice", "audiodg", "spoolsv",
    "applicationframehost", "systemsettings", "taskhostw", "backgroundtaskhost", "wmiprvse", "dllhost",
    "lockapp", "logonui", "userinit", "dashost", "wudfhost", "unsecapp", "conhost", "msiexec",
    "trustedinstaller", "tiworker", "mousocoreworker", "usocoreworker", "wuauclt", "sgrmbroker",
    "sppsvc", "wlanext", "phoneexperiencehost", "yourphone", "crossdeviceservice", "crossdeviceresume",
    "nvcontainer", "nvdisplay.container", "nvidia share", "nvidia web helper", "nvidia app",
    "nvidia overlay", "nvbroadcast", "nvsphelper64", "rtkauduservice64", "rtkaudioservice",
    "igfxem", "igfxcuiservice", "amdrsserv", "radeonsoftware", "atieclxx", "atiesrxx", "aggregatorhost",
    "systemsettingsbroker", "smartscreen", "sppextcomobj", "wmiapsrv", "vssvc", "spoolsv", "dasHost",
    "gamebarftserver", "wscript", "sdxhelper", "officeclicktorun", "msoia", "ai", "vctip", "compattelrunner",
    "musnotifyicon", "musnotification", "wermgr", "werfault", "werfaultsecure", "consent", "credentialuihost",
    "hxtsr", "hxoutlook", "microsoft.photos", "photos", "calculator", "calculatorapp", "notepad", "mspaint",
    "snippingtool", "screenclippinghost", "microsoftedgeupdate", "googleupdate", "googlecrashhandler",
    "googlecrashhandler64", "brave update", "braveupdate", "updater", "adobe crash processor",
    "adobeupdateservice", "armsvc", "ccxprocess", "coresync", "creative cloud", "adobe desktop service",
    "adobeipcbroker", "node_lib", "openvpnserv", "tailscaled", "tailscale-ipn", "wireguard", "wgtunnel",
    "logioptionsplus_agent", "logioptionsplus", "lghub", "lghub_agent", "lghub_updater", "icue", "razer central",
    "razer synapse", "steelseriesgg", "steelseriesengine", "wacom_tablet", "wacomhost", "synaptics",
    "etdctrl", "hidmonitor", "hotkeyservice", "quickshare", "nearby share", "microsoft.sharepoint",
    "wispr flow updater", "powertoys", "powertoys.powerlauncher", "powertoys.awake", "powertoys.fancyzones",
    "powertoys.peek.ui", "powertoys.crophost", "powertoys.keyboardmanagerengine", "powertoys.mousewithoutborders",
    "everything", "listary", "flow.launcher", "translucenttb", "rainmeter", "wallpaper32", "wallpaper64",
    "lively", "displayfusion", "displayfusionhookapp64", "monitorswitcher", "ramdog",
    "systemd", "init", "kthreadd", "dbus-daemon", "dbus-broker", "dbus-broker-launch",
    "systemd-journald", "systemd-logind", "systemd-udevd", "systemd-resolved", "systemd-timesyncd",
    "systemd-networkd", "systemd-oomd", "systemd-homed", "networkmanager", "wpa_supplicant", "iwd",
    "polkitd", "polkit-agent-helper-1", "udisksd", "pipewire", "pipewire-pulse", "wireplumber", "pulseaudio",
    "xorg", "xwayland", "gnome-shell", "gsd-xsettings", "mutter", "kwin_x11", "kwin_wayland", "plasmashell",
    "gdm", "gdm-session-worker", "sddm", "lightdm", "accounts-daemon", "colord", "fwupd", "avahi-daemon",
    "bluetoothd", "modemmanager", "cupsd", "cron", "crond", "rsyslogd", "irqbalance", "snapd",
];

/// Host genérico (shell, runtime, ferramenta) que não diz nada sobre "quem" é o dono do processo.
pub fn is_generic_host(name_lower: &str) -> bool {
    in_list(GENERIC_HOSTS, base_name(name_lower))
}

fn base_name(name_lower: &str) -> &str {
    name_lower.strip_suffix(".exe").unwrap_or(name_lower)
}

fn in_list(list: &[&str], b: &str) -> bool {
    list.iter().any(|n| *n == b)
}

/// Categoria por regra própria (sem olhar o pai). None = genérico / indefinido.
fn own_rule(p: &ProcInfo, base: &str) -> Option<Category> {
    if in_list(AI_NAMES, base) {
        return Some(Category::Ai);
    }
    if in_list(BROWSER_NAMES, base) {
        return Some(Category::Browser);
    }
    if in_list(GAMES_NAMES, base) {
        return Some(Category::Games);
    }
    if in_list(PERSONAL_NAMES, base) {
        return Some(Category::Personal);
    }
    let cmd = p.cmdline.to_lowercase();
    if !cmd.is_empty() && AI_CMD_HINTS.iter().any(|h| cmd.contains(h)) {
        return Some(Category::Ai);
    }
    let path = p.exe_path.to_lowercase();
    if !path.is_empty()
        && (path.contains("\\steam\\") || path.contains("\\steamapps\\") || path.contains("/steam/")
            || path.contains("/steamapps/") || path.contains("\\epic games\\")
            || path.contains("\\riot games\\") || path.contains("\\gog galaxy\\") || path.contains("\\ea games\\")
            || path.contains("\\ubisoft\\") || path.contains("\\battle.net\\") || path.contains("\\xboxgames\\"))
    {
        return Some(Category::Games);
    }
    if in_list(DEV_NAMES, base) && !in_list(GENERIC_HOSTS, base) {
        return Some(Category::Dev);
    }
    if in_list(SYSTEM_NAMES, base) && !in_list(GENERIC_HOSTS, base) {
        return Some(Category::System);
    }
    None
}

fn fallback_rule(p: &ProcInfo, base: &str) -> Category {
    if in_list(DEV_NAMES, base) {
        return Category::Dev;
    }
    if in_list(SYSTEM_NAMES, base) {
        return Category::System;
    }
    let path = p.exe_path.to_lowercase();
    // session 0 no Windows é a sessão de serviços. No Linux o campo começa em 0 quando
    // sysinfo não acha sid — tratar 0 como "sistema" classificava *todo* processo como Sistema.
    if cfg!(windows)
        && (p.session == 0 || path.starts_with("c:\\windows\\") || path.contains("\\windows\\system32\\"))
    {
        return Category::System;
    }
    if is_linux_kernel_thread(base) {
        return Category::System;
    }
    if path.starts_with("/usr/lib/systemd")
        || path.starts_with("/lib/systemd")
        || path.contains("/lib/systemd/")
        || path.starts_with("/usr/libexec/")
    {
        return Category::System;
    }
    if base == "node" || base == "python" || base == "python3" || base == "pythonw" || base == "java" || base == "javaw" {
        return Category::Dev;
    }
    Category::Other
}

/// Classifica todos os processos. `overrides`: nome minúsculo (com .exe) → categoria.
pub fn classify(procs: &[ProcInfo], overrides: &HashMap<String, Category>) -> HashMap<u32, Category> {
    let idx: HashMap<u32, usize> = procs.iter().enumerate().map(|(i, p)| (p.pid, i)).collect();
    let mut result: HashMap<u32, Category> = HashMap::with_capacity(procs.len());
    // Ordem topológica simples: resolve recursivamente com memo.
    fn resolve(
        i: usize,
        procs: &[ProcInfo],
        idx: &HashMap<u32, usize>,
        overrides: &HashMap<String, Category>,
        result: &mut HashMap<u32, Category>,
        depth: usize,
    ) -> Category {
        let p = &procs[i];
        if let Some(c) = result.get(&p.pid) {
            return *c;
        }
        let base = base_name(&p.name_lower).to_string();
        let generic = in_list(GENERIC_HOSTS, &base) || in_list(BROWSER_NAMES, &base);
        let mut parent_cat: Option<Category> = None;
        if depth < 64 && p.ppid != 0 && generic {
            if let Some(&pi) = idx.get(&p.ppid) {
                parent_cat = Some(resolve(pi, procs, idx, overrides, result, depth + 1));
            }
        }
        let cat = if let Some(c) = overrides.get(&p.name_lower) {
            *c
        } else if generic && (parent_cat == Some(Category::Ai) || p.launcher.agent.is_some()) {
            // agentes de IA dirigindo navegadores/hosts (node, chrome, conhost...) contam como IA —
            // inclusive quando o pai já morreu e só a impressão digital do ambiente sobrou
            Category::Ai
        } else if let Some(c) = own_rule(p, &base) {
            c
        } else {
            let inherited = parent_cat.filter(|pc| !matches!(pc, Category::System | Category::Other));
            inherited.unwrap_or_else(|| fallback_rule(p, &base))
        };
        result.insert(p.pid, cat);
        cat
    }
    for i in 0..procs.len() {
        resolve(i, procs, &idx, overrides, &mut result, 0);
    }
    result
}

/// Kernel thread do Linux: nome entre colchetes ou os clássicos do kthreadd.
fn is_linux_kernel_thread(name_lower: &str) -> bool {
    let n = name_lower.trim_start_matches('[').trim_end_matches(']');
    n == "kthreadd"
        || n == "kdevtmpfs"
        || n == "kauditd"
        || n == "kswapd0"
        || n == "kswapd1"
        || n == "kcompactd0"
        || n == "khugepaged"
        || n == "oom_reaper"
        || n == "writeback"
        || n == "kblockd"
        || n == "kintegrityd"
        || n == "kthrotld"
        || n == "kstrp"
        || n == "watchdogd"
        || n.starts_with("kswapd")
        || n.starts_with("kcompactd")
        || n.starts_with("kworker")
        || n.starts_with("ksoftirqd")
        || n.starts_with("migration/")
        || n.starts_with("rcu_")
        || n.starts_with("watchdog/")
        || n.starts_with("cpuhp/")
        || n.starts_with("idle_inject")
        || n.starts_with("irq/")
        || n.starts_with("scsi_")
        || n.starts_with("nvme-")
        || n.starts_with("jbd2/")
        || n.starts_with("ext4-")
        || n.starts_with("ipv6_addrconf")
}

/// Processos que o SO não deixa (ou não deve deixar) matar sem derrubar a sessão.
pub fn is_critical(name_lower: &str, pid: u32) -> bool {
    if pid == 0 || pid == 1 || pid == 2 || pid == 4 {
        return true;
    }
    let base = base_name(name_lower);
    if is_linux_kernel_thread(base) {
        return true;
    }
    matches!(
        base,
        "system" | "registry" | "memory compression" | "secure system" | "smss" | "csrss" | "wininit"
            | "winlogon" | "services" | "lsass" | "fontdrvhost" | "dwm" | "sihost" | "logonui" | "lsaiso"
            | "kernel_task" | "launchd" | "windowserver" | "loginwindow" | "syspolicyd"
            | "systemd" | "init" | "kthreadd" | "dbus-broker" | "dbus-daemon"
            | "hyprland" | "hyprland-wrapp" | "uwsm" | "quickshell" | "gnome-shell" | "kwin_wayland" | "kwin_x11" | "xorg" | "xwayland"
            | "gdm" | "gdm-session-worker" | "sddm" | "lightdm"
    )
}

#[cfg(test)]
mod protection_tests {
    #[test]
    fn omarchy_session_is_protected() {
        for name in ["hyprland", "hyprland-wrapp", "uwsm", "quickshell"] {
            assert!(super::is_critical(name, 4242));
        }
        assert!(!super::is_critical("ordinary-app", 4242));
    }
}
