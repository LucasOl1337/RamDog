//! Visão "Limpeza" no Linux: RAM (sobras, zombies, apps pesados em segundo plano) e
//! disco (caches do usuário, lixeira, cache do pacman, journal, coredumps, órfãos).
//!
//! O que é do usuário o app apaga direto. O que é do sistema passa pelo `pkexec` com
//! `ramdog --clean-helper <op>`: o helper roda como root, mas só conhece operações fixas
//! e nunca recebe caminho ou nome de pacote pela linha de comando — recalcula tudo lá
//! dentro. Nada passa por shell.

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use egui::{Color32, RichText};

use crate::app::{fmt_bytes, fmt_bytes_short, ACCENT, LINE, MUTED, SURFACE};
use crate::config::Locale;
use crate::identity;
use crate::linux::{self, Job};
use crate::procs::ProcInfo;

pub enum CleanOut {
    Toast(String, bool),
    Kill(Vec<u32>),
}

/// Leitura de `/proc/meminfo`, em bytes.
#[derive(Default, Clone, Copy, Debug, PartialEq)]
pub struct Mem {
    pub total: u64,
    pub available: u64,
    pub free: u64,
    pub cached: u64,
    pub buffers: u64,
    pub reclaimable: u64,
    pub shmem: u64,
    pub swap_total: u64,
    pub swap_free: u64,
}

impl Mem {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.available)
    }
    /// Cache que o kernel solta sozinho sob pressão (page cache + slab recuperável).
    pub fn droppable(&self) -> u64 {
        (self.cached + self.buffers + self.reclaimable).saturating_sub(self.shmem)
    }
}

pub fn meminfo() -> Mem {
    parse_meminfo(&std::fs::read_to_string("/proc/meminfo").unwrap_or_default())
}

fn parse_meminfo(text: &str) -> Mem {
    let mut m = Mem::default();
    for line in text.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let kb: u64 = rest
            .split_whitespace()
            .next()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let b = kb * 1024;
        match key {
            "MemTotal" => m.total = b,
            "MemAvailable" => m.available = b,
            "MemFree" => m.free = b,
            "Cached" => m.cached = b,
            "Buffers" => m.buffers = b,
            "SReclaimable" => m.reclaimable = b,
            "Shmem" => m.shmem = b,
            "SwapTotal" => m.swap_total = b,
            "SwapFree" => m.swap_free = b,
            _ => {}
        }
    }
    m
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Conteúdo de uma pasta do usuário (`~/.cache/<x>`). Apaga o conteúdo, mantém a pasta.
    UserDir,
    Trash,
    /// `paccache -rk1` + `-ruk0`: mantém a versão instalada de cada pacote.
    Pacman,
    /// `journalctl --vacuum-size=64M`.
    Journal,
    Coredump,
    /// `pacman -Rns` no que `pacman -Qtdq` devolve.
    Orphans,
}

impl Kind {
    fn needs_root(self) -> bool {
        matches!(
            self,
            Kind::Pacman | Kind::Journal | Kind::Coredump | Kind::Orphans
        )
    }
    fn helper_op(self) -> &'static str {
        match self {
            Kind::Pacman => "paccache",
            Kind::Journal => "journal",
            Kind::Coredump => "coredump",
            Kind::Orphans => "orphans",
            Kind::UserDir | Kind::Trash => "",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Target {
    pub kind: Kind,
    pub name: String,
    pub path: PathBuf,
    pub detail: String,
    pub bytes: u64,
    pub items: u64,
}

impl Target {
    fn name_for(&self, locale: Locale) -> String {
        match self.kind {
            Kind::UserDir => self.name.clone(),
            Kind::Trash => locale.text("Lixeira", "Trash").to_string(),
            Kind::Pacman => locale.text("Cache do pacman", "Pacman cache").to_string(),
            Kind::Journal => locale
                .text("Journal do systemd", "systemd journal")
                .to_string(),
            Kind::Coredump => "Coredumps".to_string(),
            Kind::Orphans => {
                if locale == Locale::Portuguese {
                    self.name.clone()
                } else {
                    format!("Orphan packages ({})", self.items)
                }
            }
        }
    }

    fn detail_for(&self, locale: Locale) -> String {
        match self.kind {
            Kind::UserDir => self
                .name
                .rsplit('/')
                .next()
                .map(|name| cache_hint_for(name, locale).to_string())
                .unwrap_or_else(|| self.detail.clone()),
            Kind::Trash => locale.text(
                "arquivos apagados pelo gerenciador de arquivos; sem volta depois daqui",
                "files deleted by the file manager; there is no undo after this",
            ).to_string(),
            Kind::Pacman => locale.text(
                "paccache: mantém a versão instalada de cada pacote e apaga as antigas e as desinstaladas",
                "paccache: keeps the installed version of each package and removes older and uninstalled versions",
            ).to_string(),
            Kind::Journal => locale.text(
                "logs antigos; encolhe para 64 MB, o log atual continua",
                "old logs; shrinks to 64 MB while keeping the current log",
            ).to_string(),
            Kind::Coredump => locale.text(
                "despejos de programas que travaram; só servem para depurar (coredumpctl)",
                "dumps from crashed programs; useful only for debugging (coredumpctl)",
            ).to_string(),
            Kind::Orphans => self.detail.clone(),
        }
    }
}

#[derive(Default, Clone, Debug)]
pub struct Report {
    pub targets: Vec<Target>,
    pub warnings: Vec<String>,
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

fn cache_home() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".cache"))
}

