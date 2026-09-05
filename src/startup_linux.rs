//! systemd services/timers/sockets and XDG desktop autostart, shared by Partida/Desperdício.
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub enum Source {
    Unit { user: bool, unit: String },
    Desktop { path: PathBuf, filename: String },
}
#[derive(Clone, Debug)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub kind: String,
    pub enabled: bool,
    pub active: bool,
    pub state: String,
    pub can_toggle: bool,
    pub protected: bool,
    pub pid: u32,
    pub memory: Option<u64>,
    pub source: Source,
}
#[derive(Default, Clone)]
pub struct Inventory {
    pub entries: Vec<Entry>,
    pub warnings: Vec<String>,
}

#[derive(Deserialize)]
struct UnitFile {
    unit_file: String,
    state: String,
}
#[derive(Deserialize)]
struct LiveUnit {
    unit: String,
    active: String,
    description: String,
}

pub fn protected(unit: &str) -> bool {
    [
        "dbus",
        "systemd",
        "user@",
        "getty",
        "sshd",
        "NetworkManager",
        "display-manager",
        "sddm",
        "gdm",
        "hyprland",
        "uwsm",
        "quickshell",
        "polkit",
    ]
    .iter()
    .any(|name| unit.starts_with(name))
}

pub fn scan() -> Result<Inventory, String> {
    let mut inventory = Inventory::default();
    for user in [true, false] {
        match scan_units(user) {
            Ok(mut entries) => inventory.entries.append(&mut entries),
            Err(e) => inventory.warnings.push(e),
        }
    }
    inventory.entries.extend(scan_desktops());
    inventory.entries.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(inventory)
}

fn systemctl(user: bool, args: &[&str]) -> Result<String, String> {
    let mut full = vec!["--no-pager"];
    if user {
        full.push("--user");
    }
    full.extend(args);
    crate::linux::command("systemctl", &full)
}

pub fn scan_units(user: bool) -> Result<Vec<Entry>, String> {
    let files: Vec<UnitFile> = serde_json::from_str(&systemctl(
        user,
        &[
            "list-unit-files",
            "--type=service",
            "--type=timer",
            "--type=socket",
            "--output=json",
        ],
    )?)
    .map_err(|e| e.to_string())?;
    let live: Vec<LiveUnit> = serde_json::from_str(&systemctl(
        user,
        &[
            "list-units",
            "--all",
            "--type=service",
            "--type=timer",
            "--type=socket",
            "--output=json",
        ],
    )?)
    .map_err(|e| e.to_string())?;
    let live: HashMap<_, _> = live.into_iter().map(|u| (u.unit.clone(), u)).collect();
    let active_names: Vec<&str> = live
        .values()
        .filter(|u| u.active == "active")
        .map(|u| u.unit.as_str())
        .collect();
    let mut args = vec!["show", "--property=Id,MainPID,MemoryCurrent"];
    args.extend(active_names);
    let properties = if args.len() > 2 {
        systemctl(user, &args).unwrap_or_default()
    } else {
        String::new()
    };
    let details = parse_properties(&properties);
    let mut files: BTreeMap<_, _> = files.into_iter().map(|u| (u.unit_file, u.state)).collect();
    for unit in live.keys() {
        files.entry(unit.clone()).or_insert("transient".into());
    }
    Ok(files
        .into_iter()
        .map(|(name, state)| {
            let running = live.get(&name);
            let detail = details.get(&name).copied().unwrap_or_default();
            let protected = protected(&name);
            Entry {
                id: format!("{}:{name}", if user { "user" } else { "system" }),
                name: name.clone(),
                description: running.map(|r| r.description.clone()).unwrap_or_default(),
                kind: format!(
                    "{} · {}",
                    if user { "Usuário" } else { "Sistema" },
                    name.rsplit('.').next().unwrap_or("unit")
                ),
                enabled: matches!(
                    state.as_str(),
                    "enabled" | "enabled-runtime" | "linked" | "linked-runtime"
                ),
                active: running.is_some_and(|r| r.active == "active"),
                can_toggle: !protected
                    && matches!(state.as_str(), "enabled" | "disabled" | "enabled-runtime"),
                protected,
                state,
                pid: detail.0,
                memory: detail.1,
                source: Source::Unit { user, unit: name },
            }
        })
        .collect())
}

