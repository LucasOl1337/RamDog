//! Interface egui: lista / árvore / categorias, detalhes, kill, lock.

mod table;
use table::{ProcessQuery, ResourceFilter, Row, RowCache, RowCacheAction, SortKey, SysRow};

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::Ordering;
use std::time::Instant;

use egui::{Align, Color32, Layout, Rect, RichText, Stroke, StrokeKind, TextureHandle, Vec2};
use egui_extras::{Column, TableBuilder, TableRow};

use crate::boot::{Boot, BootOut};
use crate::categories::{self, classify, is_critical, Category};
use crate::clean::{Clean, CleanOut};
use crate::config::{Config, Locale, MemMetric, ViewMode};
use crate::drains::{DrainOut, Drains};
use crate::hwtemp::HwTemp;
use crate::identity;
use crate::knowledge;
use crate::metrics::SysSample;
use crate::pressure::{self, StealKind};
use crate::procs::{self, KernelMem, MemStatus, ProcInfo};
use crate::sampler::{self, SamplerHandle};
use crate::screens::{ScreenOut, Screens};
use crate::signature::{self, SigInfo};
use crate::sweep::{Sweep, SweepOut};
use crate::usage;

const MB: u64 = 1024 * 1024;
const GB: u64 = 1024 * 1024 * 1024;
const ROW_H: f32 = 40.0;
/// Avatar da linha (ícone ou inicial).
const AVATAR: f32 = 26.0;
/// Barra lateral, cards e histórico dos medidores.
const NAV_W: f32 = 200.0;

fn launcher_label(label: String, locale: Locale) -> String {
    if locale == Locale::English {
        label.replace("desktop (navegador)", "desktop (browser)")
    } else {
        label
    }
}
const NAV_ITEM_H: f32 = 34.0;
const CARD_R: f32 = 12.0;
const CARD_PAD: f32 = 12.0;
const SPARK_H: f32 = 46.0;
const HIST_LEN: usize = 90;
const C_CPU: Color32 = Color32::from_rgb(53, 132, 228);
const C_RAM: Color32 = Color32::from_rgb(230, 97, 0);
const C_GPU: Color32 = Color32::from_rgb(51, 209, 122);
const C_DISK: Color32 = Color32::from_rgb(145, 65, 172);
/// Altura das barrinhas dos medidores do topo (CPU/RAM/GPU/Disco) — uma só constante pras
/// quatro pra elas ficarem realmente alinhadas, não só "parecidas".
const TOP_BAR_H: f32 = 8.0;
const TILE_H: f32 = 47.0;

/// Modo mini — HUD de monitoramento. Tamanho fixo: quatro blocos 2x2, a faixa de controles
/// e a faixa de fans. Fixo de propósito; um HUD que o usuário arrasta de tamanho volta a ter
/// os problemas de layout da janela grande, sem ganho nenhum.
pub const MINI_W: f32 = 366.0;
pub const MINI_H: f32 = 166.0;
/// Mínimo da janela completa — repetido aqui porque sair do mini precisa restaurar
/// exatamente o mesmo limite que `main` aplica na abertura.
pub const FULL_MIN_W: f32 = 1000.0;
pub const FULL_MIN_H: f32 = 420.0;

/// Temperatura de um bloco de medidor. `Missing` existe porque "sem sensor" e "este
/// medidor não tem temperatura" são coisas diferentes: a CPU sem admin tem que mostrar um
/// traço e dizer o motivo no hover, e não sumir com o campo como se não existisse.
enum Temp {
    /// O medidor não tem temperatura nenhuma (disco).
    None,
    C(u32),
    Missing(String),
}

/// Um app na visão Lista: todos os processos do mesmo executável somados numa linha.
///
/// É a diferença que fazia o Gerenciador de Tarefas parecer melhor: lá o Chrome com 30
/// renderizadores é uma linha de 90%, aqui eram 30 linhas de 3% que não chegavam nem
/// perto do topo da lista ordenada por CPU. O maior consumidor da máquina ficava
/// invisível por estar picado.
struct AppGroup {
    /// Família conhecida (`app:claude`); demais apps mantêm o caminho do executável.
    key: String,
    /// Caminho do exe de um membro, só para o ícone.
    icon_key: String,
    name: String,
    name_lower: String,
    cat: Category,
    /// Ordenados pelo critério da tabela; o primeiro é o que o clique seleciona.
    pids: Vec<u32>,
    ram: u64,
    cpu: f32,
    gpu: f32,
    gpu_known: bool,
    vram: u64,
    vram_known: bool,
    disk: f64,
    /// FILETIME do processo mais antigo do grupo — é a idade do app, não a do último
    /// aba/renderizador que abriu.
    oldest: i64,
    has_window: bool,
    focused: bool,
    leftover: Option<String>,
    origin: String,
}

impl SysRow {
    fn label(self) -> &'static str {
        self.label_for(Locale::Portuguese)
    }

    fn label_for(self, locale: Locale) -> &'static str {
        match self {
            SysRow::PagedPool => locale.text("Kernel — pool paginado", "Kernel — paged pool"),
            SysRow::NonPagedPool => {
                locale.text("Kernel — pool não-paginado", "Kernel — non-paged pool")
            }
            SysRow::SharedAndCache => locale.text(
                "Compartilhado, cache e tabelas",
                "Shared, cache, and tables",
            ),
        }
    }

    fn tip(self) -> &'static str {
        self.tip_for(Locale::Portuguese)
    }

    fn tip_for(self, locale: Locale) -> &'static str {
        match self {
            SysRow::PagedPool => locale.text(
                concat!(
                    "Memória do kernel e dos drivers que pode ser paginada ao disco.\n\n",
                    "Não pertence a processo nenhum, por isso nunca aparece no Gerenciador de ",
                    "Tarefas. Acima de ~2 GB costuma indicar vazamento de driver."
                ),
                concat!(
                    "Kernel and driver memory that can be paged to disk.\n\n",
                    "It belongs to no process, so it never appears in Task Manager. Above ~2 GB often points to a driver leak."
                ),
            ),
            SysRow::NonPagedPool => locale.text(
                concat!(
                    "Memória do kernel que nunca sai da RAM física — filas de I/O, estruturas de ",
                    "driver, buffers de rede.\n\nSempre residente, sempre invisível na lista de processos."
                ),
                concat!(
                    "Kernel memory that never leaves physical RAM — I/O queues, driver structures, and network buffers.\n\nAlways resident, always invisible in the process list."
                ),
            ),
            SysRow::SharedAndCache => locale.text(
                concat!(
                    "O que sobra do \"em uso\" depois de descontar a memória privada dos processos ",
                    "e os dois pools do kernel.\n\n",
                    "É sobretudo memória compartilhada residente (DLLs e seções mapeadas em vários ",
                    "processos, contadas uma vez só aqui), mais cache de arquivos residente, tabelas ",
                    "de página e páginas travadas por driver de GPU.\n\n",
                    "Calculado por diferença — é um resto, não uma medição direta."
                ),
                concat!(
                    "What remains of \"in use\" after subtracting private process memory and the two kernel pools.\n\n",
                    "Mostly resident shared memory (DLLs and mapped sections counted once here), resident file cache, page tables, and GPU-driver locked pages.\n\n",
                    "Calculated by difference — it is a remainder, not a direct measurement."
                ),
            ),
        }
    }

    fn color(self) -> Color32 {
        match self {
            SysRow::PagedPool => Color32::from_rgb(216, 130, 88),
            SysRow::NonPagedPool => Color32::from_rgb(190, 108, 74),
            SysRow::SharedAndCache => Color32::from_rgb(120, 132, 150),
        }
    }
}

/// Como o "em uso" se reparte entre processos e o que não é processo.
/// Tudo aqui é calculado sobre memória **privada**, a única base que soma sem contar a
/// mesma página física duas vezes. Por construção `privado + pools + resto == em uso`.
#[derive(Clone, Copy, Default)]
struct MemBreakdown {
    used: u64,
    /// Soma do working set privado de todos os processos.
    private: u64,
    paged_pool: u64,
    nonpaged_pool: u64,
    /// Resto: compartilhado residente + cache + tabelas de página + driver locked.
    shared_and_cache: u64,
    /// `false` quando `GetPerformanceInfo` falhou — sem separar os pools do resto.
    kernel_ok: bool,
}

/// Agregados que dependem da amostra inteira. A UI pode repintar dezenas de vezes por
/// movimento do mouse; nenhum desses valores precisa ser refeito até chegar outra amostra
/// (ou mudar a métrica de memória/classificação).
#[derive(Default)]
struct UiDerived {
    breakdown: MemBreakdown,
    pressure: pressure::Snapshot,
    thieves: Vec<pressure::Thief>,
    cpu_split: Option<CpuSplit>,
    cat_totals: HashMap<Category, (u64, usize)>,
    private_cat_totals: HashMap<Category, (u64, usize)>,
    linux_memory_summary: String,
    linux_memory_available: usize,
    metric_total: u64,
}

pub struct App {
    #[cfg(target_os = "linux")]
    gpu_index: usize,
    #[cfg(target_os = "linux")]
    smoke_started: Option<Instant>,
    cfg: Config,
    cfg_dirty: bool,
    sampler: SamplerHandle,

    procs: Vec<ProcInfo>,
    by_pid: HashMap<u32, usize>,
    children: HashMap<u32, Vec<u32>>,
    cats: HashMap<u32, Category>,
    subtree: HashMap<u32, u64>,
    subtree_count: HashMap<u32, usize>,
    /// CPU da subárvore (% da máquina). Sem isso a Árvore ordenava por CPU do pai
    /// e um app com o consumo espalhado nos filhos afundava na lista.
    subtree_cpu: HashMap<u32, f32>,
    mem: MemStatus,
    kernel: KernelMem,

    /// Qual serviço roda em cada PID — o que dá nome aos svchost.exe idênticos.
    /// Relido a cada poucos segundos: o SCM é caro demais para o ritmo da amostra.
    services: HashMap<u32, Vec<(String, String)>>,
    services_at: Option<Instant>,
    /// Nome de todo PID já visto, para nunca mostrar só "(pid 1208 encerrado)".
    seen_names: HashMap<u32, String>,

    /// Assinatura digital por caminho de executável. WinVerifyTrust custa dezenas de ms:
    /// roda numa thread, só para o processo selecionado, e o resultado fica cacheado.
    sigs: HashMap<String, SigInfo>,
    sig_pending: std::collections::HashSet<String>,
    sig_tx: std::sync::mpsc::Sender<(String, SigInfo)>,
    sig_rx: std::sync::mpsc::Receiver<(String, SigInfo)>,
    last_sample: Option<Instant>,
    sample_ms: f32,
    /// Núcleos lógicos — só para explicar na interface o que "100%" quer dizer.
    ncpu: usize,
    sys: SysSample,
    gpu_per_proc: bool,
    hwtemp: HwTemp,

    icons: HashMap<String, Option<TextureHandle>>,
    /// Quanto tempo cada exe fica aberto. Alimenta o Scan da Partida.
    usage: usage::Tracker,

