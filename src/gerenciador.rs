//! Modo Gerenciador: a pergunta "quais apps eu tenho abertos", numa janela flutuante.
//!
//! Roda como uma instância própria (`ramdog --gerenciador`, classe `ramdog-gerenciador`)
//! para ter regra de janela e atalho próprios sem mexer na janela da lista completa.
//! Só lê a config: quem grava é a instância principal.

pub mod model;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use egui::{Align, Color32, Layout, Rect, RichText, Sense, Stroke, TextureHandle, Vec2};

use crate::app::{fmt_bytes_short, set_number_locale};
use crate::categories::{self, is_critical, Category};
use crate::config::{Config, Locale};
use crate::metrics::SysSample;
use crate::procs::{self, MemStatus, ProcInfo};
use crate::sampler::{self, SamplerHandle};
use model::{Entry, Section, SortKey, Win};

pub const APP_ID: &str = "ramdog-gerenciador";
const W: f32 = 880.0;
const H: f32 = 580.0;
const MIN_W: f32 = 620.0;
const MIN_H: f32 = 380.0;
const ROW_H: f32 = 42.0;
const ICON: f32 = 22.0;
/// Quanto esperar um app fechar sozinho antes de oferecer o Forçar.
const CLOSE_GRACE: Duration = Duration::from_secs(5);
/// Em Segundo plano, abaixo disso a linha fica recolhida em "+N pequenos".
const SMALL_RAM: u64 = 50 * 1024 * 1024;
const SMALL_CPU: f32 = 0.5;

pub fn wanted() -> bool {
    std::env::args().any(|a| a == "--gerenciador")
}

pub fn run() -> eframe::Result<()> {
    #[cfg(target_os = "linux")]
    if std::env::args().any(|a| a == "--dump") {
        dump();
        return Ok(());
    }
    procs::enable_debug_privilege();
    let viewport = egui::ViewportBuilder::default()
        .with_title("Gerenciador · RamDog")
        .with_app_id(APP_ID)
        .with_icon(crate::app_icon())
        .with_inner_size([W, H])
        .with_min_inner_size([MIN_W, MIN_H])
        .with_decorations(false);
    let options = eframe::NativeOptions {
        viewport,
        vsync: !cfg!(target_os = "linux"),
        ..Default::default()
    };
    eframe::run_native(
        "RamDog Gerenciador",
        options,
        Box::new(|cc| Ok(Box::new(Gerenciador::new(cc)))),
    )
}

// ---------- cores: tema do Omarchy quando houver, paleta do RamDog senão ----------

#[derive(Clone, Copy)]
struct Palette {
    dark: bool,
    bg: Color32,
    surface: Color32,
    hover: Color32,
    line: Color32,
    text: Color32,
    muted: Color32,
    accent: Color32,
    selected: Color32,
    danger: Color32,
}

fn hex(s: &str) -> Option<Color32> {
    let s = s.trim().trim_matches('"').trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some(Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

impl Palette {
    fn ramdog() -> Self {
        Self {
            dark: true,
            bg: crate::app::BG,
            surface: crate::app::PANEL,
            hover: crate::app::SURFACE,
            line: crate::app::LINE,
            text: crate::app::TEXT,
            muted: crate::app::MUTED,
            accent: crate::app::ACCENT,
            selected: crate::app::ACCENT_BG,
            danger: Color32::from_rgb(224, 108, 117),
        }
    }

    /// `colors.toml` do tema atual do Omarchy: fundo, texto e acento viram os do sistema.
    fn from_toml(text: &str) -> Option<Self> {
        let mut kv: HashMap<&str, &str> = HashMap::new();
        for line in text.lines() {
            if let Some((k, v)) = line.split_once('=') {
                kv.insert(k.trim(), v.trim());
            }
        }
        let bg = hex(kv.get("background")?)?;
        let text = hex(kv.get("foreground")?)?;
        let accent = hex(kv.get("accent")?)?;
        let dark = kv.get("mode").map(|m| !m.contains("light")).unwrap_or(true);
        let surface = kv
            .get("lighter_background")
            .and_then(|v| hex(v))
            .unwrap_or_else(|| mix(bg, text, 0.05));
        Some(Self {
            dark,
            bg,
            surface,
            hover: mix(surface, text, 0.06),
            line: mix(bg, text, 0.12),
            text,
            muted: kv
                .get("light_foreground")
                .and_then(|v| hex(v))
                .unwrap_or_else(|| mix(bg, text, 0.55)),
            accent,
            selected: mix(bg, accent, 0.22),
            danger: kv
                .get("red")
                .and_then(|v| hex(v))
                .unwrap_or(Color32::from_rgb(224, 108, 117)),
        })
    }

    fn load() -> Self {
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        let state = std::env::var_os("XDG_STATE_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| home.as_ref().map(|h| h.join(".local/state")));
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| home.as_ref().map(|h| h.join(".config")));
        [state, config]
            .into_iter()
            .flatten()
            .map(|d| d.join("omarchy/current/theme/colors.toml"))
            .find_map(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| Self::from_toml(&t))
            .unwrap_or_else(Self::ramdog)
    }

    fn apply(&self, ctx: &egui::Context) {
        crate::app::setup_fonts(ctx);
        let mut v = if self.dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        v.panel_fill = self.bg;
        v.window_fill = self.surface;
        v.extreme_bg_color = self.surface;
        v.override_text_color = Some(self.text);
        v.window_stroke = Stroke::new(1.0_f32, self.line);
        v.selection.bg_fill = self.selected;
        v.selection.stroke = Stroke::new(1.0_f32, self.accent);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, self.line);
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, self.text);
        v.widgets.inactive.bg_fill = self.surface;
        v.widgets.inactive.weak_bg_fill = self.surface;
        v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, self.line);
        v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, self.text);
        v.widgets.hovered.bg_fill = self.hover;
        v.widgets.hovered.weak_bg_fill = self.hover;
        v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, self.accent);
        v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, self.text);
        v.widgets.active.bg_fill = self.selected;
        v.widgets.active.weak_bg_fill = self.selected;
        v.widgets.active.bg_stroke = Stroke::new(1.0_f32, self.accent);
        v.widgets.active.fg_stroke = Stroke::new(1.0_f32, self.text);
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = 6.0.into();
        }
        ctx.set_visuals(v);
        ctx.style_mut(|s| {
            use egui::{FontFamily, FontId, TextStyle};
            s.text_styles
                .insert(TextStyle::Body, FontId::new(13.0, FontFamily::Proportional));
            s.text_styles.insert(
                TextStyle::Button,
                FontId::new(13.0, FontFamily::Proportional),
            );
            s.text_styles.insert(
                TextStyle::Small,
                FontId::new(11.5, FontFamily::Proportional),
            );
            s.spacing.item_spacing = egui::vec2(8.0, 4.0);
            s.spacing.button_padding = egui::vec2(12.0, 5.0);
            s.interaction.selectable_labels = false;
            s.interaction.tooltip_delay = 0.35;
            s.animation_time = 0.0;
        });
    }
}