fn data_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local/share"))
}

/// Tamanho em disco (blocos, como o `du`) e número de arquivos, sem seguir symlinks.
pub fn du(path: &Path) -> (u64, u64) {
    let mut bytes = 0u64;
    let mut items = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let Ok(md) = e.metadata() else { continue };
            // `DirEntry::metadata` não segue symlink: um link para /usr não conta como /usr.
            if md.file_type().is_symlink() {
                items += 1;
                continue;
            }
            if md.is_dir() {
                stack.push(e.path());
            } else {
                bytes += md.blocks() * 512;
                items += 1;
            }
        }
    }
    (bytes, items)
}

/// Apaga o conteúdo de `dir` (não a pasta). Devolve bytes e itens removidos; erros
/// individuais (arquivo em uso, permissão) são contados, não abortam.
pub fn remove_contents(dir: &Path) -> Result<(u64, u64, u64), String> {
    let rd = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut freed = 0u64;
    let mut removed = 0u64;
    let mut failed = 0u64;
    for e in rd.flatten() {
        let p = e.path();
        let Ok(md) = e.metadata() else {
            failed += 1;
            continue;
        };
        let (b, n) = if md.is_dir() && !md.file_type().is_symlink() {
            du(&p)
        } else {
            (md.blocks() * 512, 1)
        };
        let r = if md.is_dir() && !md.file_type().is_symlink() {
            std::fs::remove_dir_all(&p)
        } else {
            std::fs::remove_file(&p)
        };
        if r.is_ok() {
            freed += b;
            removed += n;
        } else {
            failed += 1;
        }
    }
    Ok((freed, removed, failed))
}

fn journal_bytes() -> Option<u64> {
    // "Archived and active journals take up 376.6M in the file system."
    let out = linux::command("journalctl", &["--disk-usage"]).ok()?;
    let word = out
        .split_whitespace()
        .find(|w| w.ends_with(['B', 'K', 'M', 'G', 'T']))?;
    let (num, unit) = word.split_at(word.len() - 1);
    let num: f64 = num.parse().ok()?;
    // systemd formata em múltiplos de 1024, mesmo escrevendo "M".
    let mult = match unit {
        "K" => 1024.0,
        "M" => 1024.0 * 1024.0,
        "G" => 1024.0 * 1024.0 * 1024.0,
        "T" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => 1.0,
    };
    Some((num * mult) as u64)
}

