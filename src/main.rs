#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod categories;
mod config;
mod drains;
mod hwtemp;
mod icons;
mod metrics;
mod procs;
mod sampler;
mod sys;

fn main() -> eframe::Result<()> {
    procs::enable_debug_privilege();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("RamDog")
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([760.0, 420.0])
            .with_icon(app_icon()),
        vsync: true,
        ..Default::default()
    };
    eframe::run_native("RamDog", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}

fn app_icon() -> egui::IconData {
    let img = image::load_from_memory(include_bytes!("../assets/ramdog-256.png"))
        .expect("ramdog icon")
        .into_rgba8();
    let (width, height) = img.dimensions();
    egui::IconData { rgba: img.into_raw(), width, height }
}