// ---------- estado ----------

/// Pedido de fechar em andamento: quem era (PID + criação) e desde quando.
struct Closing {
    name: String,
    identities: Vec<(u32, i64)>,
    at: Instant,
}

struct Gerenciador {
    cfg: Config,
    pal: Palette,
    sampler: SamplerHandle,
    procs: Vec<ProcInfo>,
    idx: HashMap<u32, usize>,
    mem: MemStatus,
    sys: SysSample,
    icons: HashMap<String, Option<TextureHandle>>,
    entries: Vec<Entry>,
    samples: u32,
    selected: Option<String>,
    search: String,
    focus_search: bool,
    sort: SortKey,
    desc: bool,
    show_system: bool,
    show_small: bool,
    closing: HashMap<String, Closing>,
    confirm: Option<String>,
    status: Option<(Instant, String, bool)>,
    list_rect: Option<Rect>,
    /// Próxima janela a focar em apps com várias: o Mostrar repetido passa por todas.
    cycle: HashMap<String, usize>,
    scroll_to_selected: bool,
    logo: Option<TextureHandle>,
}

impl Gerenciador {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let pal = Palette::load();
        pal.apply(&cc.egui_ctx);
        #[cfg(target_os = "linux")]
        crate::desktop_linux::set_poll_ms(1000);
        let cfg = Config::load();
        let logo = image::load_from_memory(include_bytes!("../assets/ramdog-256.png"))
            .ok()
            .map(|img| {
                let img = img
                    .resize(64, 64, image::imageops::FilterType::Triangle)
                    .into_rgba8();
                let (w, h) = img.dimensions();
                let ci = egui::ColorImage::from_rgba_unmultiplied(
                    [w as usize, h as usize],
                    &img.into_raw(),
                );
                cc.egui_ctx
                    .load_texture("ramdog-logo", ci, egui::TextureOptions::LINEAR)
            });
        Self {
            sampler: sampler::spawn(cc.egui_ctx.clone(), 1000),
            cfg,
            pal,
            procs: Vec::new(),
            idx: HashMap::new(),
            mem: MemStatus::default(),
            sys: SysSample::default(),
            icons: HashMap::new(),
            entries: Vec::new(),
            samples: 0,
            selected: None,
            search: String::new(),
            focus_search: true,
            sort: SortKey::Ram,
            desc: true,
            show_system: false,
            show_small: false,
            closing: HashMap::new(),
            confirm: None,
            status: None,
            list_rect: None,
            cycle: HashMap::new(),
            scroll_to_selected: false,
            logo,
        }
    }

    fn t(&self, pt: &'static str, en: &'static str) -> &'static str {
        self.cfg.locale.text(pt, en)
    }

    fn pt(&self) -> bool {
        self.cfg.locale == Locale::Portuguese
    }

    fn toast(&mut self, msg: String, err: bool) {
        self.status = Some((Instant::now(), msg, err));
    }

    fn ingest(&mut self, ctx: &egui::Context) {
        let mut got = None;
        while let Ok(s) = self.sampler.rx.try_recv() {
            got = Some(s);
        }
        let Some(snap) = got else { return };
        for (key, icon) in snap.new_icons {
            let tex = icon.map(|ic| {
                let img = egui::ColorImage::from_rgba_unmultiplied([ic.width, ic.height], &ic.rgba);
                ctx.load_texture(format!("icon:{key}"), img, egui::TextureOptions::LINEAR)
            });
            self.icons.insert(key, tex);
        }
        self.procs = snap.procs;
        self.mem = snap.mem;
        let keep_cpu = snap
            .sys
            .cpu_pct
            .is_none()
            .then_some(self.sys.cpu_pct)
            .flatten();
        self.sys = snap.sys;
        if keep_cpu.is_some() {
            self.sys.cpu_pct = keep_cpu;
        }
        #[cfg(target_os = "linux")]
        {
            self.sys.gpu = self.sys.gpu_linux.cards.first().cloned();
        }
        self.samples += 1;
        self.rebuild(ctx);
    }

    fn rebuild(&mut self, ctx: &egui::Context) {
        self.idx = self
            .procs
            .iter()
            .enumerate()
            .map(|(i, p)| (p.pid, i))
            .collect();
        let overrides: HashMap<String, Category> = self
            .cfg
            .overrides
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        let cats = categories::classify(&self.procs, &overrides);
        let windows = windows();
        let me = std::process::id();
        let locked = &self.cfg.locked;
        let home = std::env::var("HOME").ok();
        let mem = |p: &ProcInfo| Some(mem_of(p));
        let protected = |p: &ProcInfo| {
            is_critical(&p.name_lower, p.pid) || locked.contains(&p.name_lower) || p.pid == me
        };
        let cwd = |pid: u32| cwd_of(pid);
        let app_name = |exe: &str, class: &str| app_name(exe, class);
        let mut entries = model::build(&model::Inputs {
            procs: &self.procs,
            windows: &windows,
            cats: &cats,
            mem: &mem,
            protected: &protected,
            cwd: &cwd,
            app_name: &app_name,
            home: home.as_deref(),
            system: &other_user,
            me,
        });
        let hovering = match (self.list_rect, ctx.pointer_latest_pos()) {
            (Some(r), Some(p)) => r.contains(p),
            _ => false,
        };
        if hovering && !self.entries.is_empty() {
            let previous: Vec<String> = self.entries.iter().map(|e| e.key.clone()).collect();
            entries = model::keep_order(entries, &previous);
        } else {
            model::sort(&mut entries, self.sort, self.desc);
        }
        self.entries = entries;

        // Quem pediu para fechar e já saiu vira aviso; quem segue vivo continua marcado.
        let alive = |ids: &[(u32, i64)], idx: &HashMap<u32, usize>, procs: &[ProcInfo]| {
            ids.iter().any(|(pid, created)| {
                idx.get(pid)
                    .is_some_and(|&i| procs[i].create_time == *created)
            })
        };
        let mut done = Vec::new();
        for (key, c) in &self.closing {
            if !alive(&c.identities, &self.idx, &self.procs) {
                done.push((key.clone(), c.name.clone()));
            }
        }
        for (key, name) in done {
            self.closing.remove(&key);
            if self.selected.as_deref() == Some(key.as_str()) {
                self.selected = None;
            }
            let msg = if self.pt() {
                format!("{name} fechado")
            } else {
                format!("{name} closed")
            };
            self.toast(msg, false);
        }
    }

    fn entry(&self, key: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.key == key)
    }

    fn selected_entry(&self) -> Option<Entry> {
        self.selected
            .as_deref()
            .and_then(|k| self.entry(k))
            .cloned()
    }

    fn is_small(e: &Entry) -> bool {
        e.ram < SMALL_RAM && e.cpu < SMALL_CPU
    }

    /// Linhas na ordem da tela, já com busca e seções recolhidas aplicadas.
    fn visible(&self) -> Vec<&Entry> {
        let searching = !self.search.trim().is_empty();
        self.entries
            .iter()
            .filter(|e| {
                if searching {
                    return model::matches(e, &self.procs, &self.idx, &self.search);
                }
                match e.section {
                    Section::Apps => true,
                    Section::Background => self.show_small || !Self::is_small(e),
                    Section::System => self.show_system,
                }
            })
            .collect()
    }

    fn alive(&self, pid: u32, created: i64) -> bool {
        self.idx
            .get(&pid)
            .is_some_and(|&i| self.procs[i].create_time == created)
            && std::path::Path::new(&format!("/proc/{pid}")).exists()
            || (!cfg!(target_os = "linux") && self.idx.contains_key(&pid))
    }

    // ---------- ações ----------

    fn show(&mut self, e: &Entry) {
        if e.windows.is_empty() {
            self.details(e);
            return;
        }
        let i = self.cycle.get(&e.key).copied().unwrap_or(0) % e.windows.len();
        self.cycle.insert(e.key.clone(), i + 1);
        if let Err(err) = focus_window(&e.windows[i].address, self.cfg.locale) {
            self.toast(err, true);
        }
    }

    /// Fechar educado: pede às janelas para fechar (o app pode perguntar se salva) ou,
    /// sem janela, manda SIGTERM. O Forçar só aparece se não sair em alguns segundos.
    fn close(&mut self, e: &Entry) {
        if e.protected {
            let msg = if self.pt() {
                format!("{} é protegido: o RamDog não fecha", e.name)
            } else {
                format!("{} is protected: RamDog will not close it", e.name)
            };
            self.toast(msg, true);
            return;
        }
        let mut errs = Vec::new();
        if e.windows.is_empty() {
            for &(pid, created) in &e.identities {
                if !self.alive(pid, created) || self.is_protected_pid(pid) {
                    continue;
                }
                if let procs::KillOutcome::Denied | procs::KillOutcome::Failed(_) =
                    procs::terminate(pid)
                {
                    errs.push(pid);
                }
            }
        } else {
            for w in &e.windows {
                if let Err(err) = close_window(&w.address, self.cfg.locale) {
                    errs.push(w.pid);
                    self.toast(err, true);
                }
            }
        }
        self.closing.insert(
            e.key.clone(),
            Closing {
                name: e.name.clone(),
                identities: e.identities.clone(),
                at: Instant::now(),
            },
        );
        self.sampler.force.store(true, Ordering::Relaxed);
        if !errs.is_empty() && e.windows.is_empty() {
            let msg = if self.pt() {
                format!("{}: sem permissão para {} processo(s)", e.name, errs.len())
            } else {
                format!("{}: no permission for {} process(es)", e.name, errs.len())
            };
            self.toast(msg, true);
        }
    }

    fn is_protected_pid(&self, pid: u32) -> bool {
        match self.idx.get(&pid) {
            Some(&i) => {
                let p = &self.procs[i];
                is_critical(&p.name_lower, p.pid)
                    || self.cfg.locked.contains(&p.name_lower)
                    || p.pid == std::process::id()
            }
            None => true,
        }
    }

    /// SIGKILL em todos os membros não protegidos, depois da confirmação.
    fn force(&mut self, e: &Entry) {
        let (mut killed, mut skipped, mut denied) = (0, 0, 0);
        for &(pid, created) in &e.identities {
            if !self.alive(pid, created) {
                continue;
            }
            if self.is_protected_pid(pid) {
                skipped += 1;
                continue;
            }
            match procs::kill(pid) {
                procs::KillOutcome::Signaled | procs::KillOutcome::AlreadyGone => killed += 1,
                _ => denied += 1,
            }
        }
        self.closing.insert(
            e.key.clone(),
            Closing {
                name: e.name.clone(),
                identities: e.identities.clone(),
                at: Instant::now() - CLOSE_GRACE,
            },
        );
        self.sampler.force.store(true, Ordering::Relaxed);
        let mut msg = if self.pt() {
            format!("{}: {killed} processo(s) forçado(s)", e.name)
        } else {
            format!("{}: {killed} process(es) force-quit", e.name)
        };
        if skipped > 0 {
            msg.push_str(&if self.pt() {
                format!(", {skipped} protegido(s) poupado(s)")
            } else {
                format!(", {skipped} protected skipped")
            });
        }
        if denied > 0 {
            msg.push_str(&if self.pt() {
                format!(", {denied} sem permissão")
            } else {
                format!(", {denied} denied")
            });
        }
        self.toast(msg, denied > 0);
    }

    /// Abre a lista completa já no processo principal do app.
    fn details(&mut self, e: &Entry) {
        match open_full(e.pids.first().copied()) {
            Ok(()) => {}
            Err(err) => self.toast(err, true),
        }
    }

    fn move_selection(&mut self, delta: i32) {
        let keys: Vec<String> = self.visible().iter().map(|e| e.key.clone()).collect();
        if keys.is_empty() {
            return;
        }
        let cur = self
            .selected
            .as_ref()
            .and_then(|k| keys.iter().position(|x| x == k));
        let next = match cur {
            None if delta > 0 => 0,
            None => keys.len() - 1,
            Some(i) => (i as i32 + delta).clamp(0, keys.len() as i32 - 1) as usize,
        };
        self.selected = Some(keys[next].clone());
        self.scroll_to_selected = true;
    }

    fn keyboard(&mut self, ctx: &egui::Context) {
        // As setas são da lista. Sem consumir aqui, o egui as usa para passear o foco
        // pelos botões e o Enter seguinte apertaria o Forçar em vez de mostrar o app.
        let (up, down) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
            )
        });
        let (ctrl_f, enter, del, shift, esc) = ctx.input(|i| {
            (
                i.modifiers.command && i.key_pressed(egui::Key::F),
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Delete),
                i.modifiers.shift,
                i.key_pressed(egui::Key::Escape),
            )
        });
        if ctrl_f {
            self.focus_search = true;
        }
        if esc {
            if self.confirm.is_some() {
                self.confirm = None;
            } else if !self.search.is_empty() {
                self.search.clear();
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            return;
        }
        if let Some(key) = self.confirm.clone() {
            if enter {
                self.confirm = None;
                if let Some(e) = self.entry(&key).cloned() {
                    self.force(&e);
                }
            }
            return;
        }
        if up {
            self.move_selection(-1);
        }
        if down {
            self.move_selection(1);
        }
        let Some(e) = self.selected_entry() else {
            // Enter na busca com um resultado só: é esse que se quer.
            if enter {
                let hits: Vec<Entry> = self.visible().into_iter().cloned().collect();
                if hits.len() == 1 {
                    self.selected = Some(hits[0].key.clone());
                    self.show(&hits[0]);
                }
            }
            return;
        };
        if enter {
            self.show(&e);
        }
        if del && !(ctx.wants_keyboard_input() && !self.search.is_empty()) {
            if shift {
                if !e.protected {
                    self.confirm = Some(e.key.clone());
                }
            } else {
                self.close(&e);
            }
        }
    }

    // ---------- tela ----------

    fn ui_header(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let rect = ui.max_rect();
        let drag = ui.interact(rect, ui.id().with("drag"), Sense::click_and_drag());
        if drag.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        ui.horizontal(|ui| {
            if let Some(logo) = &self.logo {
                ui.add(egui::Image::new(logo).fit_to_exact_size(Vec2::splat(22.0)));
            }
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(
                    RichText::new(self.t("Gerenciador", "Task manager"))
                        .size(16.0)
                        .strong(),
                );
                let apps = self
                    .entries
                    .iter()
                    .filter(|e| e.section == Section::Apps)
                    .count();
                let bg = self.entries.len() - apps;
                let sub = if self.pt() {
                    format!("{apps} apps abertos · {bg} em segundo plano")
                } else {
                    format!("{apps} open apps · {bg} in background")
                };
                ui.label(RichText::new(sub).small().color(pal.muted));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // Desenhado a traço: a fonte do Linux não tem o ✕ e ele vira um quadrado.
                let (r, close) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
                let c = if close.hovered() { pal.text } else { pal.muted };
                let x = r.center();
                for (a, b) in [(-5.0, 5.0), (5.0, -5.0)] {
                    ui.painter().line_segment(
                        [x + Vec2::new(-5.0, a), x + Vec2::new(5.0, b)],
                        Stroke::new(1.5_f32, c),
                    );
                }
                if close
                    .on_hover_text(self.t("Fechar (Esc)", "Close (Esc)"))
                    .clicked()
                {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
                ui.add_space(10.0);
                let gpu = self.sys.gpu.as_ref().and_then(|g| g.util_pct);
                if let Some(g) = gpu {
                    self.metric(ui, "GPU", g);
                }
                let total = self.mem.total_phys.max(1);
                let used = self.mem.used_phys();
                let mem_label = self.t("MEMÓRIA", "MEMORY");
                self.metric(ui, mem_label, used as f32 / total as f32 * 100.0)
                    .on_hover_text(format!(
                        "{} / {}",
                        fmt_bytes_short(used),
                        fmt_bytes_short(total)
                    ));
                if let Some(c) = self.sys.cpu_pct {
                    self.metric(ui, "CPU", c);
                }
            });
        });
    }

    fn metric(&self, ui: &mut egui::Ui, label: &str, pct: f32) -> egui::Response {
        let pal = self.pal;
        ui.allocate_ui_with_layout(Vec2::new(72.0, 38.0), Layout::top_down(Align::Max), |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.label(RichText::new(label).size(10.5).color(pal.muted));
            ui.label(
                RichText::new(format!("{}%", num(pct as f64, 1)))
                    .monospace()
                    .size(19.0)
                    .color(pal.accent),
            );
        })
        .response
    }

    fn ui_search(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        ui.horizontal(|ui| {
            let hint = self.t(
                "Buscar app, processo ou PID    Ctrl+F",
                "Search app, process or PID    Ctrl+F",
            );
            let edit = egui::TextEdit::singleline(&mut self.search)
                .hint_text(RichText::new(hint).color(pal.muted))
                .desired_width(ui.available_width())
                .margin(egui::Margin::symmetric(10, 6));
            let r = ui.add(edit);
            if self.focus_search {
                r.request_focus();
                self.focus_search = false;
            }
            if r.changed() {
                self.selected = None;
            }
        });
    }

    fn header_cell(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        key: SortKey,
        label: &str,
        right: bool,
    ) {
        let pal = self.pal;
        let active = self.sort == key;
        let arrow = if active {
            if self.desc {
                " ↓"
            } else {
                " ↑"
            }
        } else {
            ""
        };
        let resp = ui.interact(rect, ui.id().with(("sort", label)), Sense::click());
        let color = if active || resp.hovered() {
            pal.text
        } else {
            pal.muted
        };
        let align = if right {
            egui::Align2::RIGHT_CENTER
        } else {
            egui::Align2::LEFT_CENTER
        };
        let pos = if right {
            rect.right_center()
        } else {
            rect.left_center()
        };
        ui.painter().text(
            pos,
            align,
            format!("{label}{arrow}"),
            egui::FontId::proportional(11.5),
            color,
        );
        if resp.clicked() {
            if active {
                self.desc = !self.desc;
            } else {
                self.sort = key;
                self.desc = key != SortKey::Name;
            }
            model::sort(&mut self.entries, self.sort, self.desc);
        }
    }

    fn columns(rect: Rect) -> [Rect; 4] {
        let mem_w = 96.0;
        let cpu_w = 72.0;
        let tasks_w = 64.0;
        let pad = 12.0;
        let r = rect.right() - pad;
        let mem = Rect::from_x_y_ranges(r - mem_w..=r, rect.y_range());
        let cpu = Rect::from_x_y_ranges(mem.left() - cpu_w..=mem.left(), rect.y_range());
        let tasks = Rect::from_x_y_ranges(cpu.left() - tasks_w..=cpu.left(), rect.y_range());
        let name = Rect::from_x_y_ranges(rect.left() + pad..=tasks.left() - 8.0, rect.y_range());
        [name, tasks, cpu, mem]
    }

    fn ui_list(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let (head, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), Sense::hover());
        let [n, t, c, m] = Self::columns(head);
        let (ln, lt, lc, lm) = (
            self.t("Nome", "Name"),
            self.t("Tarefas", "Tasks"),
            "CPU",
            self.t("Memória", "Memory"),
        );
        self.header_cell(
            ui,
            n.shrink2(Vec2::new(ICON + 10.0, 0.0))
                .translate(Vec2::new(ICON + 10.0, 0.0)),
            SortKey::Name,
            ln,
            false,
        );
        self.header_cell(ui, t, SortKey::Tasks, lt, true);
        self.header_cell(ui, c, SortKey::Cpu, lc, true);
        self.header_cell(ui, m, SortKey::Ram, lm, true);
        ui.painter().hline(
            head.x_range(),
            head.bottom(),
            Stroke::new(1.0_f32, pal.line),
        );

        if self.samples == 0 {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(self.t("Lendo processos…", "Reading processes…"))
                        .color(pal.muted),
                );
            });
            return;
        }

        let searching = !self.search.trim().is_empty();
        let visible: Vec<Entry> = self.visible().into_iter().cloned().collect();
        // Na busca, o número da seção é quantos batem, não quantos existem.
        let count = |s: Section| {
            if searching {
                visible.iter().filter(|e| e.section == s).count()
            } else {
                self.entries.iter().filter(|e| e.section == s).count()
            }
        };
        let small: Vec<&Entry> = self
            .entries
            .iter()
            .filter(|e| e.section == Section::Background && Self::is_small(e))
            .collect();
        let small_n = small.len();
        let small_ram: u64 = small.iter().map(|e| e.ram).sum();
        let sys_ram: u64 = self
            .entries
            .iter()
            .filter(|e| e.section == Section::System)
            .map(|e| e.ram)
            .sum();
        let (n_apps, n_bg, n_sys) = (
            count(Section::Apps),
            count(Section::Background),
            count(Section::System),
        );

        let scroll = egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let mut last: Option<Section> = None;
                let mut shown_small_toggle = false;
                for e in &visible {
                    if last != Some(e.section) {
                        if last == Some(Section::Background) && !searching {
                            self.small_toggle(ui, small_n, small_ram);
                            shown_small_toggle = true;
                        }
                        let (label, n) = match e.section {
                            Section::Apps => (self.t("Apps", "Apps"), n_apps),
                            Section::Background => (self.t("Segundo plano", "Background"), n_bg),
                            Section::System => (self.t("Sistema", "System"), n_sys),
                        };
                        if e.section == Section::System && !searching {
                            self.system_toggle(ui, n, sys_ram);
                        } else {
                            self.section_title(ui, label, n);
                        }
                        last = Some(e.section);
                    }
                    self.row(ui, e);
                }
                if !searching {
                    if !shown_small_toggle && n_bg > 0 {
                        if last != Some(Section::Background) {
                            self.section_title(ui, self.t("Segundo plano", "Background"), n_bg);
                        }
                        self.small_toggle(ui, small_n, small_ram);
                    }
                    if !self.show_system && n_sys > 0 {
                        self.system_toggle(ui, n_sys, sys_ram);
                    }
                } else if visible.is_empty() {
                    ui.add_space(20.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            RichText::new(self.t("Nada com esse nome.", "Nothing matches."))
                                .color(pal.muted),
                        );
                    });
                }
                ui.add_space(6.0);
            });
        self.list_rect = Some(scroll.inner_rect);
    }

    fn section_title(&self, ui: &mut egui::Ui, label: &str, n: usize) {
        let pal = self.pal;
        let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), Sense::hover());
        ui.painter().text(
            r.left_bottom() + Vec2::new(12.0, -7.0),
            egui::Align2::LEFT_BOTTOM,
            format!("{}  {n}", label.to_uppercase()),
            egui::FontId::proportional(10.5),
            pal.muted,
        );
    }

    fn system_toggle(&mut self, ui: &mut egui::Ui, n: usize, ram: u64) {
        let pal = self.pal;
        let (r, resp) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), Sense::click());
        let arrow = if self.show_system { "▾" } else { "▸" };
        let color = if resp.hovered() { pal.text } else { pal.muted };
        ui.painter().text(
            r.left_bottom() + Vec2::new(12.0, -7.0),
            egui::Align2::LEFT_BOTTOM,
            format!(
                "{arrow} {}  {n} · {}",
                self.t("SISTEMA", "SYSTEM"),
                fmt_bytes_short(ram)
            ),
            egui::FontId::proportional(10.5),
            color,
        );
        if resp.clicked() {
            self.show_system = !self.show_system;
        }
    }

    fn small_toggle(&mut self, ui: &mut egui::Ui, n: usize, ram: u64) {
        if n == 0 {
            return;
        }
        let pal = self.pal;
        let (r, resp) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.0), Sense::click());
        let text = match (self.show_small, self.pt()) {
            (false, true) => format!("+ {n} pequenos · {}", fmt_bytes_short(ram)),
            (false, false) => format!("+ {n} small · {}", fmt_bytes_short(ram)),
            (true, true) => "recolher os pequenos".into(),
            (true, false) => "hide small ones".into(),
        };
        ui.painter().text(
            r.left_center() + Vec2::new(12.0 + ICON + 10.0, 0.0),
            egui::Align2::LEFT_CENTER,
            text,
            egui::FontId::proportional(11.5),
            if resp.hovered() { pal.text } else { pal.muted },
        );
        if resp.clicked() {
            self.show_small = !self.show_small;
        }
    }

    fn row(&mut self, ui: &mut egui::Ui, e: &Entry) {
        let pal = self.pal;
        let (rect, resp) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
        let selected = self.selected.as_deref() == Some(e.key.as_str());
        if selected && self.scroll_to_selected {
            ui.scroll_to_rect(rect, None);
            self.scroll_to_selected = false;
        }
        let p = ui.painter();
        if selected {
            p.rect_filled(rect.shrink2(Vec2::new(4.0, 1.0)), 6.0, pal.selected);
        } else if resp.hovered() {
            p.rect_filled(rect.shrink2(Vec2::new(4.0, 1.0)), 6.0, pal.hover);
        }
        let [n, t, c, m] = Self::columns(rect);
        // ícone
        let icon_rect = Rect::from_center_size(
            n.left_center() + Vec2::new(ICON / 2.0, 0.0),
            Vec2::splat(ICON),
        );
        match self.icons.get(&e.icon_key).and_then(|t| t.as_ref()) {
            Some(tex) => {
                p.image(
                    tex.id(),
                    icon_rect,
                    Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            None => {
                let color = e.cat.color();
                p.rect_filled(icon_rect, 5.0, mix(pal.bg, color, 0.35));
                let letter = e
                    .name
                    .chars()
                    .next()
                    .unwrap_or('?')
                    .to_uppercase()
                    .to_string();
                p.text(
                    icon_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    letter,
                    egui::FontId::proportional(12.0),
                    pal.text,
                );
            }
        }
        // nome + detalhe
        let text_x = icon_rect.right() + 10.0;
        let name_w = (n.right() - text_x).max(40.0);
        let closing = self.closing.get(&e.key);
        let late = closing.is_some_and(|c| c.at.elapsed() >= CLOSE_GRACE);
        let mut tag: Option<(String, Color32)> = None;
        if let Some(_) = closing {
            tag = Some(if late {
                (
                    self.t("não respondeu", "not responding").to_string(),
                    pal.danger,
                )
            } else {
                (self.t("fechando…", "closing…").to_string(), pal.muted)
            });
        } else if e.protected {
            tag = Some((self.t("protegido", "protected").to_string(), pal.muted));
        }
        let detail = if e.windows.len() > 1 && !e.agent {
            if self.pt() {
                format!("{} janelas", e.windows.len())
            } else {
                format!("{} windows", e.windows.len())
            }
        } else if e.windows.is_empty() && e.sessions > 0 && e.detail.is_empty() {
            match (e.sessions, self.pt()) {
                (1, true) => "1 sessão sem janela".into(),
                (n, true) => format!("{n} sessões sem janela"),
                (1, false) => "1 session without a window".into(),
                (n, false) => format!("{n} sessions without a window"),
            }
        } else {
            e.detail.clone()
        };
        let name_y = if detail.is_empty() {
            rect.center().y
        } else {
            rect.center().y - 7.0
        };
        let galley = p.layout_no_wrap(e.name.clone(), egui::FontId::proportional(13.0), pal.text);
        let name_clip = Rect::from_x_y_ranges(text_x..=text_x + name_w, rect.y_range());
        let pn = p.with_clip_rect(name_clip.intersect(p.clip_rect()));
        let name_pos = egui::pos2(text_x, name_y - galley.size().y / 2.0);
        let name_end = name_pos.x + galley.size().x;
        pn.galley(name_pos, galley, pal.text);
        if let Some((tag, color)) = tag {
            pn.text(
                egui::pos2(name_end + 8.0, name_y),
                egui::Align2::LEFT_CENTER,
                tag,
                egui::FontId::proportional(10.5),
                color,
            );
        }
        if !detail.is_empty() {
            pn.text(
                egui::pos2(text_x, rect.center().y + 9.0),
                egui::Align2::LEFT_CENTER,
                detail,
                egui::FontId::proportional(11.0),
                pal.muted,
            );
        }
        // números
        let mono = egui::FontId::monospace(12.0);
        p.text(
            t.right_center(),
            egui::Align2::RIGHT_CENTER,
            e.pids.len().to_string(),
            mono.clone(),
            pal.muted,
        );
        let cpu_color = if e.cpu >= 25.0 {
            pal.accent
        } else if e.cpu < 0.05 {
            pal.muted
        } else {
            pal.text
        };
        p.text(
            c.right_center(),
            egui::Align2::RIGHT_CENTER,
            format!("{}%", num(e.cpu as f64, 1)),
            mono.clone(),
            cpu_color,
        );
        let ram_color = if e.ram >= 2 * 1024 * 1024 * 1024 {
            pal.accent
        } else {
            pal.text
        };
        p.text(
            m.right_center(),
            egui::Align2::RIGHT_CENTER,
            if e.ram_partial {
                format!("≥ {}", fmt_bytes_short(e.ram))
            } else {
                fmt_bytes_short(e.ram)
            },
            mono,
            ram_color,
        );

        if resp.clicked() {
            self.selected = Some(e.key.clone());
            self.confirm = None;
        }
        if resp.double_clicked() {
            self.show(e);
        }
        let e2 = e.clone();
        resp.context_menu(|ui| {
            self.selected = Some(e2.key.clone());
            if ui
                .add_enabled(
                    !e2.windows.is_empty(),
                    egui::Button::new(self.t("Mostrar", "Show")),
                )
                .clicked()
            {
                self.show(&e2);
                ui.close_menu();
            }
            if ui
                .add_enabled(!e2.protected, egui::Button::new(self.t("Fechar", "Close")))
                .clicked()
            {
                self.close(&e2);
                ui.close_menu();
            }
            if ui
                .add_enabled(
                    !e2.protected,
                    egui::Button::new(self.t("Forçar…", "Force quit…")),
                )
                .clicked()
            {
                self.confirm = Some(e2.key.clone());
                ui.close_menu();
            }
            ui.separator();
            if ui
                .button(self.t("Ver na lista completa", "Open in full list"))
                .clicked()
            {
                self.details(&e2);
                ui.close_menu();
            }
        });
        let tip = self.row_tip(e);
        resp.on_hover_text_at_pointer(tip);
    }

    fn row_tip(&self, e: &Entry) -> String {
        let mut names: Vec<String> = Vec::new();
        let mut counts: HashMap<String, usize> = HashMap::new();
        for pid in &e.pids {
            if let Some(&i) = self.idx.get(pid) {
                let n = self.procs[i].name.clone();
                if !counts.contains_key(&n) {
                    names.push(n.clone());
                }
                *counts.entry(n).or_default() += 1;
            }
        }
        let list: Vec<String> = names
            .iter()
            .take(8)
            .map(|n| match counts[n] {
                1 => n.clone(),
                k => format!("{n} ×{k}"),
            })
            .collect();
        let more = names.len().saturating_sub(8);
        let mut tip = if self.pt() {
            format!("PID {} · {}", e.pids[0], list.join(", "))
        } else {
            format!("PID {} · {}", e.pids[0], list.join(", "))
        };
        if more > 0 {
            tip.push_str(&format!(" +{more}"));
        }
        tip.push_str(self.t(
            "\nMemória em PSS: páginas compartilhadas divididas entre quem usa, sem contar duas vezes.",
            "\nMemory in PSS: shared pages split between users, never counted twice.",
        ));
        tip
    }

    fn ui_footer(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        if let Some(key) = self.confirm.clone() {
            let Some(e) = self.entry(&key).cloned() else {
                self.confirm = None;
                return;
            };
            ui.horizontal(|ui| {
                let q = if self.pt() {
                    format!(
                        "Forçar {} ({} processo(s))? O que não foi salvo se perde.",
                        e.name,
                        e.pids.len()
                    )
                } else {
                    format!(
                        "Force quit {} ({} process(es))? Unsaved work is lost.",
                        e.name,
                        e.pids.len()
                    )
                };
                ui.label(RichText::new(q).color(pal.danger));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button(self.t("Cancelar", "Cancel")).clicked() {
                        self.confirm = None;
                    }
                    let force = egui::Button::new(
                        RichText::new(self.t("Forçar", "Force quit")).color(pal.bg),
                    )
                    .fill(pal.danger);
                    if ui.add(force).clicked() {
                        self.confirm = None;
                        self.force(&e);
                    }
                });
            });
            return;
        }
        let sel = self.selected_entry();
        ui.horizontal(|ui| {
            let has = sel.is_some();
            let has_win = sel.as_ref().is_some_and(|e| !e.windows.is_empty());
            let closable = sel.as_ref().is_some_and(|e| !e.protected);
            let late = sel.as_ref().is_some_and(|e| {
                self.closing
                    .get(&e.key)
                    .is_some_and(|c| c.at.elapsed() >= CLOSE_GRACE)
            });
            if ui
                .add_enabled(has_win, egui::Button::new(self.t("Mostrar", "Show")))
                .on_hover_text(self.t("Enter ou duplo clique", "Enter or double-click"))
                .clicked()
            {
                if let Some(e) = &sel {
                    self.show(e);
                }
            }
            if ui
                .add_enabled(closable, egui::Button::new(self.t("Fechar", "Close")))
                .on_hover_text(self.t(
                    "Pede para o app fechar, como o Super+W (Del)",
                    "Asks the app to close, like Super+W (Del)",
                ))
                .clicked()
            {
                if let Some(e) = &sel {
                    self.close(e);
                }
            }
            let force_label = RichText::new(self.t("Forçar…", "Force quit…"));
            let force = if late {
                egui::Button::new(force_label.color(pal.bg)).fill(pal.danger)
            } else {
                egui::Button::new(force_label)
            };
            if ui
                .add_enabled(closable, force)
                .on_hover_text(self.t(
                    "Encerra na marra, com confirmação (Shift+Del)",
                    "Kills it outright, after confirming (Shift+Del)",
                ))
                .clicked()
            {
                if let Some(e) = &sel {
                    self.confirm = Some(e.key.clone());
                }
            }
            if let Some((at, msg, err)) = &self.status {
                if at.elapsed().as_secs_f32() < 6.0 {
                    ui.add_space(6.0);
                    ui.label(RichText::new(msg.clone()).small().color(if *err {
                        pal.danger
                    } else {
                        pal.muted
                    }));
                    ui.ctx().request_repaint_after(Duration::from_millis(500));
                }
            } else if late {
                ui.label(
                    RichText::new(self.t(
                        "não fechou sozinho: dá para forçar",
                        "did not close on its own: you can force it",
                    ))
                    .small()
                    .color(pal.danger),
                );
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = self.t("Lista completa ↗", "Full list ↗");
                let tip = self.t(
                    "Abre o RamDog completo no processo selecionado",
                    "Opens the full RamDog at the selected process",
                );
                if ui.button(label).on_hover_text(tip).clicked() {
                    match &sel {
                        Some(e) => self.details(e),
                        None => {
                            if let Err(err) = open_full(None) {
                                self.toast(err, true);
                            }
                        }
                    }
                }
                let _ = has;
            });
        });
    }

    fn resize_grip(&self, ctx: &egui::Context) {
        let screen = ctx.screen_rect();
        let grip =
            Rect::from_min_size(screen.right_bottom() - Vec2::splat(14.0), Vec2::splat(14.0));
        egui::Area::new(egui::Id::new("grip"))
            .fixed_pos(grip.min)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let (r, resp) = ui.allocate_exact_size(grip.size(), Sense::drag());
                let c = if resp.hovered() {
                    self.pal.accent
                } else {
                    self.pal.line
                };
                for i in 1..=3 {
                    let o = i as f32 * 4.0;
                    ui.painter().line_segment(
                        [
                            r.right_bottom() - Vec2::new(o, 2.0),
                            r.right_bottom() - Vec2::new(2.0, o),
                        ],
                        Stroke::new(1.0_f32, c),
                    );
                }
                if resp.drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(
                        egui::viewport::ResizeDirection::SouthEast,
                    ));
                }
            });
    }
}