    search: String,
    sort: SortKey,
    sort_desc: bool,
    cat_enabled: HashSet<Category>,
    selected: Option<u32>,
    selected_keep: Option<(ProcInfo, Category)>,
    expanded: HashSet<u32>,
    collapsed_cats: HashSet<Category>,
    /// Grupos da visão Lista, reconstruídos junto com as linhas. `Row::AppHeader` guarda
    /// só o índice aqui dentro.
    groups: Vec<AppGroup>,
    /// Apps que o usuário pediu para abrir. O resto fica numa linha só.
    expanded_apps: HashSet<String>,
    status: Option<(Instant, String, bool)>,
    is_admin: bool,
    scroll_to_selected: bool,
    /// Frame cache owns invalidation, pending snapshots and hover-protected order.
    row_cache: RowCache,
    derived: UiDerived,
    derived_dirty: bool,
    table_rect: Option<egui::Rect>,
    drains: Drains,
    boot: Boot,
    screens: Screens,
    clean: Clean,
    sweep: Sweep,
    /// Última visão de processo (Lista/Árvore/Categorias) antes de entrar num addon.
    /// Clicar de novo no addon aceso volta para ela, em vez de cair sempre em Lista.
    last_core: ViewMode,
    /// Visão Térmico: valor local de slider por fan + quando o usuário mexeu pela última vez.
    /// Por ~2.5s depois de mexer, o slider mostra o valor local em vez do reportado pelo
    /// helper — sem isso o slider "volta" enquanto o helper ainda não aplicou/reportou.
    thermal_edit: HashMap<String, (f32, Instant)>,
    /// Estado que o ESTABILIZAR deve assumir logo depois do clique, até o helper confirmar.
    /// Sem isso o botão só muda quando o relatório do hwtemp chega (uma amostra inteira
    /// depois, no ritmo escolhido), e o clique parece não ter funcionado — o usuário fica
    /// esperando junto do hardware. Some sozinho quando o relatório bate ou em 3s.
    stab_pending: Option<(bool, Instant)>,
    /// Em qual modo a janela (decoração, tamanho, always-on-top) já está configurada.
    /// Diferente de `cfg.mini` significa que a troca ainda não foi enviada ao sistema.
    applied_mini: bool,
    /// Tamanho da janela completa guardado ao entrar no mini, para restaurar ao sair.
    full_size: Option<Vec2>,
    /// Histórico dos medidores, em % — alimenta os gráficos dos cards.
    hist_cpu: VecDeque<f32>,
    hist_ram: VecDeque<f32>,
    hist_gpu: VecDeque<f32>,
    hist_disk: VecDeque<f32>,
    show_prefs: bool,
    logo: Option<TextureHandle>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_style(&cc.egui_ctx);
        let mut cfg = Config::load();
        if !cfg.view.available() {
            cfg.view = ViewMode::List;
        }
        let mini = cfg.mini;
        // Abrir direto num addon é legítimo (foi assim que fechou), mas o botão de voltar
        // precisa de um destino desde o primeiro frame.
        let last_core = if cfg.view.is_addon() {
            ViewMode::List
        } else {
            cfg.view
        };
        let is_admin = procs::is_admin();
        let sampler = sampler::spawn(cc.egui_ctx.clone(), cfg.refresh_ms);
        let (sig_tx, sig_rx) = std::sync::mpsc::channel();
        Self {
            #[cfg(target_os = "linux")]
            gpu_index: 0,
            #[cfg(target_os = "linux")]
            smoke_started: std::env::args()
                .any(|a| a == "--smoke-test")
                .then(Instant::now),
            cfg,
            cfg_dirty: false,
            sampler,
            procs: Vec::new(),
            by_pid: HashMap::new(),
            children: HashMap::new(),
            cats: HashMap::new(),
            subtree: HashMap::new(),
            subtree_count: HashMap::new(),
            subtree_cpu: HashMap::new(),
            mem: MemStatus::default(),
            kernel: KernelMem::default(),
            services: HashMap::new(),
            services_at: None,
            seen_names: HashMap::new(),
            sigs: HashMap::new(),
            sig_pending: std::collections::HashSet::new(),
            sig_tx,
            sig_rx,
            last_sample: None,
            sample_ms: 0.0,
            ncpu: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
            sys: SysSample::default(),
            gpu_per_proc: true,
            hwtemp: HwTemp::default(),
            icons: HashMap::new(),
            usage: usage::Tracker::load(),
            search: String::new(),
            sort: SortKey::Ram,
            sort_desc: true,
            cat_enabled: Category::ALL.iter().copied().collect(),
            selected: None,
            selected_keep: None,
            expanded: HashSet::new(),
            collapsed_cats: HashSet::new(),
            groups: Vec::new(),
            expanded_apps: HashSet::new(),
            status: None,
            is_admin,
            scroll_to_selected: false,
            row_cache: RowCache::default(),
            derived: UiDerived::default(),
            derived_dirty: true,
            table_rect: None,
            drains: Drains::new(),
            boot: Boot::new(),
            screens: Screens::new(),
            clean: Clean::new(),
            sweep: Sweep::new(),
            last_core,
            thermal_edit: HashMap::new(),
            stab_pending: None,
            // `main` já abriu a janela no modo lido da config — nada a aplicar no 1º frame.
            applied_mini: mini,
            full_size: None,
            hist_cpu: VecDeque::with_capacity(HIST_LEN),
            hist_ram: VecDeque::with_capacity(HIST_LEN),
            hist_gpu: VecDeque::with_capacity(HIST_LEN),
            hist_disk: VecDeque::with_capacity(HIST_LEN),
            show_prefs: false,
            logo: image::load_from_memory(include_bytes!("../assets/ramdog-256.png"))
                .ok()
                .map(|img| {
                    let img = img.into_rgba8();
                    let (w, h) = img.dimensions();
                    let ci = egui::ColorImage::from_rgba_unmultiplied(
                        [w as usize, h as usize],
                        &img.into_raw(),
                    );
                    cc.egui_ctx
                        .load_texture("ramdog-logo", ci, egui::TextureOptions::LINEAR)
                }),
        }
    }

    // ---------- dados ----------

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
        self.kernel = snap.kernel;
        self.last_sample = Some(snap.taken);
        self.sample_ms = snap.sample_ms;
        // Primeira amostra de GetSystemTimes não tem delta — guarda o % anterior
        // pra o medidor não piscar "–" no primeiro tick.
        let cpu_keep = if snap.sys.cpu_pct.is_none() {
            self.sys.cpu_pct
        } else {
            None
        };
        self.sys = snap.sys;
        if let Some(p) = cpu_keep {
            self.sys.cpu_pct = Some(p);
        }
        #[cfg(target_os = "linux")]
        {
            self.sys.gpu = self
                .sys
                .gpu_linux
                .cards
                .get(self.gpu_index)
                .or_else(|| self.sys.gpu_linux.cards.first())
                .cloned();
        }
        self.gpu_per_proc = snap.gpu_per_proc;
        self.hwtemp = snap.hwtemp;
        self.push_hist();
        self.attach_windows();
        self.usage.tick(&self.procs);
        self.usage.save_if_due();
        // Nome de cada PID guardado enquanto ele existe: é o que permite dizer
        // "smss.exe (1208), já encerrado" em vez do número solto quando o pai morre.
        for p in &self.procs {
            self.seen_names
                .entry(p.pid)
                .or_insert_with(|| p.name.clone());
        }
        self.refresh_services();
        self.rebuild_indexes();
        {
            let metric = self.cfg.mem_metric;
            let locked = &self.cfg.locked;
            let me = std::process::id();
            let cats = &self.cats;
            self.sweep.observe(
                &self.procs,
                &|p| Self::metric_of(metric, p),
                &|p| {
                    is_critical(&p.name_lower, p.pid)
                        || locked.contains(&p.name_lower)
                        || p.pid == me
                },
                &|pid| cats.get(&pid).copied().unwrap_or(Category::Other),
            );
        }
        self.row_cache.snapshot_changed();
        self.derived_dirty = true;
        if let Some(pid) = self.selected {
            if let Some(&i) = self.by_pid.get(&pid) {
                self.selected_keep = Some((self.procs[i].clone(), self.cat(pid)));
            }
        }
    }

    fn rebuild_indexes(&mut self) {
        self.by_pid = self
            .procs
            .iter()
            .enumerate()
            .map(|(i, p)| (p.pid, i))
            .collect();
        self.children.clear();
        for p in &self.procs {
            if p.ppid != 0 {
                self.children.entry(p.ppid).or_default().push(p.pid);
            }
        }
        let overrides: HashMap<String, Category> = self
            .cfg
            .overrides
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        self.cats = classify(&self.procs, &overrides);
        // soma de subárvore (memo por DFS)
        self.subtree.clear();
        self.subtree_count.clear();
        self.subtree_cpu.clear();
        let pids: Vec<u32> = self.procs.iter().map(|p| p.pid).collect();
        for pid in pids {
            self.subtree_total(pid, 0);
        }
    }

    fn subtree_total(&mut self, pid: u32, depth: usize) -> (u64, usize, f32) {
        if let (Some(&t), Some(&c), Some(&u)) = (
            self.subtree.get(&pid),
            self.subtree_count.get(&pid),
            self.subtree_cpu.get(&pid),
        ) {
            return (t, c, u);
        }
        let m = self.cfg.mem_metric;
        let (own, own_cpu) = self
            .by_pid
            .get(&pid)
            .map(|&i| (Self::metric_of(m, &self.procs[i]), self.procs[i].cpu_pct))
            .unwrap_or((0, 0.0));
        let mut total = own;
        let mut count = 1usize;
        let mut cpu = own_cpu;
        if depth < 128 {
            let kids = self.children.get(&pid).cloned().unwrap_or_default();
            for k in kids {
                let (t, c, u) = self.subtree_total(k, depth + 1);
                total += t;
                count += c;
                cpu += u;
            }
        }
        self.subtree.insert(pid, total);
        self.subtree_count.insert(pid, count);
        self.subtree_cpu.insert(pid, cpu);
        (total, count, cpu)
    }

    fn gpu_pid_available(&self, pid: u32) -> bool {
        #[cfg(target_os = "linux")]
        {
            self.sys.gpu_linux.by_pid.contains_key(&pid)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = pid;
            self.gpu_per_proc
        }
    }

    fn proc(&self, pid: u32) -> Option<&ProcInfo> {
        self.by_pid.get(&pid).map(|&i| &self.procs[i])
    }

    /// O número que a coluna RAM mostra, conforme a métrica escolhida.
    ///
    /// Até 2026-08 tudo aqui era `private_ws` fixo, herdado do Gerenciador de Tarefas. O
    /// efeito era a lista somar 9,7 GB numa máquina com 35 GB em uso — metade da RAM dos
    /// processos estava em páginas compartilhadas que o privado não conta.
    /// Relê o mapa serviço→PID de tempos em tempos. Enumerar o SCM custa alguns
    /// milissegundos; a 1 Hz seria desperdício, já que serviço quase não troca de PID.
    fn refresh_services(&mut self) {
        #[cfg(windows)]
        {
            let due = self
                .services_at
                .map(|t| t.elapsed().as_secs() >= 10)
                .unwrap_or(true);
            if due {
                self.services = crate::sys::services_by_pid();
                self.services_at = Some(Instant::now());
            }
        }
        // Sem esse teto o cache de nomes cresceria para sempre num PC que fica dias ligado.
        if self.seen_names.len() > 8192 {
            let alive: std::collections::HashSet<u32> = self.procs.iter().map(|p| p.pid).collect();
            self.seen_names.retain(|pid, _| alive.contains(pid));
        }
    }

    /// Serviços hospedados por um PID, já no formato de exibição.
    fn services_of(&self, pid: u32) -> &[(String, String)] {
        self.services.get(&pid).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Assinatura do executável, verificando em segundo plano na primeira vez.
    /// `None` = ainda verificando.
    fn signature_of(&mut self, path: &str, ctx: &egui::Context) -> Option<SigInfo> {
        if path.is_empty() {
            return Some(SigInfo {
                trust: signature::Trust::Unknown("sem acesso ao caminho".into()),
                signer: String::new(),
            });
        }
        let key = path.to_lowercase();
        if let Some(s) = self.sigs.get(&key) {
            return Some(s.clone());
        }
        if self.sig_pending.insert(key.clone()) {
            let tx = self.sig_tx.clone();
            let ctx = ctx.clone();
            let p = path.to_string();
            std::thread::spawn(move || {
                let info = signature::verify(&p);
                let _ = tx.send((key, info));
                ctx.request_repaint();
            });
        }
        None
    }

    /// Recolhe as verificações de assinatura que terminaram desde o quadro anterior.
    fn drain_sigs(&mut self) {
        while let Ok((key, info)) = self.sig_rx.try_recv() {
            self.sig_pending.remove(&key);
            self.sigs.insert(key, info);
        }
    }

    fn subtree_memory_available(&self, pid: u32) -> bool {
        let mut pending = vec![pid];
        let mut seen = HashSet::new();
        while let Some(pid) = pending.pop() {
            if !seen.insert(pid) {
                continue;
            }
            if self
                .proc(pid)
                .is_some_and(|p| !metric_available(self.cfg.mem_metric, p))
            {
                return false;
            }
            if let Some(children) = self.children.get(&pid) {
                pending.extend(children);
            }
        }
        true
    }

    fn mem_of(&self, p: &ProcInfo) -> u64 {
        Self::metric_of(self.cfg.mem_metric, p)
    }

    fn metric_of(m: MemMetric, p: &ProcInfo) -> u64 {
        table::metric_of(m, p)
    }

    /// Reparte o "em uso" em parcelas que somam exatamente o total.
    ///
    /// Sempre sobre memória **privada**, independentemente da métrica escolhida na coluna:
    /// o working set conta a mesma página compartilhada em cada processo que a mapeia, então
    /// somá-lo daria mais que a RAM instalada. O resto sai por diferença.
    fn calculate_breakdown(&self) -> MemBreakdown {
        let used = self.mem.used_phys();
        let private: u64 = self.procs.iter().map(|p| p.private_ws).sum();
        let (paged, nonpaged) = if self.kernel.ok {
            (self.kernel.paged_pool, self.kernel.nonpaged_pool)
        } else {
            (0, 0)
        };
        // Clamp: o pool paginado do GetPerformanceInfo inclui a fração paginada ao disco, e o
        // privado dos processos é amostrado num instante diferente do MEMORYSTATUSEX. Em
        // máquina com pouca RAM livre as duas folgas podem estourar o total — preferimos um
        // resto zerado a um número negativo travestido de dado.
        let attributed = private.saturating_add(paged).saturating_add(nonpaged);
        MemBreakdown {
            used,
            private,
            paged_pool: paged,
            nonpaged_pool: nonpaged,
            shared_and_cache: used.saturating_sub(attributed),
            kernel_ok: self.kernel.ok,
        }
    }

    fn attach_windows(&mut self) {
        #[cfg(target_os = "linux")]
        {
            let focused = crate::desktop_linux::focused_pid();
            let by_pid = crate::desktop_linux::windows_by_pid();
            for p in self.procs.iter_mut() {
                if let Some(w) = by_pid.get(&p.pid) {
                    p.has_window = w.mapped;
                    p.window_title = if w.title.is_empty() {
                        None
                    } else {
                        Some(w.title.clone())
                    };
                    p.window_class = if w.class.is_empty() {
                        None
                    } else {
                        Some(w.class.clone())
                    };
                }
                p.focused = focused == Some(p.pid);
            }
        }
    }

    fn cat(&self, pid: u32) -> Category {
        self.cats.get(&pid).copied().unwrap_or(Category::Other)
    }

    fn is_locked(&self, p: &ProcInfo) -> bool {
        is_critical(&p.name_lower, p.pid)
            || self.cfg.locked.contains(&p.name_lower)
            || p.pid == std::process::id()
    }

    fn table_query(&self) -> ProcessQuery<'_> {
        ProcessQuery {
            procs: &self.procs,
            by_pid: &self.by_pid,
            cats: &self.cats,
            subtree: &self.subtree,
            subtree_cpu: &self.subtree_cpu,
            cat_enabled: &self.cat_enabled,
            metric: self.cfg.mem_metric,
            resources: ResourceFilter {
                min_mb: self.cfg.min_mb,
                min_cpu: self.cfg.min_cpu,
                min_gpu: self.cfg.min_gpu,
                min_vram_mb: self.cfg.min_vram_mb,
            },
            grouped: self.cfg.view == ViewMode::List && self.cfg.group_apps,
        }
    }

    fn descendants(&self, pid: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let mut stack = vec![pid];
        let mut guard = 0;
        while let Some(x) = stack.pop() {
            guard += 1;
            if guard > 100_000 {
                break;
            }
            if let Some(kids) = self.children.get(&x) {
                for &k in kids {
                    out.push(k);
                    stack.push(k);
                }
            }
        }
        out
    }

    /// Origem "inteligente" para a coluna: primeiro ancestral vivo que não seja host genérico
    /// (cmd, node, bash...). Se a cadeia morreu, cai na impressão digital do ambiente
    /// (Claude Code / Codex / Maestri...). Retorna (rótulo, pid clicável, dica, via_ambiente).
    fn origin_label(&self, p: &ProcInfo, locale: Locale) -> (String, Option<u32>, String, bool) {
        let id = identity::of(p);
        if matches!(
            id.kind,
            identity::Kind::Game | identity::Kind::Project | identity::Kind::Emulator
        ) {
            if let Some(origin) = id.origin {
                let mut tip = origin.clone();
                if let Some(title) = &p.window_title {
                    tip.push_str(&format!("\n{}: {title}", locale.text("janela", "window")));
                }
                return (origin, None, tip, true);
            }
        }
        let l = &p.launcher;
        let unit = l.unit_label().map(|u| launcher_label(u, locale));
        let mut cur = p.ppid;
        let mut chain: Vec<String> = Vec::new();
        let mut guard = 0;
        while cur != 0 && guard < 64 {
            guard += 1;
            let Some(a) = self.proc(cur) else { break };
            // O systemd --user é pai de tudo que veio de `uwsm app`/`systemd-run`: como origem
            // não diz nada. A unidade do cgroup (abaixo) diz.
            if a.name_lower == "systemd" || a.pid == 1 {
                break;
            }
            chain.push(format!("{} ({})", a.name, a.pid));
            if !categories::is_generic_host(&a.name_lower) {
                let mut tip = if chain.len() > 1 {
                    format!(
                        "{}\n{}",
                        chain.iter().rev().cloned().collect::<Vec<_>>().join(" › "),
                        locale.text("clique para selecionar", "click to select")
                    )
                } else {
                    format!(
                        "PID {} — {}",
                        a.pid,
                        locale.text("clique para selecionar o pai", "click to select the parent")
                    )
                };
                // `devin (1896666)` dentro de `hermes-gateway.service`: o pai imediato é o
                // devin, mas quem mandou foi o Hermes. Os dois aparecem.
                let mut label = a.name.clone();
                let redundant = |u: &str| {
                    let (u, n) = (
                        u.to_lowercase(),
                        a.name_lower.trim_end_matches(": server").to_string(),
                    );
                    u.starts_with("desktop") || u.contains(&n) || n.contains(&u)
                };
                if let Some(u) = unit.as_deref().filter(|u| !redundant(u)) {
                    label = format!("{label} · {u}");
                }
                if let Some(raw) = &l.unit {
                    tip.push_str(&format!("\n{}: {raw}", locale.text("unidade", "unit")));
                }
                return (label, Some(a.pid), tip, false);
            }
            cur = a.ppid;
        }
        // Cadeia só de hosts genéricos ou interrompida: usa o ambiente herdado e o cgroup.
        if !l.short().is_empty() {
            let mut tip = if l.agent.is_some() || l.host.is_some() {
                locale
                    .text(
                        "Deduzido das variáveis de ambiente herdadas",
                        "Inferred from inherited environment variables",
                    )
                    .to_string()
            } else {
                locale
                    .text(
                        "Deduzido da unidade do systemd (cgroup) que contém o processo",
                        "Inferred from the systemd unit (cgroup) containing the process",
                    )
                    .to_string()
            };
            if let Some(raw) = &l.unit {
                tip.push_str(&format!("\n{}: {raw}", locale.text("unidade", "unit")));
            }
            if let Some(sid) = &l.session {
                tip.push_str(&format!(" · {} {sid}", locale.text("sessão", "session")));
            }
            if let Some(apid) = l.agent_pid.filter(|x| self.proc(*x).is_some()) {
                let an = self.proc(apid).map(|a| a.name.clone()).unwrap_or_default();
                tip.push_str(&format!(
                    "\n{an} (PID {apid}) — {}",
                    locale.text("clique para selecionar", "click to select")
                ));
                return (
                    format!("{} · {an}", l.agent.clone().unwrap_or_default()),
                    Some(apid),
                    tip,
                    true,
                );
            }
            if !chain.is_empty() {
                tip.push_str(&format!(
                    "\n{}: {}",
                    locale.text("cadeia", "chain"),
                    chain.iter().rev().cloned().collect::<Vec<_>>().join(" › ")
                ));
            } else if p.raw_ppid != 0 {
                tip.push_str(&format!(
                    "\n{} (PID {})",
                    locale.text("pai já encerrado", "parent exited"),
                    p.raw_ppid
                ));
            }
            return (launcher_label(l.short(), locale), None, tip, true);
        }
        if let Some(pp) = self.proc(p.ppid) {
            return (
                pp.name.clone(),
                Some(pp.pid),
                format!(
                    "PID {} — {}",
                    pp.pid,
                    locale.text("clique para selecionar o pai", "click to select the parent")
                ),
                false,
            );
        }
        if p.raw_ppid != 0 {
            (
                format!(
                    "(pid {} {})",
                    p.raw_ppid,
                    locale.text("encerrado", "exited")
                ),
                None,
                String::new(),
                false,
            )
        } else {
            ("–".into(), None, String::new(), false)
        }
    }

    /// Coluna "Quem abriu": a cadeia de quem chamou quem, da raiz até o pai imediato.
    ///
    /// É o que responde "de onde saiu isso" sem ler 300 caracteres de argumentos: `foot ›
    /// bash › claude` diz mais que `--type=utility --utility-sub-type=network`. A unidade do
    /// systemd e o agente deduzido do ambiente entram na frente quando a cadeia não os mostra
    /// (o `python` de `hermes-gateway.service` vira `Hermes (gateway) › python`).
    ///
    /// Devolve (texto da célula, tooltip, PID do pai imediato para o clique).
    fn invoker_of(&self, p: &ProcInfo) -> (String, String, Option<u32>) {
        let locale = self.cfg.locale;
        let mut chain: Vec<(String, u32)> = Vec::new();
        let mut cur = p.ppid;
        let mut guard = 0;
        let mut from_desktop = false;
        while cur != 0 && guard < 64 {
            guard += 1;
            let Some(a) = self.proc(cur) else { break };
            if a.pid == 1 || a.name_lower == "systemd" {
                break;
            }
            // O compositor/shell é a raiz de tudo que o usuário abriu; como "quem chamou"
            // ele é ruído: `start-hyprland › Hyprland › foot › bash` vira `foot › bash`.
            if is_desktop_shell(&a.name_lower) {
                from_desktop = true;
                break;
            }
            chain.push((a.name.clone(), a.pid));
            cur = a.ppid;
        }
        chain.reverse();
        let target = chain.last().map(|(_, pid)| *pid);
        // `chromium › chromium › chromium` é um só chromium para quem lê.
        let mut names: Vec<String> = Vec::new();
        for (n, _) in &chain {
            if names.last() != Some(n) {
                names.push(n.clone());
            }
        }
        let l = &p.launcher;
        let mut tip = String::new();
        let known = |what: &str| {
            let w = what.to_lowercase();
            let head = w
                .split(|c: char| !c.is_alphanumeric())
                .next()
                .unwrap_or("")
                .to_string();
            let hit = |n: &str| {
                let n = n.to_lowercase();
                n == w || (head.len() >= 4 && (n.contains(&head) || head.contains(&n)))
            };
            hit(&p.name) || names.iter().any(|n| hit(n))
        };
        let mut prefix: Vec<String> = Vec::new();
        if let Some(u) = l.unit_label().map(|u| launcher_label(u, locale)) {
            // "desktop" (menu, atalho, `uwsm app`) só vale quando não há cadeia para mostrar.
            let desktop = u.to_lowercase().starts_with("desktop");
            if (!desktop || names.is_empty()) && !known(&u) {
                prefix.push(u);
            }
        }
        if let Some(a) = l.agent.as_deref().filter(|a| !known(a)) {
            prefix.push(a.to_string());
        }
        if let Some(h) = l.host.as_deref().filter(|h| !known(h)) {
            prefix.push(h.to_string());
        }
        let mut parts = prefix.clone();
        if names.len() > 4 {
            parts.push("…".into());
            parts.extend(names[names.len() - 4..].iter().cloned());
        } else {
            parts.extend(names.iter().cloned());
        }
        let label = if parts.is_empty() {
            if from_desktop {
                "desktop".to_string()
            } else if p.raw_ppid == 1
                || self
                    .proc(p.raw_ppid)
                    .map(|a| a.name_lower == "systemd")
                    .unwrap_or(false)
            {
                locale
                    .text("sistema (systemd)", "system (systemd)")
                    .to_string()
            } else if p.raw_ppid != 0 {
                match self.seen_names.get(&p.raw_ppid) {
                    Some(n) if locale == Locale::Portuguese => format!("{n} (já saiu)"),
                    Some(n) => format!("{n} (exited)"),
                    None if locale == Locale::Portuguese => {
                        format!("pai {} (já saiu)", p.raw_ppid)
                    }
                    None => format!("parent {} (exited)", p.raw_ppid),
                }
            } else {
                "–".to_string()
            }
        } else {
            parts.join(" › ")
        };
        if !chain.is_empty() {
            tip.push_str(locale.text(
                "Quem chamou quem, da raiz até este processo:\n",
                "Who launched whom, from the root to this process:\n",
            ));
            for (n, pid) in &chain {
                tip.push_str(&format!("  {n} ({pid})\n"));
            }
            tip.push_str(&format!(
                "  {} ({})\n{}",
                p.name,
                p.pid,
                locale.text("clique: selecionar o pai", "click: select the parent")
            ));
        } else if p.raw_ppid > 1 {
            if locale == Locale::Portuguese {
                tip.push_str(&format!(
                    "O pai (PID {}) já saiu; a cadeia acima dele não dá para reconstruir.",
                    p.raw_ppid
                ));
            } else {
                tip.push_str(&format!(
                    "The parent (PID {}) has exited; its ancestry cannot be reconstructed.",
                    p.raw_ppid
                ));
            }
        }
        if let Some(raw) = &l.unit {
            tip.push_str(&format!("\n{}: {raw}", locale.text("unidade", "unit")));
        }
        if let Some(a) = &l.agent {
            tip.push_str(&format!("\n{}: {a}", locale.text("agente", "agent")));
            if let Some(sid) = &l.session {
                tip.push_str(&format!(" ({} {sid})", locale.text("sessão", "session")));
            }
        }
        if let Some(cwd) = &l.init_cwd {
            tip.push_str(&format!("\n{}: {cwd}", locale.text("projeto", "project")));
        }
        let cmd = if p.cmdline.is_empty() {
            p.exe_path.as_str()
        } else {
            p.cmdline.as_str()
        };
        if !cmd.is_empty() {
            let short: String = cmd.chars().take(400).collect();
            tip.push_str(&format!(
                "\n\n{}: {short}{}",
                locale.text("comando", "command"),
                if cmd.chars().count() > 400 { "…" } else { "" }
            ));
        }
        (label, tip.trim_start_matches('\n').to_string(), target)
    }

    fn ancestry(&self, pid: u32) -> Vec<u32> {
        let mut chain = Vec::new();
        let mut cur = self.proc(pid).map(|p| p.ppid).unwrap_or(0);
        let mut guard = 0;
        while cur != 0 && guard < 64 {
            guard += 1;
            chain.push(cur);
            cur = self.proc(cur).map(|p| p.ppid).unwrap_or(0);
        }
        chain.reverse();
        chain
    }

    fn sort_pids(&self, pids: &mut [u32], tree: bool) {
        self.table_query()
            .sort_pids(pids, tree, self.sort, self.sort_desc, |p| {
                self.steal_rank(p)
            });
    }

    fn proc_age_secs(p: &ProcInfo) -> u64 {
        ((procs::now_filetime() - p.create_time).max(0) / 10_000_000) as u64
    }

    fn steal_of(&self, p: &ProcInfo) -> Option<StealKind> {
        let leftover =
            identity::leftover_reason(&p.cmdline, p.kernel_state, p.has_window).is_some();
        pressure::steal_kind(
            leftover,
            &p.cmdline,
            p.cpu_pct,
            self.mem_of(p),
            self.ncpu as u32,
            Self::proc_age_secs(p),
        )
    }

    fn steal_rank(&self, p: &ProcInfo) -> u8 {
        self.steal_of(p).map(StealKind::rank).unwrap_or(0)
    }

    fn group_steal_rank(&self, g: &AppGroup) -> u8 {
        g.pids
            .iter()
            .filter_map(|pid| self.proc(*pid))
            .map(|p| self.steal_rank(p))
            .max()
            .unwrap_or(0)
    }

    fn calculate_pressure_snap(&self) -> pressure::Snapshot {
        pressure::Snapshot {
            load1: self.sys.load1,
            ncpu: self.ncpu as u32,
            swap_used: self.mem.swap_used,
            swap_total: self.mem.swap_total,
            gpu_pct: self.sys.gpu.as_ref().and_then(|g| g.util_pct),
            game_open: self
                .procs
                .iter()
                .any(|p| self.cat(p.pid) == Category::Games),
        }
    }

    fn calculate_thieves(&self, game: bool) -> Vec<pressure::Thief> {
        let mut out: Vec<pressure::Thief> = self
            .procs
            .iter()
            .filter_map(|p| {
                let kind = self.steal_of(p)?;
                let cores = pressure::cores(p.cpu_pct, self.ncpu as u32);
                if !pressure::notable(kind, cores, game) {
                    return None;
                }
                Some(pressure::Thief {
                    pid: p.pid,
                    label: identity::of(p).label,
                    kind,
                    cores,
                })
            })
            .collect();
        out.sort_by(|a, b| {
            b.kind
                .rank()
                .cmp(&a.kind.rank())
                .then(
                    b.cores
                        .partial_cmp(&a.cores)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
                .then(a.pid.cmp(&b.pid))
        });
        out.truncate(5);
        out
    }

    fn cpu_cell(cpu_pct: f32, ncpu: usize) -> (String, Color32) {
        // Sempre % da máquina: uma unidade só na coluna. "0.8×" acima de "0.9%" não
        // ordenava aos olhos de ninguém. Os núcleos equivalentes ficam na cor e no hover —
        // e na própria célula quando a Disputa está ligada (`cpu_cell_disputa`).
        let cores = pressure::cores(cpu_pct, ncpu as u32);
        if cores >= 0.8 {
            (format!("{cpu_pct:.0}%"), Color32::from_rgb(255, 150, 90))
        } else if cores >= 0.25 {
            (format!("{cpu_pct:.1}%"), Color32::from_rgb(230, 210, 120))
        } else if cpu_pct >= 0.05 {
            (format!("{cpu_pct:.1}%"), Color32::from_rgb(200, 200, 200))
        } else {
            ("–".into(), Color32::from_gray(110))
        }
    }

    /// Célula de CPU com a Disputa ligada: núcleos equivalentes em destaque ("14.2×"), o %
    /// da máquina menor ao lado. É a unidade que faz um processo problemático saltar da lista.
    fn cpu_cell_disputa(ui: &mut egui::Ui, cpu_pct: f32, ncpu: usize, muted_pct: Color32) {
        let (pct_txt, c) = Self::cpu_cell(cpu_pct, ncpu);
        let cores = pressure::cores(cpu_pct, ncpu as u32);
        if cores < 0.05 {
            ui.label(num("–").color(Color32::from_gray(110)));
            return;
        }
        let cores_txt = if cores >= 10.0 {
            format!("{cores:.0}×")
        } else {
            format!("{cores:.1}×")
        };
        // Layout da célula é direita→esquerda: o × fica encostado na borda, o % antes dele.
        ui.label(num(cores_txt).color(c).strong());
        ui.add_space(3.0);
        ui.label(RichText::new(pct_txt).size(10.5).color(muted_pct));
    }

    /// Linhas de memória que não é de processo, para a soma da lista bater com o topo.
    ///
    /// Só aparecem sem busca e sem filtro de categoria — buscar "chrome" não pode devolver
    /// o pool do kernel. Ordenadas por tamanho, junto do resto.
    fn system_rows(&self, search: &str) -> Vec<Row> {
        if !self.cfg.show_kernel_rows
            || !search.is_empty()
            || self.cat_enabled.len() != Category::ALL.len()
        {
            return Vec::new();
        }
        let b = self.breakdown();
        // Sem os pools medidos (macOS, ou GetPerformanceInfo falhando) o "resto" deixaria de
        // ser compartilhado+cache e viraria um saco com o kernel inteiro dentro, rotulado
        // errado. Melhor não mostrar linha nenhuma do que mostrar uma que mente.
        if !b.kernel_ok {
            return Vec::new();
        }
        let mut out = vec![
            (SysRow::PagedPool, b.paged_pool),
            (SysRow::NonPagedPool, b.nonpaged_pool),
            (SysRow::SharedAndCache, b.shared_and_cache),
        ];
        out.retain(|(_, bytes)| *bytes > 0);
        out.sort_by_key(|(_, bytes)| std::cmp::Reverse(*bytes));
        out.into_iter()
            .map(|(kind, bytes)| Row::System { kind, bytes })
            .collect()
    }

    fn build_rows(&mut self) -> Vec<Row> {
        let search = self.search.trim().to_lowercase();
        let hits = self.table_query().matching_pids(&self.search);
        let sys_rows = self.system_rows(&search);
        match self.cfg.view {
            // Térmico, Partida e Telas não desenham tabela de processos — o braço só
            // existe pra exaustividade.
            ViewMode::List
            | ViewMode::Drains
            | ViewMode::Thermal
            | ViewMode::Boot
            | ViewMode::Screens
            | ViewMode::Clean
            | ViewMode::Sweep => {
                let list = self.cfg.view == ViewMode::List;
                // Os addons não desenham esta tabela; nas outras as linhas de sistema
                // ficam no topo, onde o usuário procura "quem está comendo a RAM".
                let mut rows = if list { sys_rows } else { Vec::new() };
                if list && self.cfg.group_apps {
                    rows.extend(self.build_app_rows(hits));
                    return rows;
                }
                self.groups.clear();
                let mut pids = hits;
                self.sort_pids(&mut pids, false);
                rows.extend(pids.into_iter().map(|pid| Row::Proc {
                    pid,
                    depth: 0,
                    has_children: false,
                    expanded: false,
                    dim: false,
                }));
                rows
            }
            ViewMode::Category => {
                let mut groups: HashMap<Category, Vec<u32>> = HashMap::new();
                for pid in hits {
                    groups.entry(self.cat(pid)).or_default().push(pid);
                }
                let mut cats: Vec<(Category, u64, Vec<u32>)> = groups
                    .into_iter()
                    .map(|(c, pids)| {
                        let total = pids
                            .iter()
                            .map(|p| self.proc(*p).map(|x| self.mem_of(x)).unwrap_or(0))
                            .sum();
                        (c, total, pids)
                    })
                    .collect();
                cats.sort_by(|a, b| b.1.cmp(&a.1));
                let mut rows = Vec::new();
                for (cat, total, mut pids) in cats {
                    let collapsed = self.collapsed_cats.contains(&cat);
                    rows.push(Row::CatHeader {
                        cat,
                        count: pids.len(),
                        total,
                        collapsed,
                    });
                    if !collapsed {
                        self.sort_pids(&mut pids, false);
                        for pid in pids {
                            rows.push(Row::Proc {
                                pid,
                                depth: 1,
                                has_children: false,
                                expanded: false,
                                dim: false,
                            });
                        }
                    }
                }
                rows
            }
            ViewMode::Tree => {
                let filtering = !search.is_empty()
                    || self.cat_enabled.len() != Category::ALL.len()
                    || self.cfg.min_mb > 0
                    || self.cfg.min_cpu > 0.0
                    || self.cfg.min_gpu > 0.0
                    || self.cfg.min_vram_mb > 0;
                let hitset: HashSet<u32> = hits.iter().copied().collect();
                let mut visible: HashSet<u32> = hitset.clone();
                let mut auto_expand: HashSet<u32> = HashSet::new();
                for &h in &hits {
                    for a in self.ancestry(h) {
                        visible.insert(a);
                        auto_expand.insert(a);
                    }
                }
                let mut roots: Vec<u32> = visible
                    .iter()
                    .copied()
                    .filter(|pid| {
                        let pp = self.proc(*pid).map(|p| p.ppid).unwrap_or(0);
                        pp == 0 || !visible.contains(&pp)
                    })
                    .collect();
                self.sort_pids(&mut roots, true);
                let mut rows = sys_rows;
                let mut stack: Vec<(u32, u8)> = roots.into_iter().rev().map(|p| (p, 0u8)).collect();
                while let Some((pid, depth)) = stack.pop() {
                    let mut kids: Vec<u32> = self
                        .children
                        .get(&pid)
                        .map(|k| k.iter().copied().filter(|c| visible.contains(c)).collect())
                        .unwrap_or_default();
                    let has_children = !kids.is_empty();
                    let expanded = has_children
                        && (self.expanded.contains(&pid)
                            || (filtering && auto_expand.contains(&pid)));
                    rows.push(Row::Proc {
                        pid,
                        depth,
                        has_children,
                        expanded,
                        dim: filtering && !hitset.contains(&pid),
                    });
                    if expanded && depth < 60 {
                        self.sort_pids(&mut kids, true);
                        for k in kids.into_iter().rev() {
                            stack.push((k, depth + 1));
                        }
                    }
                }
                rows
            }
        }
    }

    /// Linhas da visão Lista agrupadas por app.
    ///
    /// App de um processo só não ganha cabeçalho: uma linha "▶ Bloco de Notas (1)" que
    /// abre em uma linha idêntica é ruído. O agrupamento existe para o caso do Chrome
    /// e das dezenas de Claude/Codex, não para enfeitar o resto da lista.
    fn build_app_rows(&mut self, hits: Vec<u32>) -> Vec<Row> {
        let mut by_key: HashMap<String, Vec<u32>> = HashMap::new();
        for pid in hits {
            let Some(p) = self.proc(pid) else { continue };
            by_key
                .entry(categories::group_key(p))
                .or_default()
                .push(pid);
        }
        let mut groups: Vec<AppGroup> = Vec::with_capacity(by_key.len());
        for (key, mut pids) in by_key {
            self.sort_pids(&mut pids, false);
            let Some(p) = pids.first().and_then(|pid| self.proc(*pid)) else {
                continue;
            };
            let mut best = identity::of(p);
            for pid in &pids {
                if let Some(m) = self.proc(*pid) {
                    let id = identity::of(m);
                    if identity::richness(&id) > identity::richness(&best) {
                        best = id;
                    }
                }
            }
            let name = best.label;
            let mut g = AppGroup {
                key,
                icon_key: p.exe_path.to_lowercase(),
                name_lower: name.to_lowercase(),
                name,
                cat: self.cat(pids[0]),
                pids,
                ram: 0,
                cpu: 0.0,
                gpu: 0.0,
                gpu_known: false,
                vram: 0,
                vram_known: false,
                disk: 0.0,
                oldest: 0,
                has_window: false,
                focused: false,
                leftover: None,
                origin: best.origin.clone().unwrap_or_default(),
            };
            self.fill_group(&mut g);
            if !self.table_query().passes_resources(
                g.ram,
                g.cpu,
                if g.gpu_known { Some(g.gpu) } else { None },
                if g.vram_known { Some(g.vram) } else { None },
            ) {
                continue;
            }
            groups.push(g);
        }

        let (sort, desc) = (self.sort, self.sort_desc);
        let mut order: Vec<usize> = (0..groups.len()).collect();
        order.sort_by(|&a, &b| {
            let (ga, gb) = (&groups[a], &groups[b]);
            let ord = match sort {
                SortKey::Name => ga.name_lower.cmp(&gb.name_lower),
                SortKey::Ram => ga.ram.cmp(&gb.ram),
                SortKey::Cpu => ga
                    .cpu
                    .partial_cmp(&gb.cpu)
                    .unwrap_or(std::cmp::Ordering::Equal),
                SortKey::Gpu => ga
                    .gpu
                    .partial_cmp(&gb.gpu)
                    .unwrap_or(std::cmp::Ordering::Equal),
                SortKey::Vram => ga.vram.cmp(&gb.vram),
                SortKey::State => ga
                    .focused
                    .cmp(&gb.focused)
                    .then(ga.has_window.cmp(&gb.has_window)),
                SortKey::Disk => ga
                    .disk
                    .partial_cmp(&gb.disk)
                    .unwrap_or(std::cmp::Ordering::Equal),
                SortKey::Cat => ga.cat.cmp(&gb.cat).then(gb.ram.cmp(&ga.ram)),
                SortKey::Pid => ga.pids.iter().min().cmp(&gb.pids.iter().min()),
                SortKey::Age => gb.oldest.cmp(&ga.oldest),
                // "Origem" é uma relação entre processos; no nível do app ela não
                // significa nada, então cai no nome em vez de inventar uma ordem.
                SortKey::Parent => ga.name_lower.cmp(&gb.name_lower),
                SortKey::Steal => self
                    .group_steal_rank(ga)
                    .cmp(&self.group_steal_rank(gb))
                    .then(
                        ga.cpu
                            .partial_cmp(&gb.cpu)
                            .unwrap_or(std::cmp::Ordering::Equal),
                    ),
            };
            let ord = if desc { ord.reverse() } else { ord };
            ord.then(ga.key.cmp(&gb.key))
        });

        self.groups = groups;
        let mut rows: Vec<Row> = Vec::with_capacity(self.groups.len() + 8);
        for gi in order {
            let g = &self.groups[gi];
            if g.pids.len() < 2 {
                rows.push(Row::Proc {
                    pid: g.pids[0],
                    depth: 0,
                    has_children: false,
                    expanded: false,
                    dim: false,
                });
                continue;
            }
            rows.push(Row::AppHeader { gi });
            let searching = !self.search.trim().is_empty();
            if searching || self.expanded_apps.contains(&g.key) {
                for &pid in &g.pids {
                    rows.push(Row::Proc {
                        pid,
                        depth: 1,
                        has_children: false,
                        expanded: false,
                        dim: false,
                    });
                }
            }
        }
        rows
    }

    /// Refaz as somas de um grupo a partir do estado atual dos processos.
    fn fill_group(&self, g: &mut AppGroup) {
        let (mut ram, mut cpu, mut disk, mut oldest) = (0u64, 0.0f32, 0.0f64, 0i64);
        let mut gpu_max = 0.0f32;
        let mut gpu_known = false;
        let mut vram = 0u64;
        let mut vram_known = false;
        let mut has_window = false;
        let mut focused = false;
        let mut leftover = None;
        let mut origin = g.origin.clone();
        let mut best = None::<identity::Identity>;
        for &pid in &g.pids {
            let Some(p) = self.proc(pid) else { continue };
            ram += self.mem_of(p);
            cpu += p.cpu_pct;
            disk += p.disk_bps;
            if let Some(load) = p.gpu_load {
                gpu_known = true;
                if load > gpu_max {
                    gpu_max = load;
                }
            }
            if let Some(v) = p.gpu_vram {
                vram_known = true;
                vram += v;
            }
            has_window |= p.has_window;
            focused |= p.focused;
            if oldest == 0 || p.create_time < oldest {
                oldest = p.create_time;
            }
            let id = identity::of(p);
            if best.as_ref().map(|b| identity::richness(b)).unwrap_or(0) < identity::richness(&id) {
                if origin.is_empty() {
                    origin = id.origin.clone().unwrap_or_default();
                }
                best = Some(id);
            }
            if leftover.is_none() {
                leftover = identity::leftover_reason_for(
                    &p.cmdline,
                    p.kernel_state,
                    p.has_window,
                    self.cfg.locale,
                )
                .map(|s| s.to_string());
            }
        }
        g.ram = ram;
        g.cpu = cpu;
        g.gpu = gpu_max;
        g.gpu_known = gpu_known;
        g.vram = vram;
        g.vram_known = vram_known;
        g.disk = disk;
        g.oldest = oldest;
        g.has_window = has_window;
        g.focused = focused;
        g.leftover = leftover;
        g.origin = origin;
        if let Some(id) = best {
            g.name = id.label;
            g.name_lower = g.name.to_lowercase();
        }
    }

    /// Atualiza as somas dos grupos sem mexer na ordem quando chega uma amostra enquanto
    /// o mouse está sobre a tabela. Congelar a ordem é proposital; congelar os números
    /// junto não seria.
    fn refresh_groups(&mut self) {
        if self.groups.is_empty() {
            return;
        }
        let mut gs = std::mem::take(&mut self.groups);
        for g in gs.iter_mut() {
            g.pids.retain(|pid| self.proc(*pid).is_some());
            self.fill_group(g);
        }
        self.groups = gs;
    }

    /// Chave do estado que define a ordenação; se mudar, a ordem é recalculada mesmo congelada.
    fn rows_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (self.cfg.view as u8).hash(&mut h);
        (self.sort as u8).hash(&mut h);
        self.sort_desc.hash(&mut h);
        self.search.trim().to_lowercase().hash(&mut h);
        self.cfg.min_mb.hash(&mut h);
        ((self.cfg.min_cpu * 100.0) as u32).hash(&mut h);
        ((self.cfg.min_gpu * 100.0) as u32).hash(&mut h);
        self.cfg.min_vram_mb.hash(&mut h);
        let mut cats: Vec<u8> = self.cat_enabled.iter().map(|c| *c as u8).collect();
        cats.sort_unstable();
        cats.hash(&mut h);
        let mut ex: Vec<u32> = self.expanded.iter().copied().collect();
        ex.sort_unstable();
        ex.hash(&mut h);
        let mut cc: Vec<u8> = self.collapsed_cats.iter().map(|c| *c as u8).collect();
        cc.sort_unstable();
        cc.hash(&mut h);
        self.cfg.group_apps.hash(&mut h);
        let mut ca: Vec<&String> = self.expanded_apps.iter().collect();
        ca.sort_unstable();
        ca.hash(&mut h);
        h.finish()
    }

    fn rows_for_frame(&mut self, ctx: &egui::Context) -> Vec<Row> {
        let hovering = match (self.table_rect, ctx.pointer_latest_pos()) {
            (Some(r), Some(p)) => r.contains(p) && ctx.input(|i| i.pointer.has_pointer()),
            _ => false,
        };
        let key = self.rows_key();
        match self.row_cache.action(key, hovering) {
            RowCacheAction::Reuse => self.row_cache.rows(),
            RowCacheAction::ReconcileSnapshot => {
                // Keep existing order, but update membership and values once per snapshot.
                let mut keep: HashSet<u32> = self
                    .table_query()
                    .matching_pids(&self.search)
                    .into_iter()
                    .collect();
                let shown_count = keep.len();
                if self.cfg.view == ViewMode::Tree {
                    let hits: Vec<u32> = keep.iter().copied().collect();
                    for h in hits {
                        for a in self.ancestry(h) {
                            keep.insert(a);
                        }
                    }
                }
                self.refresh_groups();
                self.row_cache.reconcile(&keep, shown_count, |gi| {
                    self.groups.get(gi).map(|g| g.pids.as_slice())
                });
                self.row_cache.rows()
            }
            RowCacheAction::Rebuild => {
                let rows = self.build_rows();
                let shown_count = self.table_query().count_matches(&self.search);
                self.row_cache.rebuilt(key, rows.clone(), shown_count);
                rows
            }
        }
    }

    // ---------- ações ----------

    /// Encerra um processo (e, com `tree`, os descendentes dele) na hora.
    ///
    /// Não há caixa de confirmação: o RamDog mostra quanto cada linha come antes do clique,
    /// e um diálogo que se responde no automático não protege ninguém — só atrasa. O lock
    /// é a proteção de verdade, e ele é verificado aqui.
    fn request_kill(&mut self, pid: u32, tree: bool) {
        let Some(p) = self.proc(pid).cloned() else {
            self.toast(
                format!(
                    "PID {pid} {}",
                    self.cfg.locale.text("já tinha saído", "had already exited")
                ),
                false,
            );
            self.after_kill();
            return;
        };
        let mut pids = vec![];
        let mut skipped_locked = 0;
        if self.is_locked(&p) {
            self.toast(
                format!(
                    "{} {}",
                    identity::of(&p).label,
                    self.cfg
                        .locale
                        .text("está protegido (lock)", "is protected (lock)")
                ),
                true,
            );
            return;
        }
        if p.kernel_state == Some('Z') {
            self.request_reap_zombie(&p, tree);
            return;
        }
        pids.push((
            p.pid,
            p.create_time,
            identity::of(&p).label,
            self.mem_of(&p),
        ));
        if tree {
            for d in self.descendants(pid) {
                if let Some(c) = self.proc(d) {
                    if self.is_locked(c) {
                        skipped_locked += 1;
                    } else {
                        pids.push((c.pid, c.create_time, identity::of(c).label, self.mem_of(c)));
                    }
                }
            }
        }
        self.execute_kill(pids, skipped_locked);
    }

    /// Nome de um PID para mensagem: da amostra, do histórico de nomes ou direto do kernel.
    fn name_for(&self, pid: u32) -> String {
        if let Some(p) = self.proc(pid) {
            return identity::of(p).label;
        }
        if let Some(n) = procs::comm_of(pid) {
            return n;
        }
        self.seen_names
            .get(&pid)
            .cloned()
            .unwrap_or_else(|| format!("PID {pid}"))
    }

    /// Quem segura um zumbi agora: pai vivo (nome, pid) ou `None` quando o pai é o init /
    /// já saiu e o recolhimento é automático. Lê o kernel, não a amostra: o zumbi pode ter
    /// sido reparentado depois que o pai original morreu.
    fn zombie_holder(&self, pid: u32, sampled_ppid: u32) -> Option<(String, u32)> {
        let ppid = match procs::live_state(pid) {
            Some(('Z', ppid)) => ppid,
            Some(_) => return None,
            None => return None,
        };
        let ppid = if ppid != 0 { ppid } else { sampled_ppid };
        if ppid <= 1 {
            return None;
        }
        Some((self.name_for(ppid), ppid))
    }

    /// Zombie: o kernel aceita SIGTERM/SIGKILL num processo `Z` (retorna 0) e nada acontece,
    /// porque ele já morreu — o que falta é o pai chamar `wait`. Sem Shift, cutuca o pai com
    /// SIGCHLD e confere; com Shift (ou se o pai já sumiu), finaliza o pai, que é quem segura.
    ///
    /// Em qualquer caso a mensagem diz o essencial: um zumbi não ocupa memória. Quem quer a
    /// linha sumindo tem dois caminhos, os dois nomeados aqui e no painel de detalhes.
    fn request_reap_zombie(&mut self, p: &ProcInfo, kill_parent: bool) {
        let label = identity::of(p).label;
        let sampled_ppid = if p.ppid != 0 { p.ppid } else { p.raw_ppid };
        let Some((pname, ppid)) = self.zombie_holder(p.pid, sampled_ppid) else {
            let msg = if self.cfg.locale == Locale::Portuguese {
                format!(
                    "{label} ({}) é zumbi e o pai já saiu: o init recolhe sozinho em instantes",
                    p.pid
                )
            } else {
                format!(
                    "{label} ({}) is a zombie and its parent exited: init will reap it shortly",
                    p.pid
                )
            };
            self.toast(msg, false);
            self.after_kill();
            return;
        };
        let pname = format!("{pname} ({ppid})");
        if kill_parent {
            let Some(parent) = self.proc(ppid).cloned() else {
                // O pai existe no kernel mas ainda não entrou numa amostra: sinal direto.
                match procs::terminate(ppid) {
                    procs::KillOutcome::Signaled | procs::KillOutcome::AlreadyGone => {
                        let msg = if self.cfg.locale == Locale::Portuguese {
                            format!("pai {pname} finalizado; o zumbi {label} some em instantes")
                        } else {
                            format!(
                                "parent {pname} terminated; zombie {label} will disappear shortly"
                            )
                        };
                        self.toast(msg, false)
                    }
                    procs::KillOutcome::Denied => {
                        let msg = if self.cfg.locale == Locale::Portuguese {
                            format!("sem permissão para finalizar o pai {pname}")
                        } else {
                            format!("permission denied while terminating parent {pname}")
                        };
                        self.toast(msg, true)
                    }
                    other => {
                        let msg = if self.cfg.locale == Locale::Portuguese {
                            format!("falha ao finalizar o pai {pname}: {other:?}")
                        } else {
                            format!("failed to terminate parent {pname}: {other:?}")
                        };
                        self.toast(msg, true)
                    }
                }
                self.after_kill();
                return;
            };
            if is_critical(&parent.name_lower, parent.pid) {
                let msg = if self.cfg.locale == Locale::Portuguese {
                    format!("{label} é zumbi segurado por {pname}, que é crítico do sistema: não dá para finalizar. Não ocupa memória")
                } else {
                    format!("{label} is a zombie held by {pname}, which is system-critical and cannot be terminated. It uses no memory")
                };
                self.toast(msg, true);
                return;
            }
            if self.is_locked(&parent) {
                let msg = if self.cfg.locale == Locale::Portuguese {
                    format!("{label} é zumbi segurado por {pname}, que está protegido (lock). Não ocupa memória; desproteja o pai para finalizar")
                } else {
                    format!("{label} is a zombie held by {pname}, which is protected (lock). It uses no memory; unlock the parent to terminate it")
                };
                self.toast(msg, true);
                return;
            }
            let entry = (
                parent.pid,
                parent.create_time,
                identity::of(&parent).label,
                self.mem_of(&parent),
            );
            self.execute_kill(vec![entry], 0);
            return;
        }
        match procs::nudge_parent(ppid) {
            procs::KillOutcome::Signaled => {}
            procs::KillOutcome::Denied => {
                let msg = if self.cfg.locale == Locale::Portuguese {
                    format!("{label} é zumbi; sem permissão para sinalizar o pai {pname}. Não ocupa memória")
                } else {
                    format!("{label} is a zombie; permission denied while signaling parent {pname}. It uses no memory")
                };
                self.toast(msg, true);
                return;
            }
            _ => {
                let msg = if self.cfg.locale == Locale::Portuguese {
                    format!("{label} é zumbi e o pai {pname} já saiu: o init recolhe em instantes")
                } else {
                    format!(
                        "{label} is a zombie and parent {pname} exited: init will reap it shortly"
                    )
                };
                self.toast(msg, false);
                self.after_kill();
                return;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
        let still = procs::kernel_state(p.pid) == Some('Z');
        self.after_kill();
        if still {
            // O painel de detalhes tem o botão "Finalizar pai"; selecionar aqui poupa o
            // usuário de descobrir o Shift.
            self.selected = Some(p.pid);
            let msg = if self.cfg.locale == Locale::Portuguese {
                format!("{label} ({}) já morreu e não ocupa memória. A linha fica porque {pname} não recolheu: some quando ele recolher ou for finalizado (Shift+✖ ou o botão no painel)", p.pid)
            } else {
                format!("{label} ({}) is already dead and uses no memory. Its row remains because {pname} has not reaped it; it disappears when the parent reaps it or is terminated (Shift+✖ or the button in the details panel)", p.pid)
            };
            self.toast(msg, true);
        } else {
            let msg = if self.cfg.locale == Locale::Portuguese {
                format!("zumbi {label} ({}) recolhido por {pname}", p.pid)
            } else {
                format!("zombie {label} ({}) reaped by {pname}", p.pid)
            };
            self.toast(msg, false);
        }
    }

    /// Encerra todos os processos de um app agrupado.
    ///
    /// Ao contrário do "finalizar árvore", aqui não há relação de parentesco: são os
    /// processos que compartilham o executável, que é o que a linha mostra somado. Um
    /// processo protegido (lock) não entra na lista e nem impede o resto — mas se *todos*
    /// forem protegidos a ação vira um aviso, não um kill silenciosamente vazio.
    fn request_kill_app(&mut self, gi: usize) {
        let Some(g) = self.groups.get(gi) else { return };
        let (name, count) = (g.name.clone(), g.pids.len());
        let mut pids = Vec::new();
        let mut skipped = 0;
        for &pid in &g.pids {
            match self.proc(pid) {
                Some(p) if self.is_locked(p) => skipped += 1,
                Some(p) => pids.push((p.pid, p.create_time, identity::of(p).label, self.mem_of(p))),
                None => {}
            }
        }
        if pids.is_empty() {
            let msg = if self.cfg.locale == Locale::Portuguese {
                format!("{name}: todos os {count} processos estão protegidos (lock)")
            } else {
                format!("{name}: all {count} processes are protected (lock)")
            };
            self.toast(msg, true);
            return;
        }
        self.execute_kill(pids, skipped);
    }

    /// Mata a lista e resume o estrago na barra de status. `skipped_locked` são os que o
    /// lock poupou pelo caminho — dizer só "3 finalizados" quando eram 5 esconde o motivo.
    ///
    /// Depois do SIGKILL espera o kernel recolher. Quem vira `Z` nesse meio tempo morreu de
    /// verdade, mas o pai não chamou `wait`: o RamDog cutuca o pai (SIGCHLD) e, se a linha
    /// continuar, diz quem está segurando. Antes isso era "1 finalizado" seguido de uma linha
    /// que não saía nunca, e o clique seguinte caía na mensagem de zumbi sem contexto.
    fn execute_kill(&mut self, pids: Vec<(u32, i64, String, u64)>, skipped_locked: usize) {
        let english = self.cfg.locale == Locale::English;
        let mut signaled = 0;
        let mut gone = 0;
        let mut freed = 0u64;
        let mut errs: Vec<String> = Vec::new();
        let mut pending = Vec::new();
        // (pid, nome, ppid amostrado) de quem já era zumbi antes do clique: sinal nele não
        // faz nada, então nem tenta — vai direto para o acerto com o pai.
        let mut zombies: Vec<(u32, String, u32)> = Vec::new();
        for (pid, created, name, ram) in &pids {
            if !pid_still_same(*pid, *created) {
                gone += 1;
                continue;
            }
            if let Some(('Z', ppid)) = procs::live_state(*pid) {
                zombies.push((*pid, name.clone(), ppid));
                continue;
            }
            match procs::terminate(*pid) {
                procs::KillOutcome::Signaled => pending.push((*pid, *created, name.clone(), *ram)),
                procs::KillOutcome::AlreadyGone => gone += 1,
                procs::KillOutcome::Denied => errs.push(if english {
                    format!("{name} ({pid}): access denied")
                } else {
                    format!("{name} ({pid}): acesso negado")
                }),
                procs::KillOutcome::Invalid => errs.push(if english {
                    format!("{name} ({pid}): invalid PID")
                } else {
                    format!("{name} ({pid}): PID inválido")
                }),
                procs::KillOutcome::Failed(e) => errs.push(format!("{name} ({pid}): {e}")),
            }
        }
        if !pending.is_empty() {
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
        let mut killed: Vec<(u32, String, u32)> = Vec::new();
        for (pid, created, name, ram) in pending {
            if !pid_still_same(pid, created) {
                signaled += 1;
                freed += ram;
                continue;
            }
            match procs::kill(pid) {
                procs::KillOutcome::Signaled | procs::KillOutcome::AlreadyGone => {
                    signaled += 1;
                    freed += ram;
                    killed.push((pid, name, 0));
                }
                procs::KillOutcome::Denied => errs.push(if english {
                    format!("{name} ({pid}): access denied")
                } else {
                    format!("{name} ({pid}): acesso negado")
                }),
                procs::KillOutcome::Invalid => errs.push(if english {
                    format!("{name} ({pid}): invalid PID")
                } else {
                    format!("{name} ({pid}): PID inválido")
                }),
                procs::KillOutcome::Failed(e) => errs.push(format!("{name} ({pid}): {e}")),
            }
        }
        killed.extend(zombies.iter().cloned());
        let held = self.settle_zombies(&killed);
        self.after_kill();
        let poupados = if skipped_locked > 0 {
            if english {
                format!(", {skipped_locked} protected process(es) skipped")
            } else {
                format!(", {skipped_locked} protegido(s) poupado(s)")
            }
        } else {
            String::new()
        };
        let mut parts = Vec::new();
        if signaled > 0 {
            parts.push(if english {
                format!("{signaled} terminated, ~{} freed", fmt_bytes(freed))
            } else {
                format!("{signaled} finalizado(s), ~{} liberados", fmt_bytes(freed))
            });
        }
        if gone > 0 {
            parts.push(if english {
                format!("{gone} had already exited")
            } else {
                format!("{gone} já tinham saído")
            });
        }
        // Zumbis que já eram zumbis e o pai recolheu no cutucão: contam como resolvidos.
        let reaped = zombies
            .iter()
            .filter(|(pid, _, _)| !held.iter().any(|h| h.0 == *pid))
            .count();
        if reaped > 0 {
            parts.push(if english {
                format!("{reaped} zombie(s) reaped by the parent")
            } else {
                format!("{reaped} zumbi(s) recolhido(s) pelo pai")
            });
        }
        if parts.is_empty() && errs.is_empty() && held.is_empty() {
            parts.push(
                self.cfg
                    .locale
                    .text("nada a encerrar", "nothing to terminate")
                    .into(),
            );
        }
        let mut msg = parts.join(", ");
        let mut warn = false;
        if !held.is_empty() {
            warn = true;
            if !msg.is_empty() {
                msg.push_str("; ");
            }
            msg.push_str(&self.held_zombies_note(&held));
        }
        if !errs.is_empty() {
            warn = true;
            if !msg.is_empty() {
                msg.push_str("; ");
            }
            msg.push_str(&format!(
                "{} {}: {}",
                errs.len(),
                self.cfg.locale.text("falha(s)", "failure(s)"),
                errs[0]
            ));
            if errs.len() > 1 {
                msg.push_str(&format!(" (+{})", errs.len() - 1));
            }
        }
        msg.push_str(&poupados);
        self.toast(msg, warn);
    }

    /// Espera até ~400 ms os PIDs recém-mortos sumirem. Quem aparece como `Z` recebe um
    /// SIGCHLD no pai (uma vez por pai); devolve quem continuou zumbi: (pid, nome, ppid).
    fn settle_zombies(&self, killed: &[(u32, String, u32)]) -> Vec<(u32, String, u32)> {
        if killed.is_empty() {
            return Vec::new();
        }
        let deadline = Instant::now() + std::time::Duration::from_millis(400);
        let mut nudged: HashSet<u32> = HashSet::new();
        let mut held = Vec::new();
        loop {
            held.clear();
            let mut settling = false;
            for (pid, name, sampled_ppid) in killed {
                match procs::live_state(*pid) {
                    None => {}
                    Some(('Z', ppid)) => {
                        let ppid = if ppid != 0 { ppid } else { *sampled_ppid };
                        if ppid > 1 && nudged.insert(ppid) {
                            let _ = procs::nudge_parent(ppid);
                        }
                        // Pai é o init (ou sumiu): recolhe sozinho, não é caso para avisar.
                        if ppid > 1 {
                            held.push((*pid, name.clone(), ppid));
                        }
                        settling = true;
                    }
                    // SIGKILL entregue mas ainda saindo (memória grande, estado D): espera.
                    Some(_) => settling = true,
                }
            }
            if !settling || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
        held
    }

    /// Frase da barra de status para zumbis que o pai não recolheu, agrupada por pai.
    fn held_zombies_note(&self, held: &[(u32, String, u32)]) -> String {
        let english = self.cfg.locale == Locale::English;
        let mut by_parent: Vec<(u32, Vec<&(u32, String, u32)>)> = Vec::new();
        for h in held {
            match by_parent.iter_mut().find(|(pp, _)| *pp == h.2) {
                Some((_, list)) => list.push(h),
                None => by_parent.push((h.2, vec![h])),
            }
        }
        let mut parts = Vec::new();
        for (ppid, list) in by_parent {
            let pname = self.name_for(ppid);
            let who = if list.len() == 1 {
                format!("{} ({})", list[0].1, list[0].0)
            } else if english {
                format!("{} processes ({})", list.len(), list[0].1)
            } else {
                format!("{} processos ({})", list.len(), list[0].1)
            };
            parts.push(if english {
                format!("{who} exited but remained a zombie: {pname} ({ppid}) did not reap it")
            } else {
                format!("{who} morreu mas ficou como zumbi: {pname} ({ppid}) não recolheu")
            });
        }
        if english {
            format!("{}. Zombies use no memory; they disappear when the parent reaps them or is terminated (select the row: the details panel has the button)", parts.join("; "))
        } else {
            format!("{}. Zumbi não ocupa memória; some quando o pai recolher ou for finalizado (selecione a linha: o painel tem o botão)", parts.join("; "))
        }
    }

    fn after_kill(&mut self) {
        self.sampler.force.store(true, Ordering::Relaxed);
        self.row_cache.clear();
    }

    fn toggle_lock(&mut self, name_lower: &str) {
        if self.cfg.locked.contains(name_lower) {
            self.cfg.locked.remove(name_lower);
        } else {
            self.cfg.locked.insert(name_lower.to_string());
        }
        self.cfg_dirty = true;
    }

    fn set_override(&mut self, name_lower: &str, cat: Option<Category>) {
        match cat {
            Some(c) => {
                self.cfg.overrides.insert(name_lower.to_string(), c);
            }
            None => {
                self.cfg.overrides.remove(name_lower);
            }
        }
        self.cfg_dirty = true;
        self.rebuild_indexes();
        self.derived_dirty = true;
        self.row_cache.invalidate();
    }

    fn toast(&mut self, msg: String, err: bool) {
        self.status = Some((Instant::now(), msg, err));
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    fn relaunch_as_admin(&mut self) {
        #[cfg(windows)]
        {
            use windows::core::{w, PCWSTR};
            use windows::Win32::UI::Shell::ShellExecuteW;
            use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
            let exe = std::env::current_exe()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            let wide: Vec<u16> = exe.encode_utf16().chain(std::iter::once(0)).collect();
            let r = unsafe {
                ShellExecuteW(
                    None,
                    w!("runas"),
                    PCWSTR(wide.as_ptr()),
                    PCWSTR::null(),
                    PCWSTR::null(),
                    SW_SHOWNORMAL,
                )
            };
            if r.0 as usize > 32 {
                std::process::exit(0);
            } else {
                self.toast(
                    self.cfg
                        .locale
                        .text(
                            "Elevação cancelada ou falhou",
                            "Elevation was cancelled or failed",
                        )
                        .into(),
                    true,
                );
            }
        }
        #[cfg(target_os = "macos")]
        {
            self.toast(self.cfg.locale.text("No macOS, rode o RamDog com sudo se precisar matar processos de outros usuários.", "On macOS, run RamDog with sudo to terminate other users' processes.").into(), true);
        }
        #[cfg(target_os = "linux")]
        {
            self.toast(self.cfg.locale.text("No Linux, rode o RamDog com sudo se precisar matar processos de outros usuários.", "On Linux, run RamDog with sudo to terminate other users' processes.").into(), true);
        }
    }

    // ---------- UI ----------

    /// Totais por categoria na métrica escolhida — alimenta os chips de filtro, que precisam
    /// bater com o que a coluna RAM mostra em cada linha.
    fn cat_totals(&self) -> HashMap<Category, (u64, usize)> {
        self.derived.cat_totals.clone()
    }

    /// Totais por categoria numa métrica específica. O medidor do topo pede sempre
    /// `Private`, porque lá as faixas precisam caber dentro do "em uso" — com working set a
    /// soma das categorias passa da largura da barra.
    fn calculate_cat_totals(&self, m: MemMetric) -> HashMap<Category, (u64, usize)> {
        let mut totals: HashMap<Category, (u64, usize)> = HashMap::new();
        for p in &self.procs {
            let e = totals.entry(self.cat(p.pid)).or_default();
            e.0 += Self::metric_of(m, p);
            e.1 += 1;
        }
        totals
    }

    /// Medidor empilhado: mostra *para onde* foi a RAM, não só quanto sobrou.
    ///
    /// As faixas coloridas são as categorias de processo (mesma cor dos chips), depois vêm os
    /// dois pools do kernel e, por último, o resto compartilhado/cache. Por construção as
    /// faixas somam exatamente o "em uso" — antes tudo que não fosse privado de processo
    /// virava um único bloco cinza de 70% da barra, e o medidor não explicava nada.
    ///
    /// Aqui é sempre memória privada, mesmo quando a coluna RAM está em working set: as
    /// faixas precisam caber dentro do total, e o working set conta página compartilhada
    /// uma vez por processo que a mapeia.
    fn ram_gauge(&self, ui: &mut egui::Ui, width: f32) {
        #[cfg(target_os = "linux")]
        if cfg!(target_os = "linux") {
            let used = self.mem.used_phys();
            let total = self.mem.total_phys.max(1);
            let mut tip = format!(
                "{} {} {} (MemTotal − MemAvailable).\n{}",
                fmt_gb(used),
                self.cfg.locale.text("em uso de", "used of"),
                fmt_gb(total),
                self.linux_memory_summary()
            );
            if let Some((committed, limit)) = self.mem.linux_commit {
                if self.cfg.locale == Locale::Portuguese {
                    tip.push_str(&format!("\nCommit global: {} / {} (Committed_AS / CommitLimit). Pode exceder o limite conforme a política de overcommit.", fmt_gb(committed), fmt_gb(limit)));
                } else {
                    tip.push_str(&format!("\nGlobal commit: {} / {} (Committed_AS / CommitLimit). It may exceed the limit depending on the overcommit policy.", fmt_gb(committed), fmt_gb(limit)));
                }
            } else {
                tip.push_str(self.cfg.locale.text(
                    "\nCommit global: indisponível.",
                    "\nGlobal commit: unavailable.",
                ));
            }
            Self::meter_bar(ui, width, Some(used as f32 / total as f32 * 100.0), tip);
            return;
        }
        let b = self.breakdown();
        let total = self.mem.total_phys.max(1);
        let cats = &self.derived.private_cat_totals;
        let (rect, resp) =
            ui.allocate_exact_size(Vec2::new(width, TOP_BAR_H), egui::Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, 3.0, Color32::from_rgb(30, 34, 41));
        let scale = rect.width() / total as f32;
        let mut segs: Vec<(Category, u64)> = cats
            .iter()
            .map(|(c, (t, _))| (*c, *t))
            .filter(|(_, t)| *t > 0)
            .collect();
        segs.sort_by_key(|(_, t)| std::cmp::Reverse(*t));
        let mut x = rect.left();
        let band = |x: &mut f32, bytes: u64, color: Color32| {
            let w = bytes as f32 * scale;
            if w < 0.5 {
                return;
            }
            let right = (*x + w).min(rect.right());
            let seg = Rect::from_min_max(
                egui::pos2(*x, rect.top() + 1.0),
                egui::pos2(right, rect.bottom() - 1.0),
            );
            p.rect_filled(seg, 0.0, color);
            *x = right;
        };
        for (c, t) in &segs {
            band(&mut x, *t, c.color().gamma_multiply(0.85));
        }
        band(&mut x, b.paged_pool, SysRow::PagedPool.color());
        band(&mut x, b.nonpaged_pool, SysRow::NonPagedPool.color());
        band(&mut x, b.shared_and_cache, SysRow::SharedAndCache.color());
        p.rect_stroke(rect, 3.0, Stroke::new(1.0_f32, LINE), StrokeKind::Inside);
        // O compromisso saía numa linha extra embaixo da barra — era a única linha que só a
        // RAM tinha, e era ela que desalinhava a fileira inteira dos medidores. Vive aqui.
        let mut tip = if self.cfg.locale == Locale::Portuguese {
            format!(
                "{} em uso de {}\ncompromisso {} / {} (RAM + arquivo de paginação)\n",
                fmt_gb(b.used),
                fmt_gb(total),
                fmt_gb(self.mem.used_commit()),
                fmt_gb(self.mem.total_commit)
            )
        } else {
            format!(
                "{} used of {}\ncommit {} / {} (RAM + paging file)\n",
                fmt_gb(b.used),
                fmt_gb(total),
                fmt_gb(self.mem.used_commit()),
                fmt_gb(self.mem.total_commit)
            )
        };
        for (c, t) in &segs {
            tip.push_str(&format!(
                "\n{}  {}",
                c.label_for(self.cfg.locale),
                fmt_bytes_short(*t)
            ));
        }
        tip.push_str(&format!(
            "\n\n{}  {}",
            self.cfg
                .locale
                .text("Processos (privado)", "Processes (private)"),
            fmt_bytes_short(b.private)
        ));
        if b.kernel_ok {
            tip.push_str(&format!(
                "\n{}  {}",
                SysRow::PagedPool.label_for(self.cfg.locale),
                fmt_bytes_short(b.paged_pool)
            ));
            tip.push_str(&format!(
                "\n{}  {}",
                SysRow::NonPagedPool.label_for(self.cfg.locale),
                fmt_bytes_short(b.nonpaged_pool)
            ));
        }
        tip.push_str(&format!(
            "\n{}  {}",
            SysRow::SharedAndCache.label_for(self.cfg.locale),
            fmt_bytes_short(b.shared_and_cache)
        ));
        if !self.hwtemp.dimm_temps.is_empty() {
            tip.push_str(
                self.cfg
                    .locale
                    .text("\n\nTemperatura por pente:", "\n\nTemperature by module:"),
            );
            for (i, t) in self.hwtemp.dimm_temps.iter().enumerate() {
                tip.push_str(&format!("\nDIMM #{i}  {t:.1}°C"));
            }
        }
        resp.on_hover_text(tip);
    }

    /// Cor por faixa de uso — mesmos limiares em todo o app (CPU, GPU, disco).
    fn load_color(frac: f32) -> Color32 {
        if frac > 0.9 {
            Color32::from_rgb(222, 92, 84)
        } else if frac > 0.75 {
            Color32::from_rgb(226, 166, 72)
        } else {
            Color32::from_rgb(92, 178, 122)
        }
    }

    /// Cor por temperatura — verde/amarelo/vermelho calibrados para GPU (a única com sensor
    /// exposto neste host); CPU usaria os mesmos limiares se um dia ganhar leitura.
    fn temp_color(c: u32) -> Color32 {
        if c >= 85 {
            Color32::from_rgb(222, 92, 84)
        } else if c >= 70 {
            Color32::from_rgb(226, 166, 72)
        } else {
            Color32::from_rgb(92, 178, 122)
        }
    }

    /// Percentual com resolução variável: abaixo de 10% mostra uma casa decimal, porque um
    /// medidor que só sabe dizer "0%" quando a carga real oscila em 0,3–2% parece travado —
    /// mesmo funcionando certo. Acima de 10% a casa decimal só seria ruído.
    fn fmt_pct(p: f32) -> String {
        if p < 10.0 {
            format!("{:.1}%", p)
        } else {
            format!("{:.0}%", p)
        }
    }

    fn meter_bar(
        ui: &mut egui::Ui,
        width: f32,
        pct: Option<f32>,
        tip: impl Into<egui::WidgetText>,
    ) -> egui::Response {
        let (rect, resp) =
            ui.allocate_exact_size(Vec2::new(width, TOP_BAR_H), egui::Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, 2.0, Color32::from_rgb(30, 34, 41));
        if let Some(pct) = pct {
            let frac = (pct / 100.0).clamp(0.0, 1.0);
            if frac > 0.01 {
                let bar =
                    Rect::from_min_size(rect.min, Vec2::new(rect.width() * frac, rect.height()));
                p.rect_filled(bar, 2.0, Self::load_color(frac));
            }
        }
        resp.on_hover_text(tip)
    }

    // ---------- modo mini ----------

    /// Aplica na janela o modo atual: decoração, tamanho, limite mínimo e always-on-top.
    /// Só roda quando `cfg.mini` diverge do que já foi aplicado.
    fn apply_window_mode(&mut self, ctx: &egui::Context) {
        use egui::ViewportCommand as Vc;
        if self.cfg.mini {
            if self.full_size.is_none() {
                self.full_size = ctx.input(|i| i.viewport().inner_rect).map(|r| r.size());
            }
            ctx.send_viewport_cmd(Vc::Decorations(false));
            ctx.send_viewport_cmd(Vc::Resizable(false));
            // O mínimo antigo (760x420) barraria o InnerSize do HUD — tem que cair antes.
            ctx.send_viewport_cmd(Vc::MinInnerSize(Vec2::new(MINI_W, MINI_H)));
            ctx.send_viewport_cmd(Vc::InnerSize(Vec2::new(MINI_W, MINI_H)));
            self.apply_on_top(ctx);
        } else {
            ctx.send_viewport_cmd(Vc::WindowLevel(egui::WindowLevel::Normal));
            ctx.send_viewport_cmd(Vc::Decorations(true));
            ctx.send_viewport_cmd(Vc::Resizable(true));
            // An instance launched in Mini has an explicit maximum in its viewport.
            // Resizable alone does not clear that startup constraint on X11.
            ctx.send_viewport_cmd(Vc::MaxInnerSize(Vec2::INFINITY));
            ctx.send_viewport_cmd(Vc::MinInnerSize(Vec2::new(FULL_MIN_W, FULL_MIN_H)));
            let size = self.full_size.take().unwrap_or(Vec2::new(1180.0, 760.0));
            ctx.send_viewport_cmd(Vc::InnerSize(size.max(Vec2::new(FULL_MIN_W, FULL_MIN_H))));
        }
        self.applied_mini = self.cfg.mini;
    }

    fn apply_on_top(&self, ctx: &egui::Context) {
        let level = if self.cfg.mini_on_top {
            egui::WindowLevel::AlwaysOnTop
        } else {
            egui::WindowLevel::Normal
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
    }

    fn set_mini(&mut self, on: bool) {
        if self.cfg.mini != on {
            self.cfg.mini = on;
            self.cfg_dirty = true;
        }
    }

    /// HUD compacto: CPU, RAM, GPU e disco em 2x2, com temperatura ao lado de cada um.
    /// Sem lista, sem detalhes — é a resposta de relance a "o que está pesando agora".
    fn ui_mini(&mut self, ctx: &egui::Context) {
        let locale = self.cfg.locale;
        let frame = egui::Frame::new()
            .fill(BG)
            .stroke(Stroke::new(1.0_f32, LINE))
            .inner_margin(egui::Margin::symmetric(6, 5));
        egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
            // Sem decoração não há barra de título: arrastar qualquer parte vazia move a
            // janela. A interação vem antes dos widgets para os botões ganharem o clique.
            let bg = ui.interact(
                ui.max_rect(),
                ui.id().with("mini_drag"),
                egui::Sense::click_and_drag(),
            );
            if bg.drag_started() {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if bg.double_clicked() {
                self.set_mini(false);
            }
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
            self.mini_controls(ui);

            let w = ((ui.available_width() - 6.0) / 2.0).max(80.0);
            let cpu_pct = self.sys.cpu_pct;
            let cpu_sub = self
                .sys
                .load1
                .map(|l| format!("{} {}", locale.text("carga", "load"), pt_num(l as f64, 1)))
                .unwrap_or_default();
            ui.horizontal(|ui| {
                Self::meter_tile(ui, w, "CPU", cpu_pct, self.cpu_temp(), &cpu_sub, |ui, w| {
                    Self::meter_bar(ui, w, cpu_pct, locale.text("Uso de CPU", "CPU usage"));
                });
                let used = self.mem.used_phys();
                let total = self.mem.total_phys.max(1);
                let sub = if self.mem.swap_used >= GB {
                    format!("{} / {} +swap", fmt_gb(used), fmt_gb(total))
                } else {
                    format!("{} / {}", fmt_gb(used), fmt_gb(total))
                };
                Self::meter_tile(
                    ui,
                    w,
                    "RAM",
                    Some(used as f32 / total as f32 * 100.0),
                    self.ram_temp(),
                    &sub,
                    |ui, w| self.ram_gauge(ui, w),
                );
            });
            ui.horizontal(|ui| {
                let gpu = self.sys.gpu.clone();
                let (gpu_pct, gpu_temp, gpu_sub, gpu_tip) = match &gpu {
                    Some(g) => (
                        g.util_pct,
                        match g.temp_c {
                            Some(t) => Temp::C(t),
                            None => Temp::Missing(
                                locale
                                    .text(
                                        "O driver não reportou temperatura desta GPU.",
                                        "The driver did not report a temperature for this GPU.",
                                    )
                                    .into(),
                            ),
                        },
                        if g.mem_total > 0 {
                            format!("{} / {}", fmt_gb(g.mem_used), fmt_gb(g.mem_total))
                        } else {
                            String::new()
                        },
                        g.name.clone(),
                    ),
                    None => (
                        None,
                        Temp::Missing(
                            locale
                                .text(
                                    "Sem leitura de GPU: leitura de GPU indisponível.",
                                    "No GPU reading: GPU telemetry is unavailable.",
                                )
                                .into(),
                        ),
                        String::new(),
                        locale
                            .text(
                                "Sem leitura de GPU neste host",
                                "No GPU reading on this host",
                            )
                            .to_string(),
                    ),
                };
                Self::meter_tile(ui, w, "GPU", gpu_pct, gpu_temp, &gpu_sub, |ui, w| {
                    Self::meter_bar(ui, w, gpu_pct, gpu_tip);
                });
                let disk_pct = self.sys.disk_pct;
                let disk_sub = self
                    .sys
                    .disk_bps
                    .filter(|bps| *bps >= 1024.0)
                    .map(fmt_bps)
                    .unwrap_or_default();
                Self::meter_tile(
                    ui,
                    w,
                    locale.text("DISCO", "DISK"),
                    disk_pct,
                    Temp::None,
                    &disk_sub,
                    |ui, w| {
                        Self::meter_bar(ui, w, disk_pct, disk_usage_tip(locale));
                    },
                );
            });
            self.mini_fans(ui);
        });
    }

    /// Se o ESTABILIZAR está ligado, do ponto de vista da tela: o que o helper reportou,
    /// ou o que o usuário acabou de pedir enquanto a confirmação não chega.
    fn stab_on(&mut self) -> bool {
        let reported = self.hwtemp.stab.on;
        match self.stab_pending {
            Some((want, at)) if reported != want && at.elapsed().as_secs_f32() <= 3.0 => want,
            Some(_) => {
                self.stab_pending = None;
                reported
            }
            None => reported,
        }
    }

    /// Liga/desliga a curva do helper. A tela vira na hora; o hardware leva o tempo dele.
    /// Também força uma amostra imediata para a confirmação real chegar o quanto antes.
    fn toggle_stab(&mut self) {
        let want = !self.stab_on();
        if let Some(c) = &self.sampler.hw_cmd {
            c.send(if want { "stab on" } else { "stab off" });
            self.thermal_edit.clear();
            self.stab_pending = Some((want, Instant::now()));
            self.sampler.force.store(true, Ordering::Relaxed);
        }
    }

    /// Temperatura da CPU para os medidores — e o motivo exato quando ela não vem.
    /// Um "–°C" que explica no hover é a diferença entre "está frio" e "eu não sei ler".
    fn cpu_temp(&self) -> Temp {
        match self.hwtemp.cpu_temp {
            Some(t) => Temp::C(t.round() as u32),
            None if cfg!(target_os = "macos") => {
                Temp::Missing(self.cfg.locale.text("Temperatura de CPU no macOS ainda não está ligada.", "CPU temperature on macOS is not wired up yet.").into())
            }
            None if cfg!(target_os = "linux") => {
                Temp::Missing(self.cfg.locale.text("Sem sensor de CPU em /sys/class/hwmon (coretemp, k10temp ou zenpower).", "No CPU sensor in /sys/class/hwmon (coretemp, k10temp, or zenpower).").into())
            }
            None if !self.is_admin => {
                Temp::Missing(self.cfg.locale.text("Temperatura de CPU precisa de admin: o sensor Tctl só responde por driver de hardware. Volte ao modo completo e use ⬆ Admin.", "CPU temperature requires admin rights: the Tctl sensor is exposed by a hardware driver. Return to full view and use ⬆ Admin.").into())
            }
            None => Temp::Missing(
                self.cfg.locale.text("Temperatura indisponível: hwtemp.exe não está ao lado do ramdog.exe, ou a placa-mãe não tem sensor suportado.", "Temperature unavailable: hwtemp.exe is not beside ramdog.exe, or the motherboard has no supported sensor.").into(),
            ),
        }
    }

    fn ram_temp(&self) -> Temp {
        match self.hwtemp.ram_max() {
            Some(t) => Temp::C(t.round() as u32),
            None if cfg!(target_os = "linux") => {
                Temp::Missing(self.cfg.locale.text("Nenhum pente expõe sensor no hwmon (spd5118/jc42). Sem isso o RamDog não inventa °C.", "No memory module exposes a sensor in hwmon (spd5118/jc42). RamDog will not invent a temperature.").into())
            }
            None if cfg!(target_os = "macos") => {
                Temp::Missing(self.cfg.locale.text("Temperatura de RAM no macOS ainda não está ligada.", "RAM temperature on macOS is not wired up yet.").into())
            }
            None if !self.is_admin => Temp::Missing(self.cfg.locale.text("Temperatura dos pentes precisa de admin (leitura SMBus).", "Memory temperature requires admin rights (SMBus reading).").into()),
            None => Temp::Missing(self.cfg.locale.text("Nenhum pente desta máquina expõe sensor de temperatura.", "No memory module on this machine exposes a temperature sensor.").into()),
        }
    }

    /// Faixa de fans do HUD: o mesmo ESTABILIZAR da visão Térmico em um botão só, com os RPM
    /// de leve ao lado. Sem fans (sem admin ou sem helper) o botão fica desabilitado e diz o
    /// motivo — some da tela seria mentir que o controle não existe.
    fn mini_fans(&mut self, ui: &mut egui::Ui) {
        let held = self.hwtemp.stab.held;
        let stab_on = self.stab_on();
        let has_fans = !self.hwtemp.fans.is_empty();
        let (label, bg, fg) = if !has_fans {
            (
                self.cfg.locale.text("ESTABILIZAR", "STABILIZE").to_owned(),
                SURFACE,
                MUTED,
            )
        } else if !stab_on {
            (
                self.cfg.locale.text("ESTABILIZAR", "STABILIZE").to_owned(),
                ACCENT_BG,
                ACCENT,
            )
        } else if held > 50.5 {
            (format!("FANS {held:.0}%"), THERM_WARN_BG, THERM_WARN_FG)
        } else {
            ("FANS 50%".to_owned(), THERM_STAB_BG, THERM_STAB_FG)
        };
        let tip = if !has_fans && cfg!(target_os = "linux") {
            self.cfg.locale.text(
                "Abra Térmico e ative o controle de ventoinhas com autenticação.",
                "Open Thermal and enable authenticated fan control.",
            )
        } else if !has_fans {
            self.cfg.locale.text("Controle de fans indisponível: precisa de admin e do hwtemp.exe ao lado do ramdog.exe.", "Fan control is unavailable: it needs admin rights and hwtemp.exe beside ramdog.exe.")
        } else if stab_on {
            self.cfg.locale.text(
                "Curva do TempHUD ligada. Clique para devolver os fans à BIOS.",
                "TempHUD curve is enabled. Click to return fans to BIOS.",
            )
        } else {
            self.cfg.locale.text("Trava os fans SuperIO em 50% e sobe em rampa a partir de 80°C (100% aos 92°C). Clicar de novo devolve à BIOS.", "Locks SuperIO fans at 50% and ramps from 80°C (100% at 92°C). Click again to return to BIOS.")
        };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 7.0;
            let btn = egui::Button::new(RichText::new(label).strong().size(11.0).color(fg))
                .fill(bg)
                .stroke(Stroke::new(1.0_f32, fg.gamma_multiply(0.6)))
                .corner_radius(4.0)
                .min_size(Vec2::new(108.0, 20.0));
            if ui.add_enabled(has_fans, btn).on_hover_text(tip).clicked() {
                self.toggle_stab();
            }
            // RPM discreto: só os que giram, no máximo quatro. O detalhe completo (nome e %
            // de cada fan) fica no hover — a faixa é para olhar de canto de olho.
            let spinning: Vec<&crate::hwtemp::FanRow> = self
                .hwtemp
                .fans
                .iter()
                .filter(|f| f.rpm.unwrap_or(0.0) > 0.0)
                .collect();
            if spinning.is_empty() {
                if has_fans {
                    ui.label(
                        RichText::new(self.cfg.locale.text("fans parados", "fans idle"))
                            .color(MUTED)
                            .size(10.5),
                    );
                }
                return;
            }
            let shown: Vec<String> = spinning
                .iter()
                .take(4)
                .map(|f| format!("{:.0}", f.rpm.unwrap_or(0.0)))
                .collect();
            let mut detail = String::new();
            for f in &spinning {
                let pct = f
                    .pct
                    .map(|p| format!("{p:.0}%"))
                    .unwrap_or_else(|| "–".into());
                let mode = if f.guard {
                    self.cfg.locale.text(" (proteção)", " (protected)")
                } else if f.auto {
                    " (BIOS)"
                } else {
                    ""
                };
                detail.push_str(&format!(
                    "{}  {}  {:.0} rpm{}\n",
                    f.name,
                    pct,
                    f.rpm.unwrap_or(0.0),
                    mode
                ));
            }
            ui.label(
                RichText::new(format!("{} rpm", shown.join(" · ")))
                    .color(MUTED)
                    .size(10.5),
            )
            .on_hover_text(detail.trim_end().to_string());
        });
    }

    /// Faixa de controles do HUD. Sem ComboBox de propósito: o popup estouraria uma janela
    /// de 330x140 e apareceria cortado — o ritmo cicla no clique.
    fn mini_controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.spacing_mut().button_padding = Vec2::new(5.0, 1.0);
            ui.label(RichText::new("RamDog").strong().size(11.5).color(MUTED))
                .on_hover_text(self.cfg.locale.text("Arraste para mover", "Drag to move"));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .small_button("✕")
                    .on_hover_text(self.cfg.locale.text("Fechar", "Close"))
                    .clicked()
                {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if ui
                    .small_button("⤢")
                    .on_hover_text(self.cfg.locale.text(
                        "Voltar ao completo (ou duplo clique)",
                        "Return to full view (or double-click)",
                    ))
                    .clicked()
                {
                    self.set_mini(false);
                }
                // Sem decoração não há botão de minimizar do Windows — o HUD precisa do
                // seu. Volta pela barra de tarefas, como qualquer janela.
                if ui
                    .small_button("–")
                    .on_hover_text(self.cfg.locale.text("Minimizar", "Minimize"))
                    .clicked()
                {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }
                let mut on_top = self.cfg.mini_on_top;
                if ui
                    .selectable_label(
                        on_top,
                        RichText::new(self.cfg.locale.text("topo", "top")).size(11.0),
                    )
                    .on_hover_text(self.cfg.locale.text("Manter por cima", "Keep on top"))
                    .clicked()
                {
                    on_top = !on_top;
                    self.cfg.mini_on_top = on_top;
                    self.cfg_dirty = true;
                    self.apply_on_top(ui.ctx());
                }
                let mut paused = self.sampler.paused.load(Ordering::Relaxed);
                let (icon, tip) = if paused {
                    ("▶", self.cfg.locale.text("Retomar", "Resume"))
                } else {
                    ("⏸", self.cfg.locale.text("Pausar", "Pause"))
                };
                if ui
                    .selectable_label(paused, RichText::new(icon).size(11.0))
                    .on_hover_text(tip)
                    .clicked()
                {
                    paused = !paused;
                    self.sampler.paused.store(paused, Ordering::Relaxed);
                }
                let iv = self.cfg.refresh_ms;
                if ui
                    .small_button(format!("{:.1}s", iv as f32 / 1000.0))
                    .on_hover_text(
                        self.cfg
                            .locale
                            .text("Ritmo — clique para alternar", "Interval — click to cycle"),
                    )
                    .clicked()
                {
                    const STEPS: [u64; 4] = [500, 1000, 2000, 5000];
                    let next = STEPS.iter().find(|v| **v > iv).copied().unwrap_or(STEPS[0]);
                    self.cfg.refresh_ms = next;
                    self.sampler.interval_ms.store(next, Ordering::Relaxed);
                    self.cfg_dirty = true;
                }
            });
        });
    }

    /// Um bloco de medidor: rótulo e temperatura na primeira linha, número grande com o detalhe
    /// ao lado na segunda, barra na terceira. `bar` desenha a barra (a RAM usa o medidor
    /// por categoria, os outros a barra simples).
    fn meter_tile(
        ui: &mut egui::Ui,
        w: f32,
        label: &str,
        pct: Option<f32>,
        temp: Temp,
        sub: &str,
        bar: impl FnOnce(&mut egui::Ui, f32),
    ) {
        ui.allocate_ui_with_layout(Vec2::new(w, TILE_H), Layout::top_down(Align::Min), |ui| {
            ui.set_width(w);
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            ui.spacing_mut().item_spacing = Vec2::new(5.0, 1.0);
            // Temperatura colada no rótulo, não alinhada à direita do bloco: encostada na
            // borda ela ficava mais perto do rótulo do medidor seguinte do que do próprio
            // — na fileira do topo o 50°C da GPU parecia ser do DISCO.
            ui.horizontal(|ui| {
                ui.label(RichText::new(label).color(MUTED).size(10.5));
                match temp {
                    Temp::C(t) => {
                        ui.label(
                            RichText::new(format!("{t}°C"))
                                .monospace()
                                .size(11.0)
                                .strong()
                                .color(Self::temp_color(t)),
                        );
                    }
                    Temp::Missing(why) => {
                        ui.label(
                            RichText::new("–°C")
                                .monospace()
                                .size(11.0)
                                .color(Color32::from_gray(90)),
                        )
                        .on_hover_text(why);
                    }
                    Temp::None => {}
                }
            });
            ui.horizontal(|ui| {
                match pct {
                    Some(p) => {
                        ui.label(
                            RichText::new(Self::fmt_pct(p))
                                .monospace()
                                .size(19.0)
                                .strong()
                                .color(Self::load_color(p / 100.0)),
                        );
                    }
                    None => {
                        ui.label(
                            RichText::new("–")
                                .monospace()
                                .size(19.0)
                                .color(Color32::from_gray(90)),
                        );
                    }
                }
                if !sub.is_empty() {
                    ui.label(RichText::new(sub).color(MUTED).size(10.5));
                }
            });
            bar(ui, w);
        });
    }

    /// Ritmo da amostragem, no rodapé — ao lado do "amostra 7 ms", que é o resultado dele.
    ///
    /// Estava no bloco do topo, e era o que fazia os quatro addons não caberem na fileira
    /// dos medidores: os controles desciam para uma segunda fileira quase vazia e sobrava
    /// uma faixa morta à direita dos medidores.
    fn ui_sampling_controls(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().interact_size.y = 17.0;
        ui.spacing_mut().button_padding = Vec2::new(6.0, 0.0);
        let mut paused = self.sampler.paused.load(Ordering::Relaxed);
        if ui
            .selectable_label(
                paused,
                RichText::new(if paused {
                    self.cfg.locale.text("▶ retomar", "▶ resume")
                } else {
                    self.cfg.locale.text("⏸ pausar", "⏸ pause")
                })
                .small(),
            )
            .on_hover_text(self.cfg.locale.text(
                "Congela a amostragem — os números param no último valor lido",
                "Freezes sampling — values stay at the last reading",
            ))
            .clicked()
        {
            paused = !paused;
            self.sampler.paused.store(paused, Ordering::Relaxed);
        }
        let mut iv = self.cfg.refresh_ms;
        egui::ComboBox::from_id_salt("refresh")
            .selected_text(
                RichText::new(format!(
                    "{} {:.1}s",
                    self.cfg.locale.text("a cada", "every"),
                    iv as f32 / 1000.0
                ))
                .small(),
            )
            .width(74.0)
            .show_ui(ui, |ui| {
                for v in [500u64, 1000, 2000, 5000] {
                    ui.selectable_value(
                        &mut iv,
                        v,
                        format!(
                            "{} {:.1}s",
                            self.cfg.locale.text("a cada", "every"),
                            v as f32 / 1000.0
                        ),
                    );
                }
            });
        if iv != self.cfg.refresh_ms {
            self.cfg.refresh_ms = iv;
            self.sampler.interval_ms.store(iv, Ordering::Relaxed);
            self.cfg_dirty = true;
        }
    }

    /// Conteúdo de um addon e o que ele devolve (avisos, pedidos de matar, gravar config).
    fn ui_addon_body(&mut self, ui: &mut egui::Ui, v: ViewMode) {
        match v {
            ViewMode::Drains => {
                let is_admin = self.is_admin;
                let procs = std::mem::take(&mut self.procs);
                let evs = self.drains.ui(ui, &procs, is_admin, self.cfg.locale);
                self.procs = procs;
                for ev in evs {
                    match ev {
                        DrainOut::Toast(m, err) => self.toast(m, err),
                        DrainOut::Kill(pids) => self.request_kill_many(&pids),
                    }
                }
            }
            ViewMode::Boot => {
                // A fileira de filtros some enquanto um addon está na tela, então o Partida
                // traz a própria busca — sem ela não há como achar uma entrada numa lista
                // que tem tudo que sobe com o PC. No Linux o próprio addon desenha a dele.
                if cfg!(windows) {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.search)
                                .hint_text(self.cfg.locale.text(
                                    "Buscar nome, comando ou origem…",
                                    "Search name, command, or source…",
                                ))
                                .desired_width(260.0),
                        );
                        if !self.search.is_empty() && ui.button("✖").clicked() {
                            self.search.clear();
                        }
                    });
                    ui.add_space(4.0);
                }
                let is_admin = self.is_admin;
                let search = self.search.clone();
                let procs = std::mem::take(&mut self.procs);
                let evs = self
                    .boot
                    .ui(ui, &procs, &search, is_admin, &mut self.cfg, &self.usage);
                self.procs = procs;
                for ev in evs {
                    match ev {
                        BootOut::Toast(m, err) => self.toast(m, err),
                        BootOut::SaveCfg => self.cfg_dirty = true,
                        BootOut::Kill(pids) => self.request_kill_many(&pids),
                    }
                }
            }
            ViewMode::Screens => {
                let procs = std::mem::take(&mut self.procs);
                let evs = self.screens.ui(ui, &procs, &mut self.cfg);
                self.procs = procs;
                for ev in evs {
                    match ev {
                        ScreenOut::Toast(m, err) => self.toast(m, err),
                        ScreenOut::SaveCfg => self.cfg_dirty = true,
                    }
                }
            }
            ViewMode::Thermal => self.ui_thermal(ui, self.cfg.locale),
            ViewMode::Clean => {
                // A Limpeza precisa da métrica de RAM escolhida e do lock do usuário, sem
                // conhecer o App: recebe os dois como closures.
                let metric = self.cfg.mem_metric;
                let locked = self.cfg.locked.clone();
                let me = std::process::id();
                let mem = move |p: &ProcInfo| Self::metric_of(metric, p);
                let is_locked = move |p: &ProcInfo| {
                    is_critical(&p.name_lower, p.pid)
                        || locked.contains(&p.name_lower)
                        || p.pid == me
                };
                let procs = std::mem::take(&mut self.procs);
                let evs = self.clean.ui(ui, &procs, &mem, &is_locked, self.cfg.locale);
                self.procs = procs;
                for ev in evs {
                    match ev {
                        CleanOut::Toast(m, err) => self.toast(m, err),
                        CleanOut::Kill(pids) => {
                            // O addon já filtra protegidos, mas a lista pode ter mudado
                            // entre o frame e o clique.
                            let pids: Vec<u32> = pids
                                .into_iter()
                                .filter(|pid| self.proc(*pid).is_some_and(|p| !self.is_locked(p)))
                                .collect();
                            self.request_kill_many(&pids)
                        }
                    }
                }
            }
            ViewMode::Sweep => {
                for ev in self.sweep.ui(ui, self.cfg.locale) {
                    match ev {
                        SweepOut::Kill(pids) => {
                            // A classificação é da última amostra; confere o lock de novo.
                            let pids: Vec<u32> = pids
                                .into_iter()
                                .filter(|pid| self.proc(*pid).is_some_and(|p| !self.is_locked(p)))
                                .collect();
                            self.request_kill_many(&pids)
                        }
                        SweepOut::Toast(m) => self.toast(m, false),
                    }
                }
            }
            _ => {}
        }
    }

    /// Finaliza uma lista de PIDs vinda de um addon.
    fn request_kill_many(&mut self, pids: &[u32]) {
        let list: Vec<(u32, i64, String, u64)> = pids
            .iter()
            .filter_map(|pid| {
                self.proc(*pid)
                    .map(|p| (p.pid, p.create_time, identity::of(p).label, self.mem_of(p)))
            })
            .collect();
        if list.is_empty() {
            self.toast(
                self.cfg
                    .locale
                    .text(
                        "esses processos já tinham saído",
                        "those processes had already exited",
                    )
                    .into(),
                false,
            );
            self.after_kill();
            return;
        }
        self.execute_kill(list, 0);
    }

    /// Cabeçalho de coluna numérica: alinhado à direita, igual aos valores embaixo.
    fn header_btn_right(&mut self, ui: &mut egui::Ui, key: SortKey, label: &str) {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(2.0);
            self.header_btn(ui, key, label);
        });
    }

    fn header_btn(&mut self, ui: &mut egui::Ui, key: SortKey, label: &str) -> egui::Response {
        let active = self.sort == key;
        let arrow = if active {
            if self.sort_desc {
                " ▾"
            } else {
                " ▴"
            }
        } else {
            ""
        };
        // Todos os títulos com o mesmo peso; só a cor marca a coluna ordenada — antes a
        // ativa virava uma caixa cinza que parecia um botão perdido no cabeçalho.
        let text = RichText::new(format!("{label}{arrow}"))
            .size(11.5)
            .strong()
            .color(if active { ACCENT } else { MUTED });
        let r = ui
            .add(egui::Button::new(text).frame(false))
            .on_hover_text(self.cfg.locale.text(
                "Clique para ordenar por esta coluna",
                "Click to sort by this column",
            ));
        if r.clicked() {
            if active {
                self.sort_desc = !self.sort_desc;
            } else {
                self.sort = key;
                self.sort_desc = !matches!(key, SortKey::Name | SortKey::Parent | SortKey::Cat);
            }
        }
        r
    }

    fn ui_table(&mut self, ui: &mut egui::Ui) {
        self.table_rect = Some(ui.max_rect());
        let rows = self.rows_for_frame(&ui.ctx().clone());
        let n = rows.len();
        let now_ft = procs::now_filetime();
        let tree = self.cfg.view == ViewMode::Tree;
        // Com grupos, toda linha de processo cede a mesma goteira que a seta do cabeçalho
        // ocupa. Sem isso o filho fica desenhado à esquerda do nome do app e a hierarquia
        // aparece invertida.
        let group_gutter = self.cfg.view == ViewMode::List && self.cfg.group_apps;
        let disputa_on = self.sort == SortKey::Steal;
        let mut click_select: Option<u32> = None;
        let mut toggle_expand: Option<u32> = None;
        let mut toggle_cat: Option<Category> = None;
        let mut toggle_app: Option<String> = None;
        let mut kill_group: Option<usize> = None;
        let mut kill: Option<(u32, bool)> = None;
        let mut lock: Option<String> = None;
        let mut copy: Option<String> = None;
        let mut open_folder: Option<String> = None;
        let mut set_cat: Option<(String, Option<Category>)> = None;
        let mut sort_by: Option<SortKey> = None;

        // Escala do mini-gráfico da coluna RAM: o maior valor visível vira 100%.
        let max_ram = rows
            .iter()
            .filter_map(|r| match r {
                Row::Proc { pid, .. } => {
                    if tree {
                        self.subtree
                            .get(pid)
                            .copied()
                            .or_else(|| self.proc(*pid).map(|p| self.mem_of(p)))
                    } else {
                        self.proc(*pid).map(|p| self.mem_of(p))
                    }
                }
                Row::AppHeader { gi } => self.groups.get(*gi).map(|g| g.ram),
                _ => None,
            })
            .max()
            .unwrap_or(1)
            .max(1);
        let row_x = self.table_rect.map(|r| r.x_range());
        // Divisórias de coluna quase invisíveis: com listras zebradas, linha vertical em
        // toda coluna é tinta dobrada.
        ui.visuals_mut().widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, SURFACE);

        // Maior GPU%/disco visível — escala das barras de magnitude dessas colunas.
        let max_gpu: f32 = rows
            .iter()
            .filter_map(|r| match r {
                Row::Proc { pid, .. } => self.proc(*pid).map(|p| p.gpu_pct),
                Row::AppHeader { gi } => self.groups.get(*gi).map(|g| g.gpu),
                _ => None,
            })
            .fold(1.0_f32, f32::max);
        let max_disk: f64 = rows
            .iter()
            .filter_map(|r| match r {
                Row::Proc { pid, .. } => self.proc(*pid).map(|p| p.disk_bps),
                Row::AppHeader { gi } => self.groups.get(*gi).map(|g| g.disk),
                _ => None,
            })
            .fold(1.0_f64, f64::max);

        let mut table = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::initial(300.0).at_least(180.0).clip(true))
            .column(Column::initial(96.0).at_least(72.0))
            // Com a Disputa ligada a célula mostra "87% 14×", que não cabe em 60 px.
            .column(if disputa_on {
                Column::initial(96.0).at_least(84.0)
            } else {
                Column::initial(60.0).at_least(48.0)
            })
            .column(Column::initial(56.0).at_least(44.0))
            .column(Column::initial(68.0).at_least(52.0))
            .column(Column::initial(72.0).at_least(56.0))
            .column(Column::initial(68.0).at_least(52.0))
            // Não redimensionável de propósito: com `resizable`, o egui_extras congela a
            // largura do "resto" na primeira passada e a coluna não acompanha a janela ao
            // maximizar. A borda da coluna Tempo continua arrastável, e é ela que reparte.
            .column(
                Column::remainder()
                    .at_least(80.0)
                    .clip(true)
                    .resizable(false),
            )
            .column(Column::exact(60.0))
            .min_scrolled_height(0.0);
        if self.scroll_to_selected {
            if let Some(sel) = self.selected {
                if let Some(i) = rows
                    .iter()
                    .position(|r| matches!(r, Row::Proc { pid, .. } if *pid == sel))
                {
                    table = table.scroll_to_row(i, Some(Align::Center));
                }
            }
            self.scroll_to_selected = false;
        }

        table
            .header(30.0, |mut header| {
                header.col(|ui| {
                    ui.add_space(4.0);
                    self.header_btn(ui, SortKey::Name, self.cfg.locale.text("Nome", "Name"));
                });
                let ram_label = match (tree, self.cfg.mem_metric) {
                    (true, MemMetric::Private) => self.cfg.locale.text("Priv. (árvore)", "Private (tree)").to_string(),
                    (true, MemMetric::Commit) => format!("{} ({})", MemMetric::Commit.short_for(self.cfg.locale), self.cfg.locale.text("árvore", "tree")),
                    (true, _) => format!("RAM ({})", self.cfg.locale.text("árvore", "tree")),
                    (false, m) => m.short_for(self.cfg.locale).to_string(),
                };
                header.col(|ui| {
                    self.header_btn_right(ui, SortKey::Ram, &ram_label);
                });
                header.col(|ui| {
                    // Sempre CPU: quando o cabeçalho virava "Disputa", clicar nele só
                    // invertia a Disputa e a ordenação por CPU ficava inalcançável.
                    self.header_btn_right(ui, SortKey::Cpu, if disputa_on { "CPU · ×" } else { "CPU" });
                });
                header
                    .col(|ui| { self.header_btn_right(ui, SortKey::Gpu, "GPU"); })
                    .1
                    .on_hover_text(if self.gpu_per_proc {
                        self.cfg.locale.text("% de carga da GPU (engine mais ocupada, pico dos últimos segundos). – = processo sem contexto na GPU ou driver sem leitura.", "% GPU load (busiest engine, peak over the last few seconds). – means no GPU context or unavailable driver telemetry.").to_string()
                    } else {
                        self.cfg.locale.text("Contador de GPU por processo indisponível neste host", "Per-process GPU counter is unavailable on this host").to_string()
                    });
                header
                    .col(|ui| self.header_btn_right(ui, SortKey::Vram, "VRAM"))
                    .1
                    .on_hover_text(self.cfg.locale.text("Memória da GPU deste processo, quando o driver expõe. Não é RAM.", "This process's GPU memory when exposed by the driver. It is not system RAM."));
                header.col(|ui| self.header_btn_right(ui, SortKey::Disk, self.cfg.locale.text("Disco", "Disk")));
                header.col(|ui| self.header_btn_right(ui, SortKey::Age, self.cfg.locale.text("Tempo", "Age")));
                header.col(|ui| {
                    let r = if self.cfg.cmd_column {
                        ui.add(egui::Button::new(RichText::new(self.cfg.locale.text("Comando", "Command")).size(11.5).strong().color(MUTED)).frame(false))
                            .on_hover_text(self.cfg.locale.text("Argumentos da linha de comando (o caminho do exe já está no nome). Botão direito aqui troca para \"Quem abriu\".", "Command-line arguments (the executable path is already in the name). Right-click here to switch to \"Launched by\"."))
                    } else {
                        self.header_btn(ui, SortKey::Parent, self.cfg.locale.text("Quem abriu", "Launched by"))
                            .on_hover_text(self.cfg.locale.text("Quem chamou quem, da raiz até o pai: terminal › shell › agente. Clique ordena pelo pai. A linha de comando fica no tooltip da célula e no painel de detalhes. Botão direito aqui troca para \"Comando\".", "Who launched whom, from the root to the parent: terminal › shell › agent. Click to sort by parent. The command line remains in the cell tooltip and details panel. Right-click here to switch to \"Command\"."))
                    };
                    r.context_menu(|ui| {
                        let (on, off) = if self.cfg.cmd_column {
                            (self.cfg.locale.text("Comando", "Command"), self.cfg.locale.text("Quem abriu", "Launched by"))
                        } else {
                            (self.cfg.locale.text("Quem abriu", "Launched by"), self.cfg.locale.text("Comando", "Command"))
                        };
                        ui.label(RichText::new(format!("{}: {on}", self.cfg.locale.text("Coluna", "Column"))).weak());
                        if ui.button(format!("{} {off}", self.cfg.locale.text("Mostrar", "Show"))).clicked() {
                            self.cfg.cmd_column = !self.cfg.cmd_column;
                            self.cfg_dirty = true;
                            ui.close_menu();
                        }
                    });
                });
                header.col(|_ui| {});
            })
            .body(|body| {
                body.rows(ROW_H, n, |mut row: TableRow| {
                    let i = row.index();
                    match &rows[i] {
                        Row::System { kind, bytes } => {
                            let (kind, bytes) = (*kind, *bytes);
                            row.col(|ui| {
                                Self::row_line(ui, row_x);
                                ui.add_space(4.0);
                                let (r, _) = ui.allocate_exact_size(Vec2::splat(AVATAR), egui::Sense::hover());
                                ui.painter().rect_filled(r, 7.0, kind.color().gamma_multiply(0.22));
                                ui.painter().rect_filled(Rect::from_center_size(r.center(), Vec2::splat(9.0)), 2.0, kind.color());
                                ui.add_space(2.0);
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 1.0;
                                    ui.label(RichText::new(kind.label_for(self.cfg.locale)).color(kind.color()).size(13.0));
                                    ui.label(RichText::new(self.cfg.locale.text("sistema · não é processo", "system · not a process")).color(MUTED).size(11.5));
                                });
                            });
                            row.col(|ui| {
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.add_space(2.0);
                                    ui.label(num(fmt_bytes(bytes)).color(kind.color()).strong());
                                });
                            });
                            // CPU, GPU, VRAM, Disco, Tempo
                            for _ in 0..5 {
                                row.col(|_ui| {});
                            }
                            row.col(|ui| {
                                ui.label(RichText::new(self.cfg.locale.text("não pode ser encerrado", "cannot be terminated")).weak().small());
                            });
                            row.col(|_ui| {});
                            row.response().on_hover_text(kind.tip_for(self.cfg.locale));
                        }
                        Row::CatHeader { cat, count, total, collapsed } => {
                            let (cat, count, total, collapsed) = (*cat, *count, *total, *collapsed);
                            row.col(|ui| {
                                Self::row_line(ui, row_x);
                                ui.add_space(4.0);
                                let arrow = if collapsed { "▶" } else { "▼" };
                                if ui
                                    .add(egui::Label::new(RichText::new(format!("{arrow} {}", cat.label_for(self.cfg.locale))).color(cat.color()).strong().size(13.5)).sense(egui::Sense::click()))
                                    .clicked()
                                {
                                    toggle_cat = Some(cat);
                                }
                            });
                            row.col(|ui| {
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.add_space(2.0);
                                    ui.label(num(fmt_bytes(total)).strong().color(cat.color()));
                                });
                            });
                            for _ in 0..5 {
                                row.col(|_ui| {});
                            }
                            row.col(|ui| {
                                ui.label(RichText::new(format!("{count} {}", self.cfg.locale.text("processos", "processes"))).weak().size(11.5));
                            });
                            row.col(|_ui| {});
                            if row.response().clicked() {
                                toggle_cat = Some(cat);
                            }
                        }
                        Row::AppHeader { gi } => {
                            let gi = *gi;
                            let Some(g) = self.groups.get(gi) else {
                                for _ in 0..9 {
                                    row.col(|_ui| {});
                                }
                                return;
                            };
                            let (key, icon_key, name, cat) = (g.key.clone(), g.icon_key.clone(), g.name.clone(), g.cat);
                            let (ram, cpu, gpu, gpu_known, vram, vram_known, disk, oldest) =
                                (g.ram, g.cpu, g.gpu, g.gpu_known, g.vram, g.vram_known, g.disk, g.oldest);
                            let (has_window, focused, leftover, origin) =
                                (g.has_window, g.focused, g.leftover.clone(), g.origin.clone());
                            let ram_complete = g.pids.iter().filter_map(|pid| self.proc(*pid)).all(|p| metric_available(self.cfg.mem_metric, p));
                            let ram_label = aggregate_memory_text(ram, ram_complete);
                            let count = g.pids.len();
                            let collapsed = !self.expanded_apps.contains(&key) && self.search.trim().is_empty();
                            row.col(|ui| {
                                Self::row_line(ui, row_x);
                                let arrow = if collapsed { "▶" } else { "▼" };
                                let b = egui::Button::new(RichText::new(arrow).weak().small())
                                    .frame(false)
                                    .min_size(Vec2::new(18.0, ROW_H - 4.0));
                                if ui.add(b).clicked() {
                                    toggle_app = Some(key.clone());
                                }
                                let tex = self.icons.get(&icon_key).and_then(|t| t.as_ref());
                                avatar(ui, tex, cat, &name);
                                ui.add_space(2.0);
                                let steal = g
                                    .pids
                                    .iter()
                                    .filter_map(|pid| self.proc(*pid).and_then(|p| self.steal_of(p)))
                                    .max_by_key(|k| k.rank());
                                let chip_info = if leftover.is_some() {
                                    Some((self.cfg.locale.text("sobra", "leftover"), Color32::from_rgb(230, 170, 90)))
                                } else if let Some(kind) = steal {
                                    Some((kind.chip_for(self.cfg.locale), Color32::from_rgb(255, 150, 90)))
                                } else if focused {
                                    Some((self.cfg.locale.text("em foco", "focused"), ACCENT_FG))
                                } else {
                                    None
                                };
                                let mut sub = format!("{count} {} · {}", self.cfg.locale.text("processos", "processes"), cat.label_for(self.cfg.locale));
                                if !origin.is_empty() {
                                    sub.push_str(" · ");
                                    sub.push_str(&origin);
                                }
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 1.0);
                                    ui.horizontal(|ui| {
                                        let reserve = chip_info.map(|(t, _)| chip_w(ui, t)).unwrap_or(0.0);
                                        ui.scope(|ui| {
                                            ui.set_max_width((ui.available_width() - reserve).max(20.0));
                                            ui.add(egui::Label::new(RichText::new(&name).size(13.0)).truncate());
                                        });
                                        if let Some((t, c)) = chip_info {
                                            let r = chip(ui, t, c);
                                            if let Some(why) = &leftover {
                                                r.on_hover_text(why);
                                            }
                                        }
                                    });
                                    ui.add(egui::Label::new(RichText::new(sub).size(11.5).color(MUTED)).truncate());
                                });
                            });
                            row.col(|ui| {
                                let frac = (ram as f32 / max_ram as f32).clamp(0.0, 1.0).sqrt();
                                Self::cell_bar(ui, frac, ram_color(ram, MUTED));
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.add_space(2.0);
                                    ui.label(num(ram_label).color(ram_color(ram, ui_text_color(ui_dark()))).strong())
                                        .on_hover_text(self.cfg.locale.text(
                                            "Soma dos processos do app na métrica escolhida. RSS pode repetir páginas compartilhadas; PSS reparte essas páginas. ≥ indica soma parcial por leituras indisponíveis.",
                                            "Sum of this app's processes in the selected metric. RSS may repeat shared pages; PSS distributes them. ≥ means the sum is partial because some readings are unavailable.",
                                        ));
                                });
                            });
                            row.col(|ui| {
                                let (txt, c) = Self::cpu_cell(cpu, self.ncpu);
                                let tip = if self.cfg.locale == Locale::Portuguese {
                                    format!("{cpu:.1}% da máquina ({} núcleos) = {:.1} núcleos equivalentes.\n\n100% = a máquina inteira. Processo que come 1 núcleo aparece como {:.1}% — por isso some na lista por RAM.", self.ncpu, pressure::cores(cpu, self.ncpu as u32), 100.0 / self.ncpu.max(1) as f32)
                                } else {
                                    format!("{cpu:.1}% of the machine ({} threads) = {:.1} equivalent cores.\n\n100% = the whole machine. A process using one core appears as {:.1}% — which is why it can disappear in a memory-sorted list.", self.ncpu, pressure::cores(cpu, self.ncpu as u32), 100.0 / self.ncpu.max(1) as f32)
                                };
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if disputa_on {
                                        ui.scope(|ui| Self::cpu_cell_disputa(ui, cpu, self.ncpu, MUTED)).response.on_hover_text(tip);
                                    } else {
                                        ui.label(num(txt).color(c).strong()).on_hover_text(tip);
                                    }
                                });
                            });
                            row.col(|ui| {
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.add_space(2.0);
                                    if !self.gpu_per_proc || !gpu_known {
                                        ui.label(num("–").color(MUTED))
                                            .on_hover_text(self.cfg.locale.text("Sem leitura de carga GPU neste grupo — não é zero.", "No GPU load reading for this group — this is not zero."));
                                    } else if gpu < 0.05 {
                                        ui.label(num("–").color(MUTED)).on_hover_text(self.cfg.locale.text("Carga GPU ~0% (máximo entre os PIDs).", "GPU load ~0% (maximum across PIDs)."));
                                    } else {
                                        ui.label(num(format!("{gpu:.0}%")).color(ui_text_color(ui_dark())).strong())
                                            .on_hover_text(self.cfg.locale.text("Máximo entre os processos do app, não a soma.", "Maximum across this app's processes, not a sum."));
                                    }
                                });
                            });
                            row.col(|ui| {
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.add_space(2.0);
                                    if !vram_known {
                                        ui.label(num("–").color(MUTED));
                                    } else {
                                        ui.label(num(fmt_bytes(vram)).color(ui_text_color(ui_dark())).strong());
                                    }
                                });
                            });
                            row.col(|ui| {
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.add_space(2.0);
                                    if disk < 1024.0 {
                                        ui.label(num("–").color(MUTED));
                                    } else {
                                        ui.label(num(fmt_bps(disk)).color(ui_text_color(ui_dark())).strong());
                                    }
                                });
                            });
                            row.col(|ui| {
                                let secs = ((now_ft - oldest).max(0) / 10_000_000) as u64;
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.add_space(2.0);
                                    ui.label(RichText::new(fmt_age(secs)).color(MUTED))
                                        .on_hover_text(self.cfg.locale.text("Idade do processo mais antigo do app", "Age of the app's oldest process"));
                                });
                            });
                            row.col(|ui| {
                                let _ = has_window;
                                ui.label(RichText::new(format!("{count} {}", self.cfg.locale.text("processos", "processes"))).weak().size(11.5));
                            });
                            row.col(|ui| {
                                let cell = ui.max_rect();
                                let row_rect = Rect::from_x_y_ranges(row_x.unwrap_or(cell.x_range()), cell.y_range());
                                let hot = ui.rect_contains_pointer(row_rect);
                                let kc = if hot { Color32::from_rgb(235, 90, 90) } else { Color32::from_gray(96) };
                                let b = egui::Button::new(RichText::new("✖").color(kc).size(11.0))
                                    .fill(if hot { SURFACE_HI } else { Color32::TRANSPARENT })
                                    .stroke(Stroke::NONE)
                                    .corner_radius(8.0)
                                    .min_size(Vec2::new(24.0, 24.0));
                                let tip = if self.cfg.locale == Locale::Portuguese {
                                    format!("Finalizar os {count} processos de {name}")
                                } else {
                                    format!("Terminate {count} processes from {name}")
                                };
                                if ui.add(b).on_hover_text(tip).clicked() {
                                    kill_group = Some(gi);
                                }
                            });
                            if row.response().clicked() {
                                toggle_app = Some(key.clone());
                            }
                        }
                        Row::Proc { pid, depth, has_children, expanded, dim } => {
                            let (pid, depth, has_children, expanded, dim) = (*pid, *depth, *has_children, *expanded, *dim);
                            let Some(p) = self.proc(pid).cloned() else {
                                for _ in 0..9 {
                                    row.col(|_ui| {});
                                }
                                return;
                            };
                            let task = identity::of(&p);
                            let cat = self.cat(pid);
                            let locked = self.is_locked(&p);
                            let critical = is_critical(&p.name_lower, p.pid);
                            let selected = self.selected == Some(pid);
                            row.set_selected(selected);
                            let text_color = if dim { Color32::from_gray(120) } else { ui_text_color(ui_dark()) };
                            // Nome: avatar + duas linhas (nome e "PID · categoria · origem") + chip de estado
                            row.col(|ui| {
                                Self::row_line(ui, row_x);
                                if group_gutter {
                                    ui.add_space(18.0);
                                } else {
                                    ui.add_space(4.0);
                                }
                                ui.add_space(depth as f32 * 14.0);
                                if tree {
                                    if has_children {
                                        let arrow = if expanded { "▼" } else { "▶" };
                                        let b = egui::Button::new(RichText::new(arrow).weak().small())
                                            .frame(false)
                                            .min_size(Vec2::new(18.0, ROW_H - 4.0));
                                        if ui.add(b).on_hover_text(self.cfg.locale.text("Expandir / recolher (duplo clique na linha também)", "Expand / collapse (double-click the row also works)")).clicked() {
                                            toggle_expand = Some(pid);
                                        }
                                    } else {
                                        ui.add_space(18.0);
                                    }
                                }
                                let key = p.exe_path.to_lowercase();
                                let tex = self.icons.get(&key).and_then(|t| t.as_ref());
                                avatar(ui, tex, cat, &task.label);
                                ui.add_space(2.0);
                                let leftover = identity::leftover_reason_for(&p.cmdline, p.kernel_state, p.has_window, self.cfg.locale);
                                let steal = self.steal_of(&p);
                                let chip_info = if p.kernel_state == Some('Z') {
                                    Some((self.cfg.locale.text("zombie", "zombie"), Color32::from_rgb(230, 120, 120)))
                                } else if leftover.is_some() {
                                    Some((self.cfg.locale.text("sobra", "leftover"), Color32::from_rgb(230, 170, 90)))
                                } else if let Some(kind) = steal.filter(|k| *k != StealKind::Leftover) {
                                    Some((kind.chip_for(self.cfg.locale), Color32::from_rgb(255, 150, 90)))
                                } else if p.focused {
                                    Some((self.cfg.locale.text("em foco", "focused"), ACCENT_FG))
                                } else if critical {
                                    Some((self.cfg.locale.text("protegido", "protected"), MUTED))
                                } else if locked {
                                    Some(("lock", Color32::from_rgb(120, 200, 255)))
                                } else {
                                    None
                                };
                                let (origin, target, otip, _via_env) = self.origin_label(&p, self.cfg.locale);
                                let overridden = self.cfg.overrides.contains_key(&p.name_lower);
                                let mut sub = format!("PID {}", p.pid);
                                if tree && has_children && !expanded {
                                    let c = self.subtree_count.get(&pid).copied().unwrap_or(1) - 1;
                                    sub.push_str(&format!(" · +{c} {}", self.cfg.locale.text("filhos", "children")));
                                }
                                sub.push_str(" · ");
                                sub.push_str(cat.label_for(self.cfg.locale));
                                if overridden {
                                    sub.push_str(self.cfg.locale.text(" (manual)", " (manual)"));
                                }
                                if !origin.is_empty() {
                                    sub.push_str(" · ");
                                    sub.push_str(&origin);
                                }
                                let mut name = RichText::new(&task.label).size(13.0).color(text_color);
                                if locked {
                                    name = name.color(Color32::from_rgb(120, 200, 255));
                                }
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 1.0);
                                    ui.horizontal(|ui| {
                                        let reserve = chip_info.map(|(t, _)| chip_w(ui, t)).unwrap_or(0.0);
                                        let lbl = ui
                                            .scope(|ui| {
                                                ui.set_max_width((ui.available_width() - reserve).max(20.0));
                                                ui.add(egui::Label::new(name).truncate())
                                            })
                                            .inner;
                                        if locked {
                                            lbl.on_hover_text(if critical { self.cfg.locale.text("Processo crítico do sistema", "Critical system process") } else { self.cfg.locale.text("Protegido (lock)", "Protected (lock)") });
                                        }
                                        if let Some((t, c)) = chip_info {
                                            let r = chip(ui, t, c);
                                            if let Some(why) = leftover {
                                                r.on_hover_text(why);
                                            } else if let Some(wt) = p.window_title.as_deref().filter(|s| !s.is_empty()) {
                                                r.on_hover_text(wt);
                                            }
                                        }
                                    });
                                    let st = RichText::new(sub).size(11.5).color(MUTED);
                                    if let Some(tp) = target {
                                        if ui.add(egui::Label::new(st).truncate().sense(egui::Sense::click())).on_hover_text(otip).clicked() {
                                            click_select = Some(tp);
                                        }
                                    } else {
                                        let r = ui.add(egui::Label::new(st).truncate());
                                        if !otip.is_empty() {
                                            r.on_hover_text(otip);
                                        }
                                    }
                                });
                            });
                            // RAM
                            row.col(|ui| {
                                let (shown, own) = if tree {
                                    (self.subtree.get(&pid).copied().unwrap_or(self.mem_of(&p)), self.mem_of(&p))
                                } else {
                                    (self.mem_of(&p), self.mem_of(&p))
                                };
                                // Barra de magnitude no pé da célula: 419 linhas de texto viram
                                // uma forma — dá pra ver a distribuição sem ler valor por valor.
                                // Escala raiz quadrada: a distribuição de RAM tem cauda longa
                                // (um processo de 1,4 GB e centenas de 200 MB). No linear tudo
                                // abaixo de 300 MB virava o mesmo tracinho de 20 px.
                                let frac = (shown as f32 / max_ram as f32).clamp(0.0, 1.0).sqrt();
                                Self::cell_bar(ui, frac, ram_color(shown, MUTED));
                                let label = if !metric_available(self.cfg.mem_metric, &p) && !(tree && has_children) {
                                    "—".to_string()
                                } else if tree && has_children {
                                    aggregate_memory_text(shown, self.subtree_memory_available(pid))
                                } else { fmt_bytes(shown) };
                                let mut t = num(label).color(ram_color(shown, text_color));
                                if tree && has_children {
                                    t = t.strong();
                                }
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.add_space(2.0);
                                    let r = ui.label(t);
                                    if tree && has_children && shown != own {
                                        let n = self.subtree_count.get(&pid).copied().unwrap_or(1) - 1;
                                        let tip = if self.cfg.locale == Locale::Portuguese {
                                            format!("Deste processo: {}\nCom os {} filhos: {}\n\nA coluna soma a subárvore inteira — o número grande costuma ser dos filhos, não deste processo.", fmt_bytes(own), n, fmt_bytes(shown))
                                        } else {
                                            format!("This process: {}\nWith its {} children: {}\n\nThe column sums the entire subtree — the large number usually belongs to children, not this process.", fmt_bytes(own), n, fmt_bytes(shown))
                                        };
                                        r.on_hover_text(tip);
                                    }
                                });
                            });
                            // CPU — na Árvore soma a subárvore, como a coluna RAM: o pai
                            // mostra o que o app inteiro come, não só o processo raiz.
                            row.col(|ui| {
                                let cpu_shown = if tree {
                                    self.subtree_cpu.get(&pid).copied().unwrap_or(p.cpu_pct)
                                } else {
                                    p.cpu_pct
                                };
                                let (txt, c) = Self::cpu_cell(cpu_shown, self.ncpu);
                                let c = if c == Color32::from_rgb(200, 200, 200) { text_color } else { c };
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    let r = if disputa_on {
                                        ui.scope(|ui| Self::cpu_cell_disputa(ui, cpu_shown, self.ncpu, MUTED)).response
                                    } else {
                                        ui.label(num(txt).color(c))
                                    };
                                    // Filhos que já morreram dentro da janela: o `rg` de 2 s que a
                                    // lista nunca viu. Sem isso o Claude mostrava 0,3% enquanto
                                    // os filhos dele comiam 15% da máquina.
                                    if p.cpu_children_pct >= 0.5 && !disputa_on {
                                        ui.label(
                                            RichText::new(format!("+{:.0}%", p.cpu_children_pct))
                                                .monospace()
                                                .size(10.5)
                                                .color(Color32::from_rgb(255, 171, 145)),
                                        )
                                        .on_hover_text(if self.cfg.locale == Locale::Portuguese {
                                            format!("+{:.1}% em filhos que nasceram e morreram entre duas amostras (creditado a este processo pelo kernel).", p.cpu_children_pct)
                                        } else {
                                            format!("+{:.1}% in children that started and exited between two samples (credited to this process by the kernel).", p.cpu_children_pct)
                                        });
                                    }
                                    if cpu_shown >= 0.05 || p.cpu_raw_pct >= 0.05 || p.cpu_children_pct >= 0.05 {
                                        let mut tip = if self.cfg.locale == Locale::Portuguese {
                                            if tree && has_children {
                                                format!("deste processo: {:.1}%\ncom os filhos: {:.1}% = {:.1} núcleos\n", p.cpu_pct, cpu_shown, pressure::cores(cpu_shown, self.ncpu as u32))
                                            } else {
                                                format!("média: {:.1}% da máquina = {:.1} núcleos\núltimo intervalo: {:.1}%\n", p.cpu_pct, pressure::cores(p.cpu_pct, self.ncpu as u32), p.cpu_raw_pct)
                                            }
                                        } else if tree && has_children {
                                            format!("this process: {:.1}%\nwith children: {:.1}% = {:.1} cores\n", p.cpu_pct, cpu_shown, pressure::cores(cpu_shown, self.ncpu as u32))
                                        } else {
                                            format!("average: {:.1}% of the machine = {:.1} cores\nlast interval: {:.1}%\n", p.cpu_pct, pressure::cores(p.cpu_pct, self.ncpu as u32), p.cpu_raw_pct)
                                        };
                                        if p.cpu_children_pct >= 0.05 {
                                            tip.push_str(&if self.cfg.locale == Locale::Portuguese {
                                                format!("filhos já encerrados neste intervalo: +{:.1}%\n", p.cpu_children_pct)
                                            } else {
                                                format!("children already exited in this interval: +{:.1}%\n", p.cpu_children_pct)
                                            });
                                        }
                                        if self.cfg.locale == Locale::Portuguese {
                                            tip.push_str(&format!("\n100% = a máquina inteira ({} núcleos). 1 núcleo cheio vira {:.1}%.", self.ncpu, 100.0 / self.ncpu.max(1) as f32));
                                        } else {
                                            tip.push_str(&format!("\n100% = the whole machine ({} threads). One full core is {:.1}%.", self.ncpu, 100.0 / self.ncpu.max(1) as f32));
                                        }
                                        r.on_hover_text(tip);
                                    }
                                });
                            });
                            // GPU
                            row.col(|ui| {
                                if p.gpu_load.unwrap_or(0.0) >= 0.05 {
                                    let frac = (p.gpu_load.unwrap_or(0.0) / max_gpu).clamp(0.0, 1.0).sqrt();
                                    Self::cell_bar(ui, frac, Color32::from_rgb(180, 130, 230));
                                }
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.add_space(2.0);
                                    match p.gpu_load {
                                        None => {
                                            ui.label(RichText::new("–").color(Color32::from_gray(90)))
                                                .on_hover_text(self.cfg.locale.text("Sem leitura de carga GPU para este PID — não é 0%.", "No GPU load reading for this PID — this is not 0%."));
                                        }
                                        Some(load) if load < 0.05 => {
                                            ui.label(num("–").color(MUTED)).on_hover_text(self.cfg.locale.text("Carga GPU ~0%.", "GPU load ~0%."));
                                        }
                                        Some(load) => {
                                            let c = if load >= 50.0 {
                                                Color32::from_rgb(255, 150, 90)
                                            } else if load >= 10.0 {
                                                Color32::from_rgb(230, 210, 120)
                                            } else {
                                                text_color
                                            };
                                            ui.label(num(format!("{load:.0}%")).color(c));
                                        }
                                    }
                                });
                            });
                            row.col(|ui| {
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.add_space(2.0);
                                    match p.gpu_vram {
                                        None => ui.label(num("–").color(MUTED)),
                                        Some(0) => ui.label(num("–").color(MUTED)),
                                        Some(v) => ui.label(num(fmt_bytes(v)).color(text_color)),
                                    };
                                });
                            });
                            // Disco (bytes/s, raiz quadrada para não deixar tudo achatado)
                            row.col(|ui| {
                                let frac = (p.disk_bps as f32 / max_disk as f32).clamp(0.0, 1.0).sqrt();
                                Self::cell_bar(ui, frac, Color32::from_rgb(120, 150, 220));
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.add_space(2.0);
                                    if p.disk_bps < 1024.0 {
                                        ui.label(num("–").color(MUTED));
                                    } else {
                                        ui.label(num(fmt_bps(p.disk_bps)).color(text_color));
                                    }
                                });
                            });
                            // Tempo
                            row.col(|ui| {
                                let secs = ((now_ft - p.create_time).max(0) / 10_000_000) as u64;
                                let mut t = RichText::new(fmt_age(secs)).color(MUTED);
                                if secs < 5 {
                                    t = t.color(Color32::from_rgb(90, 220, 130));
                                }
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.add_space(2.0);
                                    ui.label(t);
                                });
                            });
                            row.col(|ui| {
                                // Para quem hospeda serviço, o nome do serviço vale mil vezes
                                // mais que "-k netsvcs -p". Mesma coluna, conteúdo útil.
                                let svcs = self.services_of(pid);
                                if !svcs.is_empty() {
                                    let names: Vec<&str> = svcs.iter().map(|(_, d)| d.as_str()).collect();
                                    let list: Vec<String> = svcs.iter().map(|(n, d)| format!("{d}  ({n})")).collect();
                                    ui.add(egui::Label::new(RichText::new(names.join(" · ")).color(Color32::from_rgb(130, 175, 215)).size(11.5)).truncate())
                                        .on_hover_text(format!("{}:\n{}", self.cfg.locale.text("Serviços hospedados neste processo", "Services hosted by this process"), list.join("\n")));
                                } else if self.cfg.cmd_column {
                                    let full = if p.cmdline.is_empty() { p.exe_path.clone() } else { p.cmdline.clone() };
                                    let r = ui.add(egui::Label::new(RichText::new(cmd_args(&p, self.cfg.locale)).color(MUTED).size(11.5)).truncate());
                                    if !full.is_empty() {
                                        r.on_hover_text(full);
                                    }
                                } else {
                                    let (who, tip, target) = self.invoker_of(&p);
                                    let color = if target.is_some() { Color32::from_gray(175) } else { MUTED };
                                    let lbl = egui::Label::new(RichText::new(&who).color(color).size(11.5)).truncate();
                                    let r = match target {
                                        Some(_) => ui.add(lbl.sense(egui::Sense::click())),
                                        None => ui.add(lbl),
                                    };
                                    if !tip.is_empty() {
                                        r.clone().on_hover_text(tip);
                                    }
                                    if let (true, Some(tp)) = (r.clicked(), target) {
                                        click_select = Some(tp);
                                    }
                                }
                            });
                            row.col(|ui| {
                                ui.spacing_mut().item_spacing.x = 2.0;
                                // 419 linhas × 2 glifos coloridos era ruído puro. Os ícones ficam
                                // apagados e só "acendem" na linha sob o mouse (ou selecionada) —
                                // nada some, mas a tabela para de piscar vermelho inteira.
                                let cell = ui.max_rect();
                                let row_rect = Rect::from_x_y_ranges(row_x.unwrap_or(cell.x_range()), cell.y_range());
                                let hot = selected || ui.rect_contains_pointer(row_rect);
                                let btn_size = Vec2::new(24.0, 24.0);
                                if critical {
                                    ui.add_sized(btn_size, egui::Label::new(RichText::new("🔒").color(Color32::from_gray(if hot { 130 } else { 90 }))))
                                        .on_hover_text(self.cfg.locale.text("Crítico do sistema — não pode ser encerrado", "Critical system process — cannot be terminated"));
                                } else {
                                    let (icon, tip) = if locked { ("🔒", self.cfg.locale.text("Protegido — clique para desproteger", "Protected — click to unlock")) } else { ("🔓", self.cfg.locale.text("Clique para proteger (lock)", "Click to protect (lock)")) };
                                    let col = if locked {
                                        Color32::from_rgb(120, 200, 255)
                                    } else if hot {
                                        Color32::from_gray(160)
                                    } else {
                                        Color32::from_gray(90)
                                    };
                                    let b = egui::Button::new(RichText::new(icon).color(col)).frame(false).min_size(btn_size);
                                    if ui.add(b).on_hover_text(tip).clicked() {
                                        lock = Some(p.name_lower.clone());
                                    }
                                    if !locked {
                                        let kc = if hot { Color32::from_rgb(235, 90, 90) } else { Color32::from_gray(96) };
                                        let kb = egui::Button::new(RichText::new("✖").color(kc).size(11.0))
                                            .fill(if hot { SURFACE_HI } else { Color32::TRANSPARENT })
                                            .stroke(Stroke::NONE)
                                            .corner_radius(8.0)
                                            .min_size(btn_size);
                                        let r = ui.add(kb).on_hover_text(if p.kernel_state == Some('Z') {
                                            self.cfg.locale.text("Zumbi: já morreu, sinal nele não faz nada. Clique pede ao pai para recolher; Shift+clique finaliza o pai", "Zombie: it is already dead, so signals do nothing. Click asks the parent to reap it; Shift-click terminates the parent")
                                        } else {
                                            self.cfg.locale.text("Finalizar processo (Shift: árvore inteira)", "Terminate process (Shift: entire tree)")
                                        });
                                        if r.clicked() {
                                            let shift = ui.input(|i| i.modifiers.shift);
                                            kill = Some((pid, shift));
                                        }
                                    }
                                }
                            });
                            let resp = row.response();
                            if resp.clicked() {
                                click_select = Some(pid);
                            }
                            if resp.double_clicked() && tree && has_children {
                                toggle_expand = Some(pid);
                            }
                            resp.context_menu(|ui| {
                                ui.set_min_width(220.0);
                                ui.label(RichText::new(format!("{} — PID {}", p.name, p.pid)).strong());
                                ui.separator();
                                if !locked {
                                    if ui.button(self.cfg.locale.text("✖ Finalizar processo", "✖ Terminate process")).clicked() {
                                        kill = Some((pid, false));
                                        ui.close_menu();
                                    }
                                    let n = self.subtree_count.get(&pid).copied().unwrap_or(1);
                                    if p.kernel_state == Some('Z') {
                                        if let Some((pname, ppid)) = self.zombie_holder(pid, if p.ppid != 0 { p.ppid } else { p.raw_ppid }) {
                                            if ui.button(if self.cfg.locale == Locale::Portuguese { format!("✖ Finalizar pai: {pname} ({ppid})") } else { format!("✖ Terminate parent: {pname} ({ppid})") }).on_hover_text(self.cfg.locale.text("Zumbi não morre com sinal: já morreu. Some quando o pai recolhe ou cai", "Signals cannot kill a zombie because it is already dead. It disappears when the parent reaps it or exits")).clicked() {
                                                kill = Some((pid, true));
                                                ui.close_menu();
                                            }
                                        }
                                    } else if n > 1 {
                                        let label = if self.cfg.locale == Locale::Portuguese { format!("✖ Finalizar árvore ({n} processos)") } else { format!("✖ Terminate tree ({n} processes)") };
                                        if ui.button(label).clicked() {
                                        kill = Some((pid, true));
                                        ui.close_menu();
                                        }
                                    }
                                }
                                if !critical {
                                    let lt = if locked { self.cfg.locale.text("🔓 Desproteger", "🔓 Unlock") } else { self.cfg.locale.text("🔒 Proteger (lock)", "🔒 Protect (lock)") };
                                    if ui.button(lt).clicked() {
                                        lock = Some(p.name_lower.clone());
                                        ui.close_menu();
                                    }
                                }
                                ui.separator();
                                ui.menu_button(self.cfg.locale.text("Categoria", "Category"), |ui| {
                                    for c in Category::ALL {
                                        let cur = cat == c;
                                        if ui.selectable_label(cur, RichText::new(c.label_for(self.cfg.locale)).color(c.color())).clicked() {
                                            set_cat = Some((p.name_lower.clone(), Some(c)));
                                            ui.close_menu();
                                        }
                                    }
                                    ui.separator();
                                    if ui.button(self.cfg.locale.text("Regra automática", "Automatic rule")).clicked() {
                                        set_cat = Some((p.name_lower.clone(), None));
                                        ui.close_menu();
                                    }
                                });
                                if p.ppid != 0 && self.proc(p.ppid).is_some() && ui.button(self.cfg.locale.text("↑ Ir para o pai", "↑ Go to parent")).clicked() {
                                    click_select = Some(p.ppid);
                                    ui.close_menu();
                                }
                                ui.menu_button(self.cfg.locale.text("Ordenar por", "Sort by"), |ui| {
                                    for (k, l) in [
                                        (SortKey::Steal, self.cfg.locale.text("Disputa", "Contention")),
                                        (SortKey::Pid, "PID"),
                                        (SortKey::State, self.cfg.locale.text("Estado", "State")),
                                        (SortKey::Parent, self.cfg.locale.text("Origem", "Parent")),
                                        (SortKey::Cat, self.cfg.locale.text("Categoria", "Category")),
                                    ] {
                                        if ui.selectable_label(self.sort == k, l).clicked() {
                                            sort_by = Some(k);
                                            ui.close_menu();
                                        }
                                    }
                                });
                                ui.separator();
                                if !p.cmdline.is_empty() && ui.button(self.cfg.locale.text("Copiar linha de comando", "Copy command line")).clicked() {
                                    copy = Some(p.cmdline.clone());
                                    ui.close_menu();
                                }
                                if !p.exe_path.is_empty() {
                                    if ui.button(self.cfg.locale.text("Copiar caminho", "Copy path")).clicked() {
                                        copy = Some(p.exe_path.clone());
                                        ui.close_menu();
                                    }
                                    if ui.button(self.cfg.locale.text("Abrir pasta do executável", "Open executable folder")).clicked() {
                                        open_folder = Some(p.exe_path.clone());
                                        ui.close_menu();
                                    }
                                }
                            });
                        }
                    }
                });
            });

        // aplicar ações coletadas
        if let Some(pid) = click_select {
            self.selected = Some(pid);
            self.selected_keep = self.proc(pid).cloned().map(|p| (p, self.cat(pid)));
            self.scroll_to_selected = true;
        }
        if let Some(pid) = toggle_expand {
            if !self.expanded.remove(&pid) {
                self.expanded.insert(pid);
            }
        }
        if let Some(c) = toggle_cat {
            if !self.collapsed_cats.remove(&c) {
                self.collapsed_cats.insert(c);
            }
        }
        if let Some(k) = toggle_app {
            if !self.expanded_apps.remove(&k) {
                self.expanded_apps.insert(k);
            }
            self.row_cache.invalidate();
        }
        if let Some(gi) = kill_group {
            self.request_kill_app(gi);
        }
        if let Some((pid, tree)) = kill {
            self.request_kill(pid, tree);
        }
        if let Some(name) = lock {
            self.toggle_lock(&name);
        }
        if let Some((name, c)) = set_cat {
            self.set_override(&name, c);
        }
        if let Some(k) = sort_by {
            self.sort = k;
            self.sort_desc = !matches!(k, SortKey::Name | SortKey::Parent | SortKey::Cat);
        }
        if let Some(s) = copy {
            ui.ctx().copy_text(s);
            self.toast("Copiado".into(), false);
        }
        if let Some(path) = open_folder {
            open_in_explorer(&path);
        }
    }

    /// Barra de magnitude: um traço de 3 px no pé da célula, não um bloco atrás do número.
    fn cell_bar(ui: &egui::Ui, frac: f32, color: Color32) {
        if frac <= 0.01 {
            return;
        }
        let cell = ui.max_rect();
        let w = (cell.width() - 10.0).max(0.0) * frac.clamp(0.0, 1.0);
        let bar = Rect::from_min_size(
            egui::pos2(cell.right() - 6.0 - w, cell.bottom() - 6.0),
            Vec2::new(w, 3.0),
        );
        ui.painter()
            .rect_filled(bar, 1.5, color.gamma_multiply(0.75));
    }

    /// Linha separadora no pé de cada linha da tabela (a zebra saiu).
    fn row_line(ui: &egui::Ui, row_x: Option<egui::Rangef>) {
        let cell = ui.max_rect();
        ui.painter().hline(
            row_x.unwrap_or(cell.x_range()),
            cell.bottom() + 1.0,
            Stroke::new(1.0_f32, LINE),
        );
    }

    // ---------- visão Térmico ----------

    /// Sensores + controle de fans + ESTABILIZAR — o TempHUD embutido no RamDog. Toda leitura
    /// e toda escrita de hardware acontecem no helper `hwtemp.exe` (a curva mora lá, por
    /// segurança); aqui é só UI: comandos saem pelo stdin dele, o estado volta no snapshot.
    fn ui_thermal(&mut self, ui: &mut egui::Ui, locale: Locale) {
        let hw = self.hwtemp.clone();
        let cmd = self.sampler.hw_cmd.clone();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(8.0);
            if hw.sensors.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(40.0);
                    ui.label(RichText::new(locale.text("Sem leitura de sensores", "No sensor readings")).strong().size(15.0));
                    let why = if cfg!(target_os = "macos") {
                        locale.text("A visão Térmico no macOS ainda não está ligada.", "Thermal view is not connected on macOS yet.")
                    } else if cfg!(target_os = "linux") {
                        locale.text("Nenhum sensor em /sys/class/hwmon. Sem coretemp/k10temp/zenpower o kernel não expôs temperatura.", "No sensor under /sys/class/hwmon. Without coretemp/k10temp/zenpower, the kernel exposed no temperature.")
                    } else if !cfg!(windows) {
                        locale.text("A visão Térmico ainda é só Windows: depende do helper hwtemp.exe (LibreHardwareMonitorLib).", "Thermal view is still Windows-only here: it needs hwtemp.exe (LibreHardwareMonitorLib).")
                    } else if cmd.is_none() {
                        locale.text("hwtemp.exe não foi achado ao lado do ramdog.exe — reinstale com o helper junto.", "hwtemp.exe was not found beside ramdog.exe — reinstall with the helper included.")
                    } else {
                        locale.text("O helper subiu mas ainda não reportou. Se persistir: .NET 8 Desktop Runtime ausente, ou placa-mãe sem Super I/O suportado pela LibreHardwareMonitor.", "The helper started but has not reported yet. If it persists: missing .NET 8 Desktop Runtime, or a motherboard without LibreHardwareMonitor-supported Super I/O.")
                    };
                    ui.label(RichText::new(why).color(MUTED));
                });
                return;
            }

            #[cfg(target_os = "linux")]
            {
                if let Some(error) = &hw.control_error {
                    let display_error = match error.as_str() {
                        "O helper precisa de autenticação administrativa" => locale.text(
                            "O helper precisa de autenticação administrativa",
                            "The fan helper requires administrator authentication",
                        ),
                        "Processo pai indisponível" => {
                            locale.text("Processo pai indisponível", "Parent process unavailable")
                        }
                        "Driver sem controles PWM graváveis" => locale.text(
                            "Driver sem controles PWM graváveis",
                            "The driver exposes no writable PWM controls",
                        ),
                        "Fan desconhecido" => locale.text("Fan desconhecido", "Unknown fan"),
                        "Comando incompleto" => locale.text("Comando incompleto", "Incomplete command"),
                        "A faixa manual é 30–100%" => {
                            locale.text("A faixa manual é 30–100%", "Manual range is 30–100%")
                        }
                        "Comando desconhecido" => locale.text("Comando desconhecido", "Unknown command"),
                        _ => error.as_str(),
                    };
                    ui.colored_label(egui::Color32::LIGHT_RED, display_error);
                }
                if !hw.control_ready {
                    if crate::fans_linux::supported(){
                        crate::kit::toolbar(ui, |ui| {
                            if ui.add(crate::kit::primary(locale.text("Ativar controle de ventoinhas", "Enable fan control"))).on_hover_text(locale.text("Usa sudo autorizado ou pede senha (pkexec)", "Uses authorized sudo or requests password (pkexec)")).clicked(){crate::fans_linux::enable();}
                            ui.label(crate::kit::muted(locale.text("Um helper separado restaura o controle anterior quando o RamDog fecha. Faixa manual: 30–100%.", "A separate helper restores the previous control state when RamDog closes. Manual range: 30–100%.")));
                        });
                    } else {crate::kit::intro(ui, locale.text("Rotações disponíveis abaixo. O driver atual não oferece controles PWM graváveis.", "Read-only fan speeds are shown below. The current driver exposes no writable PWM controls."));}
                }
            }
            // Cartões de sensores: um por hardware, na ordem em que o helper reporta;
            // a temperatura mais alta do hardware vira o número-herói do cartão.
            let mut groups: Vec<(&str, Vec<&crate::hwtemp::SensorRow>)> = Vec::new();
            for s in &hw.sensors {
                match groups.iter_mut().find(|(h, _)| *h == s.hw) {
                    Some((_, rows)) => rows.push(s),
                    None => groups.push((s.hw.as_str(), vec![s])),
                }
            }
            let ncols = groups.len().clamp(1, 4);
            for chunk in groups.chunks(ncols) {
                ui.columns(ncols, |cols| {
                    for (col, (hw_name, rows)) in cols.iter_mut().zip(chunk.iter()) {
                        Self::thermal_card(col, hw_name, rows, locale);
                    }
                });
                ui.add_space(10.0);
            }

            // Console de fans: bloco ESTABILIZAR + curva à esquerda, linhas de fan à direita.
            if !hw.fans.is_empty() {
                let stab = hw.stab;
                let stab_on = self.stab_on();
                let cpu = hw.cpu_temp.unwrap_or(0.0);
                let now = Instant::now();
                egui::Frame::new()
                    .fill(BG)
                    .corner_radius(crate::kit::ROW_R)
                    .inner_margin(egui::Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(locale.text("CONTROLE DE FANS", "FAN CONTROL")).strong().color(ACCENT).size(11.0));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(
                                    RichText::new(locale.text("arraste ou digite % · Auto = BIOS · proteção no modo manual: ≥80°C força 100%, solta abaixo de 72°C", "drag or type % · Auto = BIOS · manual protection: ≥80°C forces 100%, releases below 72°C"))
                                        .color(MUTED)
                                        .size(11.0),
                                );
                            });
                        });
                        ui.add_space(8.0);
                        ui.horizontal_top(|ui| {
                            // Bloco ESTABILIZAR: botão (3 estados), curva desenhada e as regras.
                            ui.vertical(|ui| {
                                ui.set_width(286.0);
                                let (label, bg, fg) = if !stab_on {
                                    (locale.text("ESTABILIZAR  ·  fans conectados em 50%", "STABILIZE  ·  connected fans at 50%").to_owned(), ACCENT_BG, ACCENT)
                                } else if stab.held > 50.5 {
                                    let tag = if cpu >= 95.0 { locale.text("teto térmico", "thermal ceiling") } else { locale.text("rampa linear", "linear ramp") };
                                    (format!("{} {:.0}%  ·  CPU {cpu:.0}°C ({tag})", locale.text("FANS EM", "FANS AT"), stab.held), THERM_WARN_BG, THERM_WARN_FG)
                                } else {
                                    (format!("{} 50%  ·  CPU {cpu:.0}°C", locale.text("FANS TRAVADOS EM", "FANS LOCKED AT")), THERM_STAB_BG, THERM_STAB_FG)
                                };
                                let btn = egui::Button::new(RichText::new(label).strong().size(13.5).color(fg))
                                    .fill(bg)
                                    .stroke(Stroke::new(1.0_f32, fg.gamma_multiply(0.7)))
                                    .corner_radius(6.0)
                                    .min_size(Vec2::new(ui.available_width(), 34.0));
                                let resp = ui.add(btn).on_hover_text(locale.text(
                                    "Liga/desliga a curva do TempHUD. Só fans conectados (CPU e gabinete) entram na estabilização; bombas e headers sem RPM ficam na BIOS. Clicar de novo (ou fechar o app) devolve os fans controlados à BIOS.",
                                    "Toggles the TempHUD curve. Only connected CPU/case fans are stabilized; pumps and headers with no RPM stay on BIOS control. Closing the app restores the controlled fans to BIOS.",
                                ));
                                if resp.clicked() {
                                    self.toggle_stab();
                                }
                                ui.add_space(4.0);
                                Self::thermal_curve(ui);
                                ui.add_space(2.0);
                                ui.label(
                                    RichText::new(locale.text(
                                        "50% até 80°C, rampa linear até 100% aos 92°C, teto imediato a 95°C. Sobe/desce no máx. 3%/s. Soltar devolve tudo à BIOS.",
                                        "50% to 80°C, linear ramp to 100% at 92°C, immediate ceiling at 95°C. Changes by at most 3%/s. Releasing returns control to BIOS.",
                                    ))
                                        .color(MUTED)
                                        .size(11.0),
                                );
                            });
                            ui.add_space(8.0);
                            ui.separator();
                            ui.add_space(4.0);
                            // Linhas de fan em duas colunas; cor do preenchimento por estado
                            // (folha "Estados" do design): azul apagado = Auto, azul = manual,
                            // laranja = proteção, verde = curva ativa.
                            ui.vertical(|ui| {
                                ui.columns(2, |cols| {
                                    for (i, f) in hw.fans.iter().enumerate() {
                                        let col = &mut cols[i % 2];
                                        col.horizontal(|ui| {
                                            let absent = f.rpm.unwrap_or(0.0) <= 0.0;
                                            if absent {
                                                ui.set_opacity(0.45);
                                            } else if stab_on {
                                                ui.set_opacity(0.7);
                                            }
                                            ui.scope(|ui| {
                                                ui.set_width(96.0);
                                                ui.add(egui::Label::new(RichText::new(f.name.replace("System ", "Sys ")).size(12.0)).truncate());
                                            });
                                            let helper_pct = f.pct.unwrap_or(50.0);
                                            let editing = self
                                                .thermal_edit
                                                .get(&f.name)
                                                .is_some_and(|(_, t)| now.duration_since(*t).as_secs_f32() < 2.5);
                                            let mut v = if editing { self.thermal_edit[&f.name].0 } else { helper_pct };
                                            let fill = if stab_on {
                                                Color32::from_rgb(30, 74, 60)
                                            } else if f.guard {
                                                Color32::from_rgb(255, 138, 101)
                                            } else if !f.auto {
                                                ACCENT
                                            } else {
                                                Color32::from_rgb(58, 85, 124)
                                            };
                                            ui.visuals_mut().selection.bg_fill = fill;
                                            ui.spacing_mut().slider_width = (ui.available_width() - 180.0).max(60.0);
                                            let sl = ui.add_enabled(!stab_on, egui::Slider::new(&mut v, if cfg!(target_os="linux"){30.0..=100.0}else{0.0..=100.0}).show_value(false).trailing_fill(true));
                                            let dv = ui.add_enabled(!stab_on, egui::DragValue::new(&mut v).range(if cfg!(target_os="linux"){30.0..=100.0}else{0.0..=100.0}).max_decimals(0).suffix("%"));
                                            if sl.changed() || dv.changed() {
                                                self.thermal_edit.insert(f.name.clone(), (v, now));
                                            }
                                            // Comando só no fim do gesto (soltar o drag / commit do campo) — o
                                            // helper aplica em ≤100ms; mandar a cada frame só encheria o pipe.
                                            let commit = sl.drag_stopped()
                                                || dv.drag_stopped()
                                                || ((sl.changed() || dv.changed()) && !sl.dragged() && !dv.dragged());
                                            if commit {
                                                if let Some(c) = &cmd {
                                                    c.send(&format!("set {v:.0} {}", f.name));
                                                }
                                            }
                                            let rpm_color = if f.guard { THERM_WARN_FG } else { MUTED };
                                            let rpm_text = f.rpm.map(|r| format!("{r:>5.0} RPM")).unwrap_or_else(|| "    – RPM".into());
                                            let r = ui.label(num(rpm_text).color(rpm_color));
                                            if f.guard {
                                                r.on_hover_text(locale.text(
                                                    "Proteção térmica: CPU ≥80°C — 100% forçado sobre o % manual até esfriar (72°C).",
                                                    "Thermal protection: CPU ≥80°C — forces 100% over the manual percentage until it cools below 72°C.",
                                                ));
                                            }
                                            let mut auto = f.auto;
                                            if ui.add_enabled(!stab_on, egui::Checkbox::new(&mut auto, "Auto")).changed() {
                                                if let Some(c) = &cmd {
                                                    if auto {
                                                        c.send(&format!("auto {}", f.name));
                                                        self.thermal_edit.remove(&f.name);
                                                    } else {
                                                        c.send(&format!("set {v:.0} {}", f.name));
                                                        self.thermal_edit.insert(f.name.clone(), (v, now));
                                                    }
                                                }
                                            }
                                        });
                                    }
                                });
                            });
                        });
                    });
            } else if cfg!(windows) {
                ui.add_space(14.0);
                let why = if self.is_admin {
                    locale.text(
                        "Sem controles de fan: a placa-mãe não expôs Super I/O suportado pela LibreHardwareMonitor.",
                        "No fan controls: the motherboard exposed no LibreHardwareMonitor-supported Super I/O.",
                    )
                } else {
                    locale.text(
                        "Sem controles de fan: rode elevado (botão \"Reabrir como admin\" no topo) — o driver de sensores não sobe sem isso.",
                        "No fan controls: run elevated (use \"Reopen as admin\" at the top) — the sensor driver needs it.",
                    )
                };
                ui.label(RichText::new(why).color(MUTED));
            } else if cfg!(target_os = "linux") {
                ui.add_space(14.0);
                ui.label(RichText::new(locale.text(
                    "Sensores e RPM são lidos pelo hwmon. Ative o controle acima para ajustar PWM ou usar ESTABILIZAR; ao fechar, o helper restaura o estado anterior.",
                    "Sensors and RPM are read from hwmon. Enable the control above to adjust PWM or use STABILIZE; the helper restores the previous state when the app closes.",
                )).color(MUTED));
            }
        });
    }

    /// Um cartão de hardware da visão Térmico: cabeçalho, número-herói (temperatura mais
    /// alta) e as demais leituras em linhas compactas; cargas ganham uma minibarra.
    fn thermal_card(
        ui: &mut egui::Ui,
        hw_name: &str,
        rows: &[&crate::hwtemp::SensorRow],
        locale: Locale,
    ) {
        egui::Frame::new()
            .fill(BG)
            .corner_radius(crate::kit::ROW_R)
            .inner_margin(egui::Margin::symmetric(14, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 3.0;
                let display_hw = match hw_name {
                    "Placa-mãe" => locale.text("Placa-mãe", "Motherboard"),
                    "Disco" => locale.text("Disco", "Disk"),
                    _ => hw_name,
                };
                ui.label(RichText::new(display_hw).strong().color(ACCENT).size(11.0));
                let hero = rows
                    .iter()
                    .filter(|s| s.kind == "temp")
                    .max_by(|a, b| a.value.total_cmp(&b.value))
                    .copied();
                if let Some(h) = hero {
                    let n_temps = rows.iter().filter(|s| s.kind == "temp").count();
                    ui.label(
                        num(format!("{:.1} °C", h.value))
                            .size(24.0)
                            .strong()
                            .color(Self::temp_color(h.value.round() as u32)),
                    );
                    let sub = if n_temps > 1 {
                        format!(
                            "{} · {}",
                            h.name,
                            locale.text("sensor mais quente", "hottest sensor")
                        )
                    } else {
                        h.name.clone()
                    };
                    ui.label(RichText::new(sub).color(MUTED).size(11.0));
                    ui.add_space(3.0);
                }
                for s in rows {
                    if hero.is_some_and(|h| h.name == s.name && h.kind == s.kind) {
                        continue;
                    }
                    let (text, color) = match s.kind.as_str() {
                        "temp" => (
                            format!("{:>5.1} °C", s.value),
                            Self::temp_color(s.value.round() as u32),
                        ),
                        "rpm" => (format!("{:>5.0} RPM", s.value), MUTED),
                        _ => (
                            format!("{:>5.1} %", s.value),
                            Self::load_color(s.value / 100.0),
                        ),
                    };
                    let is_load = !matches!(s.kind.as_str(), "temp" | "rpm");
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        let avail = ui.available_width();
                        ui.scope(|ui| {
                            ui.set_width((avail - 148.0).max(40.0));
                            ui.add(
                                egui::Label::new(RichText::new(&s.name).color(MUTED).size(12.0))
                                    .truncate(),
                            );
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(num(text).color(color));
                            if is_load {
                                ui.add_space(8.0);
                                Self::thermal_minibar(ui, s.value / 100.0, color);
                            }
                        });
                    });
                }
            });
    }

    /// Minibarra de carga dos cartões (trilho escuro + preenchimento na cor da faixa).
    fn thermal_minibar(ui: &mut egui::Ui, frac: f32, color: Color32) {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(52.0, 5.0), egui::Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, 2.0, Color32::from_rgb(30, 34, 41));
        let mut fill = rect;
        fill.set_width(rect.width() * frac.clamp(0.0, 1.0));
        p.rect_filled(fill, 2.0, color);
    }

    /// Diagrama da curva do ESTABILIZAR: 50% até 80°C, rampa linear até 100% aos 92°C.
    fn thermal_curve(ui: &mut egui::Ui) {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(272.0, 96.0), egui::Sense::hover());
        let p = ui.painter();
        let x0 = rect.left() + 30.0;
        let x1 = rect.right() - 8.0;
        let y100 = rect.top() + 14.0;
        let y50 = rect.top() + 54.0;
        let yax = rect.top() + 68.0;
        let x80 = x0 + (x1 - x0) * 0.55;
        let x92 = x0 + (x1 - x0) * 0.82;
        p.line_segment(
            [egui::pos2(x0, rect.top() + 6.0), egui::pos2(x0, yax)],
            Stroke::new(1.0_f32, LINE),
        );
        p.line_segment(
            [egui::pos2(x0, yax), egui::pos2(x1, yax)],
            Stroke::new(1.0_f32, LINE),
        );
        p.line_segment(
            [egui::pos2(x0, y50), egui::pos2(x80, y50)],
            Stroke::new(2.0_f32, ACCENT),
        );
        p.line_segment(
            [egui::pos2(x80, y50), egui::pos2(x92, y100)],
            Stroke::new(2.0_f32, ACCENT),
        );
        p.line_segment(
            [egui::pos2(x92, y100), egui::pos2(x1, y100)],
            Stroke::new(2.0_f32, ACCENT),
        );
        p.circle_filled(egui::pos2(x80, y50), 3.0, ACCENT);
        p.circle_filled(egui::pos2(x92, y100), 3.0, Color32::from_rgb(226, 166, 72));
        let font = egui::FontId::monospace(9.5);
        p.text(
            egui::pos2(x0 - 4.0, y50),
            egui::Align2::RIGHT_CENTER,
            "50%",
            font.clone(),
            MUTED,
        );
        p.text(
            egui::pos2(x0 - 4.0, y100),
            egui::Align2::RIGHT_CENTER,
            "100%",
            font.clone(),
            MUTED,
        );
        p.text(
            egui::pos2(x80, yax + 4.0),
            egui::Align2::CENTER_TOP,
            "80°C",
            font.clone(),
            MUTED,
        );
        p.text(
            egui::pos2(x92, yax + 4.0),
            egui::Align2::CENTER_TOP,
            "92°C",
            font,
            MUTED,
        );
    }

    fn ui_details(&mut self, ui: &mut egui::Ui) {
        let locale = self.cfg.locale;
        let Some(sel) = self.selected else {
            // Estado vazio útil: em vez de só instruir, já responde "quem está comendo minha RAM".
            let m = self.cfg.mem_metric;
            let mut top: Vec<&ProcInfo> = self.procs.iter().collect();
            top.sort_by_key(|p| std::cmp::Reverse(Self::metric_of(m, p)));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                ui.label(
                    RichText::new(locale.text("Maiores agora", "Largest now"))
                        .color(MUTED)
                        .size(11.5),
                );
                for p in top.iter().take(4) {
                    let cat = self.cat(p.pid);
                    ui.add_space(6.0);
                    ui.label(RichText::new("●").color(cat.color()).size(10.0));
                    ui.label(RichText::new(&p.name).size(12.0));
                    ui.label(
                        num(if metric_available(m, p) {
                            fmt_bytes(Self::metric_of(m, p))
                        } else {
                            "—".into()
                        })
                        .color(ram_color(Self::metric_of(m, p), MUTED)),
                    );
                }
            });
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                ui.label(
                    RichText::new(locale.text(
                        "clique numa linha para ver origem e ações · botão direito abre o menu",
                        "click a row to see origin and actions · right-click opens the menu",
                    ))
                    .color(MUTED)
                    .size(11.0),
                );
            });
            return;
        };
        let alive = self.proc(sel).cloned();
        let Some((p, cat)) = alive
            .clone()
            .map(|p| (p, self.cat(sel)))
            .or_else(|| self.selected_keep.clone())
        else {
            self.selected = None;
            return;
        };
        let locked = self.is_locked(&p);
        let critical = is_critical(&p.name_lower, p.pid);
        // Só enquanto existe: depois de finalizar o pai, o `selected_keep` ainda diz `Z`.
        let zombie = alive.is_some() && p.kernel_state == Some('Z');
        let holder = if zombie {
            self.zombie_holder(p.pid, if p.ppid != 0 { p.ppid } else { p.raw_ppid })
        } else {
            None
        };
        let now_ft = procs::now_filetime();
        let secs = ((now_ft - p.create_time).max(0) / 10_000_000) as u64;
        let mut kill: Option<(u32, bool)> = None;
        let mut lock: Option<String> = None;
        let mut set_cat: Option<(String, Option<Category>)> = None;
        let mut goto: Option<u32> = None;

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let key = p.exe_path.to_lowercase();
            if let Some(Some(tex)) = self.icons.get(&key) {
                ui.add(egui::Image::new((tex.id(), Vec2::splat(20.0))));
            }
            ui.label(RichText::new(identity::of(&p).label).strong().size(15.0));
            if p.name != identity::of(&p).label {
                ui.label(RichText::new(format!("({})", p.name)).monospace().weak());
            }
            ui.label(RichText::new(format!("PID {}", p.pid)).monospace().weak());
            ui.label(
                RichText::new(format!("● {}", cat.label_for(self.cfg.locale))).color(cat.color()),
            );
            if locked {
                ui.label(
                    RichText::new(if critical {
                        locale.text("🔒 crítico", "🔒 critical")
                    } else {
                        locale.text("🔒 protegido", "🔒 protected")
                    })
                    .color(Color32::from_rgb(120, 200, 255)),
                );
            }
            if alive.is_none() {
                ui.label(
                    RichText::new(locale.text("(encerrado)", "(exited)"))
                        .color(Color32::from_rgb(235, 90, 90))
                        .strong(),
                );
            } else if zombie {
                ui.label(
                    RichText::new(locale.text(
                        "zumbi · já morreu, 0 de RAM",
                        "zombie · already dead, 0 RAM",
                    ))
                    .color(Color32::from_rgb(230, 120, 120))
                    .strong(),
                );
            }
        });
        // Ações em fileira própria: à direita da identidade elas atropelavam o nome quando a
        // janela não era larga o bastante (right_to_left não clipa).
        ui.horizontal(|ui| {
            ui.spacing_mut().button_padding = Vec2::new(10.0, 4.0);
            {
                if alive.is_some() {
                    if zombie {
                        // Sinal num zumbi não faz nada; os dois caminhos reais ganham botão
                        // com nome, em vez de um Shift que ninguém descobre.
                        if ui.button(locale.text("Pedir ao pai para recolher", "Ask parent to reap")).on_hover_text(locale.text("Manda SIGCHLD ao pai. Pai bem escrito recolhe na hora; se não recolher, só finalizando ele", "Sends SIGCHLD to the parent. A well-behaved parent reaps immediately; otherwise it must be terminated")).clicked() {
                            kill = Some((p.pid, false));
                        }
                        if let Some((pname, ppid)) = &holder {
                            if ui
                                .add(egui::Button::new(RichText::new(if locale == Locale::Portuguese { format!("✖ Finalizar pai: {pname} ({ppid})") } else { format!("✖ Terminate parent: {pname} ({ppid})") }).color(Color32::WHITE)).fill(Color32::from_rgb(170, 50, 50)))
                                .on_hover_text(locale.text("O pai é quem segura a entrada. Finalizando ele, o zumbi some junto", "The parent is holding the entry. Terminating it also removes the zombie"))
                                .clicked()
                            {
                                kill = Some((p.pid, true));
                            }
                        }
                    } else if !locked {
                        if ui.add(egui::Button::new(RichText::new(locale.text("✖ Finalizar", "✖ Terminate")).color(Color32::WHITE)).fill(Color32::from_rgb(170, 50, 50))).clicked() {
                            kill = Some((p.pid, false));
                        }
                        let n = self.subtree_count.get(&p.pid).copied().unwrap_or(1);
                        if n > 1
                            && ui
                                .add(egui::Button::new(RichText::new(if locale == Locale::Portuguese { format!("✖ Finalizar árvore ({n})") } else { format!("✖ Terminate tree ({n})") }).color(Color32::WHITE)).fill(Color32::from_rgb(140, 40, 40)))
                                .on_hover_text(if locale == Locale::Portuguese { format!("Encerra este processo e todos os {} descendentes — total {}", n - 1, fmt_bytes(self.subtree.get(&p.pid).copied().unwrap_or(0))) } else { format!("Terminates this process and all {} descendants — total {}", n - 1, fmt_bytes(self.subtree.get(&p.pid).copied().unwrap_or(0))) })
                                .clicked()
                        {
                            kill = Some((p.pid, true));
                        }
                    }
                    if !critical {
                        let lt = if locked { locale.text("🔓 Desproteger", "🔓 Unlock") } else { locale.text("🔒 Proteger", "🔒 Protect") };
                        if ui.button(lt).on_hover_text(locale.text("Lock por nome de executável: vale para todas as instâncias e persiste", "Executable-name lock: applies to every instance and persists")).clicked() {
                            lock = Some(p.name_lower.clone());
                        }
                    }
                }
                ui.add_space(6.0);
                ui.label(RichText::new(locale.text("Categoria", "Category")).weak());
                let overridden = self.cfg.overrides.contains_key(&p.name_lower);
                let mut chosen = cat;
                egui::ComboBox::from_id_salt("cat_override")
                    .selected_text(RichText::new(chosen.label_for(self.cfg.locale)).color(chosen.color()))
                    .width(130.0)
                    .show_ui(ui, |ui| {
                        for c in Category::ALL {
                            ui.selectable_value(&mut chosen, c, RichText::new(c.label_for(self.cfg.locale)).color(c.color()));
                        }
                    });
                if chosen != cat {
                    set_cat = Some((p.name_lower.clone(), Some(chosen)));
                }
                if overridden && ui.small_button("auto").on_hover_text(locale.text("Voltar para a regra automática", "Return to the automatic rule")).clicked() {
                    set_cat = Some((p.name_lower.clone(), None));
                }
            }
        });
        // Fora do closure: a verificação precisa de `&mut self` e dispara a thread na
        // primeira vez que este executável é selecionado.
        let sig = self.signature_of(&p.exe_path, &ui.ctx().clone());
        ui.add_space(2.0);
        egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
            egui::Grid::new("details_grid").num_columns(2).spacing([10.0, 3.0]).show(ui, |ui| {
                // "O que é isso" vem antes de tudo: é a pergunta que faz alguém abrir o painel.
                if let Some(k) = knowledge::lookup(&p.name_lower) {
                    ui.label(RichText::new(locale.text("O que é", "What is it")).weak());
                    ui.vertical(|ui| {
                        ui.add(egui::Label::new(RichText::new(k.what_for(locale)).size(12.5)).wrap());
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 5.0;
                            ui.label(
                                RichText::new(format!("{} {}", k.risk.dot(), k.risk.label_for(locale)))
                                    .color(k.risk.color())
                                    .strong()
                                    .size(12.0),
                            )
                            .on_hover_text(k.risk.tip_for(locale));
                            ui.add(egui::Label::new(RichText::new(k.why_for(locale)).weak().size(11.5)).wrap());
                        });
                    });
                    ui.end_row();
                }

                let svcs = self.services_of(p.pid);
                if !svcs.is_empty() {
                    ui.label(RichText::new(locale.text("Serviços", "Services")).weak());
                    ui.vertical(|ui| {
                        // Um svchost pode hospedar uma dúzia: os primeiros bastam para
                        // identificar, o resto fica atrás do expansor para não poluir.
                        for (name, display) in svcs.iter().take(3) {
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing.x = 5.0;
                                ui.label(RichText::new(display).size(12.0).color(Color32::from_rgb(140, 200, 255)));
                                ui.label(RichText::new(name).monospace().small().weak());
                            });
                        }
                        if svcs.len() > 3 {
                            egui::CollapsingHeader::new(if locale == Locale::Portuguese { format!("mais {} serviço(s)", svcs.len() - 3) } else { format!("more {} service(s)", svcs.len() - 3) })
                                .id_salt("more_svcs")
                                .show(ui, |ui| {
                                    for (name, display) in svcs.iter().skip(3) {
                                        ui.horizontal_wrapped(|ui| {
                                            ui.spacing_mut().item_spacing.x = 5.0;
                                            ui.label(RichText::new(display).size(12.0).color(Color32::from_rgb(140, 200, 255)));
                                            ui.label(RichText::new(name).monospace().small().weak());
                                        });
                                    }
                                });
                        }
                    });
                    ui.end_row();
                }

                if zombie {
                    ui.label(RichText::new(locale.text("Zumbi", "Zombie")).weak());
                    ui.vertical(|ui| {
                        ui.add(egui::Label::new(RichText::new(locale.text("Este processo já terminou: não usa memória nem CPU. A linha existe porque o pai ainda não recolheu o código de saída (wait).", "This process has already exited: it uses no memory or CPU. The row exists because its parent has not reaped the exit status yet (wait). ")).size(12.0)).wrap());
                        let txt = match &holder {
                            Some((pname, ppid)) if locale == Locale::Portuguese => format!("Quem segura: {pname} ({ppid}). Some quando ele recolher ou for finalizado."),
                            Some((pname, ppid)) => format!("Holder: {pname} ({ppid}). It disappears when the parent reaps it or is terminated."),
                            None => locale.text("O pai já saiu: o init recolhe sozinho em instantes.", "The parent has exited: init will reap it shortly.").to_string(),
                        };
                        ui.add(egui::Label::new(RichText::new(txt).size(12.0).color(Color32::from_rgb(230, 120, 120))).wrap());
                    });
                    ui.end_row();
                }

                ui.label(RichText::new(locale.text("Origem", "Origin")).weak());
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let chain = self.ancestry(p.pid);
                    if chain.is_empty() {
                        if p.raw_ppid != 0 {
                            // Com o nome guardado a linha para de ser um número sem sentido.
                            match self.seen_names.get(&p.raw_ppid) {
                                Some(n) => {
                                    ui.label(RichText::new(format!("{n} (PID {})", p.raw_ppid)).strong());
                                    ui.label(RichText::new(locale.text("— já encerrado", "— exited")).weak());
                                }
                                None => {
                                    ui.label(RichText::new(format!("{} (PID {}) {}", locale.text("pai", "parent"), p.raw_ppid, locale.text("já encerrado", "exited"))).weak());
                                }
                            }
                        } else {
                            ui.label(RichText::new(locale.text("sem pai conhecido", "parent unknown")).weak());
                        }
                    }
                    for a in chain {
                        if let Some(ap) = self.proc(a) {
                            let r = ui.add(egui::Label::new(RichText::new(format!("{} ({})", ap.name, ap.pid)).color(self.cat(a).color())).sense(egui::Sense::click()));
                            if r.on_hover_text(locale.text("Selecionar", "Select")).clicked() {
                                goto = Some(a);
                            }
                            ui.label(RichText::new("›").weak());
                        }
                    }
                    ui.label(RichText::new(format!("{} ({})", p.name, p.pid)).strong());
                });
                ui.end_row();

                if !p.launcher.is_empty() {
                    ui.label(RichText::new(locale.text("Lançado por", "Launched by")).weak());
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let l = &p.launcher;
                        let purple = Color32::from_rgb(200, 160, 255);
                        if let Some(a) = &l.agent {
                            let mut txt = a.clone();
                            if let Some(sid) = &l.session {
                                txt.push_str(&format!(" ({} {sid})", locale.text("sessão", "session")));
                            }
                            ui.label(RichText::new(txt).color(purple).strong());
                            match l.agent_pid {
                                Some(apid) => match self.proc(apid) {
                                    Some(ap) => {
                                        let r = ui.add(egui::Label::new(RichText::new(format!("→ {} ({})", ap.name, apid)).color(self.cat(apid).color())).sense(egui::Sense::click()));
                                        if r.on_hover_text(locale.text("Selecionar o processo do agente", "Select the agent process")).clicked() {
                                            goto = Some(apid);
                                        }
                                    }
                                    None => { ui.label(RichText::new(format!("→ PID {apid} ({})", locale.text("já encerrado", "exited"))).weak()); }
                                },
                                None => {}
                            }
                        }
                        if let Some(h) = &l.host {
                            ui.label(RichText::new(format!("{}em {}", if l.agent.is_some() { "· " } else { "" }, h)).color(purple));
                        }
                        if let Some(cwd) = &l.init_cwd {
                            let script = l.npm_script.clone().map(|x| format!("npm run {x} ")).unwrap_or_default();
                            ui.label(RichText::new(format!("· {script}em {cwd}")).monospace().small().weak());
                        }
                        ui.label(RichText::new(locale.text("(via variáveis de ambiente herdadas)", "(via inherited environment variables)")).weak().small());
                    });
                    ui.end_row();
                }

                ui.label(RichText::new(locale.text("Executável", "Executable")).weak());
                ui.vertical(|ui| {
                    ui.add(egui::Label::new(RichText::new(if p.exe_path.is_empty() { locale.text("(sem acesso)", "(access unavailable)") } else { &p.exe_path }).monospace().small()).wrap());
                    // Sem isto não há como distinguir o wininit.exe verdadeiro de um
                    // impostor de mesmo nome numa pasta qualquer.
                    match sig.as_ref() {
                        Some(s) => {
                            ui.label(RichText::new(s.label_for(locale)).color(s.color()).size(11.5))
                                .on_hover_text(s.tip_for(locale));
                        }
                        None => {
                            ui.label(RichText::new(locale.text("verificando assinatura…", "checking signature…")).weak().size(11.5));
                        }
                    }
                });
                ui.end_row();

                ui.label(RichText::new(locale.text("Comando", "Command")).weak());
                ui.add(egui::Label::new(RichText::new(if p.cmdline.is_empty() { locale.text("(sem acesso)", "(access unavailable)") } else { &p.cmdline }).monospace().small()).wrap());
                ui.end_row();

                ui.label(RichText::new(locale.text("Memória", "Memory")).weak());
                // Próprio e subárvore em linhas separadas e rotuladas: ler "wininit 5,12 GB"
                // e concluir que o wininit é o vilão é o mal-entendido nº 1 da visão de árvore.
                // Ele usa 8 MB; os 5 GB são dos filhos.
                let sub = self.subtree.get(&p.pid).copied().unwrap_or(self.mem_of(&p));
                let n = self.subtree_count.get(&p.pid).copied().unwrap_or(1);
                ui.vertical(|ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 5.0;
                        ui.label(RichText::new(locale.text("deste processo:", "this process:")).weak().size(11.5));
                        ui.label(RichText::new(fmt_bytes(p.working_set)).strong());
                        ui.label(
                            RichText::new(format!(
                                "({} {} · {} {})",
                                locale.text("privada", "private"),
                                private_memory_text(&p, locale),
                                MemMetric::Commit.label_for(locale),
                                fmt_bytes(p.commit)
                            ))
                            .weak()
                            .size(11.5),
                        );
                    });
                    if n > 1 {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 5.0;
                            ui.label(RichText::new(locale.text("com os filhos:", "with children:")).weak().size(11.5));
                            ui.label(RichText::new(fmt_bytes(sub)).strong().color(Color32::from_rgb(230, 190, 80)));
                            ui.label(RichText::new(format!("{} {n} {}", locale.text("em", "in"), locale.text("processos", "processes"))).weak().size(11.5))
                                .on_hover_text(locale.text("É este o número que a coluna RAM mostra na visão de árvore — a soma da subárvore, não o consumo deste processo.", "This is the number shown in the RAM tree column — the subtree sum, not this process's own usage."));
                        });
                    }
                });
                ui.end_row();

                #[cfg(target_os = "linux")]
                { ui.label(RichText::new("GPU").weak());
                  let vram=self.sys.gpu_linux.memory_by_pid.get(&p.pid).map(|m|fmt_bytes(*m)).unwrap_or_else(||locale.text("indisponível", "unavailable").into());
                  let load=self.sys.gpu_linux.by_pid.get(&p.pid).map(|n|format!("{n:.1}%")).unwrap_or_else(||locale.text("indisponível", "unavailable").into());
                  ui.label(format!("{}: {load} · {}: {vram}", locale.text("Uso", "Usage"), locale.text("memória de GPU", "GPU memory"))); ui.end_row(); }
                ui.label(RichText::new(locale.text("Execução", "Execution")).weak());
                let handles_label = if cfg!(windows) {
                    format!("{} handles", p.handles.map(|n| n.to_string()).unwrap_or_else(|| "—".into()))
                } else if cfg!(target_os = "linux") {
                    format!("{} {}", p.handles.map(|n| n.to_string()).unwrap_or_else(|| "—".into()), locale.text("descritores", "descriptors"))
                } else {
                    locale.text("handles: não aplicável", "handles: not applicable").to_string()
                };
                let execution = if locale == Locale::Portuguese {
                    format!("iniciado há {}   ·   CPU {:.1}% (último intervalo {:.1}%)   ·   {} threads   ·   {}   ·   sessão {}", fmt_age(secs), p.cpu_pct, p.cpu_raw_pct, p.threads, handles_label, p.session)
                } else {
                    format!("started {} ago   ·   CPU {:.1}% (last interval {:.1}%)   ·   {} threads   ·   {}   ·   session {}", fmt_age(secs), p.cpu_pct, p.cpu_raw_pct, p.threads, handles_label, p.session)
                };
                ui.label(execution);
                ui.end_row();
            });
        });

        if let Some((pid, tree)) = kill {
            self.request_kill(pid, tree);
        }
        if let Some(name) = lock {
            self.toggle_lock(&name);
        }
        if let Some((name, c)) = set_cat {
            self.set_override(&name, c);
        }
        if let Some(pid) = goto {
            self.selected = Some(pid);
            self.selected_keep = self.proc(pid).cloned().map(|p| (p, self.cat(pid)));
            self.scroll_to_selected = true;
        }
    }

    /// Conferência: quanto do "em uso" o app consegue atribuir a alguma coisa.
    ///
    /// Existe porque a pergunta natural diante de qualquer monitor de memória é "a lista não
    /// soma nem perto do total, cadê o resto?" — e nem o Gerenciador de Tarefas responde. A
    /// base é sempre a memória privada; com working set na coluna a soma passaria de 100% por
    /// dupla contagem do compartilhado, então o excedente é mostrado à parte, nomeado.
    #[cfg(target_os = "linux")]
    fn calculate_linux_memory_summary(&self) -> String {
        let measured: Vec<_> = self.procs.iter().filter_map(|p| p.linux_memory).collect();
        let uss: u64 = measured.iter().map(|m| m.0).sum();
        let pss: u64 = measured.iter().map(|m| m.1).sum();
        if self.cfg.locale == Locale::Portuguese {
            format!("Processos: {} PSS · {} privados · leitura de {}/{} processos.\nPSS divide páginas compartilhadas; privado conta apenas páginas exclusivas. Leituras ausentes não entram nas somas. A RAM global também inclui o kernel e outras alocações.", fmt_gb(pss), fmt_gb(uss), measured.len(), self.procs.len())
        } else {
            format!("Processes: {} PSS · {} private · readings for {}/{} processes.\nPSS divides shared pages; private counts exclusive pages only. Missing readings are excluded from sums. Global RAM also includes the kernel and other allocations.", fmt_gb(pss), fmt_gb(uss), measured.len(), self.procs.len())
        }
    }

    fn ui_accounting(&mut self, ui: &mut egui::Ui) {
        let locale = self.cfg.locale;
        #[cfg(target_os = "linux")]
        if cfg!(target_os = "linux") {
            let available = self.derived.linux_memory_available;
            let summary = if locale == Locale::Portuguese {
                format!(
                    "RAM {} em uso · memória detalhada: {available}/{} processos",
                    fmt_gb(self.mem.used_phys()),
                    self.procs.len()
                )
            } else {
                format!(
                    "RAM {} in use · detailed memory: {available}/{} processes",
                    fmt_gb(self.mem.used_phys()),
                    self.procs.len()
                )
            };
            ui.label(RichText::new(summary).small().color(MUTED))
                .on_hover_text(self.linux_memory_summary());
            return;
        }
        let b = self.breakdown();
        if b.used == 0 {
            return;
        }
        if locale == Locale::English {
            let measured = b
                .private
                .saturating_add(b.paged_pool)
                .saturating_add(b.nonpaged_pool);
            let overflow = measured > b.used;
            let head = if b.shared_and_cache > 0 {
                format!(
                    "RAM {}: {} measured + {} by difference",
                    fmt_gb(b.used),
                    fmt_gb(measured),
                    fmt_gb(b.shared_and_cache)
                )
            } else {
                format!(
                    "RAM {}: {} measured",
                    fmt_gb(b.used),
                    fmt_gb(measured.min(b.used))
                )
            };
            let color = if b.kernel_ok && !overflow {
                MUTED
            } else {
                Color32::from_rgb(200, 150, 90)
            };
            let mut tip = format!("Composition of the {} in use, always in private memory — the only basis that does not count the same physical page twice:\n\nProcesses (private)  {}\n", fmt_gb(b.used), fmt_bytes_short(b.private));
            if b.kernel_ok {
                tip.push_str(&format!(
                    "{}  {}\n",
                    SysRow::PagedPool.label_for(locale),
                    fmt_bytes_short(b.paged_pool)
                ));
                tip.push_str(&format!(
                    "{}  {}\n",
                    SysRow::NonPagedPool.label_for(locale),
                    fmt_bytes_short(b.nonpaged_pool)
                ));
            } else {
                tip.push_str("Kernel pools: unavailable (GetPerformanceInfo failed)\n");
            }
            tip.push_str(&format!(
                "{}  {}\n",
                SysRow::SharedAndCache.label_for(locale),
                fmt_bytes_short(b.shared_and_cache)
            ));
            if overflow {
                tip.push_str("\nMeasured parts already exceed the total in use: paged pool includes the portion on disk, and processes are sampled at a different instant from the memory reading. The remainder was clamped to zero instead of going negative.\n");
            }
            if self.cfg.mem_metric != MemMetric::Private {
                let shown: u64 = self.procs.iter().map(|p| self.mem_of(p)).sum();
                tip.push_str(&format!("\nThe RAM column uses {} and sums {} — above private memory because each shared page counts in every process that maps it.", self.cfg.mem_metric.label_for(locale).to_lowercase(), fmt_bytes_short(shown)));
            }
            ui.label(RichText::new(head).color(color).small())
                .on_hover_text(tip);
            return;
        }
        // "Medido" = privado dos processos + os dois pools, cada um lido de uma API. O resto
        // é um subtraendo, e o rótulo diz isso — chamá-lo de "atribuído" fingiria uma
        // medição que não existe, que é justamente o vício do Gerenciador de Tarefas.
        let measured = b
            .private
            .saturating_add(b.paged_pool)
            .saturating_add(b.nonpaged_pool);
        let overflow = measured > b.used;
        let color = if b.kernel_ok && !overflow {
            MUTED
        } else {
            Color32::from_rgb(200, 150, 90)
        };
        let head = if b.shared_and_cache > 0 {
            format!(
                "RAM {}: {} medidos + {} por diferença",
                fmt_gb(b.used),
                fmt_gb(measured),
                fmt_gb(b.shared_and_cache)
            )
        } else {
            format!(
                "RAM {}: {} medidos",
                fmt_gb(b.used),
                fmt_gb(measured.min(b.used))
            )
        };
        let mut tip = format!(
            "Composição dos {} em uso, sempre em memória privada — a única base que não conta \
             a mesma página física duas vezes:\n\n\
             Processos (privado)  {}\n",
            fmt_gb(b.used),
            fmt_bytes_short(b.private)
        );
        if b.kernel_ok {
            tip.push_str(&format!(
                "{}  {}\n",
                SysRow::PagedPool.label_for(self.cfg.locale),
                fmt_bytes_short(b.paged_pool)
            ));
            tip.push_str(&format!(
                "{}  {}\n",
                SysRow::NonPagedPool.label_for(self.cfg.locale),
                fmt_bytes_short(b.nonpaged_pool)
            ));
        } else {
            tip.push_str("Pools do kernel: indisponíveis (GetPerformanceInfo falhou)\n");
        }
        tip.push_str(&format!(
            "{}  {}\n",
            SysRow::SharedAndCache.label_for(self.cfg.locale),
            fmt_bytes_short(b.shared_and_cache)
        ));
        if overflow {
            tip.push_str(
                "\nAs parcelas medidas já passam do total em uso: o pool paginado inclui a \
                 fração que está no disco, e os processos são amostrados num instante \
                 diferente da leitura de memória. O resto foi zerado em vez de negativado.\n",
            );
        }
        if self.cfg.mem_metric != MemMetric::Private {
            let shown = self.derived.metric_total;
            tip.push_str(&format!(
                "\nA coluna RAM está em {} e soma {} — acima do privado porque cada página \
                 compartilhada conta em todo processo que a mapeia.",
                self.cfg
                    .mem_metric
                    .label_for(self.cfg.locale)
                    .to_lowercase(),
                fmt_bytes_short(shown)
            ));
        }
        ui.label(RichText::new(head).color(color).small())
            .on_hover_text(tip);
    }

    fn save_cfg_if_dirty(&mut self) {
        #[cfg(target_os = "linux")]
        if self.smoke_started.is_some() {
            self.cfg_dirty = false;
            return;
        }
        if self.cfg_dirty {
            self.cfg_dirty = false;
            if let Err(e) = self.cfg.save() {
                self.toast(
                    if self.cfg.locale == Locale::Portuguese {
                        format!("Falha ao salvar config: {e}")
                    } else {
                        format!("Could not save configuration: {e}")
                    },
                    true,
                );
            }
        }
    }

    fn ui_status(&mut self, ctx: &egui::Context) {
        if let Some((t, msg, err)) = &self.status {
            if t.elapsed().as_secs_f32() > 5.0 {
                self.status = None;
                return;
            }
            let (msg, err) = (msg.clone(), *err);
            egui::Area::new(egui::Id::new("toast"))
                .anchor(egui::Align2::RIGHT_TOP, [-12.0, 64.0])
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    let bg = if err {
                        Color32::from_rgb(120, 40, 40)
                    } else {
                        Color32::from_rgb(40, 100, 60)
                    };
                    egui::Frame::popup(ui.style()).fill(bg).show(ui, |ui| {
                        ui.label(RichText::new(msg).color(Color32::WHITE));
                    });
                });
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let locale = self.cfg.locale;
        self.ingest(ctx);
        #[cfg(target_os = "linux")]
        if let Some(start) = self.smoke_started {
            let step = (start.elapsed().as_secs() / 3) as usize;
            let views = [
                ViewMode::List,
                ViewMode::Tree,
                ViewMode::Category,
                ViewMode::Boot,
                ViewMode::Drains,
                ViewMode::Screens,
                ViewMode::Thermal,
                ViewMode::Clean,
                ViewMode::Sweep,
            ];
            self.cfg.view = views[step % views.len()];
            self.cfg.mini = step % 10 == 8;
            self.cfg.mem_metric = MemMetric::ALL[step % MemMetric::ALL.len()];
            self.cfg.group_apps = step % 2 == 0;
            self.row_cache.invalidate();
            self.derived_dirty = true;
            self.selected = self
                .procs
                .iter()
                .find(|p| p.pid == std::process::id())
                .map(|p| p.pid);
            if start.elapsed().as_secs() >= 90 {
                crate::linux::log("SMOKE PASS: 90s, todas as abas, mini e métricas");
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.refresh_derived_if_dirty();
        // Fora do ingest: ele retorna cedo quando não há amostra nova, e a verificação
        // de assinatura chega no seu próprio ritmo.
        self.drain_sigs();

        if self.applied_mini != self.cfg.mini {
            self.apply_window_mode(ctx);
        }
        if self.cfg.mini {
            self.ui_mini(ctx);
            self.save_cfg_if_dirty();
            return;
        }

        // atalhos
        let (del, shift, f5, esc) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::Delete),
                i.modifiers.shift,
                i.key_pressed(egui::Key::F5),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if del && !ctx.wants_keyboard_input() {
            if let Some(pid) = self.selected {
                self.request_kill(pid, shift);
            }
        }
        if f5 {
            self.sampler.force.store(true, Ordering::Relaxed);
            self.row_cache.clear();
        }
        if esc && !ctx.wants_keyboard_input() {
            self.selected = None;
        }

        egui::SidePanel::left("nav")
            .exact_width(NAV_W)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(10, 12)),
            )
            .show(ctx, |ui| self.ui_nav(ui));
        egui::TopBottomPanel::top("header")
            .frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin {
                left: 16,
                right: 16,
                top: 12,
                bottom: 8,
            }))
            .show_separator_line(false)
            .show(ctx, |ui| self.ui_header(ui));
        // O painel de detalhes descreve a linha selecionada da tabela. Num addon não há
        // tabela nem seleção — ele ficaria como 150 px de espaço vazio.
        if !self.cfg.view.is_addon() {
            egui::TopBottomPanel::bottom("details")
                .resizable(true)
                .default_height(150.0)
                .min_height(40.0)
                .frame(
                    egui::Frame::new()
                        .fill(BG)
                        .inner_margin(egui::Margin::symmetric(16, 6)),
                )
                .show(ctx, |ui| self.ui_details(ui));
        }
        egui::TopBottomPanel::bottom("statusbar").frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::symmetric(16, 4))).show(ctx, |ui| {
            ui.horizontal(|ui| {
                let count = if locale == Locale::Portuguese {
                    format!("{} processos ({} exibidos)", self.procs.len(), self.row_cache.shown_count())
                } else {
                    format!("{} processes ({} shown)", self.procs.len(), self.row_cache.shown_count())
                };
                ui.label(RichText::new(count).weak().small());
                ui.separator();
                let locked_n = self.cfg.locked.len();
                ui.label(RichText::new(format!("{locked_n} {}", locale.text("protegidos", "protected"))).weak().small())
                    .on_hover_text(self.cfg.locked.iter().cloned().collect::<Vec<_>>().join("\n"));
                ui.separator();
                ui.label(RichText::new(format!("{} {:.0} ms", locale.text("amostra", "sample"), self.sample_ms)).weak().small());
                self.ui_sampling_controls(ui);
                ui.separator();
                self.ui_accounting(ui);
                if self.row_cache.order_frozen() {
                    ui.separator();
                    ui.label(RichText::new(locale.text("ordem congelada", "order frozen")).weak().small())
                        .on_hover_text(locale.text(
                            "Enquanto o mouse está sobre a tabela a ordem das linhas não muda, para você não clicar no processo errado. Valores continuam atualizando.",
                            "While the pointer is over the table, row order stays fixed so you do not click the wrong process. Values continue updating.",
                        ));
                }
                // Os atalhos agem sobre a linha selecionada da tabela. Anunciá-los dentro
                // de um addon é prometer uma tecla que não faz nada ali.
                if !self.cfg.view.is_addon() {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(locale.text("atalhos", "shortcuts")).weak().small())
                            .on_hover_text(locale.text(
                                "Del: finalizar\nShift+Del: finalizar a árvore\nF5: atualizar\nEsc: limpar seleção\nBotão direito: menu da linha",
                                "Del: terminate\nShift+Del: terminate tree\nF5: refresh\nEsc: clear selection\nRight-click: row menu",
                            ));
                    });
                }
            });
        });
        let view = self.cfg.view;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin {
                left: 16,
                right: 16,
                top: 2,
                bottom: 10,
            }))
            .show(ctx, |ui| {
                if view.is_addon() {
                    egui::Frame::new()
                        .fill(SURFACE)
                        .corner_radius(CARD_R)
                        .inner_margin(egui::Margin::same(12))
                        .show(ui, |ui| {
                            ui.set_min_size(ui.available_size());
                            self.ui_addon_body(ui, view);
                        });
                } else {
                    self.ui_cards(ui);
                    ui.add_space(8.0);
                    self.ui_pressure_banner(ui);
                    self.ui_chips(ui);
                    ui.add_space(6.0);
                    egui::Frame::new()
                        .fill(SURFACE)
                        .corner_radius(CARD_R)
                        .inner_margin(egui::Margin::symmetric(6, 4))
                        .show(ui, |ui| {
                            ui.set_min_size(ui.available_size());
                            self.ui_table(ui);
                        });
                }
            });

        self.ui_prefs(ctx);
        self.ui_status(ctx);
        self.save_cfg_if_dirty();
    }
}

