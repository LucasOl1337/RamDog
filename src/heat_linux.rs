//! Calor: quem esquenta o PC. Processo comendo CPU há horas, de preferência sem ninguém
//! acima dele, e os dois serviços que seguram a temperatura (`cpu-teto` e
//! `ventoinha-calma`) num botão só.
//!
//! "Largado" é a mesma regra da Faxina (`sweep::lineage`): subindo pelos shells, quem abriu
//! já saiu e o processo ficou pendurado no systemd. CPU recente é medida aqui, por marcas
//! de `cpu_secs` a cada 15 s, porque a média da vida inteira esconde quem começou a girar
//! agora e exagera quem girou só no começo.
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use egui::{Align, Color32, Layout, RichText};

use crate::app::{fmt_age, MUTED};
use crate::categories::Category;
use crate::config::Locale;
use crate::hwtemp::HwTemp;
use crate::identity::Kind;
use crate::procs::ProcInfo;
use crate::sweep::{self, Lineage, Owner};

/// Média da vida inteira: entra com 1/4 de núcleo depois de meia hora de vida.
const LIFE_AGE: u64 = 30 * 60;
const LIFE_CORES: f64 = 0.25;
/// Média recente: meio núcleo sustentado por 10 min.
const RECENT_SPAN: u64 = 10 * 60;
const RECENT_CORES: f64 = 0.5;
const MARK_EVERY: Duration = Duration::from_secs(15);
const SCAN_EVERY: Duration = Duration::from_secs(5);
/// Morte automática: largado com 2 h de vida e 0,8 núcleo de média, ainda girando.
const AUTO_AGE: u64 = 2 * 3600;
const AUTO_CORES: f64 = 0.8;
const LOG_MAX: usize = 50;
/// Sessão e desktop: nunca morrem sozinhos, mesmo pendurados no systemd.
const PROTECTED: &[&str] = &[
    "hyprland",
    "xwayland",
    "uwsm",
    "waybar",
    "quickshell",
    "qs",
    "walker",
    "elephant",
    "mako",
    "swayosd-server",
    "hypridle",
    "hyprpaper",
    "hyprsunset",
    "pipewire",
    "pipewire-pulse",
    "wireplumber",
    "systemd",
    "dbus-daemon",
    "dbus-broker",
    "ramdog",
    "sddm",
    "fcitx5",
];
const TETO_UNIT: &str = "cpu-teto.service";
const CALMA_UNIT: &str = "ventoinha-calma.service";

pub enum HeatOut {
    Kill(Vec<u32>),
    Toast(String, bool),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    /// Quem abriu já saiu: pode matar sem dó.
    Detached,
    /// Principal de uma unit: parar e mascarar, senão o systemd sobe de novo.
    Service,
    /// Janela, sessão de agente, terminal vivo acima ou sistema.
    InUse,
}

#[derive(Clone, Debug)]
pub struct Row {
    pub pid: u32,
    pub label: String,
    pub cmdline: String,
    pub class: Class,
    pub unit: Option<String>,
    pub opener: Option<String>,
    /// App aberto pela sessão (Hyprland, autostart): pendurado no systemd por natureza.
    pub session: bool,
    pub age: u64,
    /// Núcleos de média na vida inteira.
    pub life: f64,
    /// Núcleos de média nos últimos ~10 min e há quanto tempo isso foi medido.
    pub recent: Option<(f64, u64)>,
    pub window: bool,
    pub kill: Vec<u32>,
    /// Nome da raiz em minúsculo, pra lista de protegidos.
    name: String,
}

impl Row {
    fn hot(&self) -> f64 {
        self.recent.map_or(self.life, |(r, _)| r.max(self.life))
    }
}

pub fn class_of(l: &Lineage, name: &str) -> Class {
    // `Desktop` é o padrão de qualquer executável que não é runtime: não diz nada aqui.
    let desk = matches!(l.kind, Kind::Game | Kind::Emulator);
    match &l.owner {
        Owner::Service { .. } if !l.window => Class::Service,
        Owner::Detached if !l.window && !l.agent_cli && !desk && !PROTECTED.contains(&name) => {
            Class::Detached
        }
        _ => Class::InUse,
    }
}