impl eframe::App for Gerenciador {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        set_number_locale(self.cfg.locale);
        self.ingest(ctx);
        self.keyboard(ctx);
        let pal = self.pal;
        let border = Stroke::new(1.0_f32, mix(pal.line, pal.accent, 0.35));
        egui::TopBottomPanel::top("g-header")
            .frame(egui::Frame::new().fill(pal.bg).inner_margin(egui::Margin {
                left: 16,
                right: 10,
                top: 12,
                bottom: 6,
            }))
            .show_separator_line(false)
            .show(ctx, |ui| {
                self.ui_header(ui);
                ui.add_space(8.0);
                self.ui_search(ui);
            });
        egui::TopBottomPanel::bottom("g-footer")
            .frame(
                egui::Frame::new()
                    .fill(pal.surface)
                    .inner_margin(egui::Margin::symmetric(14, 10)),
            )
            .show_separator_line(false)
            .show(ctx, |ui| self.ui_footer(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(pal.bg).inner_margin(egui::Margin {
                left: 8,
                right: 8,
                top: 4,
                bottom: 0,
            }))
            .show(ctx, |ui| self.ui_list(ui));
        // Moldura de 1 px: sem decoração, a janela flutuante sumia contra um fundo igual.
        ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("frame"),
        ))
        .rect_stroke(
            ctx.screen_rect().shrink(0.5),
            0.0,
            border,
            egui::StrokeKind::Inside,
        );
        self.resize_grip(ctx);
        if self
            .closing
            .values()
            .any(|c| c.at.elapsed() < CLOSE_GRACE + Duration::from_millis(200))
        {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }
}

