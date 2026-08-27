//! Visão Telas: monitores, janelas e cenários de trabalho.
//!
//! Três coisas que o Windows não junta em lugar nenhum:
//!
//! 1. **Mapa.** Os monitores desenhados em escala, com cada janela aberta como um
//!    retângulo que dá para arrastar de tela em tela — o que o painel Vídeo mostra só para
//!    os monitores, aqui vale para as janelas também.
//! 2. **Grade.** Um layout escolhido (metades, terços, quadrantes, principal + 2) que
//!    encaixa a janela arrastada na zona embaixo do cursor, ou distribui de uma vez todas
//!    as janelas de um monitor.
//! 3. **Cenário.** Um conjunto nomeado de "este programa, nesta tela, neste retângulo".
//!    Aplicar abre o que não estiver aberto e posiciona tudo.
//!
//! As posições do cenário são guardadas em fração da área útil do monitor, nunca em pixel:
//! assim o cenário sobrevive a trocar de resolução, plugar a TV e mudar a escala do Windows.

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::time::{Duration, Instant};

use egui::{Color32, RichText, Sense, Stroke, StrokeKind};

use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromWindow, HDC, HMONITOR, MONITORINFO,
    MONITORINFOEXW, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, GetAncestor, GetClassNameW, GetWindowLongPtrW, GetWindowRect,
    GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible, IsZoomed,
    SetForegroundWindow, SetWindowPos, ShowWindow, GA_ROOTOWNER, GWL_EXSTYLE, SWP_NOACTIVATE,
    SWP_NOSENDCHANGING, SWP_NOZORDER, SW_MINIMIZE, SW_RESTORE, SW_SHOWMAXIMIZED, WS_EX_TOOLWINDOW,
};

use crate::app::{ACCENT, ACCENT_BG, LINE, MUTED, SURFACE, SURFACE_HI, TEXT};
use crate::config::{Config, ScreenPreset, ScreenSlot};
use crate::icons::IconBank;
use crate::procs::ProcInfo;

/// `MONITORINFOF_PRIMARY` — a crate `windows` 0.61 não expõe esta constante.
const PRIMARY_FLAG: u32 = 0x0000_0001;

/// Quanto tempo um cenário espera a janela de um programa que ele acabou de abrir.
/// Depois disso desiste e diz qual não apareceu, em vez de ficar esperando calado.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(25);

pub enum ScreenOut {
    Toast(String, bool),
    /// A aba mexeu na config (cenários, grade, encaixe) — o `App` grava.
    SaveCfg,
}

// ---------- geometria ----------

/// Retângulo em pixels do desktop virtual. Existe porque `RECT` do Win32 não tem largura,
/// altura nem centro, e essas três contas aparecem em toda linha deste arquivo.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct R {
    pub l: i32,
    pub t: i32,
    pub r: i32,
    pub b: i32,
}

impl R {
    fn of(rc: RECT) -> Self {
        Self { l: rc.left, t: rc.top, r: rc.right, b: rc.bottom }
    }
    fn w(self) -> i32 {
        self.r - self.l
    }
    fn h(self) -> i32 {
        self.b - self.t
    }
    fn cx(self) -> i32 {
        self.l + self.w() / 2
    }
    fn cy(self) -> i32 {
        self.t + self.h() / 2
    }
    fn contains(self, x: i32, y: i32) -> bool {
        x >= self.l && x < self.r && y >= self.t && y < self.b
    }
    /// Sub-retângulo em fração (0..1) deste retângulo.
    fn frac(self, f: [f32; 4]) -> R {
        let (w, h) = (self.w() as f32, self.h() as f32);
        let l = self.l + (f[0] * w).round() as i32;
        let t = self.t + (f[1] * h).round() as i32;
        R { l, t, r: l + (f[2] * w).round() as i32, b: t + (f[3] * h).round() as i32 }
    }
    /// Onde este retângulo está dentro de `base`, em fração — o inverso de `frac`.
    fn frac_in(self, base: R) -> [f32; 4] {
        let (w, h) = (base.w().max(1) as f32, base.h().max(1) as f32);
        [
            (self.l - base.l) as f32 / w,
            (self.t - base.t) as f32 / h,
            self.w() as f32 / w,
            self.h() as f32 / h,
        ]
    }
}

// ---------- grades ----------

pub struct Grid {
    pub id: &'static str,
    pub name: &'static str,
    /// Zonas em fração da área útil: [x, y, largura, altura].
    pub zones: &'static [[f32; 4]],
}

/// As grades embutidas. Fração, nunca pixel — a mesma grade serve o ultrawide e o notebook.
pub const GRIDS: &[Grid] = &[
    Grid { id: "cheio", name: "Cheio", zones: &[[0.0, 0.0, 1.0, 1.0]] },
    Grid {
        id: "metades",
        name: "Metades",
        zones: &[[0.0, 0.0, 0.5, 1.0], [0.5, 0.0, 0.5, 1.0]],
    },
    Grid {
        id: "deitadas",
        name: "Deitadas",
        zones: &[[0.0, 0.0, 1.0, 0.5], [0.0, 0.5, 1.0, 0.5]],
    },
    Grid {
        id: "tercos",
        name: "Terços",
        zones: &[[0.0, 0.0, 1.0 / 3.0, 1.0], [1.0 / 3.0, 0.0, 1.0 / 3.0, 1.0], [2.0 / 3.0, 0.0, 1.0 / 3.0, 1.0]],
    },
    Grid {
        id: "quadrantes",
        name: "Quadrantes",
        zones: &[
            [0.0, 0.0, 0.5, 0.5],
            [0.5, 0.0, 0.5, 0.5],
            [0.0, 0.5, 0.5, 0.5],
            [0.5, 0.5, 0.5, 0.5],
        ],
    },
    Grid {
        id: "principal2",
        name: "Principal + 2",
        zones: &[[0.0, 0.0, 0.6, 1.0], [0.6, 0.0, 0.4, 0.5], [0.6, 0.5, 0.4, 0.5]],
    },
    Grid {
        id: "centro",
        name: "Centro largo",
        zones: &[[0.0, 0.0, 0.25, 1.0], [0.25, 0.0, 0.5, 1.0], [0.75, 0.0, 0.25, 1.0]],
    },
];

