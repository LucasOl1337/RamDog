//! Hyprland window management: monitor map, drag/drop, grids and reusable scenarios.
use crate::{
    config::{Config, ScreenPreset, ScreenSlot},
    linux::{self, Job},
    procs::ProcInfo,
};
use serde::Deserialize;
use std::collections::BTreeSet;

pub enum ScreenOut {
    Toast(String, bool),
    SaveCfg,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Monitor {
    pub id: i64,
    pub name: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub scale: f32,
    pub transform: u32,
    pub reserved: [f32; 4],
}
impl Monitor {
    pub fn area(&self) -> egui::Rect {
        let (w, h) = if self.transform % 2 == 1 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        };
        let scale = self.scale.max(0.1);
        egui::Rect::from_min_size(
            egui::pos2(self.x + self.reserved[0], self.y + self.reserved[1]),
            egui::vec2(
                (w / scale - self.reserved[0] - self.reserved[2]).max(1.0),
                (h / scale - self.reserved[1] - self.reserved[3]).max(1.0),
            ),
        )
    }
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Window {
    pub address: String,
    pub pid: u32,
    pub title: String,
    pub class: String,
    pub monitor: i64,
    pub at: [f32; 2],
    pub size: [f32; 2],
    pub floating: bool,
    pub mapped: bool,
}
#[derive(Clone, Default)]
pub struct Layout {
    pub monitors: Vec<Monitor>,
    pub windows: Vec<Window>,
}
pub fn scan() -> Result<Layout, String> {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
        return Err(
            "Esta integração controla janelas no Hyprland. Inicie na sessão Hyprland/Omarchy."
                .into(),
        );
    }
    let mut monitors: Vec<Monitor> =
        serde_json::from_str(&linux::command("hyprctl", &["-j", "monitors"])?)
            .map_err(|e| e.to_string())?;
    monitors.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    let windows: Vec<Window> =
        serde_json::from_str(&linux::command("hyprctl", &["-j", "clients"])?)
            .map_err(|e| e.to_string())?;
    Ok(Layout {
        monitors,
        windows: windows
            .into_iter()
            .filter(|w| w.mapped && w.pid > 0)
            .collect(),
    })
}
fn selector(address: &str) -> Result<String, String> {
    if !address
        .strip_prefix("0x")
        .is_some_and(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit()))
    {
        return Err("Endereço de janela inválido".into());
    }
    Ok(format!("address:{address}"))
}
fn dispatch(action: &str, arg: &str) -> Result<(), String> {
    static LUA: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let lua = *LUA.get_or_init(|| {
        linux::command("hyprctl", &["-j", "version"])
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v["version"].as_str().map(str::to_owned))
            .is_some_and(|v| {
                v.trim_start_matches('v')
                    .split('.')
                    .nth(1)
                    .and_then(|n| n.parse::<u32>().ok())
                    .is_some_and(|n| n >= 55)
            })
    });
    let response = if lua {
        let expression = lua_dispatch(action, arg)?;
        linux::command("hyprctl", &["dispatch", &expression])?
    } else {
        linux::command("hyprctl", &["dispatch", action, arg])?
    };
    if response.lines().all(|s| s.trim() == "ok") {
        Ok(())
    } else {
        Err(response)
    }
}
fn lua_dispatch(action: &str, arg: &str) -> Result<String, String> {
    let quote = |s: &str| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into());
    let window = |s: &str| -> Result<String, String> {
        selector(s.strip_prefix("address:").ok_or("Seletor inválido")?)?;
        Ok(quote(s))
    };
    Ok(match action {
        "focuswindow" => format!("hl.dsp.focus({{window={}}})", window(arg)?),
        "setfloating" | "settiled" => format!(
            "hl.dsp.window.float({{window={},action=\"{}\"}})",
            window(arg)?,
            if action == "setfloating" {
                "set"
            } else {
                "unset"
            }
        ),
        "fullscreenstate" => {
            "hl.dsp.window.fullscreen_state({internal=0,client=0,action=\"set\"})".into()
        }
        "movewindow" => {
            let (monitor, w) = arg.split_once(',').ok_or("Monitor inválido")?;
            format!(
                "hl.dsp.window.move({{monitor={},window={},follow=true}})",
                quote(monitor.trim_start_matches("mon:")),
                window(w)?
            )
        }
        "resizewindowpixel" | "movewindowpixel" => {
            let (coords, w) = arg.split_once(',').ok_or("Coordenadas inválidas")?;
            let fields: Vec<_> = coords.split_whitespace().collect();
            if fields.len() != 3 || fields[0] != "exact" {
                return Err("Coordenadas inválidas".into());
            }
            let x = fields[1].parse::<i32>().map_err(|e| e.to_string())?;
            let y = fields[2].parse::<i32>().map_err(|e| e.to_string())?;
            format!(
                "hl.dsp.window.{}({{x={x},y={y},relative=false,window={}}})",
                if action == "resizewindowpixel" {
                    "resize"
                } else {
                    "move"
                },
                window(w)?
            )
        }
        _ => return Err("Dispatcher não suportado".into()),
    })
}

