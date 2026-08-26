#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
#[cfg(windows)]
mod boot;
#[cfg(not(windows))]
#[path = "boot_stub.rs"]
mod boot;
mod categories;
mod config;
#[cfg(windows)]
mod drains;
#[cfg(not(windows))]
#[path = "drains_stub.rs"]
mod drains;
mod hwtemp;
mod icons;
mod knowledge;
mod metrics;
mod procs;
mod sampler;
mod signature;
#[cfg(windows)]
mod sys;
mod usage;

fn main() -> eframe::Result<()> {
    procs::enable_debug_privilege();
    // A janela já nasce no modo em que o app foi fechado. Abrir grande e encolher no
    // primeiro frame faria o HUD "piscar" em tela cheia a cada abertura.
    let cfg = config::Config::load();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("RamDog")
        .with_icon(app_icon());
    viewport = if cfg.mini {
        viewport
            .with_inner_size([app::MINI_W, app::MINI_H])
            .with_min_inner_size([app::MINI_W, app::MINI_H])
            .with_decorations(false)
            .with_resizable(false)
            .with_always_on_top_if(cfg.mini_on_top)
    } else {
        viewport
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([app::FULL_MIN_W, app::FULL_MIN_H])
    };
    let options = eframe::NativeOptions { viewport, vsync: true, ..Default::default() };
    eframe::run_native("RamDog", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}

trait ViewportBuilderExt {
    fn with_always_on_top_if(self, on: bool) -> Self;
}

impl ViewportBuilderExt for egui::ViewportBuilder {
    fn with_always_on_top_if(self, on: bool) -> Self {
        if on {
            self.with_always_on_top()
        } else {
            self
        }
    }
}

fn app_icon() -> egui::IconData {
    let img = image::load_from_memory(include_bytes!("../assets/ramdog-256.png"))
        .expect("ramdog icon")
        .into_rgba8();
    let (width, height) = img.dimensions();
    egui::IconData { rgba: img.into_raw(), width, height }
}
