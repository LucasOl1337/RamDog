//! Sinais que a lista por RAM esconde: load, swap e processo que come núcleo sem inchá-la.
//!
//! O caso que motivou o módulo: Overwatch no topo com 20 GB, GPU ociosa, e o FPS caindo
//! por um qemu sem janela e três `gh auth git-credential` em loop — todos com RAM perto
//! de zero. A coluna CPU em % da máquina (16 núcleos) transformava 1,5 núcleo em "9%".

use crate::config::Locale;

/// Núcleos equivalentes. `100` na coluna CPU é a máquina inteira.
pub fn cores(cpu_machine_pct: f32, ncpu: u32) -> f32 {
    if ncpu == 0 {
        return 0.0;
    }
    cpu_machine_pct / 100.0 * ncpu as f32
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StealKind {
    SoftwareGpu,
    Leftover,
    CredentialSpin,
    CheapCpu,
}

impl StealKind {
    pub fn rank(self) -> u8 {
        match self {
            StealKind::SoftwareGpu => 4,
            StealKind::Leftover => 3,
            StealKind::CredentialSpin => 2,
            StealKind::CheapCpu => 1,
        }
    }

    pub fn chip(self) -> &'static str {
        match self {
            StealKind::SoftwareGpu => "GPU no CPU",
            StealKind::Leftover => "sobra",
            StealKind::CredentialSpin => "loop",
            StealKind::CheapCpu => "núcleo",
        }
    }

    pub fn chip_for(self, locale: Locale) -> &'static str {
        if locale == Locale::Portuguese {
            return self.chip();
        }
        match self {
            StealKind::SoftwareGpu => "GPU on CPU",
            StealKind::Leftover => "leftover",
            StealKind::CredentialSpin => "loop",
            StealKind::CheapCpu => "CPU-only",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            StealKind::SoftwareGpu => "gráficos por software, a GPU real fica parada",
            StealKind::Leftover => "sobra",
            StealKind::CredentialSpin => "loop de credencial",
            StealKind::CheapCpu => "CPU sem RAM",
        }
    }

    pub fn short_for(self, locale: Locale) -> &'static str {
        if locale == Locale::Portuguese {
            return self.short();
        }
        match self {
            StealKind::SoftwareGpu => "software graphics while the real GPU sits idle",
            StealKind::Leftover => "leftover process",
            StealKind::CredentialSpin => "credential loop",
            StealKind::CheapCpu => "CPU with little RAM",
        }
    }
}

/// Helper de credencial do Git/gh que, quando trava, gira um núcleo inteiro sem alocar RAM.
pub fn is_credential_helper(cmdline: &str) -> bool {
    let c = cmdline.to_ascii_lowercase();
    c.contains("git-credential") || c.contains("gh auth git-credential")
}

/// Chrome/Chromium renderizando com SwiftShader: WebGL emulado no processador.
///
/// O caso real: Codex abriu um Chrome headless para Three.js e o Puppeteer caiu em
/// `--use-angle=swiftshader-webgl` — 12 núcleos do 9800X3D desenhando triângulos
/// enquanto a 4070 Ti marcava 11%. Na lista parecia "chrome comendo CPU do nada".
pub fn is_software_gpu(cmdline: &str) -> bool {
    cmdline.to_ascii_lowercase().contains("swiftshader")
}

/// Por que esta linha disputa CPU de um jogo mesmo com pouca RAM.
///
/// `leftover` vem de [`crate::identity::leftover_reason`] — evidência, não “CPU baixa”.
pub fn steal_kind(
    leftover: bool,
    cmdline: &str,
    cpu_machine_pct: f32,
    ram_bytes: u64,
    ncpu: u32,
    age_secs: u64,
) -> Option<StealKind> {
    if leftover {
        return Some(StealKind::Leftover);
    }
    let cores = cores(cpu_machine_pct, ncpu);
    if is_software_gpu(cmdline) && cores >= 0.6 {
        return Some(StealKind::SoftwareGpu);
    }
    if is_credential_helper(cmdline) && (cores >= 0.25 || (age_secs >= 600 && cores >= 0.15)) {
        return Some(StealKind::CredentialSpin);
    }
    const MB: u64 = 1024 * 1024;
    if cores >= 0.6 && ram_bytes < 256 * MB {
        return Some(StealKind::CheapCpu);
    }
    None
}