// ---------- plataforma ----------

fn mem_of(p: &ProcInfo) -> u64 {
    // PSS quando o smaps já foi lido; senão RSS menos o compartilhado, que também não
    // conta a mesma página duas vezes.
    #[cfg(target_os = "linux")]
    if let Some((_, pss)) = p.linux_memory {
        return pss;
    }
    p.private_ws
}

fn num(v: f64, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    if crate::app::english_numbers() {
        s
    } else {
        s.replace('.', ",")
    }
}

#[cfg(target_os = "linux")]
fn windows() -> Vec<Win> {
    crate::desktop_linux::clients()
        .into_iter()
        .map(|w| Win {
            address: w.address,
            pid: w.pid,
            title: w.title,
            class: w.class,
            focused: w.focused,
        })
        .collect()
}

#[cfg(not(target_os = "linux"))]
fn windows() -> Vec<Win> {
    Vec::new()
}

/// Processo de outro usuário (root, serviços do sistema): pertence à seção Sistema.
#[cfg(unix)]
fn other_user(p: &ProcInfo) -> bool {
    use std::os::unix::fs::MetadataExt;
    static ME: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    let me = *ME.get_or_init(|| unsafe { libc::getuid() });
    std::fs::metadata(format!("/proc/{}", p.pid))
        .map(|m| m.uid() != me)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn other_user(p: &ProcInfo) -> bool {
    p.session == 0
}

#[cfg(target_os = "linux")]
fn cwd_of(pid: u32) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/cwd"))
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

#[cfg(not(target_os = "linux"))]
fn cwd_of(_pid: u32) -> Option<String> {
    None
}

#[cfg(target_os = "linux")]
fn app_name(exe: &str, class: &str) -> Option<String> {
    crate::desktop_linux::app_name(exe, class)
}

#[cfg(not(target_os = "linux"))]
fn app_name(_exe: &str, _class: &str) -> Option<String> {
    None
}

#[cfg(target_os = "linux")]
fn focus_window(address: &str, locale: Locale) -> Result<(), String> {
    crate::screens::focus_window(address, locale)
}

#[cfg(not(target_os = "linux"))]
fn focus_window(_address: &str, locale: Locale) -> Result<(), String> {
    Err(locale
        .text(
            "Mostrar janela exige Hyprland",
            "Showing a window requires Hyprland",
        )
        .into())
}

#[cfg(target_os = "linux")]
fn close_window(address: &str, locale: Locale) -> Result<(), String> {
    crate::screens::close_window(address, locale)
}

#[cfg(not(target_os = "linux"))]
fn close_window(_address: &str, locale: Locale) -> Result<(), String> {
    Err(locale
        .text(
            "Fechar janela exige Hyprland",
            "Closing a window requires Hyprland",
        )
        .into())
}

/// Arquivo que a instância completa lê a cada amostra para selecionar um PID.
pub fn select_request_path() -> std::path::PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("ramdog")
        .join("select")
}