// ---------- Fluente: sidebar, cabeçalho, cards e preferências ----------

/// Ícones da barra lateral, desenhados a traço. Não dependem de fonte de símbolo: o
/// Segoe UI Symbol do Windows e o fallback do Linux desenham ⚡♨▦ cada um do seu jeito.
#[derive(Clone, Copy)]
enum Icon {
    List,
    Tree,
    Grid,
    Bolt,
    Warn,
    Thermo,
    Display,
    Broom,
    Check,
    Gear,
}

impl Icon {
    fn of(v: ViewMode) -> Icon {
        match v {
            ViewMode::List => Icon::List,
            ViewMode::Tree => Icon::Tree,
            ViewMode::Category => Icon::Grid,
            ViewMode::Boot => Icon::Bolt,
            ViewMode::Drains => Icon::Warn,
            ViewMode::Thermal => Icon::Thermo,
            ViewMode::Screens => Icon::Display,
            ViewMode::Clean => Icon::Broom,
            ViewMode::Sweep => Icon::Check,
        }
    }
}

fn paint_icon(p: &egui::Painter, c: egui::Pos2, color: Color32, icon: Icon) {
    use egui::pos2;
    let s = Stroke::new(1.6_f32, color);
    let (x, y) = (c.x, c.y);
    match icon {
        Icon::List => {
            for dy in [-5.0, 0.0, 5.0] {
                p.line_segment([pos2(x - 7.0, y + dy), pos2(x + 7.0, y + dy)], s);
            }
        }
        Icon::Tree => {
            p.circle_stroke(pos2(x - 5.0, y - 5.0), 2.5, s);
            p.line_segment([pos2(x - 5.0, y - 2.5), pos2(x - 5.0, y + 5.0)], s);
            p.line_segment([pos2(x - 5.0, y + 0.0), pos2(x + 2.0, y + 0.0)], s);
            p.line_segment([pos2(x - 5.0, y + 5.0), pos2(x + 2.0, y + 5.0)], s);
            p.circle_stroke(pos2(x + 4.5, y + 0.0), 2.5, s);
            p.circle_stroke(pos2(x + 4.5, y + 5.0), 2.5, s);
        }
        Icon::Grid => {
            for (dx, dy) in [(-6.5, -6.5), (0.5, -6.5), (-6.5, 0.5), (0.5, 0.5)] {
                p.rect_stroke(
                    Rect::from_min_size(pos2(x + dx, y + dy), Vec2::splat(6.0)),
                    1.5,
                    s,
                    egui::StrokeKind::Middle,
                );
            }
        }
        Icon::Bolt => {
            let pts = vec![
                pos2(x + 1.0, y - 7.5),
                pos2(x - 5.0, y + 1.0),
                pos2(x - 0.5, y + 1.0),
                pos2(x - 1.5, y + 7.5),
                pos2(x + 5.0, y - 1.0),
                pos2(x + 0.5, y - 1.0),
            ];
            p.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
        }
        Icon::Warn => {
            p.add(egui::Shape::closed_line(
                vec![
                    pos2(x, y - 7.0),
                    pos2(x + 7.5, y + 6.0),
                    pos2(x - 7.5, y + 6.0),
                ],
                s,
            ));
            p.line_segment([pos2(x, y - 2.0), pos2(x, y + 1.5)], s);
            p.circle_filled(pos2(x, y + 3.8), 1.0, color);
        }
        Icon::Thermo => {
            p.line_segment([pos2(x - 2.5, y - 6.5), pos2(x - 2.5, y + 1.0)], s);
            p.line_segment([pos2(x + 2.5, y - 6.5), pos2(x + 2.5, y + 1.0)], s);
            p.line_segment([pos2(x - 2.5, y - 6.5), pos2(x + 2.5, y - 6.5)], s);
            p.circle_stroke(pos2(x, y + 3.5), 4.0, s);
            p.circle_filled(pos2(x, y + 3.5), 1.8, color);
        }
        Icon::Display => {
            p.rect_stroke(
                Rect::from_center_size(pos2(x, y - 1.5), Vec2::new(15.0, 10.0)),
                1.5,
                s,
                egui::StrokeKind::Middle,
            );
            p.line_segment([pos2(x, y + 3.5), pos2(x, y + 7.0)], s);
            p.line_segment([pos2(x - 4.0, y + 7.0), pos2(x + 4.0, y + 7.0)], s);
        }
        Icon::Broom => {
            p.line_segment([pos2(x - 7.0, y - 3.0), pos2(x + 7.0, y - 3.0)], s);
            p.line_segment([pos2(x - 5.5, y - 3.0), pos2(x - 4.5, y + 7.0)], s);
            p.line_segment([pos2(x + 5.5, y - 3.0), pos2(x + 4.5, y + 7.0)], s);
            p.line_segment([pos2(x - 4.5, y + 7.0), pos2(x + 4.5, y + 7.0)], s);
            p.line_segment([pos2(x - 2.5, y - 3.0), pos2(x - 2.5, y - 6.5)], s);
            p.line_segment([pos2(x + 2.5, y - 3.0), pos2(x + 2.5, y - 6.5)], s);
            p.line_segment([pos2(x - 2.5, y - 6.5), pos2(x + 2.5, y - 6.5)], s);
        }
        Icon::Check => {
            // Lista com itens marcados: o que a Faxina faz.
            for (i, dy) in [-5.0_f32, 0.0, 5.0].into_iter().enumerate() {
                let bx = x - 5.5;
                if i < 2 {
                    p.line_segment([pos2(bx - 1.5, y + dy), pos2(bx, y + dy + 1.5)], s);
                    p.line_segment([pos2(bx, y + dy + 1.5), pos2(bx + 2.5, y + dy - 1.5)], s);
                } else {
                    p.circle_stroke(pos2(bx + 0.5, y + dy), 1.5, s);
                }
                p.line_segment([pos2(x - 1.0, y + dy), pos2(x + 7.0, y + dy)], s);
            }
        }
        Icon::Gear => {
            p.circle_stroke(pos2(x, y), 4.5, s);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::TAU / 8.0;
                let (sa, ca) = a.sin_cos();
                p.line_segment(
                    [
                        pos2(x + ca * 5.5, y + sa * 5.5),
                        pos2(x + ca * 7.5, y + sa * 7.5),
                    ],
                    s,
                );
            }
        }
    }
}

