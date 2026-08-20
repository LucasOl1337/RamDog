//! Persistência em %APPDATA%\RamDog\config.json

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::categories::Category;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Nomes de executáveis (minúsculo, com .exe) protegidos contra encerramento.
    pub locked: BTreeSet<String>,
    /// Override manual de categoria por nome de executável (minúsculo, com .exe).
    pub overrides: BTreeMap<String, Category>,
    pub refresh_ms: u64,
    pub confirm_kill: bool,
    /// Ocultar processos com menos que X MB de RAM privada.
    pub min_mb: u32,
    pub view: ViewMode,
    pub show_system: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum ViewMode {
    List,
    Tree,
    Category,
    Drains,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            locked: BTreeSet::new(),
            overrides: BTreeMap::new(),
            refresh_ms: 1000,
            confirm_kill: true,
            min_mb: 0,
            view: ViewMode::List,
            show_system: true,
        }
    }
}

pub fn config_path() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("RamDog").join("config.json")
}

impl Config {
    pub fn load() -> Self {
        let path = config_path();
        match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let path = config_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let s = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, s).map_err(|e| e.to_string())
    }
}