/// Sobra ociosa sem jogo aberto não entra no aviso — pode ser emulador de propósito.
pub fn notable(kind: StealKind, cores: f32, game_open: bool) -> bool {
    match kind {
        StealKind::SoftwareGpu => cores >= 0.6,
        StealKind::Leftover => game_open || cores >= 0.2,
        StealKind::CredentialSpin => true,
        StealKind::CheapCpu => cores >= 0.6,
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Snapshot {
    pub load1: Option<f32>,
    pub ncpu: u32,
    pub swap_used: u64,
    pub swap_total: u64,
    pub gpu_pct: Option<f32>,
    pub game_open: bool,
}

impl Snapshot {
    pub fn load_hot(&self) -> bool {
        match self.load1 {
            Some(l) if self.ncpu > 0 => l >= self.ncpu as f32,
            _ => false,
        }
    }

    /// 1 GiB de swap já faz jogo paginar. Zero total = host sem swap medido.
    pub fn swap_hot(&self) -> bool {
        self.swap_total > 0 && self.swap_used >= 1024 * 1024 * 1024
    }

    /// Jogo aberto, GPU folgada e máquina disputada: o FPS não é problema de gráfico.
    pub fn game_starved(&self) -> bool {
        self.game_open
            && self.gpu_pct.map(|g| g < 55.0).unwrap_or(false)
            && (self.load_hot() || self.swap_hot())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Thief {
    pub pid: u32,
    pub label: String,
    pub kind: StealKind,
    pub cores: f32,
}

pub fn parse_loadavg(text: &str) -> Option<(f32, f32, f32)> {
    let mut it = text.split_whitespace();
    let one = it.next()?.parse().ok()?;
    let five = it.next()?.parse().ok()?;
    let fifteen = it.next()?.parse().ok()?;
    Some((one, five, fifteen))
}

fn fmt_gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
}

/// Texto do aviso acima da tabela. `None` = máquina calma, não desenha faixa.
pub fn banner(p: &Snapshot, thieves: &[Thief]) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if p.game_starved() {
        parts.push("Jogo aberto e a GPU ociosa: o gargalo é CPU ou swap, não o gráfico.".into());
    } else if p.load_hot() {
        if let Some(l) = p.load1 {
            parts.push(format!(
                "Load {l:.1} em {} núcleos — a fila de CPU está cheia.",
                p.ncpu
            ));
        }
    }
    if p.swap_hot() {
        parts.push(format!("Swap em uso: {}.", fmt_gb(p.swap_used)));
    }
    if !thieves.is_empty() {
        let list = thieves
            .iter()
            .map(|t| format!("{} ({})", t.label, t.kind.short()))
            .collect::<Vec<_>>()
            .join(" · ");
        parts.push(format!("Quem disputa: {list}."));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

pub fn banner_for(p: &Snapshot, thieves: &[Thief], locale: Locale) -> Option<String> {
    if locale == Locale::Portuguese {
        return banner(p, thieves);
    }
    let mut parts: Vec<String> = Vec::new();
    if p.game_starved() {
        parts.push(
            "A game is open but the GPU is idle: the bottleneck is CPU or swap, not graphics."
                .into(),
        );
    } else if p.load_hot() {
        if let Some(l) = p.load1 {
            parts.push(format!(
                "Load {l:.1} across {} cores — the CPU queue is full.",
                p.ncpu
            ));
        }
    }
    if p.swap_hot() {
        parts.push(format!("Swap in use: {}.", fmt_gb(p.swap_used)));
    }
    if !thieves.is_empty() {
        let list = thieves
            .iter()
            .map(|t| format!("{} ({})", t.label, t.kind.short_for(locale)))
            .collect::<Vec<_>>()
            .join(" · ");
        parts.push(format!("Contention: {list}."));
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cores_are_machine_pct_times_ncpu() {
        assert!((cores(100.0, 16) - 16.0).abs() < 0.01);
        assert!((cores(9.8, 16) - 1.568).abs() < 0.01);
        assert_eq!(cores(50.0, 0), 0.0);
    }

    #[test]
    fn qemu_leftover_is_always_a_thief_kind() {
        assert_eq!(
            steal_kind(
                true,
                "qemu-system-x86_64 -avd sfr -qt-hide-window",
                0.0,
                8 * 1024 * 1024,
                16,
                10
            ),
            Some(StealKind::Leftover)
        );
    }

    #[test]
    fn leftover_idle_without_game_is_not_notable() {
        assert!(!notable(StealKind::Leftover, 0.0, false));
        assert!(notable(StealKind::Leftover, 0.0, true));
        assert!(notable(StealKind::Leftover, 1.5, false));
    }

    #[test]
    fn gh_credential_loop_is_steal_even_with_tiny_ram() {
        let cmd = "/bin/bash /home/lol/.local/bin/gh auth git-credential get";
        assert!(is_credential_helper(cmd));
        // 36% de um núcleo em 16 = 2,25% da máquina.
        let kind = steal_kind(false, cmd, 2.25, 4 * 1024 * 1024, 16, 6 * 3600);
        assert_eq!(kind, Some(StealKind::CredentialSpin));
        assert!(notable(kind.unwrap(), cores(2.25, 16), true));
    }

    #[test]
    fn swiftshader_chrome_is_software_gpu_not_cheap_cpu() {
        // O caso do Codex/Three.js: chrome headless a ~12 núcleos, 204 MB de RAM.
        let cmd = "/opt/google/chrome/chrome --type=gpu-process --headless=new \
                   --use-angle=swiftshader-webgl --user-data-dir=/tmp/puppeteer_x";
        assert!(is_software_gpu(cmd));
        let kind = steal_kind(false, cmd, 77.0, 204 * 1024 * 1024, 16, 4200);
        assert_eq!(kind, Some(StealKind::SoftwareGpu));
        assert!(notable(kind.unwrap(), cores(77.0, 16), false));
        // Ocioso (0.1 núcleo) não vira aviso: SwiftShader parado é inofensivo.
        assert_eq!(
            steal_kind(false, cmd, 0.6, 204 * 1024 * 1024, 16, 4200),
            None
        );
        // Chrome comum com GPU real não é flagrado.
        assert!(!is_software_gpu(
            "/usr/lib/chromium/chromium --type=gpu-process"
        ));
    }

    #[test]
    fn ordinary_bash_is_not_a_thief() {
        assert_eq!(
            steal_kind(false, "/usr/bin/bash", 2.25, 4 * 1024 * 1024, 16, 60),
            None
        );
    }

    #[test]
    fn cheap_cpu_needs_both_cores_and_little_ram() {
        let qemu = "qemu-system-x86_64 -avd sfr-portfolio";
        assert_eq!(
            steal_kind(false, qemu, 9.8, 12 * 1024 * 1024, 16, 120),
            Some(StealKind::CheapCpu)
        );
        assert_eq!(
            steal_kind(false, qemu, 9.8, 2 * 1024 * 1024 * 1024, 16, 120),
            None
        );
        assert_eq!(
            steal_kind(false, qemu, 1.0, 12 * 1024 * 1024, 16, 120),
            None
        );
    }

    #[test]
    fn loadavg_parser() {
        assert_eq!(
            parse_loadavg("35.30 24.37 14.59 12/1840 3182473"),
            Some((35.30, 24.37, 14.59))
        );
        assert_eq!(parse_loadavg("broken"), None);
        assert_eq!(parse_loadavg(""), None);
    }

    #[test]
    fn load_hot_when_runqueue_exceeds_cores() {
        let mut p = Snapshot {
            load1: Some(35.3),
            ncpu: 16,
            ..Snapshot::default()
        };
        assert!(p.load_hot());
        p.load1 = Some(8.0);
        assert!(!p.load_hot());
        p.ncpu = 0;
        p.load1 = Some(99.0);
        assert!(!p.load_hot());
    }

    #[test]
    fn game_starved_needs_game_idle_gpu_and_pressure() {
        let p = Snapshot {
            load1: Some(35.0),
            ncpu: 16,
            gpu_pct: Some(35.0),
            game_open: true,
            swap_used: 22 * 1024 * 1024 * 1024,
            swap_total: 120 * 1024 * 1024 * 1024,
            ..Snapshot::default()
        };
        assert!(p.swap_hot());
        assert!(p.game_starved());
        let quiet = Snapshot {
            load1: Some(2.0),
            ncpu: 16,
            gpu_pct: Some(90.0),
            game_open: true,
            ..Snapshot::default()
        };
        assert!(!quiet.game_starved());
    }

    #[test]
    fn banner_names_the_hidden_thieves() {
        let p = Snapshot {
            load1: Some(35.3),
            ncpu: 16,
            gpu_pct: Some(35.0),
            game_open: true,
            swap_used: 22 * 1024 * 1024 * 1024,
            swap_total: 120 * 1024 * 1024 * 1024,
            ..Snapshot::default()
        };
        let thieves = vec![
            Thief {
                pid: 89102,
                label: "Emulador Android (sfr-portfolio)".into(),
                kind: StealKind::Leftover,
                cores: 1.6,
            },
            Thief {
                pid: 1,
                label: "gh".into(),
                kind: StealKind::CredentialSpin,
                cores: 0.36,
            },
        ];
        let text = banner(&p, &thieves).expect("banner");
        assert!(text.contains("GPU ociosa"));
        assert!(text.contains("Swap em uso"));
        assert!(text.contains("Emulador Android (sfr-portfolio)"));
        assert!(text.contains("loop de credencial"));
        assert!(banner(&Snapshot::default(), &[]).is_none());
    }

    #[test]
    fn english_contention_banner_uses_english_labels() {
        let thieves = vec![Thief {
            pid: 1,
            label: "RamDog".into(),
            kind: StealKind::CheapCpu,
            cores: 1.0,
        }];
        let text = banner_for(&Snapshot::default(), &thieves, Locale::English).expect("banner");
        assert_eq!(text, "Contention: RamDog (CPU with little RAM).");
        assert_eq!(StealKind::CheapCpu.chip_for(Locale::English), "CPU-only");
    }
}