/// Linhas que esquentam, já classificadas. Pura: `rate` diz núcleos recentes e o tempo
/// medido de cada PID.
pub fn build_rows(
    procs: &[ProcInfo],
    lin: &[Lineage],
    rate: &dyn Fn(u32) -> Option<(f64, u64)>,
    now_ft: i64,
) -> Vec<Row> {
    let by_pid: HashMap<u32, &ProcInfo> = procs.iter().map(|p| (p.pid, p)).collect();
    let mut rows: Vec<Row> = lin
        .iter()
        .filter_map(|l| {
            if l.owner == Owner::Protected {
                return None;
            }
            let rp = by_pid.get(&l.pid)?;
            let age = ((now_ft - l.create_time).max(0) / 10_000_000) as u64;
            let life = l.cpu_secs / age.max(1) as f64;
            let recent = rate(l.pid).map(|(_, span)| {
                let sum: f64 = l
                    .members
                    .iter()
                    .filter_map(|&m| rate(m))
                    .map(|(r, _)| r)
                    .sum();
                (sum, span)
            });
            let lifelong = age >= LIFE_AGE && life >= LIFE_CORES;
            let burning = recent.is_some_and(|(r, s)| s >= RECENT_SPAN && r >= RECENT_CORES);
            if !lifelong && !burning {
                return None;
            }
            let name = sweep_name(rp);
            let class = class_of(l, &name);
            Some(Row {
                pid: l.pid,
                label: l.label.clone(),
                cmdline: rp.cmdline.clone(),
                class,
                unit: match &l.owner {
                    Owner::Service { unit, .. } => Some(unit.clone()),
                    _ => None,
                },
                opener: match &l.owner {
                    Owner::Owned(o) => o.clone(),
                    _ => None,
                },
                session: l.owner == Owner::Session,
                age,
                life,
                recent,
                window: l.window,
                kill: l.kill.clone(),
                name,
            })
        })
        .collect();
    rows.sort_by(|a, b| b.hot().total_cmp(&a.hot()));
    rows
}

/// Nome do executável sem caminho, em minúsculo.
fn sweep_name(p: &ProcInfo) -> String {
    let n = p.exe_path.rsplit('/').next().unwrap_or("");
    if n.is_empty() { &p.name_lower } else { n }.to_lowercase()
}

/// Quem a opção "Matar largados sozinho" leva: largado velho, quente e ainda girando.
pub fn auto_targets<'a>(rows: &'a [Row], me: u32) -> Vec<&'a Row> {
    rows.iter()
        .filter(|r| {
            r.class == Class::Detached
                && r.age >= AUTO_AGE
                && r.life >= AUTO_CORES
                && r.recent
                    .is_some_and(|(c, s)| s >= RECENT_SPAN && c >= RECENT_CORES)
                && !r.window
                && !PROTECTED.contains(&r.name.as_str())
                && !r.kill.contains(&me)
        })
        .collect()
}

struct Marks {
    created: i64,
    marks: VecDeque<(Instant, f64)>,
}

/// Estado dos dois serviços e dos sensores lentos, lido fora da UI.
#[derive(Clone, Default)]
struct Status {
    installed: bool,
    teto: Option<bool>,
    calma: Option<bool>,
    cap_khz: Option<u64>,
    alvo: Option<u32>,
    pwm: Option<u32>,
    fan_rpm: Option<u32>,
    clock_khz: Option<u64>,
}

fn read_num(path: &str) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn read_status() -> Result<Status, String> {
    let mut s = Status {
        installed: std::path::Path::new("/etc/systemd/system")
            .join(TETO_UNIT)
            .exists()
            || std::path::Path::new("/etc/systemd/system")
                .join(CALMA_UNIT)
                .exists(),
        ..Default::default()
    };
    if s.installed {
        // `is-active` sai com erro se algum estiver parado; o stdout vale do mesmo jeito.
        if let Ok(out) = std::process::Command::new("systemctl")
            .args(["is-active", TETO_UNIT, CALMA_UNIT])
            .env("LC_ALL", "C")
            .output()
        {
            let text = String::from_utf8_lossy(&out.stdout);
            let mut lines = text.lines().map(|l| l.trim() == "active");
            s.teto = lines.next();
            s.calma = lines.next();
        }
        s.cap_khz = read_num("/run/cpu-teto");
        s.pwm = read_num("/run/ventoinha-calma").map(|v| v as u32);
        s.alvo = std::fs::read_to_string("/etc/cpu-teto.conf")
            .ok()
            .and_then(|t| {
                t.lines()
                    .find_map(|l| l.trim().strip_prefix("ALVO="))
                    .and_then(|v| v.trim().trim_matches('"').parse().ok())
            });
    }
    s.fan_rpm = cpu_fan_rpm();
    let freqs: Vec<u64> = std::fs::read_dir("/sys/devices/system/cpu")
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            let n = e.file_name();
            let n = n.to_string_lossy();
            n.strip_prefix("cpu")
                .is_some_and(|d| d.chars().all(|c| c.is_ascii_digit()))
        })
        .filter_map(|e| {
            std::fs::read_to_string(e.path().join("cpufreq/scaling_cur_freq"))
                .ok()?
                .trim()
                .parse()
                .ok()
        })
        .collect();
    if !freqs.is_empty() {
        s.clock_khz = Some(freqs.iter().sum::<u64>() / freqs.len() as u64);
    }
    Ok(s)
}

/// Rotação do fan com "CPU" no rótulo do hwmon (nct6687: `fan1_label` = "CPU Fan").
fn cpu_fan_rpm() -> Option<u32> {
    for e in std::fs::read_dir("/sys/class/hwmon").ok()?.flatten() {
        let dir = e.path();
        for i in 1..=16 {
            let Ok(label) = std::fs::read_to_string(dir.join(format!("fan{i}_label"))) else {
                continue;
            };
            if label.to_lowercase().contains("cpu") {
                let rpm = std::fs::read_to_string(dir.join(format!("fan{i}_input"))).ok()?;
                return rpm.trim().parse().ok();
            }
        }
    }
    None
}