/// Pede à lista completa para abrir (ou vir para frente) já com `pid` selecionado.
fn open_full(pid: Option<u32>) -> Result<(), String> {
    if let Some(pid) = pid {
        let path = select_request_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(&path, pid.to_string()).map_err(|e| e.to_string())?;
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let launcher = exe.with_file_name("ramdog-launch");
    // Pelo launcher, a lista completa sobe na unidade systemd dela e não morre junto
    // com o Gerenciador quando ele fecha.
    let mut cmd = if launcher.is_file() {
        std::process::Command::new(launcher)
    } else {
        std::process::Command::new(exe)
    };
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// `ramdog --gerenciador --dump`: as linhas do Gerenciador em texto, sem abrir janela.
#[cfg(target_os = "linux")]
fn dump() {
    let _ = crate::desktop_linux::clients();
    let mut sampler = procs::Sampler::new();
    let _ = sampler.sample();
    std::thread::sleep(Duration::from_millis(1200));
    let procs = sampler.sample();
    let cfg = Config::load();
    let overrides: HashMap<String, Category> =
        cfg.overrides.iter().map(|(k, v)| (k.clone(), *v)).collect();
    let cats = categories::classify(&procs, &overrides);
    let windows = windows();
    let me = std::process::id();
    let home = std::env::var("HOME").ok();
    let mem = |p: &ProcInfo| Some(mem_of(p));
    let protected = |p: &ProcInfo| {
        is_critical(&p.name_lower, p.pid) || cfg.locked.contains(&p.name_lower) || p.pid == me
    };
    let cwd = |pid: u32| cwd_of(pid);
    let app_name = |exe: &str, class: &str| app_name(exe, class);
    let mut entries = model::build(&model::Inputs {
        procs: &procs,
        windows: &windows,
        cats: &cats,
        mem: &mem,
        protected: &protected,
        cwd: &cwd,
        app_name: &app_name,
        home: home.as_deref(),
        system: &other_user,
        me,
    });
    model::sort(&mut entries, SortKey::Ram, true);
    let seen: HashSet<u32> = entries
        .iter()
        .flat_map(|e| e.pids.iter().copied())
        .collect();
    for e in &entries {
        println!(
            "{:?}\t{}\t{}\t{}\t{:.1}%\t{}{}\t{}",
            e.section,
            e.name,
            e.detail,
            e.pids.len(),
            e.cpu,
            fmt_bytes_short(e.ram),
            if e.protected { "\tprotegido" } else { "" },
            e.key
        );
    }
    eprintln!(
        "{} linhas, {} processos cobertos de {}, {} janelas",
        entries.len(),
        seen.len(),
        procs.len(),
        windows.len()
    );
}
