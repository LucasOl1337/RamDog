//! Persistência em %APPDATA%\RamDog\config.json

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::categories::Category;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum Locale {
    Portuguese,
    English,
}

impl Default for Locale {
    fn default() -> Self {
        Self::Portuguese
    }
}

impl Locale {
    pub fn text(self, portuguese: &'static str, english: &'static str) -> &'static str {
        match self {
            Self::Portuguese => portuguese,
            Self::English => english,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Portuguese => "Português",
            Self::English => "English",
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Nomes de executáveis (minúsculo, com .exe) protegidos contra encerramento.
    pub locked: BTreeSet<String>,
    /// Override manual de categoria por nome de executável (minúsculo, com .exe).
    pub overrides: BTreeMap<String, Category>,
    pub refresh_ms: u64,
    #[serde(default)]
    pub locale: Locale,
    /// Ocultar processos com menos que X MB (na métrica escolhida em `mem_metric`).
    pub min_mb: u32,
    /// Ocultar processos com CPU abaixo deste percentual da máquina. 0 = não filtra.
    #[serde(default)]
    pub min_cpu: f32,
    /// Ocultar processos com carga de GPU abaixo deste percentual. 0 = não filtra.
    #[serde(default)]
    pub min_gpu: f32,
    /// Ocultar processos com VRAM abaixo deste valor. 0 = não filtra.
    #[serde(default)]
    pub min_vram_mb: u32,
    pub view: ViewMode,
    pub show_system: bool,
    /// Qual número a coluna RAM mostra. Ver `MemMetric`.
    pub mem_metric: MemMetric,
    /// Mostrar as linhas sintéticas de kernel/compartilhado no topo da lista.
    pub show_kernel_rows: bool,
    /// Na visão Lista, juntar os processos do mesmo executável numa linha só, que abre.
    /// Sem isto o Chrome com 30 renderizadores fica 30 linhas de 3% e some do topo,
    /// mesmo sendo o maior consumidor da máquina.
    pub group_apps: bool,
    /// Modo mini: HUD compacto, sem decoração de janela, só os medidores do topo.
    /// Persistido para o app reabrir no modo em que foi fechado.
    pub mini: bool,
    /// No modo mini, manter a janela por cima das outras.
    pub mini_on_top: bool,
    /// Presets da Partida: nome do preset → (id da entrada → deve estar ativa).
    /// Só entradas que dão para alternar entram; o resto não teria como ser restaurado.
    pub boot_presets: BTreeMap<String, BTreeMap<String, bool>>,
    /// Como a lista da Partida se divide em grupos.
    pub boot_group: BootGroup,
    /// Cenários da aba Telas: nome → janelas com monitor e retângulo alvo.
    pub screen_presets: BTreeMap<String, ScreenPreset>,
    /// Última grade escolhida na aba Telas (id em `screens::GRIDS`).
    pub screen_grid: String,
    /// Arrastar uma janela no mapa encaixa na zona da grade em vez de mover livre.
    pub screen_snap: bool,
    /// Última coluna da lista mostra os argumentos do comando em vez de quem abriu o
    /// processo. Padrão é quem abriu: `foot › bash › claude` responde "de onde saiu isso"
    /// melhor que `--type=utility --utility-sub-type=...`.
    #[serde(default)]
    pub cmd_column: bool,
}

/// Em que fatias a lista da Partida se quebra.
///
/// A pergunta que o addon responde é "isto sobe com o PC?", então o corte de fora é sempre
/// esse — o de dentro escolhe entre *quando* dispara (fase de arranque) e *de onde* vem
/// (registro, pasta Iniciar, tarefa, serviço…).
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum BootGroup {
    StatusPhase,
    StatusKind,
    Phase,
    Kind,
    Flat,
}

impl BootGroup {
    pub const ALL: [BootGroup; 5] = [
        BootGroup::StatusPhase,
        BootGroup::StatusKind,
        BootGroup::Phase,
        BootGroup::Kind,
        BootGroup::Flat,
    ];

    pub fn label(self) -> &'static str {
        self.label_for(Locale::Portuguese)
    }