fn orphan_packages() -> Vec<String> {
    // `pacman -Qtdq` sai com 1 quando não há órfãos; isso não é erro.
    match linux::command("pacman", &["-Qtdq"]) {
        Ok(out) => out
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Pastas de cache que são de um app específico e voltam sozinhas quando ele roda.
fn cache_hint(name: &str) -> &'static str {
    cache_hint_for(name, Locale::Portuguese)
}

fn cache_hint_for(name: &str, locale: Locale) -> &'static str {
    let n = name.to_ascii_lowercase();
    if n.contains("chromium")
        || n.contains("chrome")
        || n.contains("brave")
        || n.contains("firefox")
        || n.contains("mozilla")
    {
        locale.text(
            "cache do navegador; se ele estiver aberto, parte é recriada na hora",
            "browser cache; part of it is recreated while the browser is open",
        )
    } else if n == "yay" || n == "paru" {
        locale.text(
            "clones de AUR; o helper baixa de novo no próximo build",
            "AUR clones; the helper downloads them again on the next build",
        )
    } else if n == "pip"
        || n == "uv"
        || n == "pypoetry"
        || n == "npm"
        || n == "pnpm"
        || n == "yarn"
        || n == "go-build"
        || n == "cargo"
    {
        locale.text(
            "cache de pacotes de linguagem; o próximo install baixa de novo",
            "language package cache; the next install downloads it again",
        )
    } else if n == "thumbnails" {
        locale.text(
            "miniaturas do gerenciador de arquivos; refeitas ao abrir as pastas",
            "file-manager thumbnails; rebuilt when folders are opened",
        )
    } else if n.contains("mesa")
        || n.contains("nvidia")
        || n.contains("shader")
        || n.contains("radv")
    {
        locale.text(
            "cache de shaders; jogos podem engasgar no primeiro minuto depois de limpar",
            "shader cache; games may stutter for the first minute after cleaning",
        )
    } else if n.contains("huggingface")
        || n.contains("torch")
        || n.contains("whisper")
        || n.contains("models")
    {
        locale.text(
            "modelos de IA baixados; volta a baixar (pode ser gigas)",
            "downloaded AI models; they will be downloaded again (possibly gigabytes)",
        )
    } else {
        locale.text(
            "cache; o app recria o que precisar",
            "cache; the app recreates what it needs",
        )
    }
}

pub fn scan() -> Result<Report, String> {
    let mut r = Report::default();
    // ~/.cache/*: cada subpasta é um alvo próprio, para não apagar tudo de uma vez.
    let cache = cache_home();
    if let Ok(rd) = std::fs::read_dir(&cache) {
        let mut dirs: Vec<Target> = rd
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| {
                let (bytes, items) = du(&e.path());
                let name = e.file_name().to_string_lossy().into_owned();
                Target {
                    kind: Kind::UserDir,
                    detail: cache_hint(&name).into(),
                    name: format!("~/.cache/{name}"),
                    path: e.path(),
                    bytes,
                    items,
                }
            })
            .filter(|t| t.bytes >= 1 << 20)
            .collect();
        dirs.sort_by_key(|t| std::cmp::Reverse(t.bytes));
        r.targets.extend(dirs);
    } else {
        r.warnings.push(format!("Sem acesso a {}", cache.display()));
    }
    let trash = data_home().join("Trash");
    if trash.is_dir() {
        let (bytes, items) = du(&trash);
        r.targets.push(Target {
            kind: Kind::Trash,
            name: "Lixeira".into(),
            path: trash,
            detail: "arquivos apagados pelo gerenciador de arquivos; sem volta depois daqui".into(),
            bytes,
            items,
        });
    }
    let pkg = PathBuf::from("/var/cache/pacman/pkg");
    if pkg.is_dir() {
        let (bytes, items) = du(&pkg);
        r.targets.push(Target {
            kind: Kind::Pacman,
            name: "Cache do pacman".into(),
            path: pkg,
            detail: "paccache: mantém a versão instalada de cada pacote e apaga as antigas e as desinstaladas".into(),
            bytes,
            items,
        });
    }
    if let Some(bytes) = journal_bytes() {
        r.targets.push(Target {
            kind: Kind::Journal,
            name: "Journal do systemd".into(),
            path: PathBuf::from("/var/log/journal"),
            detail: "logs antigos; encolhe para 64 MB, o log atual continua".into(),
            bytes,
            items: 0,
        });
    }
    let core = PathBuf::from("/var/lib/systemd/coredump");
    if core.is_dir() {
        let (bytes, items) = du(&core);
        if items > 0 {
            r.targets.push(Target {
                kind: Kind::Coredump,
                name: "Coredumps".into(),
                path: core,
                detail: "despejos de programas que travaram; só servem para depurar (coredumpctl)"
                    .into(),
                bytes,
                items,
            });
        }
    }
    let orphans = orphan_packages();
    if !orphans.is_empty() {
        r.targets.push(Target {
            kind: Kind::Orphans,
            name: format!("Pacotes órfãos ({})", orphans.len()),
            path: PathBuf::from("/"),
            detail: orphans.join(", "),
            bytes: 0,
            items: orphans.len() as u64,
        });
    }
    Ok(r)
}