/// Unit de usuário (`user@1000.service/...`) ou de sistema, pelo cgroup do PID.
fn user_unit(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/cgroup")).is_ok_and(|c| c.contains("user@"))
}

/// Para e mascara a unit; se o arquivo da unit mora onde a máscara iria, desabilita.
fn stop_unit(unit: &str, user: bool) -> Result<String, String> {
    let run = |verb: &str| {
        if user {
            crate::linux::command("systemctl", &["--user", verb, unit])
        } else {
            crate::linux::command("sudo", &["-n", "systemctl", verb, unit])
        }
    };
    run("stop")?;
    match run("mask") {
        Ok(_) => Ok("mask".into()),
        Err(_) => run("disable").map(|_| "disable".into()),
    }
}

pub struct Heat {
    marks: HashMap<u32, Marks>,
    rows: Vec<Row>,
    scanned: Option<Instant>,
    born: Instant,
    status: crate::linux::Job<Status>,
    max_khz: Option<u64>,
    /// Energia do pacote (RAPL): última leitura e a potência calculada dela.
    rapl: Option<(Instant, u64)>,
    rapl_denied: bool,
    watts: Option<f64>,
    jobs: Vec<Receiver<Result<String, String>>>,
    /// Ação em espera de confirmação: PID da linha, ou 0 para "matar todos os largados".
    confirm: Option<u32>,
    /// Já mortos sozinhos (PID, criação), pra não repetir enquanto o processo morre.
    auto_done: HashSet<(u32, i64)>,
    log: VecDeque<(std::time::SystemTime, String)>,
}

impl Heat {
    pub fn new() -> Self {
        Self {
            marks: HashMap::new(),
            rows: Vec::new(),
            scanned: None,
            born: Instant::now(),
            status: crate::linux::Job::default(),
            max_khz: read_num("/sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq"),
            rapl: None,
            rapl_denied: false,
            watts: None,
            jobs: Vec::new(),
            confirm: None,
            auto_done: HashSet::new(),
            log: VecDeque::new(),
        }
    }

    /// Chamado a cada amostra, com a aba aberta ou não: as marcas de CPU recente e a morte
    /// automática não podem depender de alguém estar olhando.
    pub fn observe(
        &mut self,
        procs: &[ProcInfo],
        locked: &dyn Fn(&ProcInfo) -> bool,
        cat: &dyn Fn(u32) -> Category,
        auto: bool,
        locale: Locale,
    ) -> Vec<HeatOut> {
        let mut out = self.poll_jobs();
        if self.status.poll() {
            if let Some(e) = &self.status.error {
                crate::linux::log(&format!("calor: status: {e}"));
            }
        }
        if self.status.due(5) {
            self.status.start(read_status);
        }
        if self.scanned.is_some_and(|t| t.elapsed() < SCAN_EVERY) {
            return out;
        }
        let now = Instant::now();
        self.scanned = Some(now);
        self.read_rapl(now);

        let live: HashSet<u32> = procs.iter().map(|p| p.pid).collect();
        self.marks.retain(|pid, _| live.contains(pid));
        for p in procs {
            let m = self.marks.entry(p.pid).or_insert(Marks {
                created: p.create_time,
                marks: VecDeque::new(),
            });
            if m.created != p.create_time {
                m.created = p.create_time;
                m.marks.clear();
            }
            if m.marks.back().is_none_or(|(t, _)| now - *t >= MARK_EVERY) {
                m.marks.push_back((now, p.cpu_secs));
            }
            // A primeira marca é a mais nova que já tem 10 min: é dela que a média parte.
            while m.marks.len() >= 2 && now - m.marks[1].0 >= Duration::from_secs(RECENT_SPAN) {
                m.marks.pop_front();
            }
        }
        let cpu: HashMap<u32, f64> = procs.iter().map(|p| (p.pid, p.cpu_secs)).collect();
        let marks = &self.marks;
        let rate = |pid: u32| -> Option<(f64, u64)> {
            let (t, c) = *marks.get(&pid)?.marks.front()?;
            let span = (now - t).as_secs_f64();
            if span < 1.0 {
                return None;
            }
            Some((((cpu.get(&pid)? - c) / span).max(0.0), span as u64))
        };
        let lin = sweep::lineage(procs, locked, cat);
        self.rows = build_rows(procs, &lin, &rate, crate::procs::now_filetime());

        let alive: HashSet<(u32, i64)> = procs.iter().map(|p| (p.pid, p.create_time)).collect();
        self.auto_done.retain(|k| alive.contains(k));
        if auto && self.born.elapsed() >= Duration::from_secs(RECENT_SPAN) {
            let me = std::process::id();
            let created: HashMap<u32, i64> = procs.iter().map(|p| (p.pid, p.create_time)).collect();
            let mut kill = Vec::new();
            let mut lines = Vec::new();
            for r in auto_targets(&self.rows, me) {
                let key = (r.pid, created.get(&r.pid).copied().unwrap_or_default());
                if !self.auto_done.insert(key) {
                    continue;
                }
                kill.extend(r.kill.iter().copied());
                lines.push(match locale {
                    Locale::English => format!(
                        "killed {} (PID {}): {:.1} core avg over {}, no owner",
                        r.label,
                        r.pid,
                        r.life,
                        fmt_age(r.age)
                    ),
                    _ => format!(
                        "matou {} (PID {}): {} núcleo de média em {}, sem dono",
                        r.label,
                        r.pid,
                        cores(r.life, locale),
                        fmt_age(r.age)
                    ),
                });
                crate::linux::log(&format!(
                    "calor: matou largado {} pid={} cores={:.2} age={}s cmd={}",
                    r.label,
                    r.pid,
                    r.life,
                    r.age,
                    r.cmdline.chars().take(200).collect::<String>()
                ));
            }
            for l in lines {
                self.log.push_front((std::time::SystemTime::now(), l));
            }
            self.log.truncate(LOG_MAX);
            if !kill.is_empty() {
                out.push(HeatOut::Kill(kill));
            }
        }
        out
    }