/// Item da barra lateral: ícone + nome, fundo só no ativo e no hover.
fn nav_item(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    on: bool,
    enabled: bool,
    tip: &str,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), NAV_ITEM_H),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    let p = ui.painter();
    if on {
        p.rect_filled(
            rect,
            8.0,
            Color32::from_rgba_unmultiplied(255, 255, 255, 26),
        );
    } else if resp.hovered() && enabled {
        p.rect_filled(
            rect,
            8.0,
            Color32::from_rgba_unmultiplied(255, 255, 255, 12),
        );
    }
    let fg = if !enabled {
        Color32::from_gray(96)
    } else if on {
        TEXT
    } else {
        Color32::from_rgb(208, 208, 208)
    };
    paint_icon(p, egui::pos2(rect.left() + 20.0, rect.center().y), fg, icon);
    p.text(
        egui::pos2(rect.left() + 38.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(13.5),
        fg,
    );
    if enabled {
        resp.on_hover_text(tip)
    } else {
        resp.on_hover_text(tip)
    }
}

/// Chip de estado (em foco, sobra, protegido…): pílula com a cor a 18%.
fn chip(ui: &mut egui::Ui, text: &str, color: Color32) -> egui::Response {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.18))
        .corner_radius(999.0)
        .inner_margin(egui::Margin::symmetric(8, 1))
        .show(ui, |ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            ui.label(RichText::new(text).size(11.0).color(color));
        })
        .response
}