    pub fn label_for(self, locale: Locale) -> &'static str {
        match self {
            BootGroup::StatusPhase => {
                locale.text("Sobe / não sobe → fase", "Starts / does not start → phase")
            }
            BootGroup::StatusKind => {
                locale.text("Sobe / não sobe → tipo", "Starts / does not start → source")
            }
            BootGroup::Phase => locale.text("Fase do arranque", "Startup phase"),
            BootGroup::Kind => locale.text("Tipo de origem", "Source type"),
            BootGroup::Flat => locale.text("Lista plana", "Flat list"),
        }
    }

    pub fn tip(self) -> &'static str {
        self.tip_for(Locale::Portuguese)
    }

    pub fn tip_for(self, locale: Locale) -> &'static str {
        match self {
            BootGroup::StatusPhase => locale.text(
                "Primeiro separa o que sobe com o PC do que não sobe; dentro de cada bloco, por momento do arranque (kernel → serviços → logon → seus programas)",
                "First split what starts with the PC from what does not; inside each block, group by startup phase (kernel → services → logon → your programs)",
            ),
            BootGroup::StatusKind => locale.text(
                "Sobe / não sobe e, dentro, por origem: registro, pasta Iniciar, tarefa, serviço…",
                "Starts / does not start and, inside, by source: registry, Startup folder, task, service…",
            ),
            BootGroup::Phase => locale.text("Só por momento do arranque, misturando ativas e desativadas", "By startup phase, mixing enabled and disabled entries"),
            BootGroup::Kind => locale.text("Só por origem, misturando ativas e desativadas", "By source, mixing enabled and disabled entries"),
            BootGroup::Flat => locale.text("Tudo numa lista só, ordenada pela coluna escolhida", "Everything in one list, sorted by the selected column"),
        }
    }
}

/// Um cenário de trabalho: as janelas que ele quer na tela e onde cada uma fica.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ScreenPreset {
    pub slots: Vec<ScreenSlot>,
}

/// Uma janela dentro de um cenário.
///
/// A posição é guardada em fração da área útil do monitor (0..1), não em pixel: assim o
/// cenário sobrevive a trocar de resolução, a plugar o notebook numa TV e a mudar a escala
/// do Windows. O monitor é índice na ordem estável por posição, da esquerda para a direita.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ScreenSlot {
    /// Caminho completo do executável. É o que abre quando a janela não existe.
    pub exe: String,
    /// Argumentos da abertura (uma linha, com aspas se precisar).
    pub args: String,
    #[cfg(target_os = "linux")]
    pub argv: Vec<String>,
    #[cfg(target_os = "linux")]
    pub monitor_name: String,
    /// Rótulo na lista. Vazio = nome do arquivo do exe.
    pub label: String,
    pub monitor: usize,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Trecho do título, para escolher entre várias janelas do mesmo programa.
    /// Vazio = a primeira janela livre daquele exe serve.
    pub title_match: String,
    /// Abrir o programa se não houver janela. Desligado = o cenário só reposiciona
    /// o que já estiver aberto.
    pub launch: bool,
}

/// Qual das três medidas de memória a coluna RAM exibe.
///
/// O padrão histórico do RamDog (e do Gerenciador de Tarefas) era `Private`, que exclui
/// tudo que é compartilhado entre processos — DLLs, seções compartilhadas, arquivos
/// mapeados. Numa máquina de 61 GB medida em 2026-08-21 isso somava 9,7 GB contra
/// 21,0 GB de working set: mais da metade da RAM dos processos ficava invisível.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum MemMetric {
    /// Working set: RAM física que o processo ocupa agora, incluindo páginas compartilhadas.
    /// Responde "quem devo matar". Superconta o compartilhado — a soma estoura o total.
    WorkingSet,
    #[cfg(target_os = "linux")]
    Proportional,
    /// Working set privado: só o que é exclusivo do processo. Soma abaixo do real, mas é a
    /// única base que fecha a conta contra o "em uso" sem dupla contagem.
    Private,
    /// Commit (pagefile usage): o que o processo reservou, esteja na RAM ou no disco.
    Commit,
}

