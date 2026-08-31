//! Persistência em %APPDATA%\RamDog\config.json

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::categories::Category;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Nomes de executáveis (minúsculo, com .exe) protegidos contra encerramento.
    pub locked: BTreeSet<String>,
    /// Override manual de categoria por nome de executável (minúsculo, com .exe).
    pub overrides: BTreeMap<String, Category>,
    pub refresh_ms: u64,
    /// Ocultar processos com menos que X MB (na métrica escolhida em `mem_metric`).
    pub min_mb: u32,
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
        match self {
            BootGroup::StatusPhase => "Sobe / não sobe → fase",
            BootGroup::StatusKind => "Sobe / não sobe → tipo",
            BootGroup::Phase => "Fase do arranque",
            BootGroup::Kind => "Tipo de origem",
            BootGroup::Flat => "Lista plana",
        }
    }

    pub fn tip(self) -> &'static str {
        match self {
            BootGroup::StatusPhase => concat!(
                "Primeiro separa o que sobe com o PC do que não sobe; dentro de cada bloco, ",
                "por momento do arranque (kernel → serviços → logon → seus programas)"
            ),
            BootGroup::StatusKind => "Sobe / não sobe e, dentro, por origem: registro, pasta Iniciar, tarefa, serviço…",
            BootGroup::Phase => "Só por momento do arranque, misturando ativas e desativadas",
            BootGroup::Kind => "Só por origem, misturando ativas e desativadas",
            BootGroup::Flat => "Tudo numa lista só, ordenada pela coluna escolhida",
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
    /// Working set privado: só o que é exclusivo do processo. Soma abaixo do real, mas é a
    /// única base que fecha a conta contra o "em uso" sem dupla contagem.
    Private,
    /// Commit (pagefile usage): o que o processo reservou, esteja na RAM ou no disco.
    Commit,
}

impl MemMetric {
    pub const ALL: [MemMetric; 3] = [MemMetric::WorkingSet, MemMetric::Private, MemMetric::Commit];

    pub fn label(self) -> &'static str {
        match self {
            MemMetric::WorkingSet => "Working set",
            MemMetric::Private => "Privado",
            MemMetric::Commit => "Commit",
        }
    }

    /// Rótulo curto para o cabeçalho da coluna.
    pub fn short(self) -> &'static str {
        match self {
            MemMetric::WorkingSet => "RAM",
            MemMetric::Private => "RAM priv.",
            MemMetric::Commit => "Commit",
        }
    }

    pub fn tip(self) -> &'static str {
        match self {
            MemMetric::WorkingSet => concat!(
                "RAM física ocupada agora, incluindo páginas compartilhadas (DLLs, memória ",
                "compartilhada, arquivos mapeados). É o número certo para decidir quem encerrar.\n\n",
                "Uma DLL de 50 MB mapeada em 30 processos conta nos 30, então a soma da coluna ",
                "fica acima do total em uso — a conferência do rodapé usa o privado por isso."
            ),
            MemMetric::Private => concat!(
                "Só a memória exclusiva do processo — é a coluna do Gerenciador de Tarefas.\n\n",
                "Exclui DLLs e memória compartilhada, então subestima muito processos como ",
                "Chrome/Electron. Em compensação é a única base que soma sem duplicar nada."
            ),
            MemMetric::Commit => concat!(
                "Memória confirmada: o que o processo reservou, esteja na RAM ou no arquivo de ",
                "paginação.\n\nAntecipa pressão de memória, mas não diz o que está na RAM agora."
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
}

impl ViewMode {
    pub const CORE: [ViewMode; 3] = [ViewMode::List, ViewMode::Tree, ViewMode::Category];
    pub const ADDONS: [ViewMode; 4] =
        [ViewMode::Boot, ViewMode::Drains, ViewMode::Thermal, ViewMode::Screens];

    pub fn is_addon(self) -> bool {
        matches!(self, ViewMode::Boot | ViewMode::Drains | ViewMode::Thermal | ViewMode::Screens)
    }

    pub fn label(self) -> &'static str {
        match self {
            ViewMode::List => "Lista",
            ViewMode::Tree => "Árvore",
            ViewMode::Category => "Categorias",
            ViewMode::Boot => "Partida",
            ViewMode::Drains => "Desperdício",
            ViewMode::Thermal => "Térmico",
            ViewMode::Screens => "Telas",
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
            _ => "",
        }
    }

    pub fn tip(self) -> &'static str {
        match self {
            ViewMode::List => "Todos os processos, um por linha",
            ViewMode::Tree => "Pai → filhos, com a RAM da subárvore",
            ViewMode::Category => "Agrupado por categoria",
            ViewMode::Boot => concat!(
                "Tudo que sobe com o PC — registro, pasta Iniciar, tarefas, serviços. ",
                "Sem o recorte do Gerenciador de Tarefas"
            ),
            ViewMode::Drains => concat!(
                "O que o Windows gasta sem você pedir: Defender, serviços dispensáveis ",
                "e apps de sistema"
            ),
            ViewMode::Thermal => "Sensores, controle de fans e ESTABILIZAR — o TempHUD dentro do RamDog",
            ViewMode::Screens => concat!(
                "Monitores, janelas e cenários: arraste janelas no mapa, encaixe na grade ",
                "e abra vários apps já posicionados"
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
            min_mb: 0,
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