pub fn place(address: &str, monitor: &Monitor, rect: egui::Rect) -> Result<(), String> {
    let window = selector(address)?;
    if !monitor
        .name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "_-".contains(c))
    {
        return Err("Nome de monitor inválido".into());
    }
    if !rect.is_finite() || rect.width() < 1.0 || rect.height() < 1.0 {
        return Err("Retângulo inválido".into());
    }
    dispatch("focuswindow", &window)?;
    dispatch("fullscreenstate", "0 0")?;
    dispatch("setfloating", &window)?;
    dispatch("movewindow", &format!("mon:{},{}", monitor.name, window))?;
    dispatch(
        "resizewindowpixel",
        &format!(
            "exact {} {},{}",
            rect.width().round() as i32,
            rect.height().round() as i32,
            window
        ),
    )?;
    dispatch(
        "movewindowpixel",
        &format!(
            "exact {} {},{}",
            rect.left().round() as i32,
            rect.top().round() as i32,
            window
        ),
    )
}
fn zones(monitor: &Monitor, grid: &str) -> Vec<egui::Rect> {
    let (cols, rows) = match grid {
        "2" => (2, 1),
        "3" => (3, 1),
        "4" => (2, 2),
        _ => (1, 1),
    };
    let a = monitor.area();
    let gap = 8.0;
    (0..rows)
        .flat_map(|row| {
            (0..cols).map(move |col| {
                egui::Rect::from_min_size(
                    egui::pos2(
                        a.left() + col as f32 * a.width() / cols as f32 + gap,
                        a.top() + row as f32 * a.height() / rows as f32 + gap,
                    ),
                    egui::vec2(
                        (a.width() / cols as f32 - 2.0 * gap).max(1.0),
                        (a.height() / rows as f32 - 2.0 * gap).max(1.0),
                    ),
                )
            })
        })
        .collect()
}