fn grid_by_id(id: &str) -> &'static Grid {
    GRIDS.iter().find(|g| g.id == id).unwrap_or(&GRIDS[1])
}

// ---------- leitura do sistema ----------

#[derive(Clone)]
pub struct Monitor {
    pub hmon: isize,
    pub name: String,
    pub full: R,
    /// Área útil: o que sobra depois da barra de tarefas. É onde a grade vale.
    pub work: R,
    pub primary: bool,
}

#[derive(Clone)]
pub struct Win {
    pub hwnd: isize,
    pub pid: u32,
    pub title: String,
    pub exe: String,
    pub exe_name: String,
    /// Retângulo visual (borda que o usuário enxerga), já sem a sombra do DWM.
    pub rect: R,
    pub monitor: usize,
    pub maximized: bool,
    pub minimized: bool,
}

unsafe extern "system" fn push_monitor(hmon: HMONITOR, _hdc: HDC, _rc: *mut RECT, lp: LPARAM) -> BOOL {
    let out = unsafe { &mut *(lp.0 as *mut Vec<Monitor>) };
    let mut mi = MONITORINFOEXW::default();
    mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    let ok = unsafe { GetMonitorInfoW(hmon, &mut mi as *mut _ as *mut MONITORINFO) };
    if ok.as_bool() {
        let name = String::from_utf16_lossy(&mi.szDevice)
            .trim_end_matches('\0')
            .to_string();
        out.push(Monitor {
            hmon: hmon.0 as isize,
            name,
            full: R::of(mi.monitorInfo.rcMonitor),
            work: R::of(mi.monitorInfo.rcWork),
            primary: mi.monitorInfo.dwFlags & PRIMARY_FLAG != 0,
        });
    }
    BOOL(1)
}

pub fn scan_monitors() -> Vec<Monitor> {
    let mut out: Vec<Monitor> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(push_monitor),
            LPARAM(&mut out as *mut _ as isize),
        );
    }
    // Ordem estável da esquerda para a direita: o índice guardado num cenário precisa
    // apontar para o mesmo monitor depois de reabrir o app.
    out.sort_by_key(|m| (m.full.l, m.full.t));
    out
}

unsafe extern "system" fn push_hwnd(hwnd: HWND, lp: LPARAM) -> BOOL {
    let out = unsafe { &mut *(lp.0 as *mut Vec<isize>) };
    out.push(hwnd.0 as isize);
    BOOL(1)
}

/// Janelas que fazem sentido organizar: visíveis, de nível mais alto, com título, não
/// ferramenta e não "cloaked" (o fantasma que o DWM deixa das UWP suspensas).
fn is_manageable(hwnd: HWND) -> bool {
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            return false;
        }
        if GetAncestor(hwnd, GA_ROOTOWNER) != hwnd {
            return false;
        }
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        if ex & WS_EX_TOOLWINDOW.0 != 0 {
            return false;
        }
        let mut cloaked: u32 = 0;
        let ok = DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut _ as *mut c_void,
            std::mem::size_of::<u32>() as u32,
        );
        if ok.is_ok() && cloaked != 0 {
            return false;
        }
        let mut cls = [0u16; 128];
        let n = GetClassNameW(hwnd, &mut cls) as usize;
        let cls = String::from_utf16_lossy(&cls[..n]);
        !matches!(cls.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Button")
    }
}

fn window_title(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n.min(buf.len())])
}

/// Retângulo visual: `GetWindowRect` inclui a sombra invisível do DWM (uns 7 px de cada
/// lado no Windows 10/11). Alinhar duas janelas pelo retângulo cru deixa uma fresta.
fn visual_rect(hwnd: HWND) -> R {
    let mut raw = RECT::default();
    let _ = unsafe { GetWindowRect(hwnd, &mut raw) };
    let mut ext = RECT::default();
    let ok = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut ext as *mut _ as *mut c_void,
            std::mem::size_of::<RECT>() as u32,
        )
    };
    if ok.is_ok() && ext.right > ext.left && ext.bottom > ext.top {
        R::of(ext)
    } else {
        R::of(raw)
    }
}

pub fn scan_windows(mons: &[Monitor], procs: &[ProcInfo]) -> Vec<Win> {
    let mut hwnds: Vec<isize> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(push_hwnd), LPARAM(&mut hwnds as *mut _ as isize));
    }
    let by_pid: HashMap<u32, &ProcInfo> = procs.iter().map(|p| (p.pid, p)).collect();
    let mut out = Vec::new();
    for h in hwnds {
        let hwnd = HWND(h as *mut c_void);
        if !is_manageable(hwnd) {
            continue;
        }
        let title = window_title(hwnd);
        if title.trim().is_empty() {
            continue;
        }
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        let (exe, exe_name) = match by_pid.get(&pid) {
            Some(p) => (p.exe_path.clone(), p.name_lower.clone()),
            None => (String::new(), String::new()),
        };
        let hmon = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) }.0 as isize;
        let monitor = mons.iter().position(|m| m.hmon == hmon).unwrap_or(0);
        out.push(Win {
            hwnd: h,
            pid,
            title,
            exe,
            exe_name,
            rect: visual_rect(hwnd),
            monitor,
            maximized: unsafe { IsZoomed(hwnd) }.as_bool(),
            minimized: unsafe { IsIconic(hwnd) }.as_bool(),
        });
    }
    out
}

// ---------- ações ----------