fn chip_w(ui: &egui::Ui, text: &str) -> f32 {
    ui.fonts(|f| {
        f.layout_no_wrap(
            text.to_owned(),
            egui::FontId::proportional(11.0),
            Color32::WHITE,
        )
        .size()
        .x
    }) + 16.0
        + 14.0
}

/// Avatar da linha: o ícone do app quando existe; senão a inicial num quadrado da cor
/// da categoria.
fn avatar(ui: &mut egui::Ui, tex: Option<&TextureHandle>, cat: Category, name: &str) {
    let (r, _) = ui.allocate_exact_size(Vec2::splat(AVATAR), egui::Sense::hover());
    match tex {
        Some(t) => {
            ui.painter().image(
                t.id(),
                r.shrink(2.0),
                Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        None => {
            ui.painter()
                .rect_filled(r, 7.0, cat.color().gamma_multiply(0.22));
            let initial: String = name
                .chars()
                .next()
                .map(|c| c.to_uppercase().collect())
                .unwrap_or_default();
            ui.painter().text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                initial,
                egui::FontId::proportional(12.5),
                cat.color(),
            );
        }
    }
}

/// Gráfico de área dos últimos `HIST_LEN` ticks, com a escala fixa em 0–100 %.
fn sparkline(ui: &mut egui::Ui, size: Vec2, hist: &VecDeque<f32>, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let p = ui.painter();
    p.rect(
        rect,
        6.0,
        color.gamma_multiply(0.07),
        Stroke::new(1.0_f32, color.gamma_multiply(0.45)),
        egui::StrokeKind::Inside,
    );
    let n = hist.len();
    if n < 2 {
        return;
    }
    let inner = rect.shrink(2.0);
    let step = inner.width() / (HIST_LEN - 1) as f32;
    let x0 = inner.right() - (n - 1) as f32 * step;
    let pts: Vec<egui::Pos2> = hist
        .iter()
        .enumerate()
        .map(|(i, v)| {
            egui::pos2(
                x0 + i as f32 * step,
                inner.bottom() - (v / 100.0).clamp(0.0, 1.0) * inner.height(),
            )
        })
        .collect();
    // Área sob a curva como malha: retângulo por amostra deixava emenda entre um e outro.
    let fill = color.gamma_multiply(0.22);
    let mut mesh = egui::Mesh::default();
    for pt in &pts {
        mesh.colored_vertex(*pt, fill);
        mesh.colored_vertex(egui::pos2(pt.x, inner.bottom()), fill);
    }
    for i in 0..(n as u32 - 1) {
        let (t0, b0, t1, b1) = (2 * i, 2 * i + 1, 2 * i + 2, 2 * i + 3);
        mesh.add_triangle(t0, t1, b1);
        mesh.add_triangle(t0, b1, b0);
    }
    p.add(egui::Shape::mesh(mesh));
    p.add(egui::Shape::line(pts, Stroke::new(1.5_f32, color)));
}

impl App {
    /// Barra lateral: visões, addons e preferências. Substitui a fileira de abas e o bloco
    /// de botões que brigavam pela largura do topo.
    fn ui_nav(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            if let Some(t) = &self.logo {
                ui.add(egui::Image::new((t.id(), Vec2::splat(22.0))).corner_radius(6.0));
            }
            ui.label(RichText::new("RamDog").strong().size(15.0));
        });
        ui.add_space(12.0);
        let mut go: Option<ViewMode> = None;
        for v in ViewMode::CORE {
            let label = if v == ViewMode::List {
                self.cfg.locale.text("Processos", "Processes")
            } else {
                v.label_for(self.cfg.locale)
            };
            if nav_item(
                ui,
                Icon::of(v),
                label,
                self.cfg.view == v,
                true,
                v.tip_for(self.cfg.locale),
            )
            .clicked()
            {
                go = Some(v);
            }
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.label(
                RichText::new(self.cfg.locale.text("COMPLEMENTOS", "ADD-ONS"))
                    .size(10.5)
                    .color(MUTED),
            );
        });
        ui.add_space(2.0);
        for v in ViewMode::ADDONS {
            let on = self.cfg.view == v;
            let tip = if !v.available() {
                if matches!(v, ViewMode::Clean | ViewMode::Sweep) {
                    format!(
                        "{} — {}",
                        v.label_for(self.cfg.locale),
                        self.cfg
                            .locale
                            .text("por enquanto só no Linux.", "Linux only for now.")
                    )
                } else {
                    format!(
                        "{} — {}",
                        v.label_for(self.cfg.locale),
                        self.cfg.locale.text(
                            "indisponível nesta versão para Linux/macOS.",
                            "unavailable on this Linux/macOS build."
                        )
                    )
                }
            } else if on {
                format!(
                    "{}\n\n{}{}.",
                    v.tip_for(self.cfg.locale),
                    self.cfg
                        .locale
                        .text("Clique para voltar a ", "Click to return to "),
                    self.last_core.label_for(self.cfg.locale)
                )
            } else {
                v.tip_for(self.cfg.locale).to_string()
            };
            if nav_item(
                ui,
                Icon::of(v),
                v.label_for(self.cfg.locale),
                on,
                v.available(),
                &tip,
            )
            .clicked()
            {
                go = Some(v);
            }
        }
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.add_space(4.0);
            if self.is_admin {
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new("ADMIN")
                            .color(Color32::from_rgb(90, 220, 130))
                            .strong()
                            .size(11.0),
                    )
                    .on_hover_text(self.cfg.locale.text(
                        "Rodando elevado: pode encerrar processos de outros usuários/serviços",
                        "Running elevated: can terminate other users' processes and services",
                    ));
                });
            }
            if nav_item(
                ui,
                Icon::Gear,
                self.cfg.locale.text("Preferências", "Preferences"),
                self.show_prefs,
                true,
                self.cfg.locale.text(
                    "Métrica da coluna RAM, cortes de exibição e ritmo da amostragem",
                    "RAM column metric, display thresholds, and sampling interval",
                ),
            )
            .clicked()
            {
                self.show_prefs = !self.show_prefs;
            }
        });
        let Some(v) = go else { return };
        if self.cfg.view == v {
            if v.is_addon() {
                self.cfg.view = self.last_core;
                self.cfg_dirty = true;
            }
            return;
        }
        if !self.cfg.view.is_addon() {
            self.last_core = self.cfg.view;
        }
        self.cfg.view = v;
        self.cfg_dirty = true;
    }

    /// Cabeçalho: título da visão, contagem, busca, agrupamento e Mini.
    fn ui_header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let v = self.cfg.view;
            let title = if v == ViewMode::List {
                self.cfg.locale.text("Processos", "Processes")
            } else {
                v.label_for(self.cfg.locale)
            };
            ui.label(RichText::new(title).size(17.0).strong());
            if !v.is_addon() {
                let count = if self.cfg.locale == Locale::Portuguese {
                    format!("{} · {} na tela", self.procs.len(), self.row_cache.shown_count())
                } else {
                    format!("{} · {} shown", self.procs.len(), self.row_cache.shown_count())
                };
                ui.label(RichText::new(count).color(MUTED).size(12.5));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().button_padding = Vec2::new(12.0, 5.0);
                ui.spacing_mut().item_spacing.x = 8.0;
                let mini = egui::Button::new(RichText::new("Mini").size(13.0)).fill(SURFACE).stroke(Stroke::NONE).corner_radius(8.0);
                if ui
                    .add(mini)
                    .on_hover_text(self.cfg.locale.text(
                        "Modo mini: uma janelinha só com CPU, RAM, GPU, disco e temperaturas, por cima das outras janelas",
                        "Mini mode: a small window with CPU, RAM, GPU, disk, and temperatures, kept above other windows",
                    ))
                    .clicked()
                {
                    self.set_mini(true);
                }
                if v.is_addon() {
                    return;
                }
                if v == ViewMode::List {
                    let on = self.cfg.group_apps;
                    let text = RichText::new(if on {
                        self.cfg.locale.text("Agrupar por app ✓", "Group by app ✓")
                    } else {
                        self.cfg.locale.text("Agrupar por app", "Group by app")
                    }).size(13.0);
                    let b = egui::Button::new(text)
                        .fill(if on { ACCENT_BG } else { SURFACE })
                        .stroke(Stroke::NONE)
                        .corner_radius(8.0);
                    if ui
                        .add(b)
                        .on_hover_text(self.cfg.locale.text(
                            "Junta a mesma tarefa numa linha — Overwatch, Sussurro, Claude…\nNão junta dois jogos só porque compartilham o wine64.\nClica na linha para ver os PIDs. Desligado, cada PID vira uma linha.",
                            "Joins the same app into one row — Overwatch, Sussurro, Claude…\nDoes not merge two games just because they share wine64.\nClick a row to see PIDs. When off, each PID gets its own row.",
                        ))
                        .clicked()
                    {
                        self.cfg.group_apps = !on;
                        self.cfg_dirty = true;
                        self.row_cache.invalidate();
                    }
                }
                let plain = |t: &str| egui::Button::new(RichText::new(t).size(13.0)).fill(SURFACE).stroke(Stroke::NONE).corner_radius(8.0);
                if v == ViewMode::Tree {
                    if ui.add(plain(self.cfg.locale.text("Recolher", "Collapse"))).clicked() {
                        self.expanded.clear();
                    }
                    if ui.add(plain(self.cfg.locale.text("Expandir tudo", "Expand all"))).clicked() {
                        self.expanded = self.children.keys().copied().collect();
                    }
                } else if v == ViewMode::List && self.cfg.group_apps {
                    if !self.expanded_apps.is_empty() && ui.add(plain(self.cfg.locale.text("Recolher", "Collapse"))).clicked() {
                        self.expanded_apps.clear();
                        self.row_cache.invalidate();
                    }
                    if ui.add(plain(self.cfg.locale.text("Expandir tudo", "Expand all"))).clicked() {
                        self.expanded_apps = self.groups.iter().filter(|g| g.pids.len() >= 2).map(|g| g.key.clone()).collect();
                        self.row_cache.invalidate();
                    }
                }
                egui::Frame::new()
                    .fill(SURFACE)
                    .corner_radius(8.0)
                    .inner_margin(egui::Margin::symmetric(10, 4))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        if !self.search.is_empty()
                            && ui.add(egui::Button::new(RichText::new("✖").size(11.0).color(MUTED)).frame(false)).on_hover_text(self.cfg.locale.text("Limpar busca", "Clear search")).clicked()
                        {
                            self.search.clear();
                        }
                        let te = egui::TextEdit::singleline(&mut self.search)
                            .hint_text(self.cfg.locale.text("Buscar nome, PID ou comando", "Search name, PID, or command"))
                            .frame(false)
                            .desired_width(220.0);
                        if ui.add(te).changed() {
                            self.scroll_to_selected = false;
                        }
                    });
            });
        });
    }

    /// Os quatro cards de recurso com histórico. Só nas visões de processo: os addons usam
    /// a janela inteira.
    fn ui_cards(&mut self, ui: &mut egui::Ui) {
        let locale = self.cfg.locale;
        let gap = 12.0;
        let w = ((ui.available_width() - 3.0 * gap) / 4.0).floor();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;

            let cpu_pct = self.sys.cpu_pct;
            let cpu_temp = self.cpu_temp();
            let ncpu = self.ncpu;
            let press = self.pressure_snap();
            let cpu_color = if press.load_hot() { Color32::from_rgb(255, 150, 90) } else { C_CPU };
            let split = self.cpu_split();
            let unlisted = split.and_then(|s| s.unlisted_chip(locale));
            let cpu_sub = match (self.sys.load1, unlisted.is_some()) {
                (Some(l), false) => format!(
                    "{} {} · {ncpu} {}",
                    locale.text("carga", "load"),
                    pt_num(l as f64, 2),
                    locale.text("núcleos", "threads")
                ),
                (Some(l), true) => format!("{} {}", locale.text("carga", "load"), pt_num(l as f64, 2)),
                (None, _) => format!("{ncpu} {}", locale.text("núcleos", "threads")),
            };
            let mut cpu_tip = match (self.sys.load1, self.sys.load5, self.sys.load15) {
                (Some(a), Some(b), Some(c)) if locale == Locale::Portuguese => format!(
                    "Uso de CPU (todos os núcleos).\nLoad 1/5/15 min: {a:.2} / {b:.2} / {c:.2}\nLoad acima de {ncpu} significa fila cheia — processo com pouca RAM some na lista ordenada por memória."
                ),
                (Some(a), Some(b), Some(c)) => format!(
                    "CPU usage (all cores).\n1/5/15 min load: {a:.2} / {b:.2} / {c:.2}\nLoad above {ncpu} means the queue is full — a low-RAM process can disappear in a memory-sorted list."
                ),
                _ => locale.text("Uso de CPU (todos os núcleos)", "CPU usage (all cores)").into(),
            };
            if let Some(s) = split {
                cpu_tip.push_str("\n\n");
                cpu_tip.push_str(&s.explain(locale));
            }
            Self::resource_card(ui, w, "CPU", cpu_pct, cpu_color, &self.hist_cpu, &cpu_tip, |ui| {
                Self::temp_label(ui, cpu_temp);
                ui.label(RichText::new(cpu_sub).color(if press.load_hot() { Color32::from_rgb(255, 171, 145) } else { MUTED }).size(11.5));
                if let Some((chip, tip)) = unlisted {
                    ui.label(RichText::new(chip).color(Color32::from_rgb(255, 171, 145)).size(11.5))
                        .on_hover_text(tip);
                }
            });

            let used = self.mem.used_phys();
            let total = self.mem.total_phys.max(1);
            let ram_pct = Some(used as f32 / total as f32 * 100.0);
            let ram_temp = self.ram_temp();
            let ram_sub = if self.mem.swap_total > 0 {
                format!("{} / {} · swap {}", fmt_gb(used), fmt_gb(total), fmt_gb(self.mem.swap_used))
            } else {
                format!("{} / {}", fmt_gb(used), fmt_gb(total))
            };
            let ram_color = if press.swap_hot() { Color32::from_rgb(255, 150, 90) } else { C_RAM };
            let ram_tip = if self.mem.swap_total > 0 {
                if locale == Locale::Portuguese {
                    format!("RAM física em uso.\nSwap: {} / {}.", fmt_gb(self.mem.swap_used), fmt_gb(self.mem.swap_total))
                } else {
                    format!("Physical RAM in use.\nSwap: {} / {}.", fmt_gb(self.mem.swap_used), fmt_gb(self.mem.swap_total))
                }
            } else {
                locale.text("RAM física em uso", "Physical RAM in use").into()
            };
            Self::resource_card(ui, w, self.cfg.locale.text("Memória", "Memory"), ram_pct, ram_color, &self.hist_ram, &ram_tip, |ui| {
                Self::temp_label(ui, ram_temp);
                ui.label(RichText::new(ram_sub).color(if press.swap_hot() { Color32::from_rgb(255, 171, 145) } else { MUTED }).size(11.5));
            });

            let gpu = self.sys.gpu.clone();
            let (gpu_pct, gpu_temp, gpu_sub, gpu_tip) = match &gpu {
                Some(g) => {
                    let mut tip = g.name.clone();
                    if let Some(w) = g.power_w {
                        tip.push_str(&format!("\n{}: {w:.0} W", locale.text("Potência", "Power")));
                    }
                    if let Some(f) = g.fan_pct {
                        tip.push_str(&format!("\n{}: {f}%", locale.text("Cooler", "Fan")));
                    }
                    let vram = if g.mem_total > 0 { format!("{} / {}", fmt_gb(g.mem_used), fmt_gb(g.mem_total)) } else { String::new() };
                    let t = match g.temp_c {
                        Some(t) => Temp::C(t),
                        None => Temp::Missing(locale.text("O driver não reportou temperatura desta GPU.", "The driver did not report a temperature for this GPU.").into()),
                    };
                    (g.util_pct, t, vram, tip)
                }
                None => (
                    None,
                    Temp::Missing(locale.text("Leitura de GPU indisponível nesta plataforma ou driver.", "GPU telemetry is unavailable on this platform or driver.").into()),
                    String::new(),
                    locale.text("Leitura de GPU indisponível nesta plataforma ou driver; não significa utilização zero.", "GPU telemetry is unavailable on this platform or driver; this does not mean zero usage.").to_string(),
                ),
            };
            #[cfg(target_os = "linux")]
            let cards: Vec<String> = self.sys.gpu_linux.cards.iter().map(|c| c.name.clone()).collect();
            #[cfg(target_os = "linux")]
            let gpu_error = self.sys.gpu_linux.error.clone();
            #[cfg(target_os = "linux")]
            let mut gpu_index = self.gpu_index;
            let gpu_name = gpu.as_ref().map(|g| g.name.clone()).unwrap_or_default();
            Self::resource_card(ui, w, "GPU", gpu_pct, C_GPU, &self.hist_gpu, &gpu_tip, |ui| {
                Self::temp_label(ui, gpu_temp);
                #[cfg(target_os = "linux")]
                if cards.len() > 1 {
                    egui::ComboBox::from_id_salt("gpu-selection")
                        .selected_text(RichText::new(&cards[gpu_index.min(cards.len() - 1)]).size(11.5))
                        .show_ui(ui, |ui| {
                            for (i, name) in cards.iter().enumerate() {
                                ui.selectable_value(&mut gpu_index, i, name);
                            }
                        });
                } else if let Some(e) = &gpu_error {
                    ui.label(RichText::new(locale.text("sem leitura", "unavailable")).color(Color32::YELLOW).size(11.5)).on_hover_text(e);
                }
                #[cfg(target_os = "linux")]
                let many = cards.len() > 1;
                #[cfg(not(target_os = "linux"))]
                let many = false;
                let sub = if many || gpu_name.is_empty() { gpu_sub.clone() } else if gpu_sub.is_empty() { gpu_name.clone() } else { format!("{gpu_sub} · {gpu_name}") };
                ui.add(egui::Label::new(RichText::new(sub).color(MUTED).size(11.5)).truncate());
            });
            #[cfg(target_os = "linux")]
            if gpu_index != self.gpu_index {
                self.gpu_index = gpu_index;
                self.sys.gpu = self.sys.gpu_linux.cards.get(gpu_index).cloned();
                self.derived_dirty = true;
            }

            let disk_pct = self.sys.disk_pct;
            let disk_sub = self.sys.disk_bps.filter(|bps| *bps >= 1024.0).map(fmt_bps).unwrap_or_else(|| self.cfg.locale.text("parado", "idle").into());
            let disk_tip = if disk_pct.is_some() { disk_usage_tip(self.cfg.locale) } else { self.cfg.locale.text("Contador de disco indisponível neste host.", "Disk counter is unavailable on this host.") };
            Self::resource_card(ui, w, self.cfg.locale.text("Disco", "Disk"), disk_pct, C_DISK, &self.hist_disk, disk_tip, |ui| {
                ui.label(RichText::new(disk_sub).color(MUTED).size(11.5));
            });
        });
    }

    fn temp_label(ui: &mut egui::Ui, temp: Temp) {
        match temp {
            Temp::C(t) => {
                ui.label(
                    RichText::new(format!("{t} °C"))
                        .size(11.5)
                        .color(Self::temp_color(t)),
                );
            }
            Temp::Missing(why) => {
                ui.label(
                    RichText::new("– °C")
                        .size(11.5)
                        .color(Color32::from_gray(110)),
                )
                .on_hover_text(why);
            }
            Temp::None => {}
        }
    }

    fn resource_card(
        ui: &mut egui::Ui,
        w: f32,
        label: &str,
        pct: Option<f32>,
        color: Color32,
        hist: &VecDeque<f32>,
        tip: &str,
        sub: impl FnOnce(&mut egui::Ui),
    ) {
        let inner_w = w - 2.0 * CARD_PAD;
        let r = egui::Frame::new()
            .fill(SURFACE)
            .corner_radius(CARD_R)
            .inner_margin(egui::Margin::same(CARD_PAD as i8))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.set_width(inner_w);
                    ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(label).strong().size(13.5));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| match pct {
                            Some(p) => {
                                ui.label(
                                    RichText::new(Self::fmt_pct(p))
                                        .size(20.0)
                                        .strong()
                                        .color(color),
                                );
                            }
                            None => {
                                ui.label(
                                    RichText::new("–").size(20.0).color(Color32::from_gray(110)),
                                );
                            }
                        });
                    });
                    sparkline(ui, Vec2::new(inner_w, SPARK_H), hist, color);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                        sub(ui);
                    });
                })
            });
        r.response.on_hover_text(tip);
    }

    fn ui_pressure_banner(&mut self, ui: &mut egui::Ui) {
        let snap = self.pressure_snap();
        let thieves = self.thieves();
        let Some(text) = pressure::banner_for(&snap, &thieves, self.cfg.locale) else {
            return;
        };
        let go = ui
            .scope(|ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgb(74, 28, 11))
                    .corner_radius(8.0)
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            ui.add(
                                egui::Label::new(
                                    RichText::new(text)
                                        .size(12.5)
                                        .color(Color32::from_rgb(255, 171, 145)),
                                )
                                .wrap(),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let btn = egui::Button::new(
                                    RichText::new(
                                        self.cfg.locale.text("Ver disputa", "View contention"),
                                    )
                                    .size(12.5)
                                    .color(Color32::from_rgb(255, 171, 145)),
                                )
                                .fill(Color32::from_rgb(90, 36, 16))
                                .stroke(Stroke::NONE)
                                .corner_radius(8.0);
                                ui.add(btn).clicked()
                            })
                            .inner
                        })
                        .inner
                    })
                    .inner
            })
            .inner;
        if go {
            self.sort = SortKey::Steal;
            self.sort_desc = true;
            self.row_cache.invalidate();
            self.row_cache.thaw();
            if let Some(first) = thieves.first() {
                self.selected = Some(first.pid);
                self.scroll_to_selected = true;
            }
        }
        ui.add_space(8.0);
    }

    /// Chips de categoria: filtram a tabela. Duplo clique isola uma.
    fn ui_chips(&mut self, ui: &mut egui::Ui) {
        let totals = self.cat_totals();
        let locale = self.cfg.locale;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
            ui.spacing_mut().button_padding = Vec2::new(10.0, 3.0);
            let mut toggled: Option<Category> = None;
            let mut solo: Option<Category> = None;
            for c in Category::ALL {
                let (t, n) = totals.get(&c).copied().unwrap_or((0, 0));
                let on = self.cat_enabled.contains(&c);
                let col = c.color();
            let text = RichText::new(format!("● {}  {}", c.label_for(locale), fmt_bytes_short(t)))
                    .color(if on { col } else { MUTED })
                    .size(12.0);
                let btn = egui::Button::new(text)
                    .fill(if on { col.gamma_multiply(0.14) } else { SURFACE })
                    .stroke(Stroke::NONE)
                    .corner_radius(999.0);
                let r = ui.add(btn).on_hover_text(format!(
                    "{n} {}",
                    locale.text(
                        "processos — clique: alterna; duplo clique: só esta",
                        "processes — click: toggle; double-click: only this",
                    )
                ));
                if r.double_clicked() {
                    solo = Some(c);
                } else if r.clicked() {
                    toggled = Some(c);
                }
            }
            if let Some(c) = solo {
                self.cat_enabled.clear();
                self.cat_enabled.insert(c);
            } else if let Some(c) = toggled {
                if !self.cat_enabled.remove(&c) {
                    self.cat_enabled.insert(c);
                }
            }
            let disputa_on = self.sort == SortKey::Steal;
            let disputa = egui::Button::new(
                RichText::new(locale.text("● Disputa", "● Contention"))
                    .color(if disputa_on { Color32::from_rgb(255, 150, 90) } else { MUTED })
                    .size(12.0),
            )
            .fill(if disputa_on { Color32::from_rgb(255, 150, 90).gamma_multiply(0.14) } else { SURFACE })
            .stroke(Stroke::NONE)
            .corner_radius(999.0);
            if ui
                .add(disputa)
                .on_hover_text(locale.text(
                    "Ordena por quem come núcleo sem aparecer na RAM: sobra, loop de credencial, CPU barata. É o recorte que o jogo sente e a lista por memória esconde.",
                    "Sorts by processes consuming CPU without showing much RAM: leftovers, credential loops, and cheap CPU work. This is what a game feels while a memory-sorted list hides it.",
                ))
                .clicked()
            {
                if disputa_on {
                    self.sort = SortKey::Ram;
                    self.sort_desc = true;
                } else {
                    self.sort = SortKey::Steal;
                    self.sort_desc = true;
                }
                self.row_cache.invalidate();
                self.row_cache.thaw();
            }
            // Chip das linhas que não são processo: interruptor de exibição, não filtro. A
            // memória do kernel continua no medidor e na conferência do rodapé.
            let b = self.breakdown();
            if b.kernel_ok {
                let on = self.cfg.show_kernel_rows;
                let col = SysRow::PagedPool.color();
                let sys_total = b.paged_pool + b.nonpaged_pool + b.shared_and_cache;
                let text = RichText::new(format!(
                    "{}  {}",
                    locale.text("▣ Sistema (não-processo)", "▣ System (non-process)"),
                    fmt_bytes_short(sys_total)
                ))
                    .color(if on { col } else { MUTED })
                    .size(12.0);
                let btn = egui::Button::new(text)
                    .fill(if on { col.gamma_multiply(0.14) } else { SURFACE })
                    .stroke(Stroke::NONE)
                    .corner_radius(999.0);
                if ui
                    .add(btn)
                    .on_hover_text(locale.text(
                        "Mostra ou esconde as três linhas de memória que não pertencem a processo \
                         nenhum (pools do kernel e compartilhado/cache).\n\n\
                         Esconder muda só a lista: o medidor do topo e a conferência do rodapé \
                         continuam contando essa memória.",
                        "Shows or hides the three memory rows that do not belong to a process \
                         (kernel pools and shared/cache memory).\n\n\
                         Hiding them changes only the list: the top meter and footer reconciliation \
                         still include this memory.",
                    ))
                    .clicked()
                {
                    self.cfg.show_kernel_rows = !on;
                    self.cfg_dirty = true;
                    self.row_cache.invalidate();
                }
            }
            if self.cat_enabled.len() != Category::ALL.len() {
                let b = egui::Button::new(RichText::new(locale.text("todas", "all")).size(12.0).color(MUTED)).fill(SURFACE).stroke(Stroke::NONE).corner_radius(999.0);
                if ui.add(b).clicked() {
                    self.cat_enabled = Category::ALL.iter().copied().collect();
                }
            }
        });
    }

    /// Janela de preferências: o que antes ficava espalhado pela fileira de filtros.
    fn ui_prefs(&mut self, ctx: &egui::Context) {
        if !self.show_prefs {
            return;
        }
        let mut open = true;
        let old_locale = self.cfg.locale;
        let mut locale = old_locale;
        egui::Window::new(locale.text("Preferências", "Preferences"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(360.0)
            .anchor(egui::Align2::LEFT_BOTTOM, [NAV_W + 12.0, -12.0])
            .frame(egui::Frame::window(&ctx.style()).fill(SURFACE).corner_radius(CARD_R).inner_margin(egui::Margin::same(14)))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
                ui.label(RichText::new(locale.text("Idioma", "Language")).strong());
                egui::ComboBox::from_id_salt("locale")
                    .selected_text(locale.name())
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut locale, Locale::Portuguese, "Português");
                        ui.selectable_value(&mut locale, Locale::English, "English");
                    });
                ui.separator();
                ui.label(RichText::new(locale.text("Coluna de memória", "Memory column")).strong());
                let mut metric = self.cfg.mem_metric;
                egui::ComboBox::from_id_salt("mem_metric")
                    .selected_text(metric.label_for(locale))
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        for m in MemMetric::ALL {
                            ui.selectable_value(&mut metric, m, m.label_for(locale)).on_hover_text(m.tip_for(locale));
                        }
                    });
                if metric != self.cfg.mem_metric {
                    self.cfg.mem_metric = metric;
                    self.cfg_dirty = true;
                    self.derived_dirty = true;
                    self.row_cache.invalidate();
                }
                ui.label(RichText::new(self.cfg.mem_metric.tip_for(locale)).color(MUTED).size(11.5));
                ui.separator();
                ui.label(RichText::new(locale.text("Ocultar abaixo de", "Hide below")).strong())
                    .on_hover_text(locale.text("Esconde quem está abaixo destes mínimos. Com Agrupar por app, o filtro vale no total do app, não em cada helper. 0 mostra tudo.", "Hides processes below these thresholds. With Group by app, the filter applies to the app total, not each helper. 0 shows everything."));
                let (mut min_mb, mut min_cpu, mut min_gpu, mut min_vram) = (self.cfg.min_mb, self.cfg.min_cpu, self.cfg.min_gpu, self.cfg.min_vram_mb);
                let mut changed = false;
                egui::Grid::new("prefs-cuts").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    ui.label("RAM");
                    changed |= ui.add(egui::DragValue::new(&mut min_mb).range(0..=4096).speed(5).suffix(" MB")).changed();
                    ui.end_row();
                    ui.label("CPU");
                    changed |= ui.add(egui::DragValue::new(&mut min_cpu).range(0.0..=100.0).speed(0.5).suffix("%")).changed();
                    ui.end_row();
                    ui.label("GPU");
                    changed |= ui.add(egui::DragValue::new(&mut min_gpu).range(0.0..=100.0).speed(1.0).suffix("%")).changed();
                    ui.end_row();
                    ui.label("VRAM");
                    changed |= ui.add(egui::DragValue::new(&mut min_vram).range(0..=16384).speed(8).suffix(" MB")).changed();
                    ui.end_row();
                });
                if changed {
                    self.cfg.min_mb = min_mb;
                    self.cfg.min_cpu = min_cpu;
                    self.cfg.min_gpu = min_gpu;
                    self.cfg.min_vram_mb = min_vram;
                    self.cfg_dirty = true;
                    self.row_cache.invalidate();
                }
                // Cortes prontos: o caso de uso real é "cadê quem está comendo agora",
                // não calibrar quatro DragValues no susto.
                ui.horizontal_wrapped(|ui| {
                    let cortes_ativos = self.cfg.min_cpu > 0.0 || self.cfg.min_gpu > 0.0 || self.cfg.min_mb > 0 || self.cfg.min_vram_mb > 0;
                    if ui.small_button(locale.text("Só CPU ativa (≥ 3%)", "CPU active only (≥ 3%)")).on_hover_text(locale.text("Esconde quem não está usando CPU agora (corte em 3% da máquina)", "Hides processes not using CPU now (3% of the machine)")).clicked() {
                        self.cfg.min_cpu = 3.0;
                        self.cfg_dirty = true;
                        self.row_cache.invalidate();
                    }
                    if ui.small_button(locale.text("Só GPU em uso (≥ 1%)", "GPU in use only (≥ 1%)")).on_hover_text(locale.text("Esconde quem não está com carga na GPU", "Hides processes with no GPU load")).clicked() {
                        self.cfg.min_gpu = 1.0;
                        self.cfg_dirty = true;
                        self.row_cache.invalidate();
                    }
                    if ui.add_enabled(cortes_ativos, egui::Button::new(locale.text("Mostrar tudo", "Show all")).small()).on_hover_text(locale.text("Zera os quatro cortes", "Clears all four thresholds")).clicked() {
                        self.cfg.min_mb = 0;
                        self.cfg.min_cpu = 0.0;
                        self.cfg.min_gpu = 0.0;
                        self.cfg.min_vram_mb = 0;
                        self.cfg_dirty = true;
                        self.row_cache.invalidate();
                    }
                });
                ui.separator();
                ui.label(RichText::new(locale.text("Amostragem", "Sampling")).strong());
                ui.horizontal(|ui| self.ui_sampling_controls(ui));
                #[cfg(windows)]
                if !self.is_admin {
                    ui.separator();
                    if ui.button(locale.text("⬆ Reabrir como administrador", "⬆ Relaunch as administrator")).on_hover_text(locale.text("Necessário para encerrar serviços, processos de outros usuários e ler a temperatura da CPU", "Required to stop services, terminate other users' processes, and read CPU temperature")).clicked() {
                        self.relaunch_as_admin();
                    }
                }
            });
        if locale != old_locale {
            self.cfg.locale = locale;
            self.cfg_dirty = true;
        }
        self.show_prefs = open;
    }

    /// Quanto do medidor do topo a lista consegue explicar. `None` sem leitura de CPU.
    fn calculate_cpu_split(&self) -> Option<CpuSplit> {
        let total = self.sys.cpu_pct?;
        let mut listed = 0.0f32;
        let mut children = 0.0f32;
        for p in &self.procs {
            listed += p.cpu_raw_pct;
            children += p.cpu_children_pct;
        }
        Some(CpuSplit {
            total,
            listed,
            children,
        })
    }

    fn refresh_derived_if_dirty(&mut self) {
        if !self.derived_dirty {
            return;
        }
        let pressure = self.calculate_pressure_snap();
        let thieves = self.calculate_thieves(pressure.game_open);
        let breakdown = self.calculate_breakdown();
        let cpu_split = self.calculate_cpu_split();
        let cat_totals = self.calculate_cat_totals(self.cfg.mem_metric);
        let private_cat_totals = self.calculate_cat_totals(MemMetric::Private);
        let metric_total = self.procs.iter().map(|p| self.mem_of(p)).sum();
        #[cfg(target_os = "linux")]
        let linux_memory_summary = self.calculate_linux_memory_summary();
        #[cfg(not(target_os = "linux"))]
        let linux_memory_summary = String::new();
        #[cfg(target_os = "linux")]
        let linux_memory_available = self
            .procs
            .iter()
            .filter(|p| p.linux_memory.is_some())
            .count();
        #[cfg(not(target_os = "linux"))]
        let linux_memory_available = 0;
        self.derived = UiDerived {
            breakdown,
            pressure,
            thieves,
            cpu_split,
            cat_totals,
            private_cat_totals,
            linux_memory_summary,
            linux_memory_available,
            metric_total,
        };
        self.derived_dirty = false;
    }

    fn breakdown(&self) -> MemBreakdown {
        self.derived.breakdown
    }

    fn pressure_snap(&self) -> pressure::Snapshot {
        self.derived.pressure
    }

    fn thieves(&self) -> Vec<pressure::Thief> {
        self.derived.thieves.clone()
    }

    fn cpu_split(&self) -> Option<CpuSplit> {
        self.derived.cpu_split
    }

    #[cfg(target_os = "linux")]
    fn linux_memory_summary(&self) -> &str {
        &self.derived.linux_memory_summary
    }

    fn push_hist(&mut self) {
        let push = |h: &mut VecDeque<f32>, v: Option<f32>| {
            let v = v.or_else(|| h.back().copied()).unwrap_or(0.0);
            h.push_back(v.clamp(0.0, 100.0));
            while h.len() > HIST_LEN {
                h.pop_front();
            }
        };
        push(&mut self.hist_cpu, self.sys.cpu_pct);
        let total = self.mem.total_phys.max(1);
        push(
            &mut self.hist_ram,
            Some(self.mem.used_phys() as f32 / total as f32 * 100.0),
        );
        push(
            &mut self.hist_gpu,
            self.sys.gpu.as_ref().and_then(|g| g.util_pct),
        );
        push(&mut self.hist_disk, self.sys.disk_pct);
    }
}