fn parse_properties(text: &str) -> HashMap<String, (u32, Option<u64>)> {
    text.split("\n\n")
        .filter_map(|block| {
            let p: HashMap<_, _> = block.lines().filter_map(|l| l.split_once('=')).collect();
            Some((
                p.get("Id")?.to_string(),
                (
                    p.get("MainPID").and_then(|n| n.parse().ok()).unwrap_or(0),
                    p.get("MemoryCurrent")
                        .and_then(|n| n.parse::<u64>().ok())
                        .filter(|n| *n != u64::MAX),
                ),
            ))
        })
        .collect()
}

pub fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        })
}

pub fn desktop_fields(text: &str) -> BTreeMap<String, String> {
    let mut in_main = false;
    let mut fields = BTreeMap::new();
    for line in text.lines() {
        if line.starts_with('[') {
            in_main = line.trim() == "[Desktop Entry]";
        } else if in_main && !line.trim_start().starts_with('#') {
            if let Some((k, v)) = line.split_once('=') {
                fields.insert(k.trim().into(), v.trim().into());
            }
        }
    }
    fields
}

fn scan_desktops() -> Vec<Entry> {
    let mut dirs = vec![config_home()];
    dirs.extend(
        std::env::var_os("XDG_CONFIG_DIRS")
            .map(|s| std::env::split_paths(&s).collect::<Vec<_>>())
            .unwrap_or_else(|| vec!["/etc/xdg".into()]),
    );
    let mut entries = BTreeMap::new();
    for dir in dirs {
        let Ok(files) = std::fs::read_dir(dir.join("autostart")) else {
            continue;
        };
        for file in files.flatten() {
            let filename = file.file_name().to_string_lossy().into_owned();
            if !filename.ends_with(".desktop") || entries.contains_key(&filename) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(file.path()) else {
                continue;
            };
            let fields = desktop_fields(&text);
            if fields.get("Type").is_none_or(|s| s != "Application") {
                continue;
            }
            let enabled = fields.get("Hidden").is_none_or(|s| s != "true")
                && fields
                    .get("X-GNOME-Autostart-enabled")
                    .is_none_or(|s| s != "false");
            let name = fields
                .get("Name")
                .cloned()
                .unwrap_or_else(|| filename.clone());
            let restrictions = ["OnlyShowIn", "NotShowIn", "TryExec"]
                .iter()
                .filter_map(|k| fields.get(*k).map(|v| format!("{k}={v}")))
                .collect::<Vec<_>>()
                .join("; ");
            entries.insert(
                filename.clone(),
                Entry {
                    id: format!("desktop:{filename}"),
                    name,
                    description: format!(
                        "{} {}",
                        fields
                            .get("Comment")
                            .or_else(|| fields.get("Exec"))
                            .cloned()
                            .unwrap_or_default(),
                        restrictions
                    ),
                    kind: "Autostart XDG".into(),
                    enabled,
                    active: false,
                    state: if enabled {
                        "habilitado"
                    } else {
                        "desabilitado"
                    }
                    .into(),
                    can_toggle: true,
                    protected: false,
                    pid: 0,
                    memory: None,
                    source: Source::Desktop {
                        path: file.path(),
                        filename,
                    },
                },
            );
        }
    }
    entries.into_values().collect()
}

pub fn with_desktop_enabled(text: &str, enabled: bool) -> Result<String, String> {
    if !text.lines().any(|l| l.trim() == "[Desktop Entry]") {
        return Err("Arquivo sem [Desktop Entry]".into());
    }
    let mut result = String::new();
    let mut main = false;
    for line in text.lines() {
        if line.trim().starts_with('[') {
            main = line.trim() == "[Desktop Entry]";
        }
        if main && (line.starts_with("Hidden=") || line.starts_with("X-GNOME-Autostart-enabled=")) {
            continue;
        }
        result.push_str(line);
        result.push('\n');
        if line.trim() == "[Desktop Entry]" {
            result.push_str(if enabled {
                "Hidden=false\nX-GNOME-Autostart-enabled=true\n"
            } else {
                "Hidden=true\nX-GNOME-Autostart-enabled=false\n"
            });
        }
    }
    Ok(result)
}

