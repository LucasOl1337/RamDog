#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod gpu_linux;
#[cfg(target_os = "linux")]
mod fans_linux;
#[cfg(target_os = "linux")]
mod desktop_linux;
#[cfg(target_os = "linux")]
mod startup_linux;
#[cfg(windows)]
mod boot;
#[cfg(target_os = "linux")]
#[path = "boot_linux.rs"]
mod boot;
#[cfg(not(any(windows, target_os = "linux")))]
#[path = "boot_stub.rs"]
mod boot;
mod categories;
mod config;
#[cfg(windows)]
mod drains;
#[cfg(target_os = "linux")]
#[path = "drains_linux.rs"]
mod drains;
#[cfg(not(any(windows, target_os = "linux")))]
#[path = "drains_stub.rs"]
mod drains;
mod hwtemp;
mod icons;
mod knowledge;
mod metrics;
mod procs;
mod sampler;
#[cfg(windows)]
mod screens;
#[cfg(target_os = "linux")]
#[path = "screens_linux.rs"]
mod screens;
#[cfg(not(any(windows, target_os = "linux")))]
#[path = "screens_stub.rs"]
mod screens;
mod signature;
#[cfg(windows)]
mod sys;
mod usage;

fn main() -> eframe::Result<()> {
    #[cfg(target_os = "linux")]
    {
        if std::env::args().nth(1).as_deref()==Some("--fan-helper") {
            let result=std::env::args().nth(2).and_then(|p|p.parse().ok()).ok_or_else(||"PID inválido".to_string()).and_then(fans_linux::helper);
            if let Err(error)=result{eprintln!("{error}");std::process::exit(1);}return Ok(());
        }
        linux::log("iniciando");
        let old_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            linux::log(&format!("PANIC {info}\n{}", std::backtrace::Backtrace::force_capture()));
            old_hook(info);
        }));
    }
    let result = run();
    #[cfg(target_os = "linux")]
    linux::log(&format!("saída: {result:?}"));
    result
}

fn run() -> eframe::Result<()> {
    #[cfg(target_os = "linux")]
    if std::env::args().any(|a|a=="--diagnose") {
        let gpu=gpu_linux::collect(&mut gpu_linux::DrmCounters::default());
        let mut sampler=procs::Sampler::new();let processes=sampler.sample();
        let windows=screens::scan();let startup=startup_linux::scan();
        let hw=hwtemp::HwTempReader::spawn().map(|r|r.read()).unwrap_or_default();
        println!("{}",serde_json::json!({
            "processes":processes.len(),"memory_details":processes.iter().filter(|p|p.linux_memory.is_some()).count(),
            "gpus":gpu.cards.iter().map(|g|serde_json::json!({"name":g.name,"util_pct":g.util_pct,"memory_used":g.mem_used,"memory_total":g.mem_total,"temperature":g.temp_c,"power_w":g.power_w,"fan_pct":g.fan_pct})).collect::<Vec<_>>(),
            "gpu_processes":gpu.by_pid.len(),"gpu_memory_processes":gpu.memory_by_pid.len(),"gpu_error":gpu.error,
            "windows":windows.as_ref().map(|w|w.windows.len()).ok(),"monitors":windows.as_ref().map(|w|w.monitors.len()).ok(),
            "startup_entries":startup.as_ref().map(|s|s.entries.len()).ok(),"startup_warnings":startup.as_ref().map(|s|s.warnings.clone()).ok(),
            "sensors":hw.sensors.len(),"fan_control_supported":fans_linux::supported(),
        }));return Ok(());
    }
    procs::enable_debug_privilege();
    // A janela já nasce no modo em que o app foi fechado. Abrir grande e encolher no
    // primeiro frame faria o HUD "piscar" em tela cheia a cada abertura.
    let cfg = config::Config::load();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("RamDog").with_app_id("ramdog")
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
    // NVIDIA EGL/Wayland can block eglSwapBuffers waiting for a frame callback on
    // hidden workspaces. Keep winit's event loop responsive to compositor pings;
    // eframe/Wayland frame callbacks and request_repaint_after pace redraws.
    let options = eframe::NativeOptions { viewport, vsync: !cfg!(target_os = "linux"), ..Default::default() };
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