/// Põe a janela exatamente no retângulo visual pedido.
///
/// Duas correções que separam "quase encaixou" de "encaixou": desmaximizar antes (uma
/// janela maximizada ignora `SetWindowPos`) e compensar a sombra do DWM, medindo a
/// diferença entre o retângulo cru e o visual *depois* de restaurar.
pub fn place(hwnd_raw: isize, target: R) -> Result<(), String> {
    let hwnd = HWND(hwnd_raw as *mut c_void);
    unsafe {
        if IsIconic(hwnd).as_bool() || IsZoomed(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let mut raw = RECT::default();
        GetWindowRect(hwnd, &mut raw).map_err(|e| e.message())?;
        let raw = R::of(raw);
        let vis = visual_rect(hwnd);
        let (dl, dt) = (raw.l - vis.l, raw.t - vis.t);
        let (dr, db) = (raw.r - vis.r, raw.b - vis.b);
        SetWindowPos(
            hwnd,
            None,
            target.l + dl,
            target.t + dt,
            (target.w() + dr - dl).max(120),
            (target.h() + db - dt).max(80),
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOSENDCHANGING,
        )
        .map_err(|e| e.message())
    }
}

pub fn focus(hwnd_raw: isize) {
    let hwnd = HWND(hwnd_raw as *mut c_void);
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let _ = BringWindowToTop(hwnd);
        let _ = SetForegroundWindow(hwnd);
    }
}

fn show(hwnd_raw: isize, cmd: windows::Win32::UI::WindowsAndMessaging::SHOW_WINDOW_CMD) {
    unsafe {
        let _ = ShowWindow(HWND(hwnd_raw as *mut c_void), cmd);
    }
}