/// Medidor do topo (o que o kernel cobrou de todos os núcleos) contra a soma da lista.
/// A diferença é o que nenhuma linha mostra: processo que nasceu e morreu entre duas
/// amostras, interrupções e trabalho do kernel fora de qualquer PID.
#[derive(Clone, Copy, Debug, PartialEq)]
struct CpuSplit {
    total: f32,
    /// Σ `cpu_raw_pct` dos processos vivos agora, na mesma janela do medidor.
    listed: f32,
    /// Σ `cpu_children_pct`: filhos já encerrados, creditados ao pai. Parte do `unlisted`.
    children: f32,
}

impl CpuSplit {
    /// O que o medidor cobrou e a lista não mostra. Nunca negativo: a lista pode passar do
    /// medidor por 1–2 pontos porque as duas janelas não fecham no mesmo microssegundo.
    fn unlisted(&self) -> f32 {
        (self.total - self.listed).max(0.0)
    }

    /// Chip vermelho no card só quando a diferença engana de verdade: pelo menos 3 pontos
    /// e um quinto do medidor. Fora disso, a conta fecha no arredondamento e a linha some.
    fn unlisted_chip(&self, locale: Locale) -> Option<(String, String)> {
        let u = self.unlisted();
        if u < 3.0 || u < self.total * 0.2 {
            return None;
        }
        Some((
            format!(
                "{} {}",
                locale.text("fora da lista", "not listed"),
                App::fmt_pct(u)
            ),
            self.explain(locale),
        ))
    }