    fn read_rapl(&mut self, now: Instant) {
        if self.rapl_denied {
            return;
        }
        // 0400 root na maioria das distros: sem permissão, a potência some do cabeçalho
        // em vez de chamar sudo a cada amostra.
        let Some(e) = read_num("/sys/class/powercap/intel-rapl:0/energy_uj") else {
            self.rapl_denied = true;
            return;
        };
        if let Some((t, prev)) = self.rapl {
            let dt = (now - t).as_secs_f64();
            if e >= prev && dt > 0.5 {
                self.watts = Some((e - prev) as f64 / 1e6 / dt);
            }
        }
        self.rapl = Some((now, e));
    }

    fn poll_jobs(&mut self) -> Vec<HeatOut> {
        let mut out = Vec::new();
        self.jobs.retain(|rx| match rx.try_recv() {
            Ok(Ok(m)) => {
                out.push(HeatOut::Toast(m, false));
                false
            }
            Ok(Err(e)) => {
                out.push(HeatOut::Toast(e, true));
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(_) => false,
        });
        if !out.is_empty() {
            // Serviço ligado ou parado: relê o estado já, sem esperar os 5 s.
            let value = std::mem::take(&mut self.status.value);
            self.status = crate::linux::Job::default();
            self.status.value = value;
        }
        out
    }

    fn quiet_mode(&mut self, on: bool, locale: Locale) {
        let en = locale == Locale::English;
        let verb = if on { "enable" } else { "disable" };
        self.jobs.push(crate::linux::spawn(move || {
            crate::linux::command(
                "sudo",
                &["-n", "systemctl", verb, "--now", TETO_UNIT, CALMA_UNIT],
            )
            .map_err(|e| {
                if en {
                    format!("Quiet mode: {e}")
                } else {
                    format!("Modo silencioso: {e}")
                }
            })?;
            Ok(match (on, en) {
                (true, true) => "Quiet mode on: CPU cap and quiet fan running".into(),
                (true, false) => {
                    "Modo silencioso ligado: teto da CPU e ventoinha calma rodando".into()
                }
                (false, true) => {
                    "Quiet mode off: CPU back to full boost, fans back to the BIOS".into()
                }
                (false, false) => {
                    "Modo silencioso desligado: CPU no boost cheio, ventoinha de volta pra BIOS"
                        .into()
                }
            })
        }));
    }

    fn stop_service(&mut self, pid: u32, unit: String, locale: Locale) {
        let en = locale == Locale::English;
        self.jobs.push(crate::linux::spawn(move || {
            let user = user_unit(pid);
            let how = stop_unit(&unit, user).map_err(|e| {
                if en {
                    format!("Could not stop {unit}: {e}")
                } else {
                    format!("Não deu pra parar {unit}: {e}")
                }
            })?;
            crate::linux::log(&format!("calor: parou {unit} ({how}, user={user})"));
            Ok(match (how.as_str(), en) {
                ("mask", true) => format!("{unit} stopped and masked"),
                ("mask", false) => format!("{unit} parado e mascarado"),
                (_, true) => format!("{unit} stopped and disabled"),
                (_, false) => format!("{unit} parado e desabilitado"),
            })
        }));
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        locale: Locale,
        hw: &HwTemp,
        auto: &mut bool,
    ) -> Vec<HeatOut> {
        let mut out = self.poll_jobs();
        let en = locale == Locale::English;
        crate::kit::intro(ui, locale.text(
            "Quem está esquentando o PC agora e quem gira há horas sem ninguém usando. Largado é o que ficou pendurado no systemd depois que o terminal ou agente que abriu saiu: pode matar sem dó.",
            "What is heating the PC right now and what has been spinning for hours with nobody using it. Left behind means it was left hanging on systemd after the terminal or agent that started it exited: safe to kill.",
        ));
        ui.add_space(6.0);
        self.header(ui, locale, hw);
        ui.add_space(8.0);

        let detached: Vec<u32> = self
            .rows
            .iter()
            .filter(|r| r.class == Class::Detached)
            .flat_map(|r| r.kill.iter().copied())
            .collect();
        let n_detached = self
            .rows
            .iter()
            .filter(|r| r.class == Class::Detached)
            .count();
        crate::kit::toolbar(ui, |ui| {
            if ui
                .checkbox(auto, locale.text("Matar largados sozinho", "Kill left-behind ones automatically"))
                .on_hover_text(locale.text(
                    "Largado com 2 h ou mais de vida, 0,8 núcleo de média e ainda girando, sem janela e fora da sessão (Hyprland, barra, áudio, o próprio RamDog). Cada morte fica registrada aqui embaixo e no log.",
                    "Left-behind process at least 2 h old, averaging 0.8 core and still spinning, with no window and outside the session (Hyprland, bar, audio, RamDog itself). Every kill is listed below and in the log.",
                ))
                .changed()
            {
                out.push(HeatOut::Toast(
                    if *auto {
                        locale.text("Largados quentes morrem sozinhos a partir de agora", "Hot left-behind processes will be killed automatically")
                    } else {
                        locale.text("Morte automática desligada", "Automatic kill turned off")
                    }
                    .into(),
                    false,
                ));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.confirm == Some(0) && !detached.is_empty() {
                    if ui
                        .add(crate::kit::button(locale.text("Cancelar", "Cancel")))
                        .clicked()
                    {
                        self.confirm = None;
                    }
                    let label = if en {
                        format!("Confirm: kill {n_detached} left behind")
                    } else {
                        format!("Confirmar: matar {n_detached} largado(s)")
                    };
                    if ui.add(crate::kit::danger(&label)).clicked() {
                        out.push(HeatOut::Kill(detached.clone()));
                        self.confirm = None;
                    }
                } else {
                    let label = if en {
                        format!("Kill all left behind ({n_detached})")
                    } else {
                        format!("Matar todos os largados ({n_detached})")
                    };
                    if ui
                        .add_enabled(!detached.is_empty(), crate::kit::primary(&label))
                        .clicked()
                    {
                        self.confirm = Some(0);
                    }
                }
            });
        });
        ui.add_space(6.0);

        if self.born.elapsed() < SCAN_EVERY + Duration::from_secs(1) && self.rows.is_empty() {
            ui.label(crate::kit::muted(
                locale.text("lendo processos…", "reading processes…"),
            ));
            ui.ctx().request_repaint_after(Duration::from_secs(1));
            return out;
        }

        let rows = self.rows.clone();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for class in [Class::Detached, Class::Service, Class::InUse] {
                    let group: Vec<&Row> = rows.iter().filter(|r| r.class == class).collect();
                    let (title, color, hint) = match class {
                        Class::Detached => (
                            locale.text("Largado", "Left behind"),
                            Color32::from_rgb(230, 120, 120),
                            locale.text("Quem abriu já saiu. Matar leva o processo e o que roda embaixo dele.", "Whoever started it is gone. Killing takes the process and everything below it."),
                        ),
                        Class::Service => (
                            locale.text("Serviço", "Service"),
                            Color32::from_rgb(230, 170, 90),
                            locale.text("Processo principal de uma unit do systemd. Matar não adianta (ele volta): parar e mascarar.", "Main process of a systemd unit. Killing does not help (it comes back): stop and mask."),
                        ),
                        Class::InUse => (
                            locale.text("Em uso", "In use"),
                            Color32::from_rgb(120, 200, 140),
                            locale.text("Tem janela, é sessão de agente ou quem abriu ainda está vivo. Matar pede confirmação.", "Has a window, is an agent session, or its opener is still alive. Killing asks for confirmation."),
                        ),
                    };
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{title} · {}", group.len())).strong().color(color));
                        ui.label(crate::kit::muted(hint));
                    });
                    ui.add_space(4.0);
                    if group.is_empty() {
                        ui.label(crate::kit::muted(locale.text("nada aqui agora", "nothing here right now")));
                    }
                    ui.spacing_mut().item_spacing.y = 4.0;
                    for r in group {
                        self.row(ui, r, locale, &mut out);
                    }
                    ui.add_space(10.0);
                }

                if !self.log.is_empty() {
                    ui.label(RichText::new(locale.text("Mortos sozinhos", "Killed automatically")).strong());
                    ui.add_space(4.0);
                    for (when, line) in &self.log {
                        let ago = when.elapsed().map(|d| d.as_secs()).unwrap_or(0);
                        ui.label(crate::kit::muted(&if en {
                            format!("{} ago · {line}", fmt_age(ago))
                        } else {
                            format!("há {} · {line}", fmt_age(ago))
                        }));
                    }
                }
            });
        out
    }

    fn row(&mut self, ui: &mut egui::Ui, r: &Row, locale: Locale, out: &mut Vec<HeatOut>) {
        let en = locale == Locale::English;
        crate::kit::row(ui, |ui| {
            ui.horizontal(|ui| {
                let recent = match r.recent {
                    Some((c, s)) if s >= 60 => {
                        if en {
                            format!("{} core last {}", cores(c, locale), fmt_age(s))
                        } else {
                            format!("{} núcleo nos últimos {}", cores(c, locale), fmt_age(s))
                        }
                    }
                    _ => locale.text("medindo o recente…", "measuring recent…").into(),
                };
                let whose = match (r.class, &r.unit, &r.opener) {
                    (Class::Service, Some(u), _) => u.clone(),
                    (_, _, Some(o)) if en => format!("opened by {o}"),
                    (_, _, Some(o)) => format!("aberto por {o}"),
                    (Class::Detached, _, _) => locale.text("sem dono acima", "no owner above").into(),
                    _ if r.session => locale.text("aberto pela sessão", "opened by the session").into(),
                    _ if r.window => locale.text("com janela", "has a window").into(),
                    _ => String::new(),
                };
                let sub = if en {
                    format!("PID {} · {} old · {whose}", r.pid, fmt_age(r.age))
                } else {
                    format!("PID {} · {} de vida · {whose}", r.pid, fmt_age(r.age))
                };
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.add(egui::Label::new(RichText::new(&r.label).size(13.0)).truncate())
                        .on_hover_text(&r.cmdline);
                    ui.add(egui::Label::new(crate::kit::muted(&sub)).truncate())
                        .on_hover_text(&r.cmdline);
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let confirming = self.confirm == Some(r.pid);
                    match r.class {
                        Class::Detached => {
                            if ui.add(crate::kit::danger(locale.text("Matar", "Kill"))).clicked() {
                                out.push(HeatOut::Kill(r.kill.clone()));
                            }
                        }
                        Class::Service | Class::InUse if confirming => {
                            if ui.add(crate::kit::button(locale.text("Cancelar", "Cancel"))).clicked() {
                                self.confirm = None;
                            }
                            let label = if r.class == Class::Service {
                                locale.text("Confirmar: parar e mascarar", "Confirm: stop and mask")
                            } else {
                                locale.text("Confirmar: matar", "Confirm: kill")
                            };
                            if ui.add(crate::kit::danger(label)).clicked() {
                                self.confirm = None;
                                match &r.unit {
                                    Some(u) if r.class == Class::Service => {
                                        self.stop_service(r.pid, u.clone(), locale)
                                    }
                                    _ => out.push(HeatOut::Kill(r.kill.clone())),
                                }
                            }
                        }
                        Class::Service => {
                            if ui
                                .add(crate::kit::button(locale.text("Parar e mascarar", "Stop and mask")))
                                .on_hover_text(locale.text(
                                    "systemctl stop + mask: não sobe de novo nem no próximo boot. Unit de sistema usa sudo -n.",
                                    "systemctl stop + mask: it will not start again, not even on the next boot. System units use sudo -n.",
                                ))
                                .clicked()
                            {
                                self.confirm = Some(r.pid);
                            }
                        }
                        Class::InUse => {
                            if ui.add(crate::kit::button(locale.text("Matar…", "Kill…"))).clicked() {
                                self.confirm = Some(r.pid);
                            }
                        }
                    }
                    ui.add_space(6.0);
                    let color = heat_color(r.hot());
                    crate::kit::badge(ui, &recent, color);
                    crate::kit::badge(
                        ui,
                        &if en {
                            format!("{} core avg", cores(r.life, locale))
                        } else {
                            format!("{} núcleo de média", cores(r.life, locale))
                        },
                        heat_color(r.life),
                    );
                });
            });
        });
    }

    fn header(&mut self, ui: &mut egui::Ui, locale: Locale, hw: &HwTemp) {
        let en = locale == Locale::English;
        let s = self.status.value.clone();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let temp = hw.cpu_temp.filter(|t| *t > 0.0);
            crate::kit::badge(
                ui,
                &temp.map_or("CPU – °C".into(), |t| format!("CPU {t:.0} °C")),
                temp.map_or(MUTED, |t| {
                    if t >= 85.0 {
                        Color32::from_rgb(230, 100, 100)
                    } else if t >= 75.0 {
                        Color32::from_rgb(230, 170, 90)
                    } else {
                        Color32::from_rgb(120, 200, 140)
                    }
                }),
            )
            .on_hover_text("Tctl (k10temp)");
            crate::kit::badge(
                ui,
                &s.fan_rpm.map_or(
                    locale.text("ventoinha – rpm", "fan – rpm").into(),
                    |r| format!("{} {r} rpm", locale.text("ventoinha", "fan")),
                ),
                MUTED,
            );
            match self.watts {
                Some(w) => {
                    crate::kit::badge(ui, &format!("{w:.0} W"), MUTED)
                        .on_hover_text(locale.text("Pacote da CPU (RAPL)", "CPU package (RAPL)"));
                }
                None if self.rapl_denied => {
                    crate::kit::badge(ui, "– W", MUTED).on_hover_text(locale.text(
                        "A potência do pacote (RAPL) só é legível como root nesta máquina.",
                        "Package power (RAPL) is only readable as root on this machine.",
                    ));
                }
                None => {}
            }
            if let Some(k) = s.clock_khz {
                crate::kit::badge(
                    ui,
                    &if en {
                        format!("avg clock {}", ghz(k, locale))
                    } else {
                        format!("clock médio {}", ghz(k, locale))
                    },
                    MUTED,
                );
            }
        });
        if !s.installed {
            return;
        }
        ui.add_space(6.0);
        let on = s.teto == Some(true) && s.calma == Some(true);
        let any = s.teto == Some(true) || s.calma == Some(true);
        crate::kit::row(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    let teto = match (s.teto, s.cap_khz) {
                        (Some(true), Some(k)) => {
                            let max = self.max_khz.map(|m| ghz(m, locale)).unwrap_or_default();
                            let alvo = s.alvo.map(|a| format!(" · {} {a} °C", locale.text("alvo", "target"))).unwrap_or_default();
                            if en {
                                format!("CPU cap: on · {} of {max}{alvo}", ghz(k, locale))
                            } else {
                                format!("Teto da CPU: ligado · {} de {max}{alvo}", ghz(k, locale))
                            }
                        }
                        (Some(true), None) => locale.text("Teto da CPU: ligado", "CPU cap: on").into(),
                        _ => locale.text("Teto da CPU: desligado (boost cheio)", "CPU cap: off (full boost)").into(),
                    };
                    let calma = match (s.calma, s.pwm) {
                        (Some(true), Some(p)) => {
                            let pct = p as f64 * 100.0 / 255.0;
                            if en {
                                format!("Quiet fan: on · PWM {pct:.0}%")
                            } else {
                                format!("Ventoinha calma: ligada · PWM {pct:.0}%")
                            }
                        }
                        (Some(true), None) => locale.text("Ventoinha calma: ligada", "Quiet fan: on").into(),
                        _ => locale.text("Ventoinha calma: desligada (curva da BIOS)", "Quiet fan: off (BIOS curve)").into(),
                    };
                    ui.label(RichText::new(teto).size(12.5));
                    ui.label(RichText::new(calma).size(12.5));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if self.jobs.is_empty() {
                        let (label, btn) = if on {
                            (locale.text("Modo silencioso: ligado", "Quiet mode: on"), crate::kit::primary as fn(&str) -> egui::Button<'static>)
                        } else {
                            (locale.text("Modo silencioso: desligado", "Quiet mode: off"), crate::kit::button as fn(&str) -> egui::Button<'static>)
                        };
                        if ui
                            .add(btn(label))
                            .on_hover_text(locale.text(
                                "Liga ou desliga cpu-teto e ventoinha-calma juntos (sudo -n systemctl enable/disable --now).",
                                "Turns cpu-teto and ventoinha-calma on or off together (sudo -n systemctl enable/disable --now).",
                            ))
                            .clicked()
                        {
                            self.quiet_mode(!any, locale);
                        }
                    } else {
                        ui.spinner();
                        ui.ctx().request_repaint_after(Duration::from_millis(200));
                    }
                });
            });
            // A ventoinha calma escreve o mesmo pwm que o ESTABILIZAR do Térmico.
            if s.calma == Some(true) && hw.stab.on {
                ui.colored_label(
                    Color32::from_rgb(230, 170, 90),
                    locale.text(
                        "O ESTABILIZAR do Térmico também está mexendo nas ventoinhas: os dois brigam pelo mesmo PWM. Desligue um deles.",
                        "Thermal's STABILIZE is also driving the fans: both fight over the same PWM. Turn one of them off.",
                    ),
                );
            }
        });
        ui.ctx().request_repaint_after(Duration::from_secs(2));
    }
}