/// Quebra uma linha de argumentos respeitando aspas duplas.
fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for ch in s.chars() {
        match ch {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Diretório de trabalho do slot. `Path::new("mspaint.exe").parent()` devolve
/// `Some("")`, e um cwd vazio faz o CreateProcess falhar — nesse caso herdamos o do RamDog.
fn work_dir(exe: &str) -> Option<std::path::PathBuf> {
    std::path::Path::new(exe)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
}

fn launch(slot: &ScreenSlot) -> Result<(), String> {
    if slot.exe.trim().is_empty() {
        return Err("sem caminho de executável".into());
    }
    let mut cmd = std::process::Command::new(&slot.exe);
    cmd.args(split_args(&slot.args));
    if let Some(d) = work_dir(&slot.exe) {
        cmd.current_dir(d);
    }
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

fn file_name_lower(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_lowercase()
}

fn slot_label(slot: &ScreenSlot) -> String {
    if !slot.label.trim().is_empty() {
        slot.label.clone()
    } else {
        file_name_lower(&slot.exe)
    }
}

/// A janela que este slot quer, entre as que ainda não foram usadas nesta aplicação.
fn match_window(slot: &ScreenSlot, wins: &[Win], used: &HashSet<isize>) -> Option<isize> {
    let want = file_name_lower(&slot.exe);
    let title = slot.title_match.trim().to_lowercase();
    wins.iter()
        .filter(|w| !used.contains(&w.hwnd))
        .filter(|w| w.exe_name == want || file_name_lower(&w.exe) == want)
        .find(|w| title.is_empty() || w.title.to_lowercase().contains(&title))
        .map(|w| w.hwnd)
}

/// Onde este slot cai, em pixels, nos monitores de agora.
fn slot_target(slot: &ScreenSlot, mons: &[Monitor]) -> Option<R> {
    let m = mons.get(slot.monitor).or_else(|| mons.first())?;
    Some(m.work.frac([slot.x, slot.y, slot.w, slot.h]))
}

// ---------- estado ----------

/// Um slot de cenário esperando a janela aparecer depois do `launch`.
struct Waiting {
    slot: ScreenSlot,
    since: Instant,
}

struct Drag {
    hwnd: isize,
    /// Onde o cursor pegou a janela, em fração do retângulo dela — assim a janela não
    /// "pula" para o cursor no primeiro pixel de arrasto.
    grab: egui::Vec2,
    size: egui::Vec2,
}

pub struct Screens {
    mons: Vec<Monitor>,
    wins: Vec<Win>,
    last_scan: Option<Instant>,
    icons: IconBank,
    sel: Option<isize>,
    drag: Option<Drag>,
    /// Enquanto arrasta com encaixe ligado: a zona embaixo do cursor.
    hover_zone: Option<R>,
    preset_sel: String,
    preset_name: String,
    /// Cenário em andamento: o que ainda espera a janela abrir.
    waiting: Vec<Waiting>,
    filter: String,
}

impl Screens {
    pub fn new() -> Self {
        Self {
            mons: Vec::new(),
            wins: Vec::new(),
            last_scan: None,
            icons: IconBank::new(),
            sel: None,
            drag: None,
            hover_zone: None,
            preset_sel: String::new(),
            preset_name: String::new(),
            waiting: Vec::new(),
            filter: String::new(),
        }
    }

    fn rescan(&mut self, procs: &[ProcInfo]) {
        self.mons = scan_monitors();
        self.wins = scan_windows(&self.mons, procs);
        self.last_scan = Some(Instant::now());
    }

    /// Relê a cada 700 ms. Enumerar janelas é barato, mas não a ponto de valer 60x por
    /// segundo — e a lista pulando a cada frame tornaria impossível clicar num botão.
    fn maybe_rescan(&mut self, procs: &[ProcInfo]) {
        let due = match self.last_scan {
            None => true,
            Some(t) => t.elapsed() > Duration::from_millis(700),
        };
        if due {
            self.rescan(procs);
        }
    }

    fn win(&self, hwnd: isize) -> Option<&Win> {
        self.wins.iter().find(|w| w.hwnd == hwnd)
    }

    fn grid(&self, cfg: &Config) -> &'static Grid {
        grid_by_id(&cfg.screen_grid)
    }

    // ---------- UI ----------

    pub fn ui(&mut self, ui: &mut egui::Ui, procs: &[ProcInfo], cfg: &mut Config) -> Vec<ScreenOut> {
        self.maybe_rescan(procs);
        if self.icons.poll(ui.ctx()) {
            ui.ctx().request_repaint();
        }
        let mut out = Vec::new();
        self.pump_waiting(&mut out);

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Telas").strong().size(16.0));
            ui.label(
                RichText::new("— arraste as janelas no mapa, encaixe na grade e monte cenários")
                    .color(MUTED),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Atualizar").on_hover_text("Relê monitores e janelas agora").clicked() {
                    self.last_scan = None;
                }
                if !self.waiting.is_empty() {
                    ui.spinner();
                    ui.label(
                        RichText::new(format!("abrindo {}…", self.waiting.len())).small().color(MUTED),
                    );
                }
            });
        });
        ui.label(
            RichText::new(format!(
                "{} monitor(es) · {} janela(s) organizáveis",
                self.mons.len(),
                self.wins.len()
            ))
            .small()
            .color(MUTED),
        );
        ui.add_space(4.0);

        self.map(ui, cfg, &mut out);
        ui.add_space(6.0);
        self.controls(ui, cfg, &mut out);
        ui.add_space(6.0);
        ui.separator();

        ui.columns(2, |cols| {
            self.windows_panel(&mut cols[0], cfg, &mut out);
            self.preset_panel(&mut cols[1], cfg, &mut out);
        });
        out
    }

    /// O mapa: monitores em escala, janelas arrastáveis, zonas da grade em fantasma.
    fn map(&mut self, ui: &mut egui::Ui, cfg: &Config, out: &mut Vec<ScreenOut>) {
        let h = 250.0_f32;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), h), Sense::hover());
        let p = ui.painter_at(rect);
        p.rect_filled(rect, 8.0, SURFACE);
        if self.mons.is_empty() {
            p.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "nenhum monitor lido",
                egui::FontId::proportional(13.0),
                MUTED,
            );
            return;
        }

        // Envelope do desktop virtual → escala única para tudo que este mapa desenha.
        let bounds = self.mons.iter().fold(self.mons[0].full, |a, m| R {
            l: a.l.min(m.full.l),
            t: a.t.min(m.full.t),
            r: a.r.max(m.full.r),
            b: a.b.max(m.full.b),
        });
        let pad = 10.0;
        let sx = (rect.width() - pad * 2.0) / bounds.w().max(1) as f32;
        let sy = (rect.height() - pad * 2.0) / bounds.h().max(1) as f32;
        let s = sx.min(sy);
        let ox = rect.center().x - (bounds.cx() - bounds.l) as f32 * s - bounds.l as f32 * s;
        let oy = rect.center().y - (bounds.cy() - bounds.t) as f32 * s - bounds.t as f32 * s;
        let to_map = |r: R| {
            egui::Rect::from_min_max(
                egui::pos2(ox + r.l as f32 * s, oy + r.t as f32 * s),
                egui::pos2(ox + r.r as f32 * s, oy + r.b as f32 * s),
            )
        };
        let to_virt = |p: egui::Pos2| (((p.x - ox) / s) as i32, ((p.y - oy) / s) as i32);

        let grid = self.grid(cfg);
        for m in &self.mons {
            let mr = to_map(m.full);
            p.rect_filled(mr, 4.0, Color32::from_rgb(13, 15, 19));
            p.rect_stroke(mr, 4.0, Stroke::new(1.0_f32, LINE), StrokeKind::Inside);
            // Zonas da grade, no fantasma: dá para ver onde a janela vai cair antes de soltar.
            for z in grid.zones {
                let zr = to_map(m.work.frac(*z));
                p.rect_stroke(
                    zr.shrink(1.0),
                    3.0,
                    Stroke::new(1.0_f32, ACCENT.gamma_multiply(0.22)),
                    StrokeKind::Inside,
                );
            }
        }

        // `EnumWindows` devolve de cima para baixo na pilha Z. Desenhar e interagir na ordem
        // inversa faz a janela de cima ficar por cima — e ser ela que responde ao clique.
        let mut click_focus: Option<isize> = None;
        let mut drop_at: Option<(isize, R)> = None;
        // Clonado porque o laço escreve em `self.sel`/`self.drag` enquanto lê as janelas.
        let wins = self.wins.clone();
        for w in wins.iter().rev() {
            if w.minimized {
                continue;
            }
            let wr = to_map(w.rect);
            if wr.width() < 3.0 || wr.height() < 3.0 {
                continue;
            }
            let id = egui::Id::new(("scr_win", w.hwnd));
            let resp = ui.interact(wr, id, Sense::click_and_drag());
            let sel = self.sel == Some(w.hwnd);
            let dragging = self.drag.as_ref().is_some_and(|d| d.hwnd == w.hwnd);
            let fill = if sel { ACCENT_BG } else { SURFACE_HI };
            let fill = if resp.hovered() { fill.gamma_multiply(1.35) } else { fill };
            p.rect_filled(wr, 3.0, fill.gamma_multiply(if dragging { 0.4 } else { 1.0 }));
            p.rect_stroke(
                wr,
                3.0,
                Stroke::new(if sel { 1.6_f32 } else { 1.0_f32 }, if sel { ACCENT } else { LINE }),
                StrokeKind::Inside,
            );
            if wr.width() > 46.0 && wr.height() > 16.0 {
                // Clipado no próprio retângulo: sem isso o título de uma janela estreita
                // vaza por cima do monitor vizinho e o mapa vira sopa de letras.
                p.with_clip_rect(wr.shrink(2.0)).text(
                    wr.left_top() + egui::vec2(4.0, 2.0),
                    egui::Align2::LEFT_TOP,
                    w.title.chars().take(60).collect::<String>(),
                    egui::FontId::proportional(10.0),
                    TEXT.gamma_multiply(0.9),
                );
            }
            let resp = resp.on_hover_text(format!("{}\n{}", w.title, w.exe));

            if resp.drag_started() {
                if let Some(ptr) = resp.interact_pointer_pos() {
                    self.sel = Some(w.hwnd);
                    self.drag = Some(Drag {
                        hwnd: w.hwnd,
                        grab: ptr - wr.min,
                        size: wr.size(),
                    });
                }
            }
            if resp.clicked() {
                self.sel = Some(w.hwnd);
                click_focus = Some(w.hwnd);
            }
            if resp.drag_stopped() {
                if let Some(tgt) = self.hover_zone.take() {
                    drop_at = Some((w.hwnd, tgt));
                }
                self.drag = None;
            }
        }

        // Fantasma do arrasto + zona alvo. Calculado aqui, fora do laço, porque depende do
        // cursor de agora e não do retângulo que a janela tinha no último scan.
        if let Some((dh, grab, dsize)) = self.drag.as_ref().map(|d| (d.hwnd, d.grab, d.size)) {
            if let Some(ptr) = ui.ctx().pointer_interact_pos() {
                let ghost = egui::Rect::from_min_size(ptr - grab, dsize);
                let (vx, vy) = to_virt(ghost.center());
                let mon = self
                    .mons
                    .iter()
                    .position(|m| m.full.contains(vx, vy))
                    .or(if self.mons.is_empty() { None } else { Some(0) });
                let mut target = None;
                if let Some(mi) = mon {
                    let m = &self.mons[mi];
                    if cfg.screen_snap {
                        for z in grid.zones {
                            let zr = m.work.frac(*z);
                            if zr.contains(vx, vy) {
                                target = Some(zr);
                                break;
                            }
                        }
                        if target.is_none() {
                            target = Some(m.work.frac(grid.zones[0]));
                        }
                    } else {
                        let (tl_x, tl_y) = to_virt(ghost.min);
                        if let Some(w) = self.win(dh) {
                            target = Some(R {
                                l: tl_x,
                                t: tl_y,
                                r: tl_x + w.rect.w(),
                                b: tl_y + w.rect.h(),
                            });
                        }
                    }
                }
                self.hover_zone = target;
                if let Some(t) = target {
                    let tr = to_map(t);
                    p.rect_filled(tr, 3.0, ACCENT.gamma_multiply(0.22));
                    p.rect_stroke(tr, 3.0, Stroke::new(1.5_f32, ACCENT), StrokeKind::Inside);
                }
                p.rect_stroke(ghost, 3.0, Stroke::new(1.0_f32, ACCENT.gamma_multiply(0.6)), StrokeKind::Inside);
                ui.ctx().request_repaint();
            }
        }

        // Por último, por cima das janelas: com uma janela maximizada o número do monitor
        // sumia debaixo dela, e o mapa deixava de responder "qual tela é essa".
        for (i, m) in self.mons.iter().enumerate() {
            let mr = to_map(m.full);
            let tag = format!("{}{}", i + 1, if m.primary { " ★" } else { "" });
            // Canto de baixo: o de cima é onde toda janela desenha o próprio título, e o
            // número do monitor comia a primeira palavra de quem estivesse encostado ali.
            let chip = egui::Rect::from_min_size(
                mr.left_bottom() + egui::vec2(4.0, -18.0),
                egui::vec2(if m.primary { 26.0 } else { 15.0 }, 14.0),
            );
            p.rect_filled(chip, 3.0, Color32::from_rgb(13, 15, 19).gamma_multiply(0.92));
            p.text(
                chip.center(),
                egui::Align2::CENTER_CENTER,
                tag,
                egui::FontId::proportional(10.5),
                MUTED,
            );
            p.text(
                mr.right_bottom() + egui::vec2(-5.0, -3.0),
                egui::Align2::RIGHT_BOTTOM,
                format!("{}×{}", m.full.w(), m.full.h()),
                egui::FontId::proportional(10.0),
                MUTED.gamma_multiply(0.75),
            );
        }

        if let Some((hwnd, tgt)) = drop_at {
            match place(hwnd, tgt) {
                Ok(()) => self.last_scan = None,
                Err(e) => out.push(ScreenOut::Toast(format!("não deu para mover: {e}"), true)),
            }
        }
        if let Some(h) = click_focus {
            focus(h);
        }
    }

    /// Grade, encaixe e distribuição — a fileira embaixo do mapa.
    fn controls(&mut self, ui: &mut egui::Ui, cfg: &mut Config, out: &mut Vec<ScreenOut>) {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Grade:").small().color(MUTED));
            let cur = self.grid(cfg);
            let mut id = cur.id.to_string();
            egui::ComboBox::from_id_salt("scr_grid")
                .selected_text(RichText::new(cur.name).size(12.0))
                .width(130.0)
                .show_ui(ui, |ui| {
                    for g in GRIDS {
                        ui.selectable_value(&mut id, g.id.to_string(), g.name);
                    }
                });
            if id != cur.id {
                cfg.screen_grid = id;
                out.push(ScreenOut::SaveCfg);
            }
            let mut snap = cfg.screen_snap;
            if ui
                .checkbox(&mut snap, RichText::new("encaixar ao arrastar").small())
                .on_hover_text(
                    "Ligado: soltar a janela no mapa a encaixa na zona da grade embaixo do cursor.\n\
                     Desligado: a janela vai exatamente para onde você soltou, do tamanho que estava.",
                )
                .changed()
            {
                cfg.screen_snap = snap;
                out.push(ScreenOut::SaveCfg);
            }

            ui.separator();
            let grid = self.grid(cfg);
            for i in 0..self.mons.len() {
                let label = format!("Distribuir {}", i + 1);
                let n = self.wins.iter().filter(|w| w.monitor == i && !w.minimized).count();
                if ui
                    .add_enabled(n > 0, egui::Button::new(RichText::new(label).small()))
                    .on_hover_text(format!(
                        "{}
Encaixa as {n} janela(s) deste monitor nas {} zona(s) da grade \"{}\"",
                        self.mons[i].name,
                        grid.zones.len(),
                        grid.name
                    ))
                    .clicked()
                {
                    self.distribute(i, grid, out);
                }
            }
        });
    }

    /// Enfileira as janelas de um monitor nas zonas da grade, na ordem da pilha Z.
    fn distribute(&mut self, mi: usize, grid: &'static Grid, out: &mut Vec<ScreenOut>) {
        let Some(m) = self.mons.get(mi).cloned() else { return };
        let targets: Vec<isize> = self
            .wins
            .iter()
            .filter(|w| w.monitor == mi && !w.minimized)
            .map(|w| w.hwnd)
            .take(grid.zones.len())
            .collect();
        let mut erros = 0;
        for (i, hwnd) in targets.iter().enumerate() {
            let tgt = m.work.frac(grid.zones[i]);
            if place(*hwnd, tgt).is_err() {
                erros += 1;
            }
        }
        self.last_scan = None;
        if erros > 0 {
            out.push(ScreenOut::Toast(
                format!("{erros} janela(s) não aceitaram mover — provavelmente rodam como admin"),
                true,
            ));
        } else {
            out.push(ScreenOut::Toast(
                format!("{} janela(s) encaixadas em \"{}\"", targets.len(), grid.name),
                false,
            ));
        }
    }

    /// Coluna esquerda: as janelas abertas agora.
    fn windows_panel(&mut self, ui: &mut egui::Ui, cfg: &mut Config, out: &mut Vec<ScreenOut>) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Janelas abertas").strong().size(13.0));
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("filtrar…")
                    .desired_width(110.0),
            );
        });
        ui.add_space(2.0);
        let q = self.filter.trim().to_lowercase();
        let list: Vec<Win> = self
            .wins
            .iter()
            .filter(|w| {
                q.is_empty()
                    || w.title.to_lowercase().contains(&q)
                    || w.exe_name.contains(&q)
            })
            .cloned()
            .collect();

        let mut add_slot: Option<Win> = None;
        egui::ScrollArea::vertical().id_salt("scr_wins").show(ui, |ui| {
            for w in &list {
                let sel = self.sel == Some(w.hwnd);
                let frame = egui::Frame::new()
                    .fill(if sel { ACCENT_BG } else { SURFACE })
                    .stroke(Stroke::new(1.0_f32, if sel { ACCENT } else { LINE }))
                    .corner_radius(5.0)
                    .inner_margin(egui::Margin::symmetric(6, 4));
                frame.show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if !w.exe.is_empty() {
                            if let Some(tex) = self.icons.get(&w.exe) {
                                ui.add(egui::Image::new((tex.id(), egui::Vec2::splat(16.0))));
                            } else {
                                ui.add_space(16.0);
                            }
                        } else {
                            ui.add_space(16.0);
                        }
                        let mut t = w.title.clone();
                        if t.chars().count() > 40 {
                            t = t.chars().take(40).collect::<String>() + "…";
                        }
                        if ui
                            .selectable_label(sel, RichText::new(t).size(12.0))
                            .on_hover_text(format!("{}\n{}\npid {}", w.title, w.exe, w.pid))
                            .clicked()
                        {
                            self.sel = Some(w.hwnd);
                            focus(w.hwnd);
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("+").on_hover_text("Põe esta janela no cenário selecionado, na posição em que ela está").clicked() {
                                add_slot = Some(w.clone());
                            }
                            if ui
                                .add_enabled(!w.maximized, egui::Button::new("□").small())
                                .on_hover_text(if w.maximized { "já está maximizada" } else { "Maximizar" })
                                .clicked()
                            {
                                show(w.hwnd, SW_SHOWMAXIMIZED);
                                self.last_scan = None;
                            }
                            if ui.small_button("—").on_hover_text("Minimizar").clicked() {
                                show(w.hwnd, SW_MINIMIZE);
                                self.last_scan = None;
                            }
                            // Mandar para outro monitor mantendo a fração ocupada.
                            for (i, m) in self.mons.iter().enumerate() {
                                if i == w.monitor {
                                    continue;
                                }
                                if ui
                                    .small_button(RichText::new(format!("→{}", i + 1)).size(10.0))
                                    .on_hover_text(format!("Mandar para o monitor {}", i + 1))
                                    .clicked()
                                {
                                    let src = self.mons[w.monitor].work;
                                    let f = w.rect.frac_in(src);
                                    if let Err(e) = place(w.hwnd, m.work.frac(f)) {
                                        out.push(ScreenOut::Toast(format!("não deu para mover: {e}"), true));
                                    }
                                    self.last_scan = None;
                                }
                            }
                            let estado = if w.minimized {
                                " · minimizada"
                            } else if w.maximized {
                                " · máx"
                            } else {
                                ""
                            };
                            ui.label(
                                RichText::new(format!(
                                    "tela {} · {}×{}{estado}",
                                    w.monitor + 1,
                                    w.rect.w(),
                                    w.rect.h()
                                ))
                                .small()
                                .color(MUTED),
                            );
                        });
                    });
                });
                ui.add_space(2.0);
            }
        });

        if let Some(w) = add_slot {
            self.add_slot_from(&w, cfg, out);
        }
    }

    fn add_slot_from(&mut self, w: &Win, cfg: &mut Config, out: &mut Vec<ScreenOut>) {
        if self.preset_sel.is_empty() || !cfg.screen_presets.contains_key(&self.preset_sel) {
            out.push(ScreenOut::Toast("escolha ou crie um cenário antes".into(), true));
            return;
        }
        if w.exe.is_empty() {
            out.push(ScreenOut::Toast(
                format!("sem caminho do executável de \"{}\" — cenário não conseguiria reabrir", w.title),
                true,
            ));
            return;
        }
        let Some(m) = self.mons.get(w.monitor) else { return };
        let f = w.rect.frac_in(m.work);
        let slot = ScreenSlot {
            exe: w.exe.clone(),
            args: String::new(),
            label: w.title.chars().take(40).collect(),
            monitor: w.monitor,
            x: f[0],
            y: f[1],
            w: f[2],
            h: f[3],
            title_match: String::new(),
            launch: true,
        };
        let name = self.preset_sel.clone();
        if let Some(p) = cfg.screen_presets.get_mut(&name) {
            p.slots.push(slot);
        }
        out.push(ScreenOut::Toast(format!("adicionado ao cenário \"{name}\""), false));
        out.push(ScreenOut::SaveCfg);
    }

    /// Coluna direita: cenários salvos e o conteúdo do selecionado.
    fn preset_panel(&mut self, ui: &mut egui::Ui, cfg: &mut Config, out: &mut Vec<ScreenOut>) {
        ui.label(RichText::new("Cenários").strong().size(13.0));
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            let names: Vec<String> = cfg.screen_presets.keys().cloned().collect();
            let shown = if self.preset_sel.is_empty() { "—".into() } else { self.preset_sel.clone() };
            egui::ComboBox::from_id_salt("scr_preset")
                .selected_text(RichText::new(shown).size(12.0))
                .width(130.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.preset_sel, String::new(), "—");
                    for n in &names {
                        ui.selectable_value(&mut self.preset_sel, n.clone(), n);
                    }
                });
            let has = cfg.screen_presets.contains_key(&self.preset_sel);
            if ui
                .add_enabled(has, egui::Button::new(RichText::new("Aplicar").small()))
                .on_hover_text("Abre o que não estiver aberto e põe cada janela no lugar")
                .clicked()
            {
                let p = cfg.screen_presets.get(&self.preset_sel).cloned().unwrap_or_default();
                self.apply_preset(&p, out);
            }
            if ui
                .add_enabled(
                    has,
                    egui::Button::new(
                        RichText::new("Excluir").small().color(Color32::from_rgb(232, 120, 100)),
                    ),
                )
                .clicked()
            {
                cfg.screen_presets.remove(&self.preset_sel);
                out.push(ScreenOut::Toast(format!("cenário \"{}\" excluído", self.preset_sel), false));
                self.preset_sel.clear();
                out.push(ScreenOut::SaveCfg);
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.preset_name)
                    .hint_text("nome do cenário")
                    .desired_width(120.0),
            );
            let name = self.preset_name.trim().to_string();
            if ui
                .add_enabled(!name.is_empty(), egui::Button::new(RichText::new("Novo vazio").small()))
                .clicked()
            {
                cfg.screen_presets.entry(name.clone()).or_default();
                self.preset_sel = name.clone();
                self.preset_name.clear();
                out.push(ScreenOut::Toast(format!("cenário \"{name}\" criado"), false));
                out.push(ScreenOut::SaveCfg);
            }
            if ui
                .add_enabled(!name.is_empty(), egui::Button::new(RichText::new("Salvar atual").small()))
                .on_hover_text("Guarda todas as janelas de agora, cada uma com sua tela e seu retângulo")
                .clicked()
            {
                let p = self.snapshot();
                let n = p.slots.len();
                cfg.screen_presets.insert(name.clone(), p);
                self.preset_sel = name.clone();
                self.preset_name.clear();
                out.push(ScreenOut::Toast(format!("cenário \"{name}\" salvo com {n} janela(s)"), false));
                out.push(ScreenOut::SaveCfg);
            }
        });

        ui.add_space(4.0);
        let Some(preset) = cfg.screen_presets.get_mut(&self.preset_sel) else {
            ui.label(
                RichText::new(
                    "Escolha um cenário, ou arrume as janelas como você quer e clique \"Salvar atual\".",
                )
                .small()
                .color(MUTED),
            );
            return;
        };
        let n_mon = self.mons.len().max(1);
        let mut remove: Option<usize> = None;
        let mut changed = false;
        egui::ScrollArea::vertical().id_salt("scr_slots").show(ui, |ui| {
            for (i, slot) in preset.slots.iter_mut().enumerate() {
                egui::Frame::new()
                    .fill(SURFACE)
                    .stroke(Stroke::new(1.0_f32, LINE))
                    .corner_radius(5.0)
                    .inner_margin(egui::Margin::symmetric(6, 4))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if let Some(tex) = self.icons.get(&slot.exe) {
                                ui.add(egui::Image::new((tex.id(), egui::Vec2::splat(16.0))));
                            } else {
                                ui.add_space(16.0);
                            }
                            ui.label(RichText::new(slot_label(slot)).size(12.0))
                                .on_hover_text(&slot.exe);
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button("✖").on_hover_text("Tirar do cenário").clicked() {
                                    remove = Some(i);
                                }
                                let mut m = slot.monitor + 1;
                                if ui
                                    .add(egui::DragValue::new(&mut m).range(1..=n_mon).prefix("tela "))
                                    .changed()
                                {
                                    slot.monitor = m - 1;
                                    changed = true;
                                }
                                if ui
                                    .checkbox(&mut slot.launch, RichText::new("abrir").small())
                                    .on_hover_text("Abrir o programa se não houver janela dele")
                                    .changed()
                                {
                                    changed = true;
                                }
                            });
                        });
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!(
                                    "{:.0}% × {:.0}% em ({:.0}%, {:.0}%)",
                                    slot.w * 100.0,
                                    slot.h * 100.0,
                                    slot.x * 100.0,
                                    slot.y * 100.0
                                ))
                                .small()
                                .color(MUTED),
                            );
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui
                                    .add(
                                        egui::TextEdit::singleline(&mut slot.title_match)
                                            .hint_text("título contém…")
                                            .desired_width(100.0),
                                    )
                                    .changed()
                                {
                                    changed = true;
                                }
                            });
                        });
                    });
                ui.add_space(2.0);
            }
        });
        if let Some(i) = remove {
            preset.slots.remove(i);
            changed = true;
        }
        if changed {
            out.push(ScreenOut::SaveCfg);
        }
    }

    /// Foto de todas as janelas de agora, já em fração da área útil de cada monitor.
    fn snapshot(&self) -> ScreenPreset {
        let mut slots = Vec::new();
        for w in &self.wins {
            if w.exe.is_empty() || w.minimized {
                continue;
            }
            let Some(m) = self.mons.get(w.monitor) else { continue };
            let f = w.rect.frac_in(m.work);
            slots.push(ScreenSlot {
                exe: w.exe.clone(),
                args: String::new(),
                label: w.title.chars().take(40).collect(),
                monitor: w.monitor,
                x: f[0],
                y: f[1],
                w: f[2],
                h: f[3],
                title_match: String::new(),
                launch: true,
            });
        }
        ScreenPreset { slots }
    }

    /// Aplica um cenário: o que já está aberto vai para o lugar agora; o que falta é
    /// aberto e entra na fila de espera — a UI não pode travar esperando o Chrome subir.
    fn apply_preset(&mut self, preset: &ScreenPreset, out: &mut Vec<ScreenOut>) {
        let mut used: HashSet<isize> = HashSet::new();
        let mut movidas = 0;
        let mut abrindo = 0;
        let mut erros: Vec<String> = Vec::new();
        for slot in &preset.slots {
            let Some(target) = slot_target(slot, &self.mons) else { continue };
            match match_window(slot, &self.wins, &used) {
                Some(hwnd) => {
                    used.insert(hwnd);
                    match place(hwnd, target) {
                        Ok(()) => movidas += 1,
                        Err(e) => erros.push(format!("{}: {e}", slot_label(slot))),
                    }
                }
                None if slot.launch => match launch(slot) {
                    Ok(()) => {
                        abrindo += 1;
                        self.waiting.push(Waiting { slot: slot.clone(), since: Instant::now() });
                    }
                    Err(e) => erros.push(format!("{}: {e}", slot_label(slot))),
                },
                None => erros.push(format!("{}: sem janela aberta", slot_label(slot))),
            }
        }
        self.last_scan = None;
        if erros.is_empty() {
            out.push(ScreenOut::Toast(
                format!("{movidas} posicionada(s), {abrindo} abrindo"),
                false,
            ));
        } else {
            out.push(ScreenOut::Toast(
                format!("{movidas} posicionada(s), {abrindo} abrindo · {}", erros.join(" · ")),
                true,
            ));
        }
    }

    /// A cada frame: as janelas que os cenários estão esperando já apareceram?
    fn pump_waiting(&mut self, out: &mut Vec<ScreenOut>) {
        if self.waiting.is_empty() {
            return;
        }
        let mons = self.mons.clone();
        let wins = self.wins.clone();
        let mut still = Vec::new();
        for w in std::mem::take(&mut self.waiting) {
            let used = HashSet::new();
            match match_window(&w.slot, &wins, &used) {
                Some(hwnd) => {
                    if let Some(t) = slot_target(&w.slot, &mons) {
                        if let Err(e) = place(hwnd, t) {
                            out.push(ScreenOut::Toast(
                                format!("{}: {e}", slot_label(&w.slot)),
                                true,
                            ));
                        }
                    }
                    self.last_scan = None;
                }
                None if w.since.elapsed() > LAUNCH_TIMEOUT => {
                    out.push(ScreenOut::Toast(
                        format!("{}: a janela não apareceu em {}s", slot_label(&w.slot), LAUNCH_TIMEOUT.as_secs()),
                        true,
                    ));
                }
                None => still.push(w),
            }
        }
        self.waiting = still;
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn executavel_sem_pasta_nao_vira_cwd_vazio() {
        assert_eq!(work_dir("mspaint.exe"), None);
        assert_eq!(
            work_dir(r"C:\Windows\System32\notepad.exe"),
            Some(std::path::PathBuf::from(r"C:\Windows\System32"))
        );
    }

    #[test]
    fn fracao_ida_e_volta() {
        let base = R { l: 100, t: 50, r: 2020, b: 1130 };
        let alvo = base.frac([0.5, 0.0, 0.5, 1.0]);
        assert_eq!(alvo, R { l: 1060, t: 50, r: 2020, b: 1130 });
        let f = alvo.frac_in(base);
        assert!((f[0] - 0.5).abs() < 1e-3 && (f[2] - 0.5).abs() < 1e-3);
    }

    #[test]
    fn argumentos_com_aspas_ficam_inteiros() {
        assert_eq!(
            split_args("--profile \"Perfil 2\" --new-window"),
            vec!["--profile", "Perfil 2", "--new-window"]
        );
    }

    #[test]
    fn slot_cai_no_primario_quando_o_monitor_sumiu() {
        let mons = vec![Monitor {
            hmon: 1,
            name: "\\\\.\\DISPLAY1".into(),
            full: R { l: 0, t: 0, r: 1920, b: 1080 },
            work: R { l: 0, t: 0, r: 1920, b: 1040 },
            primary: true,
        }];
        let slot = ScreenSlot { monitor: 3, x: 0.0, y: 0.0, w: 1.0, h: 1.0, ..Default::default() };
        assert_eq!(slot_target(&slot, &mons), Some(R { l: 0, t: 0, r: 1920, b: 1040 }));
    }
}