    fn explain(&self, locale: Locale) -> String {
        let u = self.unlisted();
        let kids = self.children.min(u);
        let rest = u - kids;
        let mut t = if locale == Locale::Portuguese {
            format!(
                "Na lista agora: {} (soma de todos os processos, mesmo os que arredondam pra 0,0%).\nFora da lista: {}",
                App::fmt_pct(self.listed),
                App::fmt_pct(u),
            )
        } else {
            format!(
                "Listed now: {} (sum of all processes, including those rounded to 0.0%).\nNot listed: {}",
                App::fmt_pct(self.listed),
                App::fmt_pct(u),
            )
        };
        if kids >= 0.05 {
            if locale == Locale::Portuguese {
                t.push_str(&format!(
                    "\n  · {} em processos que nasceram e morreram entre duas amostras (rg, git, cc, shells). O pai mostra isso como “+filhos” na coluna CPU.",
                    App::fmt_pct(kids)
                ));
            } else {
                t.push_str(&format!(
                    "\n  · {} in processes that started and exited between two samples (rg, git, cc, shells). The parent shows this as “+children” in the CPU column.",
                    App::fmt_pct(kids)
                ));
            }
        }
        if rest >= 0.05 {
            if locale == Locale::Portuguese {
                t.push_str(&format!(
                    "\n  · {} em interrupções, trabalho do kernel sem PID e processos curtos ainda sem pai que os recolhesse.",
                    App::fmt_pct(rest)
                ));
            } else {
                t.push_str(&format!(
                    "\n  · {} in interrupts, kernel work without a PID, and short-lived processes whose parent has not reaped them yet.",
                    App::fmt_pct(rest)
                ));
            }
        }
        t.push_str(locale.text(
            "\n\nIntervalo menor (a cada 1s) pega mais processo curto na lista.",
            "\n\nA shorter interval (every 1s) captures more short-lived processes in the list.",
        ));
        t
    }
}