impl MemMetric {
    #[cfg(target_os = "linux")]
    pub const ALL: [MemMetric; 4] = [
        MemMetric::WorkingSet,
        MemMetric::Proportional,
        MemMetric::Private,
        MemMetric::Commit,
    ];
    #[cfg(not(target_os = "linux"))]
    pub const ALL: [MemMetric; 3] = [MemMetric::WorkingSet, MemMetric::Private, MemMetric::Commit];

    pub fn label(self) -> &'static str {
        self.label_for(Locale::Portuguese)
    }

    pub fn label_for(self, locale: Locale) -> &'static str {
        match self {
            #[cfg(target_os = "linux")]
            MemMetric::Proportional => locale.text("Proporcional (PSS)", "Proportional (PSS)"),
            MemMetric::WorkingSet => {
                if cfg!(target_os = "linux") {
                    locale.text("Residente (RSS)", "Resident (RSS)")
                } else {
                    "Working set"
                }
            }
            MemMetric::Private => locale.text("Privado", "Private"),
            MemMetric::Commit => {
                if cfg!(windows) {
                    "Commit"
                } else {
                    "Virtual"
                }
            }
        }
    }

    /// Rótulo curto para o cabeçalho da coluna.
    pub fn short(self) -> &'static str {
        self.short_for(Locale::Portuguese)
    }

    pub fn short_for(self, locale: Locale) -> &'static str {
        match self {
            #[cfg(target_os = "linux")]
            MemMetric::Proportional => locale.text("RAM PSS", "RAM PSS"),
            MemMetric::WorkingSet => locale.text("RAM", "RAM"),
            MemMetric::Private => locale.text("RAM priv.", "Private RAM"),
            MemMetric::Commit => {
                if cfg!(windows) {
                    locale.text("Commit", "Commit")
                } else {
                    locale.text("Virtual", "Virtual")
                }
            }
        }
    }

    pub fn tip(self) -> &'static str {
        self.tip_for(Locale::Portuguese)
    }

    pub fn tip_for(self, locale: Locale) -> &'static str {
        #[cfg(target_os = "linux")]
        {
            return match (self, locale) {
            (MemMetric::WorkingSet, Locale::Portuguese) => "RAM residente (RSS), incluindo páginas compartilhadas. A soma pode contar a mesma página em vários processos.",
            (MemMetric::WorkingSet, Locale::English) => "Resident RAM (RSS), including shared pages. The sum may count the same page in several processes.",
            (MemMetric::Proportional, Locale::Portuguese) => "RAM proporcional (PSS): divide cada página compartilhada entre os processos que a usam. Totais incluem apenas leituras acessíveis; — indica indisponível.",
            (MemMetric::Proportional, Locale::English) => "Proportional RAM (PSS): divides each shared page among the processes using it. Totals include readable values only; — means unavailable.",
            (MemMetric::Private, Locale::Portuguese) => "RAM exclusiva (USS): Private_Clean + Private_Dirty de smaps_rollup. Totais incluem apenas leituras acessíveis; — indica indisponível.",
            (MemMetric::Private, Locale::English) => "Private RAM (USS): Private_Clean + Private_Dirty from smaps_rollup. Totals include readable values only; — means unavailable.",
            (MemMetric::Commit, Locale::Portuguese) => "Espaço de endereçamento virtual reservado. Não representa RAM consumida nem memória confirmada (commit).",
            (MemMetric::Commit, Locale::English) => "Reserved virtual address space. It does not represent RAM consumed or committed memory.",
        };
        }
        #[cfg(not(target_os = "linux"))]
        match (self, locale) {
            (MemMetric::WorkingSet, Locale::Portuguese) => concat!(
                "RAM física ocupada agora, incluindo páginas compartilhadas (DLLs, memória ",
                "compartilhada, arquivos mapeados). É o número certo para decidir quem encerrar.\n\n",
                "Uma DLL de 50 MB mapeada em 30 processos conta nos 30, então a soma da coluna ",
                "fica acima do total em uso — a conferência do rodapé usa o privado por isso."
            ),
            (MemMetric::WorkingSet, Locale::English) => concat!(
                "Physical RAM currently occupied, including shared pages (DLLs, shared memory, ",
                "mapped files). This is the right number for deciding what to terminate.\n\n",
                "A 50 MB DLL mapped into 30 processes counts 30 times, so the column sum exceeds ",
                "total use — the footer uses private memory for reconciliation."
            ),
            (MemMetric::Private, Locale::Portuguese) => concat!(
                "Só a memória exclusiva do processo — é a coluna do Gerenciador de Tarefas.\n\n",
                "Exclui DLLs e memória compartilhada, então subestima muito processos como ",
                "Chrome/Electron. Em compensação é a única base que soma sem duplicar nada."
            ),
            (MemMetric::Private, Locale::English) => concat!(
                "Only the process's private memory — the Task Manager column.\n\n",
                "It excludes DLLs and shared memory, so it undercounts processes such as ",
                "Chrome/Electron. In return, it is the only basis that sums without duplication."
            ),
            (MemMetric::Commit, Locale::Portuguese) => concat!(
                "Memória confirmada: o que o processo reservou, esteja na RAM ou no arquivo de ",
                "paginação.\n\nAntecipa pressão de memória, mas não diz o que está na RAM agora."
            ),
            (MemMetric::Commit, Locale::English) => concat!(
                "Committed memory: what the process reserved, whether in RAM or the paging file.\n\n",
                "It anticipates memory pressure but does not say what is in RAM right now."
            ),
        }
    }
}