fn exe() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| format!("executável do RamDog: {e}"))
}

/// Roda `pkexec ramdog --clean-helper <op>`. Sem timeout: a senha pode demorar.
fn run_helper(op: &str) -> Result<String, String> {
    let output = std::process::Command::new("pkexec")
        .arg(exe()?)
        .arg("--clean-helper")
        .arg(op)
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| format!("pkexec: {e}"))?;
    match output.status.code() {
        Some(0) => Ok(String::from_utf8_lossy(&output.stdout).trim().to_string()),
        Some(126) | Some(127) => Err("Autenticação cancelada ou negada".into()),
        _ => {
            let why = String::from_utf8_lossy(&output.stderr);
            let why = if why.trim().is_empty() {
                String::from_utf8_lossy(&output.stdout)
            } else {
                why
            };
            Err(format!(
                "helper ({}): {}",
                output.status,
                why.trim().chars().take(400).collect::<String>()
            ))
        }
    }
}

/// Executa uma limpeza e devolve a frase para o toast.
pub fn apply(t: &Target) -> Result<String, String> {
    apply_for(t, Locale::Portuguese)
}

pub fn apply_for(t: &Target, locale: Locale) -> Result<String, String> {
    match t.kind {
        Kind::UserDir => {
            let (freed, removed, failed) = remove_contents(&t.path)?;
            Ok(done_msg_for(
                &t.name_for(locale),
                freed,
                removed,
                failed,
                locale,
            ))
        }
        Kind::Trash => {
            let mut freed = 0;
            let mut removed = 0;
            let mut failed = 0;
            for sub in ["files", "info", "expunged"] {
                let p = t.path.join(sub);
                if p.is_dir() {
                    let (f, r, x) = remove_contents(&p)?;
                    freed += f;
                    removed += r;
                    failed += x;
                }
            }
            Ok(done_msg_for(
                &t.name_for(locale),
                freed,
                removed,
                failed,
                locale,
            ))
        }
        k => run_helper(k.helper_op()),
    }
}

fn done_msg(name: &str, freed: u64, removed: u64, failed: u64) -> String {
    done_msg_for(name, freed, removed, failed, Locale::Portuguese)
}

fn done_msg_for(name: &str, freed: u64, removed: u64, failed: u64, locale: Locale) -> String {
    let mut s = if locale == Locale::Portuguese {
        format!("{name}: {} liberados, {removed} itens", fmt_bytes(freed))
    } else {
        format!("{name}: {} freed, {removed} items", fmt_bytes(freed))
    };
    if failed > 0 {
        let failed_text = if locale == Locale::Portuguese {
            format!(", {failed} em uso ou sem permissão")
        } else {
            format!(", {failed} still in use or without permission")
        };
        s.push_str(&failed_text);
    }
    s
}

/// `sync` + `echo 3 > drop_caches` + `compact_memory`. Root.
pub fn drop_caches() -> Result<String, String> {
    run_helper("dropcaches")
}