// ---------- util ----------

fn setup_fonts(ctx: &egui::Context) {
    use egui::{FontData, FontDefinitions, FontFamily};
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let fonts_dir = format!("{windir}\\Fonts\\");
    let mut defs = FontDefinitions::default();
    // Segoe UI como fonte principal (visual nativo do Windows, acentos completos)
    if let Ok(bytes) = std::fs::read(format!("{fonts_dir}segoeui.ttf")) {
        defs.font_data.insert(
            "segoeui".into(),
            std::sync::Arc::new(FontData::from_owned(bytes)),
        );
        defs.families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, "segoeui".into());
    }
    // Segoe UI Symbol: ● ▶ ▼ ⏸ e afins, como fallback
    if let Ok(bytes) = std::fs::read(format!("{fonts_dir}seguisym.ttf")) {
        defs.font_data.insert(
            "seguisym".into(),
            std::sync::Arc::new(FontData::from_owned(bytes)),
        );
        defs.families
            .entry(FontFamily::Proportional)
            .or_default()
            .push("seguisym".into());
        defs.families
            .entry(FontFamily::Monospace)
            .or_default()
            .push("seguisym".into());
    }
    // Consolas para monospace (comandos/caminhos)
    if let Ok(bytes) = std::fs::read(format!("{fonts_dir}consola.ttf")) {
        defs.font_data.insert(
            "consolas".into(),
            std::sync::Arc::new(FontData::from_owned(bytes)),
        );
        defs.families
            .entry(FontFamily::Monospace)
            .or_default()
            .insert(0, "consolas".into());
    }
    // Linux: Adwaita Sans/Mono (a fonte do GNOME, que o Omarchy traz) no lugar da
    // Ubuntu-Light embutida no egui. Sem elas, fica o padrão.
    #[cfg(target_os = "linux")]
    {
        if let Ok(bytes) = std::fs::read("/usr/share/fonts/Adwaita/AdwaitaSans-Regular.ttf") {
            defs.font_data.insert(
                "adwaita".into(),
                std::sync::Arc::new(FontData::from_owned(bytes)),
            );
            defs.families
                .entry(FontFamily::Proportional)
                .or_default()
                .insert(0, "adwaita".into());
        }
        if let Ok(bytes) = std::fs::read("/usr/share/fonts/Adwaita/AdwaitaMono-Regular.ttf") {
            defs.font_data.insert(
                "adwaitamono".into(),
                std::sync::Arc::new(FontData::from_owned(bytes)),
            );
            defs.families
                .entry(FontFamily::Monospace)
                .or_default()
                .insert(0, "adwaitamono".into());
        }
    }
    ctx.set_fonts(defs);
}

/// Paleta: os cinzas neutros do libadwaita escuro (janela 1e1e1e, barra lateral 242424,
/// card 2b2b2b) e o azul do GNOME como acento. As cores de categoria e o "calor" da RAM
/// são semânticas e ficam de fora do acento.
pub const BG: Color32 = Color32::from_rgb(30, 30, 30);
pub const PANEL: Color32 = Color32::from_rgb(36, 36, 36);
pub const SURFACE: Color32 = Color32::from_rgb(43, 43, 43);
pub const SURFACE_HI: Color32 = Color32::from_rgb(54, 54, 54);
pub const LINE: Color32 = Color32::from_rgb(51, 51, 51);
pub const TEXT: Color32 = Color32::from_rgb(245, 245, 245);
pub const MUTED: Color32 = Color32::from_rgb(154, 154, 154);
pub const ACCENT: Color32 = Color32::from_rgb(53, 132, 228);
pub const ACCENT_BG: Color32 = Color32::from_rgb(45, 62, 84);
pub const ACCENT_FG: Color32 = Color32::from_rgb(120, 174, 237);
/// Verde "segurando" / laranja "rampa" do ESTABILIZAR — portados do TempHUD pra manter a
/// mesma leitura de estado entre os dois apps.
const THERM_STAB_BG: Color32 = Color32::from_rgb(10, 61, 50);
const THERM_STAB_FG: Color32 = Color32::from_rgb(105, 240, 174);
const THERM_WARN_BG: Color32 = Color32::from_rgb(74, 28, 11);
const THERM_WARN_FG: Color32 = Color32::from_rgb(255, 171, 145);

fn setup_style(ctx: &egui::Context) {
    setup_fonts(ctx);
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = SURFACE;
    v.extreme_bg_color = BG;
    v.faint_bg_color = Color32::from_rgb(38, 38, 38);
    v.code_bg_color = SURFACE;
    v.override_text_color = Some(TEXT);
    v.hyperlink_color = ACCENT;
    v.window_stroke = egui::Stroke::new(1.0_f32, LINE);
    v.window_corner_radius = 8.0.into();
    v.menu_corner_radius = 6.0.into();
    v.selection.bg_fill = ACCENT_BG;
    v.selection.stroke = egui::Stroke::new(1.0_f32, ACCENT);
    v.widgets.noninteractive.bg_fill = PANEL;
    v.widgets.noninteractive.weak_bg_fill = PANEL;
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, LINE);
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, TEXT);
    v.widgets.inactive.bg_fill = SURFACE_HI;
    v.widgets.inactive.weak_bg_fill = SURFACE_HI;
    v.widgets.inactive.bg_stroke = egui::Stroke::NONE;
    v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, Color32::from_rgb(215, 215, 215));
    v.widgets.hovered.bg_fill = SURFACE_HI;
    v.widgets.hovered.weak_bg_fill = SURFACE_HI;
    v.widgets.hovered.bg_fill = Color32::from_rgb(64, 64, 64);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(64, 64, 64);
    v.widgets.hovered.bg_stroke = egui::Stroke::NONE;
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0_f32, Color32::WHITE);
    v.widgets.active.bg_fill = ACCENT_BG;
    v.widgets.active.weak_bg_fill = ACCENT_BG;
    v.widgets.active.bg_stroke = egui::Stroke::new(1.0_f32, ACCENT);
    v.widgets.active.fg_stroke = egui::Stroke::new(1.0_f32, Color32::WHITE);
    v.widgets.open.bg_fill = SURFACE_HI;
    v.widgets.open.weak_bg_fill = SURFACE_HI;
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = 8.0.into();
    }
    v.striped = false;
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
        s.text_styles.insert(
            TextStyle::Monospace,
            FontId::new(12.0, FontFamily::Monospace),
        );
        s.text_styles.insert(
            TextStyle::Heading,
            FontId::new(17.0, FontFamily::Proportional),
        );
        s.spacing.item_spacing = egui::vec2(8.0, 4.0);
        s.spacing.button_padding = egui::vec2(10.0, 4.0);
        s.spacing.menu_margin = egui::Margin::same(8);
        s.interaction.selectable_labels = false;
        s.interaction.tooltip_delay = 0.35;
        // O RamDog é um painel de dados, não uma cena animada. A animação padrão
        // do egui agenda vários quadros extras em cada hover/scrollbar; no Linux com
        // OpenGL isso transforma um movimento curto do mouse em uma rajada de GPU.
        // Transições instantâneas preservam a interação e pintam só o quadro útil.
        s.animation_time = 0.0;
    });
}

/// Número tabular (Consolas) — colunas numéricas alinham dígito a dígito.
fn num(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).monospace().size(12.5)
}

fn ui_dark() -> bool {
    true
}

fn ui_text_color(dark: bool) -> Color32 {
    if dark {
        TEXT
    } else {
        Color32::from_gray(30)
    }
}

fn ram_color(bytes: u64, default: Color32) -> Color32 {
    if bytes >= GB {
        Color32::from_rgb(255, 110, 110)
    } else if bytes >= 300 * MB {
        Color32::from_rgb(255, 180, 90)
    } else if bytes >= 100 * MB {
        Color32::from_rgb(230, 220, 140)
    } else {
        default
    }
}

fn wine_cmd(p: &ProcInfo) -> bool {
    let n = p.name_lower.as_str();
    n.contains("wine")
        || p.exe_path.to_ascii_lowercase().contains("wine")
        || p.launcher.wine_prefix.is_some()
        || p.launcher.steam_app_id.is_some()
}

/// Fim do primeiro `.exe` na string (ASCII, case-insensitive) — sempre em fronteira de char.
fn exe_token_end(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    if b.len() < 4 {
        return None;
    }
    (0..=b.len() - 4)
        .find(|&i| b[i..i + 4].eq_ignore_ascii_case(b".exe"))
        .map(|i| i + 4)
}

/// A coluna de comando mostrava o caminho completo do exe e truncava antes de chegar nos
/// argumentos — justamente a parte que diferencia um `brave.exe` do outro. Aqui o caminho
/// sai (já está na coluna Nome) e sobram os argumentos.
fn cmd_args(p: &ProcInfo, locale: Locale) -> String {
    let cmd = p.cmdline.trim();
    if cmd.is_empty() {
        return if p.exe_path.is_empty() {
            locale
                .text("(sem acesso)", "(access unavailable)")
                .to_string()
        } else {
            p.exe_path.clone()
        };
    }
    let rest = if let Some(stripped) = cmd.strip_prefix('"') {
        match stripped.find('"') {
            Some(i) => &stripped[i + 1..],
            None => cmd,
        }
    } else if wine_cmd(p) {
        if let Some(exe) = identity::windows_exe_from_cmdline(cmd) {
            if let Some(at) = cmd.find(exe) {
                &cmd[at..]
            } else {
                cmd
            }
        } else {
            cmd
        }
    } else if let Some(i) = exe_token_end(cmd) {
        &cmd[i..]
    } else {
        match cmd.find(' ') {
            Some(i) => &cmd[i..],
            None => "",
        }
    };
    let rest = rest.trim();
    if !rest.is_empty() {
        return rest.to_string();
    }
    // Sem argumentos: a pasta é o que resta de informação útil.
    std::path::Path::new(&p.exe_path)
        .parent()
        .map(|d| d.display().to_string())
        .unwrap_or_else(|| p.exe_path.clone())
}

/// Bytes/s de disco, mesma régua de grandeza que `fmt_bytes` mas com sufixo /s.
pub fn fmt_bps(bps: f64) -> String {
    let b = bps.max(0.0) as u64;
    if b >= GB {
        format!("{} GB/s", pt_num(b as f64 / GB as f64, 2))
    } else if b >= MB {
        format!("{} MB/s", pt_num(b as f64 / MB as f64, 1))
    } else {
        format!("{} KB/s", pt_num(b as f64 / 1024.0, 0))
    }
}

/// Compositor / shell do desktop: raiz de tudo que o usuário abriu pelo menu ou atalho.
fn is_desktop_shell(name_lower: &str) -> bool {
    matches!(
        name_lower.strip_suffix(".exe").unwrap_or(name_lower),
        "hyprland"
            | "start-hyprland"
            | "sway"
            | "niri"
            | "river"
            | "gnome-shell"
            | "gnome-session-b"
            | "plasmashell"
            | "kwin_wayland"
            | "kwin_x11"
            | "xfce4-session"
            | "explorer"
            | "finder"
            | "loginwindow"
            | "windowserver"
    )
}

fn pid_still_same(pid: u32, _created: i64) -> bool {
    #[cfg(target_os = "linux")]
    {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        true
    }
}

pub fn fmt_bytes(b: u64) -> String {
    if b >= GB {
        format!("{} GB", pt_num(b as f64 / GB as f64, 2))
    } else {
        format!("{} MB", pt_num(b as f64 / MB as f64, 1))
    }
}

pub fn fmt_bytes_short(b: u64) -> String {
    if b >= GB {
        format!("{} GB", pt_num(b as f64 / GB as f64, 1))
    } else {
        format!("{} MB", pt_num(b as f64 / MB as f64, 0))
    }
}

fn fmt_gb(b: u64) -> String {
    format!("{} GB", pt_num(b as f64 / GB as f64, 1))
}

/// Formata número no padrão pt-BR (1.234,5).
fn pt_num(v: f64, decimals: usize) -> String {
    let s = format!("{:.*}", decimals, v);
    let (int, frac) = match s.split_once('.') {
        Some((i, f)) => (i.to_string(), Some(f.to_string())),
        None => (s, None),
    };
    let mut out = String::new();
    let digits: Vec<char> = int.chars().collect();
    for (i, c) in digits.iter().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push('.');
        }
        out.push(*c);
    }
    if let Some(f) = frac {
        out.push(',');
        out.push_str(&f);
    }
    out
}

pub fn fmt_age(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}min", secs / 60)
    } else if secs < 86400 {
        format!("{}h {}min", secs / 3600, (secs % 3600) / 60)
    } else {
        format!("{}d {}h", secs / 86400, (secs % 86400) / 3600)
    }
}

pub fn open_url(url: &str) {
    #[cfg(windows)]
    let _ = std::process::Command::new("cmd")
        .args(["/c", "start", "", url])
        .spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

fn open_in_explorer(path: &str) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer.exe")
        .arg(format!("/select,{path}"))
        .spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open")
        .args(["-R", path])
        .spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(dir) = std::path::Path::new(path).parent() {
            let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
        }
    }
}

fn metric_available(metric: MemMetric, p: &ProcInfo) -> bool {
    #[cfg(target_os = "linux")]
    if matches!(metric, MemMetric::Proportional) {
        return p.linux_memory.is_some();
    }
    let _ = (metric, p);
    true
}

fn private_memory_text(p: &ProcInfo, locale: Locale) -> String {
    if metric_available(MemMetric::Private, p) {
        fmt_bytes(p.private_ws)
    } else {
        locale.text("indisponível", "unavailable").to_string()
    }
}

fn disk_usage_tip(locale: Locale) -> &'static str {
    if cfg!(target_os = "linux") {
        locale.text(
            "Tempo ocupado do disco físico mais ativo. A taxa em bytes/s soma os discos físicos; partições e dispositivos virtuais não são contados novamente.",
            "Busy time of the most active physical disk. The bytes/s rate sums physical disks; partitions and virtual devices are not counted again.",
        )
    } else {
        locale.text(
            "Tempo ocupado do disco (contador do sistema).",
            "Disk busy time (system counter).",
        )
    }
}

fn aggregate_memory_text(bytes: u64, complete: bool) -> String {
    if complete {
        fmt_bytes(bytes)
    } else {
        format!("≥ {}", fmt_bytes(bytes))
    }
}