pub fn capture(layout: &Layout, selected: &BTreeSet<String>) -> ScreenPreset {
    ScreenPreset {
        slots: layout
            .windows
            .iter()
            .filter(|w| selected.contains(&w.address))
            .filter_map(|w| {
                let (index, m) = layout
                    .monitors
                    .iter()
                    .enumerate()
                    .find(|(_, m)| m.id == w.monitor)?;
                let area = m.area();
                let exe = std::fs::read_link(format!("/proc/{}/exe", w.pid))
                    .ok()?
                    .to_string_lossy()
                    .into_owned();
                let argv = std::fs::read(format!("/proc/{}/cmdline", w.pid))
                    .unwrap_or_default()
                    .split(|b| *b == 0)
                    .filter(|b| !b.is_empty())
                    .skip(1)
                    .map(|b| String::from_utf8_lossy(b).into_owned())
                    .collect::<Vec<_>>();
                Some(ScreenSlot {
                    exe,
                    args: String::new(),
                    argv,
                    label: if w.class.is_empty() {
                        w.title.clone()
                    } else {
                        w.class.clone()
                    },
                    monitor: index,
                    monitor_name: m.name.clone(),
                    x: ((w.at[0] - area.left()) / area.width()).clamp(0.0, 1.0),
                    y: ((w.at[1] - area.top()) / area.height()).clamp(0.0, 1.0),
                    w: (w.size[0] / area.width()).clamp(0.01, 1.0),
                    h: (w.size[1] / area.height()).clamp(0.01, 1.0),
                    title_match: String::new(),
                    launch: false,
                })
            })
            .collect(),
    }
}
pub fn apply(preset: ScreenPreset) -> Result<(), String> {
    let mut layout = scan()?;
    let mut used = BTreeSet::new();
    let mut missing = Vec::new();
    for slot in preset.slots {
        let matches = |w: &&Window| {
            !used.contains(&w.address)
                && (slot.title_match.is_empty() || w.title.contains(&slot.title_match))
                && std::fs::read_link(format!("/proc/{}/exe", w.pid))
                    .is_ok_and(|p| p.to_string_lossy() == slot.exe)
        };
        let mut found = layout.windows.iter().find(matches).cloned();
        if found.is_none() && slot.launch {
            let args = if slot.argv.is_empty() {
                linux::words(&slot.args)?
            } else {
                slot.argv.clone()
            };
            let mut child = std::process::Command::new(&slot.exe)
                .args(args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map_err(|e| e.to_string())?;
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            for _ in 0..30 {
                std::thread::sleep(std::time::Duration::from_millis(300));
                layout = scan()?;
                found = layout.windows.iter().find(matches).cloned();
                if found.is_some() {
                    break;
                }
            }
        }
        let Some(window) = found else {
            missing.push(slot.label);
            continue;
        };
        let monitor = layout
            .monitors
            .iter()
            .find(|m| m.name == slot.monitor_name)
            .or_else(|| layout.monitors.get(slot.monitor))
            .ok_or("Monitor do cenário indisponível")?;
        let a = monitor.area();
        let w = slot.w.clamp(0.01, 1.0) * a.width();
        let h = slot.h.clamp(0.01, 1.0) * a.height();
        let x = (a.left() + slot.x * a.width()).clamp(a.left(), (a.right() - w).max(a.left()));
        let y = (a.top() + slot.y * a.height()).clamp(a.top(), (a.bottom() - h).max(a.top()));
        place(
            &window.address,
            monitor,
            egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h)),
        )?;
        used.insert(window.address);
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Janelas não encontradas (abrir automaticamente desativado): {}",
            missing.join(", ")
        ))
    }
}