fn cores(c: f64, locale: Locale) -> String {
    let s = format!("{c:.1}");
    if locale == Locale::English {
        s
    } else {
        s.replace('.', ",")
    }
}

fn ghz(khz: u64, locale: Locale) -> String {
    cores(khz as f64 / 1e6, locale) + " GHz"
}

fn heat_color(c: f64) -> Color32 {
    if c >= 0.8 {
        Color32::from_rgb(230, 100, 100)
    } else if c >= 0.4 {
        Color32::from_rgb(230, 170, 90)
    } else {
        MUTED
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::procs::Launcher;

    fn proc(pid: u32, ppid: u32, name: &str) -> ProcInfo {
        ProcInfo {
            pid,
            ppid,
            raw_ppid: ppid,
            name: name.into(),
            name_lower: name.to_lowercase(),
            exe_path: format!("/usr/bin/{name}"),
            cmdline: name.into(),
            create_time: pid as i64,
            ..Default::default()
        }
    }

    /// 10 h de vida, 9,5 h de CPU.
    fn burning(pid: u32, ppid: u32, name: &str) -> ProcInfo {
        let mut p = proc(pid, ppid, name);
        p.create_time = -10 * 3600 * 10_000_000;
        p.cpu_secs = 9.5 * 3600.0;
        p
    }

    fn rows(procs: &[ProcInfo], rate: &dyn Fn(u32) -> Option<(f64, u64)>) -> Vec<Row> {
        let lin = sweep::lineage(procs, &|p| p.pid == 1, &|_| Category::Other);
        build_rows(procs, &lin, rate, 0)
    }

    #[test]
    fn script_behind_two_shells_is_left_behind_and_auto_killable() {
        let mut py = burning(53, 52, "python3");
        py.cmdline =
            "python3 -c import glob; glob.glob('**/bench_fila*.py', recursive=True)".into();
        let procs = [
            proc(1, 0, "systemd"),
            proc(5, 1, "systemd"),
            proc(50, 5, "bash"),
            proc(52, 50, "bash"),
            py,
        ];
        let rows = rows(&procs, &|pid| {
            Some((if pid == 53 { 1.0 } else { 0.0 }, 900))
        });
        let r = rows.iter().find(|r| r.pid == 53).unwrap();
        assert_eq!(r.class, Class::Detached);
        assert_eq!(auto_targets(&rows, 9999).len(), 1);
        assert!(
            auto_targets(&rows, 53).is_empty(),
            "nunca mata o próprio RamDog"
        );
    }

    #[test]
    fn service_main_is_service_and_script_left_in_its_cgroup_is_left_behind() {
        let mut main = burning(40, 5, "localsearch-3");
        main.create_time -= 10_000_000;
        main.launcher.unit = Some("localsearch-3.service".into());
        let mut py = burning(41, 5, "python3");
        py.launcher.unit = Some("localsearch-3.service".into());
        let procs = [proc(1, 0, "systemd"), proc(5, 1, "systemd"), main, py];
        let rows = rows(&procs, &|_| None);
        let main = rows.iter().find(|r| r.pid == 40).unwrap();
        assert_eq!(main.class, Class::Service);
        assert_eq!(main.unit.as_deref(), Some("localsearch-3.service"));
        assert_eq!(
            rows.iter().find(|r| r.pid == 41).unwrap().class,
            Class::Detached
        );
        assert!(
            auto_targets(&rows, 9999).is_empty(),
            "sem CPU recente medida, nada morre sozinho"
        );
    }

    #[test]
    fn under_a_live_terminal_or_with_a_window_is_in_use() {
        let mut term = proc(10, 1, "foot");
        term.has_window = true;
        let mut game = burning(20, 5, "wow");
        game.has_window = true;
        let mut agent = burning(30, 5, "claude");
        agent.launcher = Launcher {
            agent: Some("Claude Code".into()),
            ..Default::default()
        };
        let procs = [
            proc(1, 0, "systemd"),
            proc(5, 1, "systemd"),
            term,
            proc(11, 10, "bash"),
            burning(12, 11, "cargo"),
            game,
            agent,
            burning(60, 5, "hyprland"),
        ];
        let rows = rows(&procs, &|_| Some((1.0, 900)));
        for pid in [12, 20, 30, 60] {
            let r = rows.iter().find(|r| r.pid == pid).unwrap();
            assert_eq!(r.class, Class::InUse, "{} deveria estar em uso", r.label);
        }
        assert!(auto_targets(&rows, 9999).is_empty());
    }

    /// Lista o que o Calor mostraria agora nesta máquina (só a média da vida inteira).
    #[test]
    #[ignore]
    fn dump_calor_real() {
        let procs = crate::procs::Sampler::new().sample();
        let cats = crate::categories::classify(&procs, &HashMap::new());
        let me = std::process::id();
        let lin = sweep::lineage(
            &procs,
            &|p| crate::categories::is_critical(&p.name_lower, p.pid) || p.pid == me,
            &|pid| cats.get(&pid).copied().unwrap_or(Category::Other),
        );
        let now_ft = crate::procs::now_filetime();
        // Abaixo do corte, só pra conferir a classe de cada dono.
        for l in &lin {
            let age = ((now_ft - l.create_time).max(0) / 10_000_000) as u64;
            let life = l.cpu_secs / age.max(1) as f64;
            if life >= 0.02 {
                let name = procs
                    .iter()
                    .find(|p| p.pid == l.pid)
                    .map(sweep_name)
                    .unwrap_or_default();
                let rp = procs.iter().find(|p| p.pid == l.pid).unwrap();
                println!(
                    "  {:?} {:.3} {} {:?} unit={:?} ppid={} cmd={}",
                    class_of(l, &name),
                    life,
                    l.label,
                    l.owner,
                    rp.launcher.unit,
                    rp.ppid,
                    rp.cmdline.chars().take(70).collect::<String>()
                );
            }
        }
        for r in build_rows(&procs, &lin, &|_| None, now_ft) {
            println!(
                "{:?} {:.2} núcleo {} {} pid={} unit={:?} opener={:?} cmd={}",
                r.class,
                r.life,
                fmt_age(r.age),
                r.label,
                r.pid,
                r.unit,
                r.opener,
                r.cmdline.chars().take(90).collect::<String>()
            );
        }
    }

    #[test]
    fn quiet_process_does_not_show_up() {
        let mut calm = proc(70, 5, "python3");
        calm.create_time = -10 * 3600 * 10_000_000;
        calm.cpu_secs = 120.0;
        let procs = [proc(1, 0, "systemd"), proc(5, 1, "systemd"), calm];
        assert!(rows(&procs, &|_| Some((0.0, 900))).is_empty());
        // Recém-nascido girando forte entra pela média recente.
        let mut hot = proc(80, 5, "node");
        hot.create_time = -15 * 60 * 10_000_000;
        hot.cpu_secs = 60.0;
        let procs = [proc(1, 0, "systemd"), proc(5, 1, "systemd"), hot];
        let rows = rows(&procs, &|_| Some((0.9, 700)));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].class, Class::Detached);
    }
}