/// As visões do app, em dois grupos.
///
/// `CORE` é o que o RamDog é — processo, RAM e CPU — e mora nas abas do topo, junto da
/// busca e dos filtros que só fazem sentido ali. `ADDONS` são assuntos vizinhos (o que sobe
/// no boot, o que o Windows gasta sozinho, temperatura, organização de telas): cada um tem
/// sua própria tela inteira e nada a ver com a busca de processo, então são botões com nome
/// no bloco de cima, longe das abas de processo, em vez de disputarem espaço com elas.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum ViewMode {
    List,
    Tree,
    Category,
    Boot,
    Drains,
    Thermal,
    Screens,
    Clean,
    Sweep,
}

impl ViewMode {
    pub const CORE: [ViewMode; 3] = [ViewMode::List, ViewMode::Tree, ViewMode::Category];
    pub const ADDONS: [ViewMode; 6] = [
        ViewMode::Sweep,
        ViewMode::Boot,
        ViewMode::Drains,
        ViewMode::Thermal,
        ViewMode::Screens,
        ViewMode::Clean,
    ];

    pub fn available(self) -> bool {
        if matches!(self, Self::Clean | Self::Sweep) {
            return cfg!(target_os = "linux");
        }
        cfg!(any(windows, target_os = "linux"))
            || matches!(
                self,
                Self::List | Self::Tree | Self::Category | Self::Thermal
            )
    }

    pub fn is_addon(self) -> bool {
        matches!(
            self,
            ViewMode::Boot
                | ViewMode::Drains
                | ViewMode::Thermal
                | ViewMode::Screens
                | ViewMode::Clean
                | ViewMode::Sweep
        )
    }