#[derive(Default)]
pub struct Screens {
    layout: Job<Layout>,
    action: Job<()>,
    selected: BTreeSet<String>,
    monitor: usize,
    preset: String,
    dragging: Option<(String, egui::Vec2)>,
}
impl Screens {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        _procs: &[ProcInfo],
        cfg: &mut Config,
    ) -> Vec<ScreenOut> {
        let mut out = Vec::new();
        self.layout.poll();
        if self.action.poll() {
            self.layout.start(scan);
        }
        if self.layout.due(3) {
            self.layout.start(scan);
        }
        ui.heading("Telas · Hyprland");
        ui.label("Selecione janelas para distribuir em uma grade. Arraste uma janela no mapa para outro monitor.");
        self.layout.status(ui);
        self.action.status(ui);
        ui.horizontal(|ui| {
            if ui.button("Atualizar").clicked() {
                self.layout.start(scan);
            }
            egui::ComboBox::from_id_salt("linux-monitor")
                .selected_text(
                    self.layout
                        .value
                        .monitors
                        .get(self.monitor)
                        .map(|m| m.name.as_str())
                        .unwrap_or("Monitor"),
                )
                .show_ui(ui, |ui| {
                    for (i, m) in self.layout.value.monitors.iter().enumerate() {
                        ui.selectable_value(&mut self.monitor, i, &m.name);
                    }
                });
            for (id, label) in [
                ("1", "Inteira"),
                ("2", "2 colunas"),
                ("3", "3 colunas"),
                ("4", "4 quadrantes"),
            ] {
                if ui
                    .selectable_value(&mut cfg.screen_grid, id.into(), label)
                    .changed()
                {
                    out.push(ScreenOut::SaveCfg);
                }
            }
            if ui
                .checkbox(&mut cfg.screen_snap, "Encaixar na grade")
                .changed()
            {
                out.push(ScreenOut::SaveCfg);
            }
            if ui
                .add_enabled(
                    !self.action.busy() && !self.selected.is_empty(),
                    egui::Button::new("Organizar selecionadas"),
                )
                .clicked()
            {
                if let Some(monitor) = self.layout.value.monitors.get(self.monitor).cloned() {
                    let addresses = self.selected.iter().cloned().collect::<Vec<_>>();
                    let grid = cfg.screen_grid.clone();
                    self.action.start(move || {
                        let zones = zones(&monitor, &grid);
                        for (i, address) in addresses.iter().enumerate() {
                            place(address, &monitor, zones[i % zones.len()])?;
                        }
                        Ok(())
                    });
                }
            }
        });
        let layout = &self.layout.value;
        if !layout.monitors.is_empty() {
            let bounds = layout
                .monitors
                .iter()
                .map(Monitor::area)
                .reduce(|a, b| a.union(b))
                .unwrap();
            let scale = (ui.available_width() / bounds.width())
                .min(230.0 / bounds.height())
                .max(0.01);
            let (canvas, _) = ui.allocate_exact_size(bounds.size() * scale, egui::Sense::hover());
            let map = |r: egui::Rect| {
                egui::Rect::from_min_size(
                    canvas.min + (r.min - bounds.min) * scale,
                    r.size() * scale,
                )
            };
            for monitor in &layout.monitors {
                let rect = map(monitor.area());
                ui.painter().rect_stroke(
                    rect,
                    3.0,
                    egui::Stroke::new(2.0_f32, egui::Color32::GRAY),
                    egui::StrokeKind::Inside,
                );
                ui.painter().text(
                    rect.min + egui::vec2(5.0, 4.0),
                    egui::Align2::LEFT_TOP,
                    &monitor.name,
                    egui::FontId::proportional(12.0),
                    egui::Color32::WHITE,
                );
            }
            for window in &layout.windows {
                let rect = map(egui::Rect::from_min_size(
                    egui::pos2(window.at[0], window.at[1]),
                    egui::vec2(window.size[0], window.size[1]),
                ))
                .intersect(canvas);
                if !rect.is_positive() {
                    continue;
                }
                let response = ui
                    .interact(
                        rect,
                        egui::Id::new(("linux-window-map", &window.address)),
                        egui::Sense::click_and_drag(),
                    )
                    .on_hover_text(&window.title);
                ui.painter().rect_filled(
                    rect.shrink(2.0),
                    2.0,
                    if self.selected.contains(&window.address) {
                        egui::Color32::from_rgb(50, 100, 150)
                    } else {
                        egui::Color32::from_rgba_unmultiplied(80, 80, 90, 150)
                    },
                );
                if response.clicked() {
                    if !self.selected.remove(&window.address) {
                        self.selected.insert(window.address.clone());
                    }
                }
                if response.drag_started() {
                    if let Some(pointer) = ui.ctx().pointer_latest_pos() {
                        let world = bounds.min + (pointer - canvas.min) / scale;
                        self.dragging = Some((
                            window.address.clone(),
                            world - egui::pos2(window.at[0], window.at[1]),
                        ));
                    }
                }
                if response.drag_stopped() {
                    if let (Some((address, offset)), Some(pos)) =
                        (self.dragging.take(), ui.ctx().pointer_latest_pos())
                    {
                        if let Some(m) = layout
                            .monitors
                            .iter()
                            .find(|m| map(m.area()).contains(pos))
                            .cloned()
                        {
                            let a = m.area();
                            let size = egui::vec2(
                                window.size[0].min(a.width()).max(1.0),
                                window.size[1].min(a.height()).max(1.0),
                            );
                            let translated = bounds.min + (pos - canvas.min) / scale - offset;
                            let target = egui::Rect::from_min_size(
                                egui::pos2(
                                    translated
                                        .x
                                        .clamp(a.left(), (a.right() - size.x).max(a.left())),
                                    translated
                                        .y
                                        .clamp(a.top(), (a.bottom() - size.y).max(a.top())),
                                ),
                                size,
                            );
                            let world = bounds.min + (pos - canvas.min) / scale;
                            let target = if cfg.screen_snap {
                                zones(&m, &cfg.screen_grid)
                                    .into_iter()
                                    .find(|zone| zone.contains(world))
                                    .unwrap_or(target)
                            } else {
                                target
                            };
                            self.action.start(move || place(&address, &m, target));
                        }
                    }
                }
            }
        }
        ui.horizontal(|ui| {
            ui.label("Cenário");
            ui.text_edit_singleline(&mut self.preset);
            if ui
                .add_enabled(
                    !self.selected.is_empty(),
                    egui::Button::new("Salvar selecionadas"),
                )
                .clicked()
                && !self.preset.trim().is_empty()
            {
                cfg.screen_presets.insert(
                    self.preset.trim().into(),
                    capture(&self.layout.value, &self.selected),
                );
                out.push(ScreenOut::SaveCfg);
            }
        });
        for (name, preset) in &mut cfg.screen_presets {
            ui.collapsing(name, |ui| {
                if ui
                    .add_enabled(!self.action.busy(), egui::Button::new("Aplicar cenário"))
                    .clicked()
                {
                    let preset = preset.clone();
                    self.action.start(move || apply(preset));
                }
                for slot in &mut preset.slots {
                    ui.horizontal(|ui| {
                        ui.label(&slot.label);
                        if ui.checkbox(&mut slot.launch, "Abrir se fechada").changed() {
                            out.push(ScreenOut::SaveCfg);
                        }
                    });
                }
            });
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            for window in &self.layout.value.windows {
                ui.horizontal(|ui| {
                    let mut selected = self.selected.contains(&window.address);
                    if ui
                        .checkbox(
                            &mut selected,
                            format!("{} · {}", window.class, window.title),
                        )
                        .changed()
                    {
                        if selected {
                            self.selected.insert(window.address.clone());
                        } else {
                            self.selected.remove(&window.address);
                        }
                    }
                    if ui.button("Focar").clicked() {
                        let a = window.address.clone();
                        self.action
                            .start(move || dispatch("focuswindow", &selector(&a)?));
                    }
                    if window.floating && ui.button("Voltar ao mosaico").clicked() {
                        let a = window.address.clone();
                        self.action
                            .start(move || dispatch("settiled", &selector(&a)?));
                    }
                });
            }
        });
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(500));
        out
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn monitor_geometry_accounts_for_scale_rotation_and_panels() {
        let m = Monitor {
            width: 2160.0,
            height: 3840.0,
            scale: 2.0,
            transform: 1,
            reserved: [0.0, 30.0, 0.0, 0.0],
            ..Default::default()
        };
        assert_eq!(m.area().size(), egui::vec2(1920.0, 1050.0));
        let grid = zones(&m, "4");
        assert_eq!(grid.len(), 4);
        for zone in grid {
            assert!(m.area().contains_rect(zone));
        }
        assert!(selector("0x123abc").is_ok());
        assert!(selector("0x123; dispatch exit").is_err());
    }
}