/// Lado root. Só operações fixas; recalcula alvos aqui dentro em vez de confiar em args.
pub fn helper(op: &str) -> Result<(), String> {
    if unsafe { libc::geteuid() } != 0 {
        return Err("O helper de limpeza precisa de autenticação administrativa".into());
    }
    let msg = match op {
        "dropcaches" => {
            let before = meminfo();
            unsafe { libc::sync() };
            std::fs::write("/proc/sys/vm/drop_caches", "3\n")
                .map_err(|e| format!("drop_caches: {e}"))?;
            let _ = std::fs::write("/proc/sys/vm/compact_memory", "1\n");
            let after = meminfo();
            format!(
                "cache do kernel solto: {} → {} livres",
                fmt_bytes(before.free),
                fmt_bytes(after.free)
            )
        }
        "paccache" => {
            let before = du(Path::new("/var/cache/pacman/pkg")).0;
            let a = linux::command("paccache", &["-rk1"])?;
            let b = linux::command("paccache", &["-ruk0"])?;
            let after = du(Path::new("/var/cache/pacman/pkg")).0;
            let _ = (a, b);
            format!(
                "cache do pacman: {} liberados",
                fmt_bytes(before.saturating_sub(after))
            )
        }
        "journal" => {
            let before = journal_bytes().unwrap_or(0);
            linux::command("journalctl", &["--vacuum-size=64M"])?;
            let after = journal_bytes().unwrap_or(0);
            format!(
                "journal: {} liberados",
                fmt_bytes(before.saturating_sub(after))
            )
        }
        "coredump" => {
            let dir = Path::new("/var/lib/systemd/coredump");
            let rd = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            let mut freed = 0;
            let mut n = 0;
            for e in rd.flatten() {
                let Ok(md) = e.metadata() else { continue };
                if md.is_file() && std::fs::remove_file(e.path()).is_ok() {
                    freed += md.blocks() * 512;
                    n += 1;
                }
            }
            format!("coredumps: {} liberados, {n} arquivos", fmt_bytes(freed))
        }
        "orphans" => {
            let orphans = orphan_packages();
            if orphans.is_empty() {
                "nenhum pacote órfão".into()
            } else {
                let mut args = vec!["-Rns", "--noconfirm"];
                args.extend(orphans.iter().map(String::as_str));
                let output = std::process::Command::new("pacman")
                    .args(&args)
                    .env("LC_ALL", "C")
                    .output()
                    .map_err(|e| format!("pacman: {e}"))?;
                if !output.status.success() {
                    return Err(format!(
                        "pacman -Rns: {}",
                        String::from_utf8_lossy(&output.stderr).trim()
                    ));
                }
                format!(
                    "{} pacote(s) órfão(s) removidos: {}",
                    orphans.len(),
                    orphans.join(", ")
                )
            }
        }
        other => return Err(format!("operação desconhecida: {other}")),
    };
    println!("{msg}");
    Ok(())
}

/// Um zombie e o pai que o segura. A faxina de apps parados é da aba Faxina; aqui fica só
/// o que ela não resolve: zombie não morre com sinal, quem resolve é o pai.
struct Zombie {
    pid: u32,
    name: String,
    /// Pai, quando dá para encerrar (não protegido, não o init).
    parent: Option<(u32, String)>,
}

pub struct Clean {
    scan: Job<Report>,
    action: Job<String>,
    drop: Job<String>,
    /// Alvo (índice em `targets`) esperando confirmação.
    pending: Option<usize>,
    last_result: Option<(String, bool)>,
}

impl Default for Clean {
    fn default() -> Self {
        Self::new()
    }
}

impl Clean {
    pub fn new() -> Self {
        Self {
            scan: Job::default(),
            action: Job::default(),
            drop: Job::default(),
            pending: None,
            last_result: None,
        }
    }