    pub fn label(self) -> &'static str {
        self.label_for(Locale::Portuguese)
    }

    pub fn label_for(self, locale: Locale) -> &'static str {
        match self {
            ViewMode::List => locale.text("Lista", "Processes"),
            ViewMode::Tree => locale.text("Árvore", "Tree"),
            ViewMode::Category => locale.text("Categorias", "Categories"),
            ViewMode::Boot => locale.text("Partida", "Startup"),
            ViewMode::Drains => locale.text("Desperdício", "Drains"),
            ViewMode::Thermal => locale.text("Térmico", "Thermal"),
            ViewMode::Screens => locale.text("Telas", "Screens"),
            ViewMode::Clean => locale.text("Limpeza", "Cleanup"),
            ViewMode::Sweep => locale.text("Faxina", "Sweep"),
        }
    }

    /// Glifo do addon, desenhado antes do nome no botão. Só BMP — o fallback é a
    /// Segoe UI Symbol, que cobre estes quatro.
    pub fn icon(self) -> &'static str {
        match self {
            ViewMode::Boot => "⚡",
            ViewMode::Drains => "⚠",
            ViewMode::Thermal => "♨",
            ViewMode::Screens => "▦",
            ViewMode::Clean => "♻",
            ViewMode::Sweep => "✔",
            _ => "",
        }
    }

    pub fn tip(self) -> &'static str {
        self.tip_for(Locale::Portuguese)
    }

    pub fn tip_for(self, locale: Locale) -> &'static str {
        match self {
            ViewMode::List => locale.text("Todos os processos, um por linha", "All processes, one per row"),
            ViewMode::Tree => locale.text("Pai → filhos, com a RAM da subárvore", "Parent → children, with subtree RAM"),
            ViewMode::Category => locale.text("Agrupado por categoria", "Grouped by category"),
            ViewMode::Boot => locale.text(
                "Tudo que sobe com o PC — registro, pasta Iniciar, tarefas, serviços. Sem o recorte do Gerenciador de Tarefas",
                "Everything that starts with the PC — registry, Startup folder, tasks, services. Beyond Task Manager's limited list",
            ),
            ViewMode::Drains => locale.text(
                "O que o Windows gasta sem você pedir: Defender, serviços dispensáveis e apps de sistema",
                "What Windows spends without asking: Defender, dispensable services, and system apps",
            ),
            ViewMode::Thermal => locale.text("Sensores, controle de fans e ESTABILIZAR — o TempHUD dentro do RamDog", "Sensors, fan control, and STABILIZE — TempHUD inside RamDog"),
            ViewMode::Screens => locale.text(
                "Monitores, janelas e cenários: arraste janelas no mapa, encaixe na grade e abra vários apps já posicionados",
                "Monitors, windows, and scenes: drag windows on the map, snap them to a grid, and open apps in position",
            ),
            ViewMode::Clean => locale.text(
                "RAM e disco: cache do kernel e zombies; caches, lixeira, pacman, journal e coredumps para apagar",
                "RAM and disk: kernel cache and zombies; remove caches, trash, pacman data, journals, and coredumps",
            ),
            ViewMode::Sweep => locale.text(
                "O que está aberto sem uso, já separado em pode fechar, talvez e em uso: marque em massa, desmarque o que fica e feche tudo de uma vez",
                "What is open but unused, sorted into can close, maybe, and in use: select in bulk, uncheck what stays, and close it all at once",
            ),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            locked: BTreeSet::new(),
            overrides: BTreeMap::new(),
            refresh_ms: 1000,
            locale: Locale::Portuguese,
            min_mb: 0,
            min_cpu: 0.0,
            min_gpu: 0.0,
            min_vram_mb: 0,
            view: ViewMode::List,
            show_system: true,
            mem_metric: MemMetric::WorkingSet,
            show_kernel_rows: true,
            group_apps: true,
            mini: false,
            mini_on_top: true,
            boot_presets: BTreeMap::new(),
            boot_group: BootGroup::StatusPhase,
            screen_presets: BTreeMap::new(),
            screen_grid: String::new(),
            screen_snap: true,
            cmd_column: false,
        }
    }
}

pub fn config_path() -> PathBuf {
    #[cfg(windows)]
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join("Library/Application Support"))
        .unwrap_or_else(|| PathBuf::from("."));
    #[cfg(all(unix, not(target_os = "macos")))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_round_trip_and_legacy_default() {
        let mut value = serde_json::to_value(Config {
            locale: Locale::English,
            ..Config::default()
        })
        .unwrap();
        assert_eq!(value["locale"], "English");

        value.as_object_mut().unwrap().remove("locale");
        assert_eq!(
            serde_json::from_value::<Config>(value).unwrap().locale,
            Locale::Portuguese
        );
    }

    #[test]
    fn locale_text_selects_the_requested_language() {
        assert_eq!(Locale::Portuguese.text("pt", "en"), "pt");
        assert_eq!(Locale::English.text("pt", "en"), "en");
    }
}