#[cfg(test)]
mod linux_integration {
    use super::*;
    #[test]
    #[ignore = "Moves only an explicitly selected RamDog test window"]
    fn window_move_resize_restore() {
        let pid = std::env::var("RAMDOG_TEST_WINDOW_PID")
            .unwrap()
            .parse::<u32>()
            .unwrap();
        let layout = scan().unwrap();
        let window = layout
            .windows
            .iter()
            .find(|w| w.pid == pid && w.class == "ramdog")
            .unwrap()
            .clone();
        let original_monitor = layout
            .monitors
            .iter()
            .find(|m| m.id == window.monitor)
            .unwrap()
            .clone();
        let original = egui::Rect::from_min_size(
            egui::pos2(window.at[0], window.at[1]),
            egui::vec2(window.size[0], window.size[1]),
        );
        struct Restore(Window, Monitor, egui::Rect);
        impl Drop for Restore {
            fn drop(&mut self) {
                let _ = place(&self.0.address, &self.1, self.2);
                if !self.0.floating {
                    let _ = dispatch("settiled", &selector(&self.0.address).unwrap());
                }
            }
        }
        let _restore = Restore(window.clone(), original_monitor.clone(), original);
        let target_monitor = layout
            .monitors
            .iter()
            .find(|m| m.id != window.monitor)
            .unwrap_or(&original_monitor);
        let target = target_monitor.area().shrink(20.0);
        place(&window.address, target_monitor, target).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(700));
        let moved = scan()
            .unwrap()
            .windows
            .into_iter()
            .find(|w| w.pid == pid)
            .unwrap();
        assert_eq!(moved.monitor, target_monitor.id);
        assert!(
            (moved.at[0] - target.left()).abs() < 4.0 && (moved.at[1] - target.top()).abs() < 4.0,
            "{:?} != {:?}",
            moved.at,
            target
        );
        assert!(
            (moved.size[0] - target.width()).abs() < 4.0,
            "{:?} != {:?}",
            moved.size,
            target
        );
    }
}