    fn zombies(procs: &[ProcInfo], locked: &dyn Fn(&ProcInfo) -> bool) -> Vec<Zombie> {
        let by_pid: HashMap<u32, &ProcInfo> = procs.iter().map(|p| (p.pid, p)).collect();
        procs
            .iter()
            .filter(|p| p.kernel_state == Some('Z') && p.pid > 2)
            .map(|p| Zombie {
                pid: p.pid,
                name: p.name.clone(),
                parent: by_pid
                    .get(&p.raw_ppid)
                    .filter(|pp| !locked(pp) && pp.pid > 1)
                    .map(|pp| (pp.pid, identity::of(pp).label)),
            })
            .collect()
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        procs: &[ProcInfo],
        _mem: &dyn Fn(&ProcInfo) -> u64,
        locked: &dyn Fn(&ProcInfo) -> bool,
        locale: Locale,
    ) -> Vec<CleanOut> {
        let mut out = Vec::new();
        self.scan.poll();
        if self.action.poll() {
            match (&self.action.error, &self.action.value) {
                (Some(e), _) => self.last_result = Some((e.clone(), true)),
                (None, msg) if !msg.is_empty() => self.last_result = Some((msg.clone(), false)),
                _ => {}
            }
            self.action.value.clear();
            self.scan.start(scan);
        }
        if self.drop.poll() {
            match (&self.drop.error, &self.drop.value) {
                (Some(e), _) => self.last_result = Some((e.clone(), true)),
                (None, msg) if !msg.is_empty() => self.last_result = Some((msg.clone(), false)),
                _ => {}
            }
            self.drop.value.clear();
        }
        if self.scan.due(60) {
            self.scan.start(scan);
        }
        if let Some((msg, err)) = self.last_result.take() {
            out.push(CleanOut::Toast(msg, err));
        }

        crate::kit::intro(ui, locale.text("Cache do kernel, zombies e o que está ocupando disco sem precisar. Nada aqui é apagado sem um clique de confirmação. Apps abertos sem uso ficam na aba Faxina.", "Kernel cache, zombies, and what is occupying disk unnecessarily. Nothing is deleted without a confirmation click. Unused open apps live in the Sweep tab."));
        ui.add_space(8.0);

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.ui_memory(ui, procs, locale);
                ui.add_space(10.0);
                self.ui_processes(ui, procs, locked, locale, &mut out);
                ui.add_space(10.0);
                self.ui_disk(ui, locale);
            });
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs(1));
        out
    }

    fn ui_memory(&mut self, ui: &mut egui::Ui, _procs: &[ProcInfo], locale: Locale) {
        // A leitura de meminfo é barata; refaz a cada frame para a barra acompanhar o kill.
        let m = meminfo();
        section(ui, "RAM", |ui| {
            let total = m.total.max(1);
            let used = m.used();
            let drop = m.droppable().min(total.saturating_sub(used));
            let w = ui.available_width().min(720.0);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 14.0), egui::Sense::hover());
            let p = ui.painter();
            p.rect_filled(rect, 3.0, SURFACE);
            let x_used = rect.left() + rect.width() * used as f32 / total as f32;
            let x_cache = x_used + rect.width() * drop as f32 / total as f32;
            p.rect_filled(
                egui::Rect::from_min_max(rect.min, egui::pos2(x_used, rect.max.y)),
                3.0,
                ACCENT,
            );
            p.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(x_used, rect.min.y),
                    egui::pos2(x_cache, rect.max.y),
                ),
                0.0,
                ACCENT.gamma_multiply(0.35),
            );
            p.rect_stroke(
                rect,
                3.0,
                egui::Stroke::new(1.0_f32, LINE),
                egui::StrokeKind::Inside,
            );
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!(
                        "{} {}",
                        fmt_bytes(used),
                        locale.text("em uso", "in use")
                    ))
                    .color(ACCENT)
                    .strong(),
                );
                ui.label(RichText::new("·").color(MUTED));
                ui.label(format!(
                    "{} {}",
                    fmt_bytes(drop),
                    locale.text("de cache do kernel", "kernel cache")
                ));
                ui.label(RichText::new("·").color(MUTED));
                ui.label(format!(
                    "{} {} {}",
                    fmt_bytes(m.available),
                    locale.text("disponíveis de", "available of"),
                    fmt_bytes(m.total)
                ));
                if m.swap_total > 0 {
                    ui.label(RichText::new("·").color(MUTED));
                    ui.label(format!(
                        "swap {} / {}",
                        fmt_bytes(m.swap_total - m.swap_free),
                        fmt_bytes(m.swap_total)
                    ));
                }
            });
            ui.horizontal(|ui| {
                let b = ui.add_enabled(!self.drop.busy(), crate::kit::button(locale.text("Soltar cache do kernel", "Drop kernel cache")));
                if b.on_hover_text(locale.text("sync + drop_caches + compact_memory. Pede senha.\n\nO kernel já solta esse cache sozinho quando um app precisa; isso aqui só antecipa. Vale antes de um jogo ou pra ver a RAM \"de verdade\". Arquivos recém-usados voltam a ser lidos do disco.", "sync + drop_caches + compact_memory. Requires a password.\n\nThe kernel releases this cache automatically when an app needs it; this only brings it forward. Useful before a game or to see \"real\" RAM. Recently used files will be read from disk again.")).clicked() {
                    self.drop.start(drop_caches);
                }
                self.drop.status(ui);
            });
        });
    }

    fn ui_processes(
        &mut self,
        ui: &mut egui::Ui,
        procs: &[ProcInfo],
        locked: &dyn Fn(&ProcInfo) -> bool,
        locale: Locale,
        out: &mut Vec<CleanOut>,
    ) {
        let zombies = Self::zombies(procs, locked);
        let title = format!("Zombies · {}", zombies.len());
        section(ui, &title, |ui| {
            ui.label(RichText::new(locale.text("Apps abertos sem uso ficam na aba Faxina, que classifica e fecha em massa. Aqui sobram os zombies: já morreram, e quem resolve é o pai.", "Unused open apps live in the Sweep tab, which sorts and closes them in bulk. What remains here are zombies: already dead, fixed only by their parent.")).color(MUTED));
            ui.add_space(4.0);
            if zombies.is_empty() {
                ui.label(
                    RichText::new(locale.text("Nenhum zombie agora.", "No zombies right now."))
                        .color(MUTED),
                );
                return;
            }
            for z in &zombies {
                ui.horizontal(|ui| {
                    ui.label(format!("{} ({})", z.name, z.pid));
                    match &z.parent {
                        Some((ppid, pname)) => {
                            if ui.small_button(format!("{} · {pname} ({ppid})", locale.text("encerrar pai", "terminate parent"))).on_hover_text(locale.text("Zombie não morre com sinal: já está morto. Some quando o pai recolhe o estado ou quando o pai cai.", "A zombie does not die from a signal: it is already dead. It disappears when the parent reaps it or exits.")).clicked() {
                                out.push(CleanOut::Kill(vec![*ppid]));
                            }
                        }
                        None => {
                            ui.label(RichText::new(locale.text("pai protegido", "parent protected")).color(MUTED).small());
                        }
                    }
                });
            }
        });
    }

    fn ui_disk(&mut self, ui: &mut egui::Ui, locale: Locale) {
        let total: u64 = self.scan.value.targets.iter().map(|t| t.bytes).sum();
        let title = if self.scan.value.targets.is_empty() {
            locale.text("Disco", "Disk").to_string()
        } else {
            format!(
                "{} · {} {}",
                locale.text("Disco", "Disk"),
                fmt_bytes(total),
                locale.text("recuperáveis", "reclaimable")
            )
        };
        section(ui, &title, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(locale.text("Caches do usuário, lixeira, cache do pacman, journal, coredumps e órfãos. Itens com 🔒 pedem senha (pkexec).", "User caches, trash, pacman cache, journal, coredumps, and orphans. 🔒 items require a password (pkexec).")).color(MUTED));
                if ui.add_enabled(!self.scan.busy(), crate::kit::button(locale.text("Atualizar", "Refresh"))).clicked() {
                    self.scan.start(scan);
                }
                self.scan.status(ui);
                self.action.status(ui);
            });
            for w in &self.scan.value.warnings {
                ui.colored_label(Color32::YELLOW, w);
            }
            if self.scan.value.targets.is_empty() && !self.scan.busy() {
                ui.label(
                    RichText::new(locale.text(
                        "Nada acima de 1 MB para limpar.",
                        "Nothing above 1 MB to clean.",
                    ))
                    .color(MUTED),
                );
                return;
            }
            ui.add_space(4.0);
            let targets = self.scan.value.targets.clone();
            // O Grid dá à coluna de texto só o que sobra depois dos botões; sem largura fixa
            // a descrição vira "cach…".
            let detail_w = (ui.available_width() - 560.0).clamp(200.0, 700.0);
            egui::Grid::new("clean-disk")
                .num_columns(4)
                .spacing([12.0, 4.0])
                .striped(true)
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(locale.text("Alvo", "Target"))
                            .color(MUTED)
                            .small(),
                    );
                    ui.label(
                        RichText::new(locale.text("Tamanho", "Size"))
                            .color(MUTED)
                            .small(),
                    );
                    ui.label(
                        RichText::new(locale.text("O que é", "Purpose"))
                            .color(MUTED)
                            .small(),
                    );
                    ui.label("");
                    ui.end_row();
                    for (i, t) in targets.iter().enumerate() {
                        let localized_name = t.name_for(locale);
                        let localized_detail = t.detail_for(locale);
                        let name = if t.kind.needs_root() {
                            format!("🔒 {localized_name}")
                        } else {
                            localized_name
                        };
                        ui.label(name).on_hover_text(t.path.display().to_string());
                        ui.label(if t.bytes > 0 {
                            fmt_bytes_short(t.bytes)
                        } else {
                            "–".into()
                        });
                        ui.allocate_ui_with_layout(
                            egui::vec2(detail_w, 18.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.add(
                                    egui::Label::new(RichText::new(&localized_detail).color(MUTED))
                                        .truncate(),
                                )
                                .on_hover_text(&localized_detail);
                            },
                        );
                        ui.horizontal(|ui| {
                            if self.pending == Some(i) {
                                let what = match t.kind {
                                    Kind::Orphans => {
                                        if locale == Locale::Portuguese {
                                            format!("Remover {} pacote(s)?", t.items)
                                        } else {
                                            format!("Remove {} package(s)?", t.items)
                                        }
                                    }
                                    Kind::Journal => locale
                                        .text("Encolher para 64 MB?", "Shrink to 64 MB?")
                                        .to_string(),
                                    _ => {
                                        if locale == Locale::Portuguese {
                                            format!(
                                                "Apagar {} ({} itens)?",
                                                fmt_bytes_short(t.bytes),
                                                t.items
                                            )
                                        } else {
                                            format!(
                                                "Delete {} ({} items)?",
                                                fmt_bytes_short(t.bytes),
                                                t.items
                                            )
                                        }
                                    }
                                };
                                ui.label(
                                    RichText::new(what).color(Color32::from_rgb(230, 170, 90)),
                                );
                                if ui
                                    .add(crate::kit::danger(locale.text("Sim", "Yes")))
                                    .clicked()
                                {
                                    let t = t.clone();
                                    self.action.start(move || apply_for(&t, locale));
                                    self.pending = None;
                                }
                                if ui
                                    .add(crate::kit::button(locale.text("Não", "No")))
                                    .clicked()
                                {
                                    self.pending = None;
                                }
                            } else if ui
                                .add_enabled(
                                    !self.action.busy(),
                                    crate::kit::button(locale.text("Limpar", "Clean")),
                                )
                                .clicked()
                            {
                                self.pending = Some(i);
                            }
                        });
                        ui.end_row();
                    }
                });
        });
    }
}