pub fn toggle(entry: &Entry, enabled: bool) -> Result<(), String> {
    if !entry.can_toggle {
        return Err("Esta entrada é protegida ou gerenciada por outra unidade.".into());
    }
    match &entry.source {
        Source::Unit { user, unit } => {
            unit_action(*user, if enabled { "enable" } else { "disable" }, unit)
        }
        Source::Desktop { path, filename } => {
            let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
            write_desktop(
                &config_home().join("autostart").join(filename),
                &with_desktop_enabled(&text, enabled)?,
            )
        }
    }
}
fn write_desktop(path: &Path, text: &str) -> Result<(), String> {
    std::fs::create_dir_all(path.parent().ok_or("Caminho inválido")?).map_err(|e| e.to_string())?;
    if path.exists() {
        std::fs::copy(path, path.with_extension("desktop.ramdog-backup"))
            .map_err(|e| e.to_string())?;
    }
    let temp = path.with_extension("desktop.ramdog-tmp");
    std::fs::write(&temp, text).map_err(|e| e.to_string())?;
    std::fs::rename(temp, path).map_err(|e| e.to_string())
}

pub fn unit_action(user: bool, action: &str, unit: &str) -> Result<(), String> {
    if protected(unit) {
        return Err("Unidade essencial protegida".into());
    }
    if ![
        "enable",
        "disable",
        "start",
        "stop",
        "restart",
        "reset-failed",
    ]
    .contains(&action)
        || unit.starts_with('-')
        || unit.contains('/')
    {
        return Err("Ação/unidade inválida".into());
    }
    if user {
        systemctl(true, &[action, "--", unit]).map(|_| ())
    } else {
        // Authentication belongs to the individual action, never to the entire GUI.
        let output = std::process::Command::new("timeout")
            .args([
                "--kill-after=2s",
                "120s",
                "pkexec",
                "/usr/bin/systemctl",
                action,
                "--",
                unit,
            ])
            .output()
            .map_err(|e| e.to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn desktop_toggle_preserves_actions() {
        let text="[Desktop Entry]\nType=Application\nName=Olá\nExec=app \"two words\"\nHidden=true\n[Desktop Action Other]\nHidden=true\nExec=other\n";
        let result = with_desktop_enabled(text, true).unwrap();
        assert_eq!(desktop_fields(&result)["Hidden"], "false");
        assert!(result.contains("[Desktop Action Other]\nHidden=true"));
        assert_eq!(
            desktop_fields(&with_desktop_enabled(&result, false).unwrap())["Hidden"],
            "true"
        );
    }
    #[test]
    fn unlimited_memory_is_not_usage() {
        let p=parse_properties("Id=a.service\nMainPID=42\nMemoryCurrent=18446744073709551615\n\nId=b.service\nMainPID=5\nMemoryCurrent=4096\n");
        assert_eq!(p["a.service"], (42, None));
        assert_eq!(p["b.service"], (5, Some(4096)));
    }
}

#[cfg(test)]
mod linux_integration {
    use super::*;
    #[test]
    #[ignore = "Creates and removes an isolated user service"]
    fn user_service_lifecycle() {
        let name = format!("ramdog-integration-{}.service", std::process::id());
        let path = config_home().join("systemd/user").join(&name);
        assert!(!path.exists());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        struct Cleanup(String, PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = unit_action(true, "stop", &self.0);
                let _ = unit_action(true, "disable", &self.0);
                let _ = std::fs::remove_file(&self.1);
                let _ = systemctl(true, &["daemon-reload"]);
            }
        }
        std::fs::write(&path,"[Unit]\nDescription=RamDog integration test\n[Service]\nExecStart=/usr/bin/sleep 120\n[Install]\nWantedBy=default.target\n").unwrap();
        let _cleanup = Cleanup(name.clone(), path);
        systemctl(true, &["daemon-reload"]).unwrap();
        unit_action(true, "enable", &name).unwrap();
        unit_action(true, "start", &name).unwrap();
        let list = scan_units(true).unwrap();
        let entry = list.iter().find(|e| e.name == name).unwrap();
        assert!(entry.enabled && entry.active && entry.pid > 0);
        toggle(entry, false).unwrap();
        unit_action(true, "stop", &name).unwrap();
        let list = scan_units(true).unwrap();
        let entry = list.iter().find(|e| e.name == name).unwrap();
        assert!(!entry.enabled && !entry.active);
    }
}
