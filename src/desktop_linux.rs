//! Desktop integration: application icons and window/focus usage history.
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
#[derive(Clone, Default)]
struct Desktop {
    windows: Option<HashSet<u32>>,
    focused: Option<u32>,
}
fn desktop() -> Desktop {
    static CACHE: OnceLock<Arc<Mutex<Desktop>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| {
        let cache = Arc::new(Mutex::new(Desktop::default()));
        let target = cache.clone();
        std::thread::spawn(move || loop {
            let windows = crate::linux::command("hyprctl", &["-j", "clients"])
                .ok()
                .and_then(|s| serde_json::from_str::<Vec<serde_json::Value>>(&s).ok())
                .map(|v| {
                    v.iter()
                        .filter(|w| w["mapped"].as_bool() == Some(true))
                        .filter_map(|w| w["pid"].as_u64().map(|n| n as u32))
                        .collect()
                });
            let focused = crate::linux::command("hyprctl", &["-j", "activewindow"])
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .and_then(|w| w["pid"].as_u64().map(|n| n as u32));
            if let Ok(mut d) = target.lock() {
                *d = Desktop { windows, focused };
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        });
        cache
    });
    cache.lock().map(|d| d.clone()).unwrap_or_default()
}
pub fn windowed_pids() -> Option<HashSet<u32>> {
    desktop().windows
}
pub fn focused_pid() -> Option<u32> {
    desktop().focused
}
fn data_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
        })];
    dirs.extend(
        std::env::var_os("XDG_DATA_DIRS")
            .map(|s| std::env::split_paths(&s).collect::<Vec<_>>())
            .unwrap_or_else(|| vec!["/usr/local/share".into(), "/usr/share".into()]),
    );
    dirs
}
fn icon_index() -> &'static HashMap<String, String> {
    static INDEX: OnceLock<HashMap<String, String>> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut index = HashMap::new();
        for dir in data_dirs() {
            let Ok(entries) = std::fs::read_dir(dir.join("applications")) else {
                continue;
            };
            for e in entries.flatten() {
                let Ok(text) = std::fs::read_to_string(e.path()) else {
                    continue;
                };
                let fields = crate::startup_linux::desktop_fields(&text);
                let (Some(exec), Some(icon)) = (fields.get("Exec"), fields.get("Icon")) else {
                    continue;
                };
                let Ok(words) = crate::linux::words(exec) else {
                    continue;
                };
                let Some(program) = words
                    .iter()
                    .find(|s| s.as_str() != "env" && !s.contains('=') && !s.starts_with('-'))
                else {
                    continue;
                };
                let name = Path::new(program)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                index.entry(name).or_insert_with(|| icon.clone());
            }
        }
        index
    })
}
pub fn icon(path: &str) -> Option<crate::icons::RgbaIcon> {
    let name = Path::new(path).file_name()?.to_string_lossy();
    let icon = icon_index().get(name.as_ref())?;
    let mut candidates = Vec::new();
    if icon.starts_with('/') {
        candidates.push(PathBuf::from(icon));
    } else {
        for dir in data_dirs() {
            for theme in ["hicolor", "Papirus", "Adwaita"] {
                for size in ["32x32", "48x48", "64x64", "128x128", "scalable", "symbolic"] {
                    for ext in ["png", "svg"] {
                        candidates
                            .push(dir.join(format!("icons/{theme}/{size}/apps/{icon}.{ext}")));
                    }
                }
            }
            for ext in ["png", "svg"] {
                candidates.push(dir.join(format!("pixmaps/{icon}.{ext}")));
            }
        }
    }
    for path in candidates {
        if !path.is_file() {
            continue;
        }
        let bytes = if path.extension().is_some_and(|e| e == "svg") {
            let output = std::process::Command::new("timeout")
                .args([
                    "--kill-after=1s",
                    "3s",
                    "rsvg-convert",
                    "-w",
                    "32",
                    "-h",
                    "32",
                ])
                .arg(&path)
                .output()
                .ok()?;
            if !output.status.success() {
                continue;
            }
            output.stdout
        } else {
            match std::fs::read(path) {
                Ok(b) => b,
                Err(_) => continue,
            }
        };
        if let Ok(img) = image::load_from_memory(&bytes) {
            let img = img
                .resize(32, 32, image::imageops::FilterType::Triangle)
                .into_rgba8();
            return Some(crate::icons::RgbaIcon {
                width: img.width() as usize,
                height: img.height() as usize,
                rgba: img.into_raw(),
            });
        }
    }
    None
}