/// Bloco com título: caixa do fundo da janela dentro do card, como as linhas dos addons.
fn section(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(crate::app::BG)
        .corner_radius(crate::kit::ROW_R)
        .inner_margin(12.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).strong().size(14.0));
            ui.add_space(4.0);
            body(ui);
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meminfo_parses_kb_into_bytes() {
        let m = parse_meminfo("MemTotal:  1000 kB\nMemAvailable: 400 kB\nCached: 300 kB\nShmem: 50 kB\nSwapTotal: 0 kB\n");
        assert_eq!(m.total, 1_024_000);
        assert_eq!(m.used(), 600 * 1024);
        assert_eq!(m.droppable(), 250 * 1024);
    }

    #[test]
    fn du_and_remove_contents_keep_the_dir_and_skip_symlink_targets() {
        let dir = std::env::temp_dir().join(format!("ramdog-clean-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::write(dir.join("a/b/f"), vec![0u8; 8192]).unwrap();
        std::fs::write(dir.join("g"), b"x").unwrap();
        let outside = std::env::temp_dir().join(format!("ramdog-clean-out-{}", std::process::id()));
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("keep"), b"keep").unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("link")).unwrap();
        let (bytes, items) = du(&dir);
        assert!(bytes >= 8192, "{bytes}");
        assert_eq!(items, 3);
        let (_, removed, failed) = remove_contents(&dir).unwrap();
        assert_eq!(removed, 3);
        assert_eq!(failed, 0);
        assert!(dir.is_dir());
        assert!(std::fs::read_dir(&dir).unwrap().next().is_none());
        assert!(
            outside.join("keep").is_file(),
            "symlink alvo não pode ser apagado"
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn helper_refuses_unknown_ops_without_root() {
        assert!(helper("rm-rf").is_err());
    }
}
