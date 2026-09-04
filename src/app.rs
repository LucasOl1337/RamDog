//! Interface egui: lista / árvore / categorias, detalhes, kill, lock.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::time::Instant;

use egui::{Align, Color32, Layout, Rect, RichText, Stroke, StrokeKind, TextureHandle, Vec2};
use egui_extras::{Column, TableBuilder, TableRow};

use crate::categories::{self, classify, is_critical, Category};
use crate::config::{Config, MemMetric, ViewMode};
use crate::boot::{Boot, BootOut};
use crate::drains::{DrainOut, Drains};
use crate::hwtemp::HwTemp;
use crate::knowledge;
use crate::metrics::SysSample;
use crate::procs::{self, KernelMem, MemStatus, ProcInfo};
use crate::sampler::{self, SamplerHandle};
use crate::screens::{ScreenOut, Screens};
use crate::signature::{self, SigInfo};
use crate::usage;

const MB: u64 = 1024 * 1024;
const GB: u64 = 1024 * 1024 * 1024;
const ROW_H: f32 = 24.0;
const ICON: f32 = 16.0;
/// Altura das barrinhas dos medidores do topo (CPU/RAM/GPU/Disco) — uma só constante pras
/// quatro pra elas ficarem realmente alinhadas, não só "parecidas".
const TOP_BAR_H: f32 = 8.0;
/// Régua dos medidores do topo, usada igual nos dois modos: um bloco é rótulo +
/// temperatura, número grande + detalhe, barra. Quatro blocos idênticos alinham sozinhos —
/// larguras diferentes por medidor eram a origem do topo desalinhado.
/// Os medidores dividem entre si toda a largura que os controles do topo não usam, dentro
/// destes limites. Largura fixa deixava uma faixa morta entre o medidor de disco e os
/// controles — quanto mais larga a janela, maior o buraco.
const TILE_MIN: f32 = 132.0;
const TILE_MAX: f32 = 230.0;
const TILE_GAP: f32 = 14.0;
const TILE_H: f32 = 47.0;
/// Altura única de todo controle das duas fileiras do topo (botão, combo, busca, chip).
/// Sem isso cada widget usa a altura natural do egui e a fileira fica serrilhada.
const CTRL_H: f32 = 24.0;

/// Modo mini — HUD de monitoramento. Tamanho fixo: quatro blocos 2x2, a faixa de controles
/// e a faixa de fans. Fixo de propósito; um HUD que o usuário arrasta de tamanho volta a ter
/// os problemas de layout da janela grande, sem ganho nenhum.
pub const MINI_W: f32 = 366.0;
pub const MINI_H: f32 = 166.0;
/// Mínimo da janela completa — repetido aqui porque sair do mini precisa restaurar
/// exatamente o mesmo limite que `main` aplica na abertura.
pub const FULL_MIN_W: f32 = 760.0;
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortKey {
    Name,
    Ram,
    Cat,
    Pid,
    Cpu,
    Gpu,
    Disk,
    Age,
    Parent,
}

#[derive(Clone, Copy)]
enum Row {
    Proc {
        pid: u32,
        depth: u8,
        has_children: bool,
        expanded: bool,
        dim: bool,
    },
    CatHeader {
        cat: Category,
        count: usize,
        total: u64,
        collapsed: bool,
    },
    /// Cabeçalho de um app: índice em `App::groups`. O conteúdo não fica aqui de
    /// propósito — as somas são recalculadas a cada quadro, inclusive quando a ordem
    /// está congelada porque o mouse está em cima da tabela.
    AppHeader {
        gi: usize,
    },
    /// Linha sintética: memória em uso que não pertence a processo nenhum.
    /// Sem PID, sem kill — existe para a soma da lista bater com o "em uso" do topo.
    System {
        kind: SysRow,
        bytes: u64,
    },
}

/// Um app na visão Lista: todos os processos do mesmo executável somados numa linha.
///
/// É a diferença que fazia o Gerenciador de Tarefas parecer melhor: lá o Chrome com 30
/// renderizadores é uma linha de 90%, aqui eram 30 linhas de 3% que não chegavam nem
/// perto do topo da lista ordenada por CPU. O maior consumidor da máquina ficava
/// invisível por estar picado.
struct AppGroup {
    /// Caminho do exe em minúsculo — ou o nome, quando não houve acesso ao caminho.
    key: String,
    name: String,
    name_lower: String,
    cat: Category,
    /// Ordenados pelo critério da tabela; o primeiro é o que o clique seleciona.
    pids: Vec<u32>,
    ram: u64,
    cpu: f32,
    gpu: f32,
    disk: f64,
    /// FILETIME do processo mais antigo do grupo — é a idade do app, não a do último
    /// aba/renderizador que abriu.
    oldest: i64,
}

/// As parcelas do "em uso" que nunca aparecem numa lista de processos.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SysRow {
    PagedPool,
    NonPagedPool,
    SharedAndCache,
}

impl SysRow {
    fn label(self) -> &'static str {
        match self {
            SysRow::PagedPool => "Kernel — pool paginado",
            SysRow::NonPagedPool => "Kernel — pool não-paginado",
            SysRow::SharedAndCache => "Compartilhado, cache e tabelas",
        }
    }

    fn tip(self) -> &'static str {
        match self {
            SysRow::PagedPool => concat!(
                "Memória do kernel e dos drivers que pode ser paginada ao disco.\n\n",
                "Não pertence a processo nenhum, por isso nunca aparece no Gerenciador de ",
                "Tarefas. Acima de ~2 GB costuma indicar vazamento de driver."
            ),
            SysRow::NonPagedPool => concat!(
                "Memória do kernel que nunca sai da RAM física — filas de I/O, estruturas de ",
                "driver, buffers de rede.\n\nSempre residente, sempre invisível na lista de processos."
            ),
            SysRow::SharedAndCache => concat!(
                "O que sobra do \"em uso\" depois de descontar a memória privada dos processos ",
                "e os dois pools do kernel.\n\n",
                "É sobretudo memória compartilhada residente (DLLs e seções mapeadas em vários ",
                "processos, contadas uma vez só aqui), mais cache de arquivos residente, tabelas ",
                "de página e páginas travadas por driver de GPU.\n\n",
                "Calculado por diferença — é um resto, não uma medição direta."
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

pub struct App {
    cfg: Config,
    cfg_dirty: bool,
    sampler: SamplerHandle,

    procs: Vec<ProcInfo>,
    by_pid: HashMap<u32, usize>,
    children: HashMap<u32, Vec<u32>>,
    cats: HashMap<u32, Category>,
    subtree: HashMap<u32, u64>,
    subtree_count: HashMap<u32, usize>,
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
    /// Apps recolhidos, por chave de `AppGroup`.
    collapsed_apps: HashSet<String>,
    status: Option<(Instant, String, bool)>,
    is_admin: bool,
    scroll_to_selected: bool,
    /// Ordem das linhas congelada enquanto o mouse está sobre a tabela (evita matar a linha errada).
    cached_rows: Option<Vec<Row>>,
    cached_key: u64,
    rows_dirty: bool,
    table_rect: Option<egui::Rect>,
    order_frozen: bool,
    drains: Drains,
    boot: Boot,
    screens: Screens,
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
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_style(&cc.egui_ctx);
        let cfg = Config::load();
        let mini = cfg.mini;
        // Abrir direto num addon é legítimo (foi assim que fechou), mas o botão de voltar
        // precisa de um destino desde o primeiro frame.
        let last_core = if cfg.view.is_addon() { ViewMode::List } else { cfg.view };
        let is_admin = procs::is_admin();
        let sampler = sampler::spawn(cc.egui_ctx.clone(), cfg.refresh_ms);
        let (sig_tx, sig_rx) = std::sync::mpsc::channel();
        Self {
            cfg,
            cfg_dirty: false,
            sampler,
            procs: Vec::new(),
            by_pid: HashMap::new(),
            children: HashMap::new(),
            cats: HashMap::new(),
            subtree: HashMap::new(),
            subtree_count: HashMap::new(),
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
            ncpu: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
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
            collapsed_apps: HashSet::new(),
            status: None,
            is_admin,
            scroll_to_selected: false,
            cached_rows: None,
            cached_key: 0,
            rows_dirty: true,
            table_rect: None,
            order_frozen: false,
            drains: Drains::new(),
            boot: Boot::new(),
            screens: Screens::new(),
            last_core,
            thermal_edit: HashMap::new(),
            stab_pending: None,
            // `main` já abriu a janela no modo lido da config — nada a aplicar no 1º frame.
            applied_mini: mini,
            full_size: None,
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
        let cpu_keep = if snap.sys.cpu_pct.is_none() { self.sys.cpu_pct } else { None };
        self.sys = snap.sys;
        if let Some(p) = cpu_keep {
            self.sys.cpu_pct = Some(p);
        }
        self.gpu_per_proc = snap.gpu_per_proc;
        self.hwtemp = snap.hwtemp;
        self.usage.tick(&self.procs);
        self.usage.save_if_due();
        // Nome de cada PID guardado enquanto ele existe: é o que permite dizer
        // "smss.exe (1208), já encerrado" em vez do número solto quando o pai morre.
        for p in &self.procs {
            self.seen_names.entry(p.pid).or_insert_with(|| p.name.clone());
        }
        self.refresh_services();
        self.rebuild_indexes();
        if let Some(pid) = self.selected {
            if let Some(&i) = self.by_pid.get(&pid) {
                self.selected_keep = Some((self.procs[i].clone(), self.cat(pid)));
            }
        }
    }

    fn rebuild_indexes(&mut self) {
        self.by_pid = self.procs.iter().enumerate().map(|(i, p)| (p.pid, i)).collect();
        self.children.clear();
        for p in &self.procs {
            if p.ppid != 0 {
                self.children.entry(p.ppid).or_default().push(p.pid);
            }
        }
        let overrides: HashMap<String, Category> = self.cfg.overrides.iter().map(|(k, v)| (k.clone(), *v)).collect();
        self.cats = classify(&self.procs, &overrides);
        // soma de subárvore (memo por DFS)
        self.subtree.clear();
        self.subtree_count.clear();
        let pids: Vec<u32> = self.procs.iter().map(|p| p.pid).collect();
        for pid in pids {
            self.subtree_total(pid, 0);
        }
    }

    fn subtree_total(&mut self, pid: u32, depth: usize) -> (u64, usize) {
        if let (Some(&t), Some(&c)) = (self.subtree.get(&pid), self.subtree_count.get(&pid)) {
            return (t, c);
        }
        let m = self.cfg.mem_metric;
        let own = self.by_pid.get(&pid).map(|&i| Self::metric_of(m, &self.procs[i])).unwrap_or(0);
        let mut total = own;
        let mut count = 1usize;
        if depth < 128 {
            let kids = self.children.get(&pid).cloned().unwrap_or_default();
            for k in kids {
                let (t, c) = self.subtree_total(k, depth + 1);
                total += t;
                count += c;
            }
        }
        self.subtree.insert(pid, total);
        self.subtree_count.insert(pid, count);
        (total, count)
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
            let due = self.services_at.map(|t| t.elapsed().as_secs() >= 10).unwrap_or(true);
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
            return Some(SigInfo { trust: signature::Trust::Unknown("sem acesso ao caminho".into()), signer: String::new() });
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

    fn mem_of(&self, p: &ProcInfo) -> u64 {
        Self::metric_of(self.cfg.mem_metric, p)
    }

    fn metric_of(m: MemMetric, p: &ProcInfo) -> u64 {
        match m {
            MemMetric::WorkingSet => p.working_set,
            MemMetric::Private => p.private_ws,
            MemMetric::Commit => p.commit,
        }
    }

    /// Reparte o "em uso" em parcelas que somam exatamente o total.
    ///
    /// Sempre sobre memória **privada**, independentemente da métrica escolhida na coluna:
    /// o working set conta a mesma página compartilhada em cada processo que a mapeia, então
    /// somá-lo daria mais que a RAM instalada. O resto sai por diferença.
    fn breakdown(&self) -> MemBreakdown {
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

    fn cat(&self, pid: u32) -> Category {
        self.cats.get(&pid).copied().unwrap_or(Category::Other)
    }

    fn is_locked(&self, p: &ProcInfo) -> bool {
        is_critical(&p.name_lower, p.pid) || self.cfg.locked.contains(&p.name_lower) || p.pid == std::process::id()
    }

    fn passes(&self, p: &ProcInfo, search: &str) -> bool {
        if !self.cat_enabled.contains(&self.cat(p.pid)) {
            return false;
        }
        if self.mem_of(p) < self.cfg.min_mb as u64 * MB {
            return false;
        }
        if !search.is_empty() {
            let pid_s = p.pid.to_string();
            let hit = p.name_lower.contains(search)
                || pid_s == search
                || p.exe_path.to_lowercase().contains(search)
                || p.cmdline.to_lowercase().contains(search)
                || p.launcher.short().to_lowercase().contains(search);
            if !hit {
                return false;
            }
        }
        true
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
    fn origin_label(&self, p: &ProcInfo) -> (String, Option<u32>, String, bool) {
        let mut cur = p.ppid;
        let mut chain: Vec<String> = Vec::new();
        let mut guard = 0;
        while cur != 0 && guard < 64 {
            guard += 1;
            let Some(a) = self.proc(cur) else { break };
            chain.push(format!("{} ({})", a.name, a.pid));
            if !categories::is_generic_host(&a.name_lower) {
                let tip = if chain.len() > 1 {
                    format!("{}\nclique para selecionar", chain.iter().rev().cloned().collect::<Vec<_>>().join(" › "))
                } else {
                    format!("PID {} — clique para selecionar o pai", a.pid)
                };
                return (a.name.clone(), Some(a.pid), tip, false);
            }
            cur = a.ppid;
        }
        // Cadeia só de hosts genéricos ou interrompida: usa o ambiente herdado.
        let l = &p.launcher;
        if !l.short().is_empty() {
            let mut tip = String::from("Deduzido das variáveis de ambiente herdadas");
            if let Some(sid) = &l.session {
                tip.push_str(&format!(" · sessão {sid}"));
            }
            if let Some(apid) = l.agent_pid.filter(|x| self.proc(*x).is_some()) {
                let an = self.proc(apid).map(|a| a.name.clone()).unwrap_or_default();
                tip.push_str(&format!("\n{an} (PID {apid}) — clique para selecionar"));
                return (format!("{} · {an}", l.agent.clone().unwrap_or_default()), Some(apid), tip, true);
            }
            if !chain.is_empty() {
                tip.push_str(&format!("\ncadeia: {}", chain.iter().rev().cloned().collect::<Vec<_>>().join(" › ")));
            } else if p.raw_ppid != 0 {
                tip.push_str(&format!("\npai (PID {}) já encerrado", p.raw_ppid));
            }
            return (l.short(), None, tip, true);
        }
        if let Some(pp) = self.proc(p.ppid) {
            return (pp.name.clone(), Some(pp.pid), format!("PID {} — clique para selecionar o pai", pp.pid), false);
        }
        if p.raw_ppid != 0 {
            (format!("(pid {} encerrado)", p.raw_ppid), None, String::new(), false)
        } else {
            ("–".into(), None, String::new(), false)
        }
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

    fn sort_pids(&self, pids: &mut Vec<u32>, tree: bool) {
        let key = self.sort;
        let desc = self.sort_desc;
        pids.sort_by(|a, b| {
            let (pa, pb) = match (self.proc(*a), self.proc(*b)) {
                (Some(x), Some(y)) => (x, y),
                _ => return std::cmp::Ordering::Equal,
            };
            let ord = match key {
                SortKey::Name => pa.name_lower.cmp(&pb.name_lower).then(pa.pid.cmp(&pb.pid)),
                SortKey::Ram => {
                    if tree {
                        self.subtree.get(a).unwrap_or(&0).cmp(self.subtree.get(b).unwrap_or(&0))
                    } else {
                        self.mem_of(pa).cmp(&self.mem_of(pb))
                    }
                }
                SortKey::Cat => self.cat(*a).cmp(&self.cat(*b)).then(self.mem_of(pb).cmp(&self.mem_of(pa))),
                SortKey::Pid => pa.pid.cmp(&pb.pid),
                SortKey::Cpu => pa.cpu_pct.partial_cmp(&pb.cpu_pct).unwrap_or(std::cmp::Ordering::Equal),
                SortKey::Gpu => pa.gpu_pct.partial_cmp(&pb.gpu_pct).unwrap_or(std::cmp::Ordering::Equal),
                SortKey::Disk => pa.disk_bps.partial_cmp(&pb.disk_bps).unwrap_or(std::cmp::Ordering::Equal),
                SortKey::Age => pb.create_time.cmp(&pa.create_time),
                SortKey::Parent => {
                    let na = self.proc(pa.ppid).map(|p| p.name_lower.as_str()).unwrap_or("");
                    let nb = self.proc(pb.ppid).map(|p| p.name_lower.as_str()).unwrap_or("");
                    na.cmp(nb).then(self.mem_of(pb).cmp(&self.mem_of(pa)))
                }
            };
            // Desempate por PID depois da inversão. Sem ele, as dezenas de processos
            // empatados em "–" na coluna CPU trocavam de lugar a cada amostra e a lista
            // inteira piscava embaixo do que você estava tentando ler.
            let ord = if desc { ord.reverse() } else { ord };
            ord.then(pa.pid.cmp(&pb.pid))
        });
    }

    /// Linhas de memória que não é de processo, para a soma da lista bater com o topo.
    ///
    /// Só aparecem sem busca e sem filtro de categoria — buscar "chrome" não pode devolver
    /// o pool do kernel. Ordenadas por tamanho, junto do resto.
    fn system_rows(&self, search: &str) -> Vec<Row> {
        if !self.cfg.show_kernel_rows || !search.is_empty() || self.cat_enabled.len() != Category::ALL.len() {
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
        out.into_iter().map(|(kind, bytes)| Row::System { kind, bytes }).collect()
    }

    fn build_rows(&mut self) -> Vec<Row> {
        let search = self.search.trim().to_lowercase();
        let hits: Vec<u32> = self.procs.iter().filter(|p| self.passes(p, &search)).map(|p| p.pid).collect();
        let sys_rows = self.system_rows(&search);
        match self.cfg.view {
            // Térmico, Partida e Telas não desenham tabela de processos — o braço só
            // existe pra exaustividade.
            ViewMode::List | ViewMode::Drains | ViewMode::Thermal | ViewMode::Boot | ViewMode::Screens => {
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
                rows.extend(
                    pids.into_iter()
                        .map(|pid| Row::Proc { pid, depth: 0, has_children: false, expanded: false, dim: false }),
                );
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
                        let total = pids.iter().map(|p| self.proc(*p).map(|x| self.mem_of(x)).unwrap_or(0)).sum();
                        (c, total, pids)
                    })
                    .collect();
                cats.sort_by(|a, b| b.1.cmp(&a.1));
                let mut rows = Vec::new();
                for (cat, total, mut pids) in cats {
                    let collapsed = self.collapsed_cats.contains(&cat);
                    rows.push(Row::CatHeader { cat, count: pids.len(), total, collapsed });
                    if !collapsed {
                        self.sort_pids(&mut pids, false);
                        for pid in pids {
                            rows.push(Row::Proc { pid, depth: 1, has_children: false, expanded: false, dim: false });
                        }
                    }
                }
                rows
            }
            ViewMode::Tree => {
                let filtering = !search.is_empty()
                    || self.cat_enabled.len() != Category::ALL.len()
                    || self.cfg.min_mb > 0;
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
                        && (self.expanded.contains(&pid) || (filtering && auto_expand.contains(&pid)));
                    rows.push(Row::Proc { pid, depth, has_children, expanded, dim: filtering && !hitset.contains(&pid) });
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

    /// Linhas da visão Lista agrupadas por executável.
    ///
    /// App de um processo só não ganha cabeçalho: uma linha "▶ Bloco de Notas (1)" que
    /// abre em uma linha idêntica é ruído. O agrupamento existe para o caso do Chrome,
    /// não para enfeitar o resto da lista.
    fn build_app_rows(&mut self, hits: Vec<u32>) -> Vec<Row> {
        // A chave é o caminho do exe, não o nome: dois `svchost.exe` de pastas diferentes
        // (ou um impostor) não podem cair no mesmo grupo. Sem acesso ao caminho sobra o
        // nome, que é o que a lista tem para mostrar de qualquer jeito.
        let mut by_key: HashMap<String, Vec<u32>> = HashMap::new();
        for pid in hits {
            let Some(p) = self.proc(pid) else { continue };
            let key = if p.exe_path.is_empty() { p.name_lower.clone() } else { p.exe_path.to_lowercase() };
            by_key.entry(key).or_default().push(pid);
        }
        let mut groups: Vec<AppGroup> = Vec::with_capacity(by_key.len());
        for (key, mut pids) in by_key {
            self.sort_pids(&mut pids, false);
            let Some(p) = pids.first().and_then(|pid| self.proc(*pid)) else { continue };
            let mut g = AppGroup {
                key,
                name: p.name.clone(),
                name_lower: p.name_lower.clone(),
                cat: self.cat(pids[0]),
                pids,
                ram: 0,
                cpu: 0.0,
                gpu: 0.0,
                disk: 0.0,
                oldest: 0,
            };
            self.fill_group(&mut g);
            groups.push(g);
        }

        let (sort, desc) = (self.sort, self.sort_desc);
        let mut order: Vec<usize> = (0..groups.len()).collect();
        order.sort_by(|&a, &b| {
            let (ga, gb) = (&groups[a], &groups[b]);
            let ord = match sort {
                SortKey::Name => ga.name_lower.cmp(&gb.name_lower),
                SortKey::Ram => ga.ram.cmp(&gb.ram),
                SortKey::Cpu => ga.cpu.partial_cmp(&gb.cpu).unwrap_or(std::cmp::Ordering::Equal),
                SortKey::Gpu => ga.gpu.partial_cmp(&gb.gpu).unwrap_or(std::cmp::Ordering::Equal),
                SortKey::Disk => ga.disk.partial_cmp(&gb.disk).unwrap_or(std::cmp::Ordering::Equal),
                SortKey::Cat => ga.cat.cmp(&gb.cat).then(gb.ram.cmp(&ga.ram)),
                SortKey::Pid => ga.pids.iter().min().cmp(&gb.pids.iter().min()),
                SortKey::Age => gb.oldest.cmp(&ga.oldest),
                // "Origem" é uma relação entre processos; no nível do app ela não
                // significa nada, então cai no nome em vez de inventar uma ordem.
                SortKey::Parent => ga.name_lower.cmp(&gb.name_lower),
            };
            let ord = if desc { ord.reverse() } else { ord };
            ord.then(ga.key.cmp(&gb.key))
        });

        self.groups = groups;
        let mut rows: Vec<Row> = Vec::with_capacity(self.groups.len() + 8);
        for gi in order {
            let g = &self.groups[gi];
            if g.pids.len() < 2 {
                rows.push(Row::Proc { pid: g.pids[0], depth: 0, has_children: false, expanded: false, dim: false });
                continue;
            }
            rows.push(Row::AppHeader { gi });
            if !self.collapsed_apps.contains(&g.key) {
                for &pid in &g.pids {
                    rows.push(Row::Proc { pid, depth: 1, has_children: false, expanded: false, dim: false });
                }
            }
        }
        rows
    }

    /// Refaz as somas de um grupo a partir do estado atual dos processos.
    fn fill_group(&self, g: &mut AppGroup) {
        let (mut ram, mut cpu, mut gpu, mut disk, mut oldest) = (0u64, 0.0f32, 0.0f32, 0.0f64, 0i64);
        for &pid in &g.pids {
            let Some(p) = self.proc(pid) else { continue };
            ram += self.mem_of(p);
            cpu += p.cpu_pct;
            gpu += p.gpu_pct;
            disk += p.disk_bps;
            if oldest == 0 || p.create_time < oldest {
                oldest = p.create_time;
            }
        }
        g.ram = ram;
        g.cpu = cpu;
        g.gpu = gpu;
        g.disk = disk;
        g.oldest = oldest;
    }

    /// Atualiza as somas dos grupos sem mexer na ordem — é o que roda enquanto a tabela
    /// está congelada porque o mouse está em cima dela. Congelar a ordem é proposital;
    /// congelar os números junto não seria.
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
        let mut ca: Vec<&String> = self.collapsed_apps.iter().collect();
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
        let can_freeze = hovering && !self.rows_dirty && key == self.cached_key;
        if can_freeze {
            if self.cached_rows.is_some() {
                // mantém a ordem; remove só o que morreu ou deixou de passar no filtro
                let search = self.search.trim().to_lowercase();
                let mut keep: HashSet<u32> = self.procs.iter().filter(|p| self.passes(p, &search)).map(|p| p.pid).collect();
                if self.cfg.view == ViewMode::Tree {
                    let hits: Vec<u32> = keep.iter().copied().collect();
                    for h in hits {
                        for a in self.ancestry(h) {
                            keep.insert(a);
                        }
                    }
                }
                // Os grupos são refeitos aqui: a ordem fica congelada, os números não.
                self.refresh_groups();
                let mut rows = self.cached_rows.take().unwrap();
                rows.retain(|r| match r {
                    Row::Proc { pid, .. } => keep.contains(pid),
                    // Grupo que ficou com um processo só perde o cabeçalho — a linha do
                    // processo que sobrou continua ali.
                    Row::AppHeader { gi } => self.groups.get(*gi).is_some_and(|g| g.pids.len() > 1),
                    Row::CatHeader { .. } | Row::System { .. } => true,
                });
                self.order_frozen = true;
                self.cached_rows = Some(rows.clone());
                return rows;
            }
        }
        let rows = self.build_rows();
        self.cached_rows = Some(rows.clone());
        self.cached_key = key;
        self.rows_dirty = false;
        self.order_frozen = false;
        rows
    }

    // ---------- ações ----------

    /// Encerra um processo (e, com `tree`, os descendentes dele) na hora.
    ///
    /// Não há caixa de confirmação: o RamDog mostra quanto cada linha come antes do clique,
    /// e um diálogo que se responde no automático não protege ninguém — só atrasa. O lock
    /// é a proteção de verdade, e ele é verificado aqui.
    fn request_kill(&mut self, pid: u32, tree: bool) {
        let Some(p) = self.proc(pid).cloned() else { return };
        let mut pids = vec![];
        let mut skipped_locked = 0;
        if self.is_locked(&p) {
            self.toast(format!("{} está protegido (lock)", p.name), true);
            return;
        }
        pids.push((p.pid, p.name.clone(), self.mem_of(&p)));
        if tree {
            for d in self.descendants(pid) {
                if let Some(c) = self.proc(d) {
                    if self.is_locked(c) {
                        skipped_locked += 1;
                    } else {
                        pids.push((c.pid, c.name.clone(), self.mem_of(c)));
                    }
                }
            }
        }
        self.execute_kill(pids, skipped_locked);
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
                Some(p) => pids.push((p.pid, p.name.clone(), self.mem_of(p))),
                None => {}
            }
        }
        if pids.is_empty() {
            self.toast(format!("{name}: todos os {count} processos estão protegidos (lock)"), true);
            return;
        }
        self.execute_kill(pids, skipped);
    }

    /// Mata a lista e resume o estrago na barra de status. `skipped_locked` são os que o
    /// lock poupou pelo caminho — dizer só "3 finalizados" quando eram 5 esconde o motivo.
    fn execute_kill(&mut self, pids: Vec<(u32, String, u64)>, skipped_locked: usize) {
        let mut ok = 0;
        let mut freed = 0u64;
        let mut errs: Vec<String> = Vec::new();
        for (pid, name, ram) in &pids {
            match procs::kill(*pid) {
                Ok(()) => {
                    ok += 1;
                    freed += ram;
                }
                Err(e) => errs.push(format!("{name} ({pid}): {e}")),
            }
        }
        self.sampler.force.store(true, Ordering::Relaxed);
        let poupados = if skipped_locked > 0 {
            format!(", {skipped_locked} protegido(s) poupado(s)")
        } else {
            String::new()
        };
        if errs.is_empty() {
            self.toast(
                format!("{ok} processo(s) finalizado(s), ~{} liberados{poupados}", fmt_bytes(freed)),
                false,
            );
        } else {
            let mut msg = format!("{ok} ok, {} falha(s): {}", errs.len(), errs[0]);
            if errs.len() > 1 {
                msg.push_str(&format!(" (+{})", errs.len() - 1));
            }
            msg.push_str(&poupados);
            self.toast(msg, true);
        }
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
        self.rows_dirty = true;
    }

    fn toast(&mut self, msg: String, err: bool) {
        self.status = Some((Instant::now(), msg, err));
    }

    fn relaunch_as_admin(&mut self) {
        #[cfg(windows)]
        {
            use windows::core::{w, PCWSTR};
            use windows::Win32::UI::Shell::ShellExecuteW;
            use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
            let exe = std::env::current_exe().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
            let wide: Vec<u16> = exe.encode_utf16().chain(std::iter::once(0)).collect();
            let r = unsafe { ShellExecuteW(None, w!("runas"), PCWSTR(wide.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
            if r.0 as usize > 32 {
                std::process::exit(0);
            } else {
                self.toast("Elevação cancelada ou falhou".into(), true);
            }
        }
        #[cfg(target_os = "macos")]
        {
            self.toast("No macOS, rode o RamDog com sudo se precisar matar processos de outros usuários.".into(), true);
        }
        #[cfg(target_os = "linux")]
        {
            self.toast("No Linux, rode o RamDog com sudo se precisar matar processos de outros usuários.".into(), true);
        }
    }

    // ---------- UI ----------

    /// Totais por categoria na métrica escolhida — alimenta os chips de filtro, que precisam
    /// bater com o que a coluna RAM mostra em cada linha.
    fn cat_totals(&self) -> HashMap<Category, (u64, usize)> {
        self.cat_totals_with(self.cfg.mem_metric)
    }

    /// Totais por categoria numa métrica específica. O medidor do topo pede sempre
    /// `Private`, porque lá as faixas precisam caber dentro do "em uso" — com working set a
    /// soma das categorias passa da largura da barra.
    fn cat_totals_with(&self, m: MemMetric) -> HashMap<Category, (u64, usize)> {
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
        let b = self.breakdown();
        let total = self.mem.total_phys.max(1);
        let cats = self.cat_totals_with(MemMetric::Private);
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, TOP_BAR_H), egui::Sense::hover());
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
            let seg = Rect::from_min_max(egui::pos2(*x, rect.top() + 1.0), egui::pos2(right, rect.bottom() - 1.0));
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
        let mut tip = format!(
            "{} em uso de {}\ncompromisso {} / {} (RAM + arquivo de paginação)\n",
            fmt_gb(b.used),
            fmt_gb(total),
            fmt_gb(self.mem.used_commit()),
            fmt_gb(self.mem.total_commit)
        );
        for (c, t) in &segs {
            tip.push_str(&format!("\n{}  {}", c.label(), fmt_bytes_short(*t)));
        }
        tip.push_str(&format!("\n\nProcessos (privado)  {}", fmt_bytes_short(b.private)));
        if b.kernel_ok {
            tip.push_str(&format!("\n{}  {}", SysRow::PagedPool.label(), fmt_bytes_short(b.paged_pool)));
            tip.push_str(&format!("\n{}  {}", SysRow::NonPagedPool.label(), fmt_bytes_short(b.nonpaged_pool)));
        }
        tip.push_str(&format!("\n{}  {}", SysRow::SharedAndCache.label(), fmt_bytes_short(b.shared_and_cache)));
        if !self.hwtemp.dimm_temps.is_empty() {
            tip.push_str("\n\nTemperatura por pente:");
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

    fn meter_bar(ui: &mut egui::Ui, width: f32, pct: Option<f32>, tip: impl Into<egui::WidgetText>) -> egui::Response {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, TOP_BAR_H), egui::Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, 2.0, Color32::from_rgb(30, 34, 41));
        if let Some(pct) = pct {
            let frac = (pct / 100.0).clamp(0.0, 1.0);
            if frac > 0.01 {
                let bar = Rect::from_min_size(rect.min, Vec2::new(rect.width() * frac, rect.height()));
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
        let frame = egui::Frame::new()
            .fill(BG)
            .stroke(Stroke::new(1.0_f32, LINE))
            .inner_margin(egui::Margin::symmetric(6, 5));
        egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
            // Sem decoração não há barra de título: arrastar qualquer parte vazia move a
            // janela. A interação vem antes dos widgets para os botões ganharem o clique.
            let bg = ui.interact(ui.max_rect(), ui.id().with("mini_drag"), egui::Sense::click_and_drag());
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
            ui.horizontal(|ui| {
                Self::meter_tile(ui, w, "CPU", cpu_pct, self.cpu_temp(), "", |ui, w| {
                    Self::meter_bar(ui, w, cpu_pct, "Uso de CPU");
                });
                let used = self.mem.used_phys();
                let total = self.mem.total_phys.max(1);
                let sub = format!("{} / {}", fmt_gb(used), fmt_gb(total));
                Self::meter_tile(ui, w, "RAM", Some(used as f32 / total as f32 * 100.0), self.ram_temp(), &sub, |ui, w| {
                    self.ram_gauge(ui, w)
                });
            });
            ui.horizontal(|ui| {
                let gpu = self.sys.gpu.clone();
                let (gpu_pct, gpu_temp, gpu_sub, gpu_tip) = match &gpu {
                    Some(g) => (
                        g.util_pct,
                        match g.temp_c {
                            Some(t) => Temp::C(t),
                            None => Temp::Missing("O driver não reportou temperatura desta GPU.".into()),
                        },
                        if g.mem_total > 0 { format!("{} / {}", fmt_gb(g.mem_used), fmt_gb(g.mem_total)) } else { String::new() },
                        g.name.clone(),
                    ),
                    None => (
                        None,
                        Temp::Missing("Sem leitura de GPU: nvml.dll não carregou.".into()),
                        String::new(),
                        "Sem leitura de GPU neste host".to_string(),
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
                Self::meter_tile(ui, w, "DISCO", disk_pct, Temp::None, &disk_sub, |ui, w| {
                    Self::meter_bar(ui, w, disk_pct, "Tempo ocupado do disco");
                });
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
                Temp::Missing("Temperatura de CPU no macOS ainda não está ligada.".into())
            }
            None if cfg!(target_os = "linux") => {
                Temp::Missing("Sem sensor de CPU em /sys/class/hwmon (coretemp, k10temp ou zenpower).".into())
            }
            None if !self.is_admin => {
                Temp::Missing("Temperatura de CPU precisa de admin: o sensor Tctl só responde por driver de hardware. Volte ao modo completo e use ⬆ Admin.".into())
            }
            None => Temp::Missing(
                "Temperatura indisponível: hwtemp.exe não está ao lado do ramdog.exe, ou a placa-mãe não tem sensor suportado.".into(),
            ),
        }
    }

    fn ram_temp(&self) -> Temp {
        match self.hwtemp.ram_max() {
            Some(t) => Temp::C(t.round() as u32),
            None if cfg!(target_os = "linux") => {
                Temp::Missing("Nenhum pente expõe sensor no hwmon (spd5118/jc42). Sem isso o RamDog não inventa °C.".into())
            }
            None if cfg!(target_os = "macos") => {
                Temp::Missing("Temperatura de RAM no macOS ainda não está ligada.".into())
            }
            None if !self.is_admin => Temp::Missing("Temperatura dos pentes precisa de admin (leitura SMBus).".into()),
            None => Temp::Missing("Nenhum pente desta máquina expõe sensor de temperatura.".into()),
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
            ("ESTABILIZAR".to_owned(), SURFACE, MUTED)
        } else if !stab_on {
            ("ESTABILIZAR".to_owned(), ACCENT_BG, ACCENT)
        } else if held > 50.5 {
            (format!("FANS {held:.0}%"), THERM_WARN_BG, THERM_WARN_FG)
        } else {
            ("FANS 50%".to_owned(), THERM_STAB_BG, THERM_STAB_FG)
        };
        let tip = if !has_fans {
            "Controle de fans indisponível: precisa de admin e do hwtemp.exe ao lado do ramdog.exe."
        } else if stab_on {
            "Curva do TempHUD ligada. Clique para devolver os fans à BIOS."
        } else {
            "Trava os fans SuperIO em 50% e sobe em rampa a partir de 80°C (100% aos 92°C). Clicar de novo devolve à BIOS."
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
            let spinning: Vec<&crate::hwtemp::FanRow> =
                self.hwtemp.fans.iter().filter(|f| f.rpm.unwrap_or(0.0) > 0.0).collect();
            if spinning.is_empty() {
                if has_fans {
                    ui.label(RichText::new("fans parados").color(MUTED).size(10.5));
                }
                return;
            }
            let shown: Vec<String> = spinning.iter().take(4).map(|f| format!("{:.0}", f.rpm.unwrap_or(0.0))).collect();
            let mut detail = String::new();
            for f in &spinning {
                let pct = f.pct.map(|p| format!("{p:.0}%")).unwrap_or_else(|| "–".into());
                let mode = if f.guard {
                    " (proteção)"
                } else if f.auto {
                    " (BIOS)"
                } else {
                    ""
                };
                detail.push_str(&format!("{}  {}  {:.0} rpm{}\n", f.name, pct, f.rpm.unwrap_or(0.0), mode));
            }
            ui.label(RichText::new(format!("{} rpm", shown.join(" · "))).color(MUTED).size(10.5))
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
                .on_hover_text("Arraste para mover");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("✕").on_hover_text("Fechar").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if ui.small_button("⤢").on_hover_text("Voltar ao completo (ou duplo clique)").clicked() {
                    self.set_mini(false);
                }
                // Sem decoração não há botão de minimizar do Windows — o HUD precisa do
                // seu. Volta pela barra de tarefas, como qualquer janela.
                if ui.small_button("–").on_hover_text("Minimizar").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }
                let mut on_top = self.cfg.mini_on_top;
                if ui.selectable_label(on_top, RichText::new("topo").size(11.0)).on_hover_text("Manter por cima").clicked() {
                    on_top = !on_top;
                    self.cfg.mini_on_top = on_top;
                    self.cfg_dirty = true;
                    self.apply_on_top(ui.ctx());
                }
                let mut paused = self.sampler.paused.load(Ordering::Relaxed);
                let (icon, tip) = if paused { ("▶", "Retomar") } else { ("⏸", "Pausar") };
                if ui.selectable_label(paused, RichText::new(icon).size(11.0)).on_hover_text(tip).clicked() {
                    paused = !paused;
                    self.sampler.paused.store(paused, Ordering::Relaxed);
                }
                let iv = self.cfg.refresh_ms;
                if ui
                    .small_button(format!("{:.1}s", iv as f32 / 1000.0))
                    .on_hover_text("Ritmo — clique para alternar")
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
                        ui.label(RichText::new(format!("{t}°C")).monospace().size(11.0).strong().color(Self::temp_color(t)));
                    }
                    Temp::Missing(why) => {
                        ui.label(RichText::new("–°C").monospace().size(11.0).color(Color32::from_gray(90)))
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
                        ui.label(RichText::new("–").monospace().size(19.0).color(Color32::from_gray(90)));
                    }
                }
                if !sub.is_empty() {
                    ui.label(RichText::new(sub).color(MUTED).size(10.5));
                }
            });
            bar(ui, w);
        });
    }

    /// Linha 1 do topo: os quatro medidores, blocos idênticos, e os controles à direita.
    ///
    /// Antes cada medidor tinha largura e barra próprias (148/220/148/150 de coluna, barras
    /// de 90 e 200) e só a RAM tinha uma terceira linha: nenhuma borda batia com a de baixo
    /// e os controles caíam numa segunda fileira solta. Agora os quatro são o mesmo bloco
    /// do modo mini — mesma largura, mesma barra, mesma baseline — e os controles ficam na
    /// mesma fileira, centrados contra ela.
    fn ui_top(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        // Os medidores tomam toda a largura que sobra depois dos controles, em partes iguais.
        // Com largura fixa, janela larga virava uma faixa morta entre o disco e os controles;
        // e quando os controles não cabiam eles desciam para uma fileira quase vazia.
        let ctrl_w = self.top_controls_w(ui);
        let free = ui.available_width();
        let share = (free - ctrl_w - 4.0 * TILE_GAP) / 4.0;
        let wrapped = share < TILE_MIN;
        // Na fileira própria os medidores esticam para ocupar a largura inteira: o buraco
        // que sobrava à direita deles era exatamente o que a quebra ia evitar.
        let tile_w = if wrapped {
            ((free - 3.0 * TILE_GAP) / 4.0).clamp(TILE_MIN, TILE_MAX)
        } else {
            share.min(TILE_MAX)
        };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = TILE_GAP;

            let cpu_pct = self.sys.cpu_pct;
            Self::meter_tile(ui, tile_w, "CPU", cpu_pct, self.cpu_temp(), "", |ui, w| {
                Self::meter_bar(ui, w, cpu_pct, "Uso de CPU (todos os núcleos)");
            });

            let used = self.mem.used_phys();
            let total = self.mem.total_phys.max(1);
            let ram_sub = format!("{} / {}", fmt_gb(used), fmt_gb(total));
            Self::meter_tile(
                ui,
                tile_w,
                "RAM",
                Some(used as f32 / total as f32 * 100.0),
                self.ram_temp(),
                &ram_sub,
                |ui, w| self.ram_gauge(ui, w),
            );

            let gpu = self.sys.gpu.clone();
            let (gpu_pct, gpu_temp, gpu_sub, gpu_tip) = match &gpu {
                Some(g) => {
                    let mut tip = g.name.clone();
                    if let Some(w) = g.power_w {
                        tip.push_str(&format!("\nPotência: {w:.0} W"));
                    }
                    if let Some(f) = g.fan_pct {
                        tip.push_str(&format!("\nCooler: {f}%"));
                    }
                    let vram = if g.mem_total > 0 {
                        format!("{} / {}", fmt_gb(g.mem_used), fmt_gb(g.mem_total))
                    } else {
                        String::new()
                    };
                    let t = match g.temp_c {
                        Some(t) => Temp::C(t),
                        None => Temp::Missing("O driver não reportou temperatura desta GPU.".into()),
                    };
                    (g.util_pct, t, vram, tip)
                }
                None => (
                    None,
                    Temp::Missing("Sem leitura de GPU: nvml.dll não carregou (placas AMD/Intel ainda não têm essa leitura aqui).".into()),
                    String::new(),
                    "Sem GPU NVIDIA detectada (nvml.dll não carregou) — sem essa leitura em placas AMD/Intel aqui ainda.".to_string(),
                ),
            };
            Self::meter_tile(ui, tile_w, "GPU", gpu_pct, gpu_temp, &gpu_sub, |ui, w| {
                Self::meter_bar(ui, w, gpu_pct, gpu_tip);
            });

            // Disco: o % sozinho fica ilegível num NVMe rápido (quase sempre <1%), por isso
            // a taxa vem junto do número.
            let disk_pct = self.sys.disk_pct;
            let disk_sub = self
                .sys
                .disk_bps
                .filter(|bps| *bps >= 1024.0)
                .map(fmt_bps)
                .unwrap_or_default();
            let disk_tip = if disk_pct.is_some() {
                "% de tempo ocupado do disco (todos os volumes) — igual ao Gerenciador de Tarefas"
            } else {
                "Contador de disco indisponível neste host."
            };
            Self::meter_tile(ui, tile_w, "DISCO", disk_pct, Temp::None, &disk_sub, |ui, w| {
                Self::meter_bar(ui, w, disk_pct, disk_tip);
            });

            if !wrapped {
                let space = ui.available_width();
                ui.allocate_ui_with_layout(Vec2::new(space, TILE_H), Layout::right_to_left(Align::Center), |ui| {
                    self.top_controls(ui);
                });
            }
        });
        // Janela estreita: os controles descem para uma fileira própria em vez de invadir
        // os medidores.
        if wrapped {
            ui.add_space(4.0);
            ui.allocate_ui_with_layout(Vec2::new(ui.available_width(), CTRL_H), Layout::right_to_left(Align::Center), |ui| {
                self.top_controls(ui);
            });
        }
        self.ui_filters(ui);
    }

    /// Largura mínima do bloco de controles do topo, medida de verdade.
    ///
    /// Os rótulos dos addons mudam de largura com a fonte e com a escala do Windows, e um
    /// número chutado aqui é exatamente o que faz o bloco transbordar por cima do medidor
    /// de disco (o layout `right_to_left` não clipa).
    fn top_controls_w(&self, ui: &egui::Ui) -> f32 {
        let font = egui::FontId::proportional(12.5);
        let text_w = |s: String| {
            ui.fonts(|f| f.layout_no_wrap(s, font.clone(), Color32::WHITE).size().x)
        };
        // Botão = texto + os 9 px de `button_padding` de cada lado + o espaço até o vizinho.
        let btn = |s: String| text_w(s) + 18.0 + 6.0;
        let addons: f32 = ViewMode::ADDONS
            .iter()
            .map(|v| btn(format!("{} {}", v.icon(), v.label())))
            .sum();
        let admin = if self.is_admin {
            text_w("ADMIN".into()) + 6.0
        } else {
            btn("⬆ Admin".into())
        };
        // 12 por divisor (6 dele + 6 até o vizinho).
        addons + admin + btn("◱ Mini".into()) + 2.0 * 12.0
    }

    /// Controles do topo, desenhados da direita para a esquerda: o Mini fica na quina, que
    /// é onde se procura um controle de janela, e os addons ficam na ponta esquerda do
    /// bloco, que é a que sobra grudada nos medidores e é lida primeiro.
    fn top_controls(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().interact_size.y = CTRL_H;
        ui.spacing_mut().button_padding = Vec2::new(9.0, 2.0);
        ui.spacing_mut().item_spacing.x = 6.0;

        let mini = egui::Button::new(RichText::new("◱ Mini").size(12.5))
            .fill(ACCENT_BG)
            .stroke(Stroke::new(1.0_f32, ACCENT.gamma_multiply(0.8)))
            .corner_radius(4.0);
        if ui
            .add(mini)
            .on_hover_text("Modo mini: uma janelinha só com CPU, RAM, GPU, disco e temperaturas, por cima das outras janelas")
            .clicked()
        {
            self.set_mini(true);
        }
        ui.separator();
        if self.is_admin {
            ui.label(RichText::new("ADMIN").color(Color32::from_rgb(90, 220, 130)).strong().size(12.0))
                .on_hover_text("Rodando elevado: pode encerrar processos de outros usuários/serviços");
        } else if cfg!(windows)
            && ui
                .button(RichText::new("⬆ Admin").size(12.5))
                .on_hover_text("Reabrir como administrador — necessário para encerrar serviços, processos de outros usuários e ler a temperatura da CPU")
                .clicked()
        {
            self.relaunch_as_admin();
        }
        ui.separator();
        self.ui_addon_buttons(ui);
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
            .selectable_label(paused, RichText::new(if paused { "▶ retomar" } else { "⏸ pausar" }).small())
            .on_hover_text("Congela a amostragem — os números param no último valor lido")
            .clicked()
        {
            paused = !paused;
            self.sampler.paused.store(paused, Ordering::Relaxed);
        }
        let mut iv = self.cfg.refresh_ms;
        egui::ComboBox::from_id_salt("refresh")
            .selected_text(RichText::new(format!("a cada {:.1}s", iv as f32 / 1000.0)).small())
            .width(74.0)
            .show_ui(ui, |ui| {
                for v in [500u64, 1000, 2000, 5000] {
                    ui.selectable_value(&mut iv, v, format!("a cada {:.1}s", v as f32 / 1000.0));
                }
            });
        if iv != self.cfg.refresh_ms {
            self.cfg.refresh_ms = iv;
            self.sampler.interval_ms.store(iv, Ordering::Relaxed);
            self.cfg_dirty = true;
        }
    }

    /// Os quatro addons, com nome escrito, no bloco de controles do topo.
    ///
    /// Partida, Desperdício, Térmico e Telas não são "mais uma visão da lista de
    /// processos": cada um tem vocabulário próprio e ignora a busca e os filtros. Por isso
    /// não são abas ao lado de Lista/Árvore/Categorias — ficam aqui em cima, longe delas,
    /// e clicar troca o conteúdo da janela inteira. Clicar no que já está aberto volta para
    /// a última visão de processo.
    ///
    /// O nome vem escrito, não só o ícone: quatro glifos que ninguém conhece obrigam a
    /// passar o mouse em cada um para descobrir o que fazem.
    fn ui_addon_buttons(&mut self, ui: &mut egui::Ui) {
        let mut go: Option<ViewMode> = None;
        // Botão em repouso invisível: o fundo só aparece no hover e no addon aberto. Sem
        // isto cada um ganha o retângulo cinza padrão e o grupo vira uma fila de caixas.
        ui.visuals_mut().widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
        // `right_to_left`: o primeiro desenhado fica mais à direita. Invertido aqui para
        // que a leitura na tela seja Partida → Desperdício → Térmico → Telas.
        for v in ViewMode::ADDONS.iter().rev() {
            let v = *v;
            let on = self.cfg.view == v;
            let fg = if on { Color32::WHITE } else { MUTED };
            let mut b = egui::Button::new(
                RichText::new(format!("{} {}", v.icon(), v.label())).size(12.5).color(fg),
            )
            .stroke(Stroke::NONE)
            .corner_radius(4.0);
            if on {
                b = b.fill(ACCENT_BG).stroke(Stroke::new(1.0_f32, ACCENT.gamma_multiply(0.7)));
            }
            let tip = if on {
                format!("{}\n\n{}\n\nClique para voltar a {}.", v.label(), v.tip(), self.last_core.label())
            } else {
                format!("{}\n\n{}", v.label(), v.tip())
            };
            if ui.add(b).on_hover_text(tip).clicked() {
                go = Some(v);
            }
        }
        let Some(v) = go else { return };
        if self.cfg.view == v {
            self.cfg.view = self.last_core;
        } else {
            if !self.cfg.view.is_addon() {
                self.last_core = self.cfg.view;
            }
            self.cfg.view = v;
        }
        self.cfg_dirty = true;
    }

    /// Conteúdo de um addon e o que ele devolve (avisos, pedidos de matar, gravar config).
    fn ui_addon_body(&mut self, ui: &mut egui::Ui, v: ViewMode) {
        match v {
            ViewMode::Drains => {
                let is_admin = self.is_admin;
                let procs = std::mem::take(&mut self.procs);
                let evs = self.drains.ui(ui, &procs, is_admin);
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
                // que tem tudo que sobe com o PC.
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.search)
                            .hint_text("Buscar nome, comando ou origem…")
                            .desired_width(260.0),
                    );
                    if !self.search.is_empty() && ui.button("✖").clicked() {
                        self.search.clear();
                    }
                });
                ui.add_space(4.0);
                let is_admin = self.is_admin;
                let search = self.search.clone();
                let procs = std::mem::take(&mut self.procs);
                let evs = self.boot.ui(ui, &procs, &search, is_admin, &mut self.cfg, &self.usage);
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
            ViewMode::Thermal => self.ui_thermal(ui),
            _ => {}
        }
    }

    /// Finaliza uma lista de PIDs vinda de um addon.
    fn request_kill_many(&mut self, pids: &[u32]) {
        let list: Vec<(u32, String, u64)> = pids
            .iter()
            .filter_map(|pid| self.proc(*pid).map(|p| (p.pid, p.name.clone(), self.mem_of(p))))
            .collect();
        if list.is_empty() {
            return;
        }
        self.execute_kill(list, 0);
    }

    /// Largura do par de controles da coluna RAM, para decidir se ele cabe na fileira.
    fn mem_controls_w(&self, ui: &egui::Ui) -> f32 {
        let font = egui::FontId::proportional(12.0);
        let text_w = |s: &str| {
            ui.fonts(|f| f.layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE).size().x)
        };
        // 104 = largura fixa do combo; 74 = o DragValue com "4096 MB"; 10 = o vão do meio.
        text_w("coluna RAM mostra") + 104.0 + 10.0 + text_w("ocultar abaixo de") + 74.0 + 4.0 * 6.0
    }

    /// O que a coluna RAM mostra e a partir de quanto a linha aparece.
    ///
    /// Desenhado da direita para a esquerda, encostado na quina: soltos no meio da fileira,
    /// "RAM:" e "mín." pareciam mais dois medidores que caíram ali por engano. Cada um vem
    /// apresentado pelo que faz, não pela unidade que usa.
    fn ui_mem_controls(&mut self, ui: &mut egui::Ui) {
        let mut min_mb = self.cfg.min_mb;
        if ui
            .add(egui::DragValue::new(&mut min_mb).range(0..=4096).speed(5).suffix(" MB"))
            .on_hover_text("Arraste ou digite. 0 mostra tudo.")
            .changed()
        {
            self.cfg.min_mb = min_mb;
            self.cfg_dirty = true;
        }
        ui.label(RichText::new("ocultar abaixo de").color(MUTED).size(12.0))
            .on_hover_text("Esconde os processos menores que isto, na medida escolhida ao lado");
        ui.add_space(10.0);
        // O default é working set: o privado (padrão do Gerenciador de Tarefas) esconde tudo
        // que é compartilhado e faz a lista somar menos de um terço do "em uso" do topo.
        let mut metric = self.cfg.mem_metric;
        egui::ComboBox::from_id_salt("mem_metric")
            .selected_text(metric.label())
            .width(104.0)
            .show_ui(ui, |ui| {
                for m in MemMetric::ALL {
                    ui.selectable_value(&mut metric, m, m.label()).on_hover_text(m.tip());
                }
            });
        if metric != self.cfg.mem_metric {
            self.cfg.mem_metric = metric;
            self.cfg_dirty = true;
            self.rows_dirty = true;
        }
        ui.label(RichText::new("coluna RAM mostra").color(MUTED).size(12.0))
            .on_hover_text("Qual das três medidas de memória vai na coluna RAM da tabela");
    }

    /// Linha 2 do topo: busca, seletor de visão, chips de categoria e métrica de RAM.
    fn ui_filters(&mut self, ui: &mut egui::Ui) {
        // Busca, abas e chips de categoria filtram a tabela de processos. Num addon não há
        // tabela: deixar a fileira ali seria controle que não controla nada, roubando as
        // duas fileiras de altura que o addon usa para mostrar o conteúdo dele.
        if self.cfg.view.is_addon() {
            return;
        }
        let totals = self.cat_totals();
        let mut mem_wrapped = false;
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            // Altura única para tudo nesta fileira. Sem isto a busca (TextEdit), as abas
            // (Frame + botões), o combo e o DragValue usam cada um a altura natural do
            // egui e as bordas ficam em quatro linhas diferentes.
            ui.spacing_mut().interact_size.y = CTRL_H;
            ui.spacing_mut().button_padding = Vec2::new(9.0, 2.0);
            let te = egui::TextEdit::singleline(&mut self.search)
                .hint_text("Buscar nome, PID, caminho ou comando…")
                .desired_width(240.0);
            let resp = ui.add(te);
            if resp.changed() {
                self.scroll_to_selected = false;
            }
            if !self.search.is_empty() && ui.button("✖").on_hover_text("Limpar busca").clicked() {
                self.search.clear();
            }
            ui.add_space(4.0);
            // Controle segmentado: as quatro visões lidas como um grupo, não como texto solto.
            let mut view = self.cfg.view;
            egui::Frame::new()
                .fill(Color32::from_rgb(24, 27, 33))
                .stroke(Stroke::new(1.0_f32, LINE))
                .corner_radius(6.0)
                .inner_margin(egui::Margin::same(2))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    // 20 + 2 de margem em cima e embaixo = CTRL_H: o grupo de abas fecha
                    // exatamente na mesma altura da busca e do combo ao lado.
                    ui.spacing_mut().interact_size.y = CTRL_H - 4.0;
                    // Só as visões de processo. Partida, Desperdício, Térmico e Telas têm
                    // assunto próprio e vivem nos botões do bloco de cima — como abas elas
                    // só roubavam largura da busca e dos filtros que nem valem para elas.
                    for v in ViewMode::CORE {
                        let on = view == v;
                        let t = RichText::new(v.label())
                            .size(12.5)
                            .color(if on { Color32::WHITE } else { MUTED });
                        let b = if on {
                            egui::Button::new(t).fill(ACCENT_BG).stroke(Stroke::new(1.0_f32, ACCENT.gamma_multiply(0.7)))
                        } else {
                            egui::Button::new(t).stroke(Stroke::NONE)
                        }
                        .corner_radius(4.0);
                        if ui.add(b).on_hover_text(v.tip()).clicked() {
                            view = v;
                        }
                    }
                });
            if view != self.cfg.view {
                self.cfg.view = view;
                self.cfg_dirty = true;
            }
            if self.cfg.view == ViewMode::Tree {
                if ui.button("Expandir tudo").clicked() {
                    self.expanded = self.children.keys().copied().collect();
                }
                if ui.button("Recolher").clicked() {
                    self.expanded.clear();
                }
            }
            if self.cfg.view == ViewMode::List {
                let mut on = self.cfg.group_apps;
                if ui
                    .checkbox(&mut on, RichText::new("Agrupar por app").size(12.5))
                    .on_hover_text(
                        "Junta os processos do mesmo executável numa linha que abre.\n\n\
                         Desligado, o Chrome com 30 renderizadores vira 30 linhas de 3% e \
                         nunca aparece no topo da ordenação por CPU, mesmo sendo o maior \
                         consumidor da máquina.",
                    )
                    .changed()
                {
                    self.cfg.group_apps = on;
                    self.cfg_dirty = true;
                    self.rows_dirty = true;
                }
                if on && !self.collapsed_apps.is_empty() && ui.button("Expandir tudo").clicked() {
                    self.collapsed_apps.clear();
                    self.rows_dirty = true;
                }
            }
            // Igual ao bloco de cima: `right_to_left` não clipa, e o que não couber vaza por
            // cima do "Agrupar por app" em vez de sumir. Só desenha aqui se couber mesmo.
            if ui.available_width() >= self.mem_controls_w(ui) {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| self.ui_mem_controls(ui));
            } else {
                mem_wrapped = true;
            }
        });
        if mem_wrapped {
            ui.add_space(4.0);
            ui.allocate_ui_with_layout(
                Vec2::new(ui.available_width(), CTRL_H),
                Layout::right_to_left(Align::Center),
                |ui| {
                    ui.spacing_mut().interact_size.y = CTRL_H;
                    ui.spacing_mut().button_padding = Vec2::new(9.0, 2.0);
                    self.ui_mem_controls(ui);
                },
            );
        }
        // Chips em linha própria: antes eles vazavam para uma terceira linha e sobrava
        // "Sistema / Outros" órfãos embaixo.
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().interact_size.y = CTRL_H - 2.0;
            ui.spacing_mut().button_padding = Vec2::new(10.0, 2.0);
            let mut toggled: Option<Category> = None;
            let mut solo: Option<Category> = None;
            for c in Category::ALL {
                let (t, n) = totals.get(&c).copied().unwrap_or((0, 0));
                let on = self.cat_enabled.contains(&c);
                let col = c.color();
                let text = RichText::new(format!("● {}  {}", c.label(), fmt_bytes_short(t)))
                    .color(if on { col } else { MUTED })
                    .size(12.0);
                let btn = egui::Button::new(text)
                    .fill(if on { col.gamma_multiply(0.14) } else { Color32::TRANSPARENT })
                    .stroke(egui::Stroke::new(1.0_f32, if on { col.gamma_multiply(0.55) } else { LINE }))
                    .corner_radius(10.0);
                let r = ui.add(btn).on_hover_text(format!("{n} processos — clique: alterna; duplo clique: só esta"));
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
            // Chip das linhas que não são processo. Fica no fim da mesma fileira porque é o
            // mesmo gesto dos outros — mas é um interruptor de exibição, não um filtro de
            // categoria: a memória do kernel continua no medidor e na conferência do rodapé
            // mesmo com as linhas ocultas, já que ela não deixa de existir por estar oculta.
            let b = self.breakdown();
            if b.kernel_ok {
                let on = self.cfg.show_kernel_rows;
                let col = SysRow::PagedPool.color();
                let sys_total = b.paged_pool + b.nonpaged_pool + b.shared_and_cache;
                let text = RichText::new(format!("▣ Sistema (não-processo)  {}", fmt_bytes_short(sys_total)))
                    .color(if on { col } else { MUTED })
                    .size(12.0);
                let btn = egui::Button::new(text)
                    .fill(if on { col.gamma_multiply(0.14) } else { Color32::TRANSPARENT })
                    .stroke(egui::Stroke::new(1.0_f32, if on { col.gamma_multiply(0.55) } else { LINE }))
                    .corner_radius(10.0);
                if ui
                    .add(btn)
                    .on_hover_text(
                        "Mostra ou esconde as três linhas de memória que não pertencem a processo \
                         nenhum (pools do kernel e compartilhado/cache).\n\n\
                         Esconder muda só a lista: o medidor do topo e a conferência do rodapé \
                         continuam contando essa memória.",
                    )
                    .clicked()
                {
                    self.cfg.show_kernel_rows = !on;
                    self.cfg_dirty = true;
                    self.rows_dirty = true;
                }
            }
            if self.cat_enabled.len() != Category::ALL.len() && ui.button("todas").clicked() {
                self.cat_enabled = Category::ALL.iter().copied().collect();
            }
        });
        ui.add_space(2.0);
    }

    /// Cabeçalho de coluna numérica: alinhado à direita, igual aos valores embaixo.
    fn header_btn_right(&mut self, ui: &mut egui::Ui, key: SortKey, label: &str) {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(2.0);
            self.header_btn(ui, key, label);
        });
    }

    fn header_btn(&mut self, ui: &mut egui::Ui, key: SortKey, label: &str) {
        let active = self.sort == key;
        let arrow = if active { if self.sort_desc { " ▾" } else { " ▴" } } else { "" };
        // Todos os títulos com o mesmo peso; só a cor marca a coluna ordenada — antes a
        // ativa virava uma caixa cinza que parecia um botão perdido no cabeçalho.
        let text = RichText::new(format!("{label}{arrow}"))
            .size(11.5)
            .strong()
            .color(if active { ACCENT } else { MUTED });
        if ui.add(egui::Button::new(text).frame(false)).on_hover_text("Clique para ordenar por esta coluna").clicked() {
            if active {
                self.sort_desc = !self.sort_desc;
            } else {
                self.sort = key;
                self.sort_desc = !matches!(key, SortKey::Name | SortKey::Parent | SortKey::Cat);
            }
        }
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

        // Escala do mini-gráfico da coluna RAM: o maior valor visível vira 100%.
        let max_ram = rows
            .iter()
            .filter_map(|r| match r {
                Row::Proc { pid, .. } => {
                    if tree {
                        self.subtree.get(pid).copied().or_else(|| self.proc(*pid).map(|p| self.mem_of(p)))
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
        ui.visuals_mut().widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(29, 33, 40));

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
            .column(Column::initial(250.0).at_least(140.0).clip(true))
            .column(Column::initial(96.0).at_least(70.0))
            .column(Column::initial(56.0).at_least(40.0))
            .column(Column::initial(56.0).at_least(40.0))
            .column(Column::initial(78.0).at_least(56.0))
            .column(Column::initial(64.0).at_least(44.0))
            .column(Column::initial(52.0).at_least(40.0))
            .column(Column::initial(56.0).at_least(44.0))
            .column(Column::initial(120.0).at_least(60.0).clip(true))
            .column(Column::remainder().at_least(80.0).clip(true))
            .column(Column::exact(58.0))
            .min_scrolled_height(0.0);
        if self.scroll_to_selected {
            if let Some(sel) = self.selected {
                if let Some(i) = rows.iter().position(|r| matches!(r, Row::Proc { pid, .. } if *pid == sel)) {
                    table = table.scroll_to_row(i, Some(Align::Center));
                }
            }
            self.scroll_to_selected = false;
        }

        table
            .header(22.0, |mut header| {
                header.col(|ui| self.header_btn(ui, SortKey::Name, "Nome"));
                let ram_label = match (tree, self.cfg.mem_metric) {
                    (true, MemMetric::Private) => "Priv. (árvore)".to_string(),
                    (true, MemMetric::Commit) => "Commit (árvore)".to_string(),
                    (true, _) => "RAM (árvore)".to_string(),
                    (false, m) => m.short().to_string(),
                };
                header.col(|ui| {
                    self.header_btn_right(ui, SortKey::Ram, &ram_label);
                });
                header.col(|ui| self.header_btn_right(ui, SortKey::Cpu, "CPU"));
                header
                    .col(|ui| { self.header_btn_right(ui, SortKey::Gpu, "GPU"); })
                    .1
                    .on_hover_text(if self.gpu_per_proc {
                        "% de uso da GPU (engine mais ocupada do processo)".to_string()
                    } else {
                        "Contador de GPU por processo indisponível neste host".to_string()
                    });
                header.col(|ui| self.header_btn_right(ui, SortKey::Disk, "Disco"));
                header.col(|ui| self.header_btn(ui, SortKey::Cat, "Cat."));
                header.col(|ui| self.header_btn(ui, SortKey::Pid, "PID"));
                header.col(|ui| self.header_btn(ui, SortKey::Age, "Idade"));
                header.col(|ui| self.header_btn(ui, SortKey::Parent, "Origem"));
                header.col(|ui| {
                    ui.label(RichText::new("Comando").size(11.5).strong().color(MUTED))
                        .on_hover_text("Argumentos da linha de comando (o caminho do exe já está no nome)");
                });
                header.col(|ui| {
                    ui.label(RichText::new("Ações").size(11.5).strong().color(MUTED));
                });
            })
            .body(|body| {
                body.rows(ROW_H, n, |mut row: TableRow| {
                    let i = row.index();
                    match &rows[i] {
                        Row::System { kind, bytes } => {
                            let (kind, bytes) = (*kind, *bytes);
                            row.col(|ui| {
                                ui.add_space(2.0);
                                let (r, _) = ui.allocate_exact_size(Vec2::splat(ICON), egui::Sense::hover());
                                ui.painter().rect_filled(
                                    Rect::from_center_size(r.center(), Vec2::splat(9.0)),
                                    2.0,
                                    kind.color().gamma_multiply(0.8),
                                );
                                ui.label(RichText::new(kind.label()).color(kind.color()).italics());
                            });
                            row.col(|ui| {
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.add_space(2.0);
                                    ui.label(num(fmt_bytes(bytes)).color(kind.color()).strong());
                                });
                            });
                            // CPU, GPU, Disco
                            for _ in 0..3 {
                                row.col(|_ui| {});
                            }
                            row.col(|ui| {
                                ui.label(RichText::new("sistema").weak().italics());
                            });
                            // PID, Idade, Origem
                            for _ in 0..3 {
                                row.col(|_ui| {});
                            }
                            row.col(|ui| {
                                ui.label(RichText::new("não é processo — não pode ser encerrado").weak().small());
                            });
                            row.col(|_ui| {});
                            row.response().on_hover_text(kind.tip());
                        }
                        Row::CatHeader { cat, count, total, collapsed } => {
                            let (cat, count, total, collapsed) = (*cat, *count, *total, *collapsed);
                            row.col(|ui| {
                                let arrow = if collapsed { "▶" } else { "▼" };
                                if ui
                                    .add(egui::Label::new(RichText::new(format!("{arrow} {}", cat.label())).color(cat.color()).strong()).sense(egui::Sense::click()))
                                    .clicked()
                                {
                                    toggle_cat = Some(cat);
                                }
                            });
                            row.col(|ui| {
                                ui.label(RichText::new(fmt_bytes(total)).strong().color(cat.color()));
                            });
                            for _ in 0..3 {
                                row.col(|_ui| {});
                            }
                            row.col(|ui| {
                                ui.label(RichText::new(format!("{count} proc.")).weak());
                            });
                            for _ in 0..5 {
                                row.col(|_ui| {});
                            }
                            if row.response().clicked() {
                                toggle_cat = Some(cat);
                            }
                        }
                        Row::AppHeader { gi } => {
                            let gi = *gi;
                            let Some(g) = self.groups.get(gi) else {
                                for _ in 0..11 {
                                    row.col(|_ui| {});
                                }
                                return;
                            };
                            let (key, name, cat) = (g.key.clone(), g.name.clone(), g.cat);
                            let (ram, cpu, gpu, disk, oldest) = (g.ram, g.cpu, g.gpu, g.disk, g.oldest);
                            let count = g.pids.len();
                            let collapsed = self.collapsed_apps.contains(&key);
                            row.col(|ui| {
                                ui.add_space(2.0);
                                let arrow = if collapsed { "▶" } else { "▼" };
                                let b = egui::Button::new(RichText::new(arrow).weak().small())
                                    .frame(false)
                                    .min_size(Vec2::new(16.0, ROW_H - 2.0));
                                if ui.add(b).clicked() {
                                    toggle_app = Some(key.clone());
                                }
                                match self.icons.get(&key) {
                                    Some(Some(tex)) => {
                                        ui.add(egui::Image::new((tex.id(), Vec2::splat(ICON))));
                                    }
                                    _ => {
                                        let (r, _) = ui.allocate_exact_size(Vec2::splat(ICON), egui::Sense::hover());
                                        ui.painter().circle_filled(r.center(), 4.0, cat.color().gamma_multiply(0.6));
                                    }
                                }
                                ui.add(egui::Label::new(RichText::new(&name).strong()).truncate());
                                ui.label(RichText::new(format!("({count})")).color(cat.color()).small());
                            });
                            row.col(|ui| {
                                let cell = ui.max_rect();
                                let frac = (ram as f32 / max_ram as f32).clamp(0.0, 1.0).sqrt();
                                if frac > 0.01 {
                                    let bar = Rect::from_min_size(
                                        egui::pos2(cell.left(), cell.top()),
                                        Vec2::new(cell.width() * frac, cell.height()),
                                    );
                                    ui.painter().rect_filled(bar, 0.0, ram_color(ram, MUTED).gamma_multiply(0.2));
                                }
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.add_space(2.0);
                                    ui.label(num(fmt_bytes(ram)).color(ram_color(ram, ui_text_color(ui_dark()))).strong())
                                        .on_hover_text(
                                            "Soma dos processos do app. Páginas compartilhadas entre eles \
                                             entram mais de uma vez — é o mesmo exagero da coluna do \
                                             Gerenciador de Tarefas, e some ao trocar a métrica para privada.",
                                        );
                                });
                            });
                            row.col(|ui| {
                                let c = if cpu >= 25.0 {
                                    Color32::from_rgb(255, 150, 90)
                                } else if cpu >= 5.0 {
                                    Color32::from_rgb(230, 210, 120)
                                } else {
                                    ui_text_color(ui_dark())
                                };
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.label(num(if cpu < 0.05 { "–".to_string() } else { format!("{cpu:.1}%") }).color(c).strong());
                                });
                            });
                            row.col(|ui| {
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.add_space(2.0);
                                    if !self.gpu_per_proc || gpu < 0.05 {
                                        ui.label(num("–").color(MUTED));
                                    } else {
                                        ui.label(num(format!("{gpu:.0}%")).color(ui_text_color(ui_dark())).strong());
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
                                ui.label(RichText::new(cat.short()).color(cat.color()).size(11.5));
                            });
                            row.col(|ui| {
                                ui.label(RichText::new("—").color(MUTED));
                            });
                            row.col(|ui| {
                                let secs = ((now_ft - oldest).max(0) / 10_000_000) as u64;
                                ui.label(RichText::new(fmt_age(secs)).color(MUTED))
                                    .on_hover_text("Idade do processo mais antigo do app");
                            });
                            row.col(|_ui| {});
                            row.col(|ui| {
                                ui.label(RichText::new(format!("{count} processos")).weak().size(11.5));
                            });
                            row.col(|ui| {
                                let cell = ui.max_rect();
                                let row_rect = Rect::from_x_y_ranges(row_x.unwrap_or(cell.x_range()), cell.y_range());
                                let hot = ui.rect_contains_pointer(row_rect);
                                let kc = if hot { Color32::from_rgb(235, 90, 90) } else { Color32::from_gray(76) };
                                let b = egui::Button::new(RichText::new("✖").color(kc))
                                    .frame(false)
                                    .min_size(Vec2::new(22.0, ROW_H - 4.0));
                                if ui.add(b).on_hover_text(format!("Finalizar os {count} processos de {name}")).clicked() {
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
                                for _ in 0..11 {
                                    row.col(|_ui| {});
                                }
                                return;
                            };
                            let cat = self.cat(pid);
                            let locked = self.is_locked(&p);
                            let critical = is_critical(&p.name_lower, p.pid);
                            let selected = self.selected == Some(pid);
                            row.set_selected(selected);
                            let text_color = if dim { Color32::from_gray(120) } else { ui_text_color(ui_dark()) };
                            // Nome
                            row.col(|ui| {
                                if group_gutter {
                                    ui.add_space(18.0);
                                }
                                ui.add_space(depth as f32 * 14.0);
                                if tree {
                                    if has_children {
                                        let arrow = if expanded { "▼" } else { "▶" };
                                        let b = egui::Button::new(RichText::new(arrow).weak().small())
                                            .frame(false)
                                            .min_size(Vec2::new(18.0, ROW_H - 2.0));
                                        if ui.add(b).on_hover_text("Expandir / recolher (duplo clique na linha também)").clicked() {
                                            toggle_expand = Some(pid);
                                        }
                                    } else {
                                        ui.add_space(18.0);
                                    }
                                }
                                let key = p.exe_path.to_lowercase();
                                match self.icons.get(&key) {
                                    Some(Some(tex)) => {
                                        ui.add(egui::Image::new((tex.id(), Vec2::splat(ICON))));
                                    }
                                    _ => {
                                        let (r, _) = ui.allocate_exact_size(Vec2::splat(ICON), egui::Sense::hover());
                                        ui.painter().circle_filled(r.center(), 4.0, cat.color().gamma_multiply(0.6));
                                    }
                                }
                                let mut name = RichText::new(&p.name).color(text_color);
                                if locked {
                                    name = name.color(Color32::from_rgb(120, 200, 255));
                                }
                                let lbl = ui.add(egui::Label::new(name).truncate());
                                if locked {
                                    lbl.on_hover_text(if critical { "Processo crítico do Windows" } else { "Protegido (lock)" });
                                }
                                if tree && has_children && !expanded {
                                    let c = self.subtree_count.get(&pid).copied().unwrap_or(1) - 1;
                                    ui.label(RichText::new(format!("(+{c})")).weak().small());
                                }
                            });
                            // RAM
                            row.col(|ui| {
                                let (shown, own) = if tree {
                                    (self.subtree.get(&pid).copied().unwrap_or(self.mem_of(&p)), self.mem_of(&p))
                                } else {
                                    (self.mem_of(&p), self.mem_of(&p))
                                };
                                // Barra de magnitude atrás do número: 419 linhas de texto viram
                                // uma forma — dá pra ver a distribuição sem ler valor por valor.
                                let cell = ui.max_rect();
                                // Escala raiz quadrada: a distribuição de RAM tem cauda longa
                                // (um processo de 1,4 GB e centenas de 200 MB). No linear tudo
                                // abaixo de 300 MB virava o mesmo tracinho de 20 px.
                                let frac = (shown as f32 / max_ram as f32).clamp(0.0, 1.0).sqrt();
                                if frac > 0.01 {
                                    let bar = Rect::from_min_size(
                                        egui::pos2(cell.left(), cell.top()),
                                        Vec2::new(cell.width() * frac, cell.height()),
                                    );
                                    ui.painter().rect_filled(bar, 0.0, ram_color(shown, MUTED).gamma_multiply(0.13));
                                }
                                let mut t = num(fmt_bytes(shown)).color(ram_color(shown, text_color));
                                if tree && has_children {
                                    t = t.strong();
                                }
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.add_space(2.0);
                                    let r = ui.label(t);
                                    if tree && has_children && shown != own {
                                        let n = self.subtree_count.get(&pid).copied().unwrap_or(1) - 1;
                                        r.on_hover_text(format!(
                                            "Deste processo: {}\nCom os {} filhos: {}\n\nA coluna soma a subárvore inteira — o número grande costuma ser dos filhos, não deste processo.",
                                            fmt_bytes(own),
                                            n,
                                            fmt_bytes(shown)
                                        ));
                                    }
                                });
                            });
                            // CPU
                            row.col(|ui| {
                                let c = if p.cpu_pct >= 25.0 {
                                    Color32::from_rgb(255, 150, 90)
                                } else if p.cpu_pct >= 5.0 {
                                    Color32::from_rgb(230, 210, 120)
                                } else {
                                    text_color
                                };
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    let r = ui.label(num(if p.cpu_pct < 0.05 { "–".to_string() } else { format!("{:.1}%", p.cpu_pct) }).color(c));
                                    // A coluna mostra a média — quem está caçando um pico
                                    // precisa do valor cru do último intervalo também.
                                    if p.cpu_pct >= 0.05 || p.cpu_raw_pct >= 0.05 {
                                        r.on_hover_text(format!(
                                            "média: {:.1}%\núltimo intervalo: {:.1}%\n\n100% = a máquina inteira ({} núcleos).",
                                            p.cpu_pct, p.cpu_raw_pct, self.ncpu
                                        ));
                                    }
                                });
                            });
                            // GPU
                            row.col(|ui| {
                                if self.gpu_per_proc && p.gpu_pct >= 0.05 {
                                    let cell = ui.max_rect();
                                    let frac = (p.gpu_pct / max_gpu).clamp(0.0, 1.0).sqrt();
                                    if frac > 0.01 {
                                        let bar = Rect::from_min_size(
                                            egui::pos2(cell.left(), cell.top()),
                                            Vec2::new(cell.width() * frac, cell.height()),
                                        );
                                        ui.painter().rect_filled(bar, 0.0, Color32::from_rgb(180, 130, 230).gamma_multiply(0.15));
                                    }
                                }
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.add_space(2.0);
                                    if !self.gpu_per_proc {
                                        ui.label(RichText::new("–").color(Color32::from_gray(90)))
                                            .on_hover_text("Contador de GPU por processo indisponível neste host");
                                    } else if p.gpu_pct < 0.05 {
                                        ui.label(num("–").color(MUTED));
                                    } else {
                                        let c = if p.gpu_pct >= 50.0 {
                                            Color32::from_rgb(255, 150, 90)
                                        } else if p.gpu_pct >= 10.0 {
                                            Color32::from_rgb(230, 210, 120)
                                        } else {
                                            text_color
                                        };
                                        ui.label(num(format!("{:.0}%", p.gpu_pct)).color(c));
                                    }
                                });
                            });
                            // Disco (bytes/s, raiz quadrada para não deixar tudo achatado)
                            row.col(|ui| {
                                let cell = ui.max_rect();
                                let frac = (p.disk_bps as f32 / max_disk as f32).clamp(0.0, 1.0).sqrt();
                                if frac > 0.01 {
                                    let bar = Rect::from_min_size(
                                        egui::pos2(cell.left(), cell.top()),
                                        Vec2::new(cell.width() * frac, cell.height()),
                                    );
                                    ui.painter().rect_filled(bar, 0.0, Color32::from_rgb(120, 150, 220).gamma_multiply(0.14));
                                }
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.add_space(2.0);
                                    if p.disk_bps < 1024.0 {
                                        ui.label(num("–").color(MUTED));
                                    } else {
                                        ui.label(num(fmt_bps(p.disk_bps)).color(text_color));
                                    }
                                });
                            });
                            // Categoria
                            row.col(|ui| {
                                let overridden = self.cfg.overrides.contains_key(&p.name_lower);
                                let mut t = RichText::new(cat.short()).color(cat.color()).size(11.5);
                                if overridden {
                                    t = t.underline();
                                }
                                ui.label(t).on_hover_text(if overridden {
                                    format!("{} (definido manualmente)", cat.label())
                                } else {
                                    cat.label().to_string()
                                });
                            });
                            row.col(|ui| {
                                ui.label(num(p.pid.to_string()).color(MUTED));
                            });
                            row.col(|ui| {
                                let secs = ((now_ft - p.create_time).max(0) / 10_000_000) as u64;
                                let mut t = RichText::new(fmt_age(secs)).color(text_color);
                                if secs < 5 {
                                    t = t.color(Color32::from_rgb(90, 220, 130));
                                }
                                ui.label(t);
                            });
                            row.col(|ui| {
                                let (label, target, tip, via_env) = self.origin_label(&p);
                                let mut t = RichText::new(label);
                                t = if via_env { t.color(Color32::from_rgb(200, 160, 255)).italics() }
                                    else if target.is_some() { t.color(text_color) } else { t.weak().small() };
                                if let Some(tp) = target {
                                    let r = ui.add(egui::Label::new(t).truncate().sense(egui::Sense::click()));
                                    if r.on_hover_text(tip).clicked() {
                                        click_select = Some(tp);
                                    }
                                } else {
                                    let r = ui.add(egui::Label::new(t).truncate());
                                    if !tip.is_empty() {
                                        r.on_hover_text(tip);
                                    }
                                }
                            });
                            row.col(|ui| {
                                // Para quem hospeda serviço, o nome do serviço vale mil vezes
                                // mais que "-k netsvcs -p". Mesma coluna, conteúdo útil.
                                let svcs = self.services_of(pid);
                                let (s, color) = if svcs.is_empty() {
                                    (cmd_args(&p), MUTED)
                                } else {
                                    let names: Vec<&str> = svcs.iter().map(|(_, d)| d.as_str()).collect();
                                    (names.join(" · "), Color32::from_rgb(130, 175, 215))
                                };
                                let full = if svcs.is_empty() {
                                    if p.cmdline.is_empty() { p.exe_path.clone() } else { p.cmdline.clone() }
                                } else {
                                    let list: Vec<String> =
                                        svcs.iter().map(|(n, d)| format!("{d}  ({n})")).collect();
                                    format!("Serviços hospedados neste processo:\n{}", list.join("\n"))
                                };
                                let r = ui.add(egui::Label::new(RichText::new(&s).color(color).size(11.5)).truncate());
                                if !full.is_empty() {
                                    r.on_hover_text(full);
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
                                let btn_size = Vec2::new(22.0, ROW_H - 4.0);
                                if critical {
                                    ui.add_sized(btn_size, egui::Label::new(RichText::new("🔒").color(Color32::from_gray(if hot { 130 } else { 80 }))))
                                        .on_hover_text("Crítico do Windows — não pode ser encerrado");
                                } else {
                                    let (icon, tip) = if locked { ("🔒", "Protegido — clique para desproteger") } else { ("🔓", "Clique para proteger (lock)") };
                                    let col = if locked {
                                        Color32::from_rgb(120, 200, 255)
                                    } else if hot {
                                        Color32::from_gray(150)
                                    } else {
                                        Color32::from_gray(76)
                                    };
                                    let b = egui::Button::new(RichText::new(icon).color(col)).frame(false).min_size(btn_size);
                                    if ui.add(b).on_hover_text(tip).clicked() {
                                        lock = Some(p.name_lower.clone());
                                    }
                                    if !locked {
                                        let kc = if hot { Color32::from_rgb(235, 90, 90) } else { Color32::from_gray(76) };
                                        let kb = egui::Button::new(RichText::new("✖").color(kc)).frame(false).min_size(btn_size);
                                        let r = ui.add(kb).on_hover_text("Finalizar processo (Shift: árvore inteira)");
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
                                    if ui.button("✖ Finalizar processo").clicked() {
                                        kill = Some((pid, false));
                                        ui.close_menu();
                                    }
                                    let n = self.subtree_count.get(&pid).copied().unwrap_or(1);
                                    if n > 1 && ui.button(format!("✖ Finalizar árvore ({n} processos)")).clicked() {
                                        kill = Some((pid, true));
                                        ui.close_menu();
                                    }
                                }
                                if !critical {
                                    let lt = if locked { "🔓 Desproteger" } else { "🔒 Proteger (lock)" };
                                    if ui.button(lt).clicked() {
                                        lock = Some(p.name_lower.clone());
                                        ui.close_menu();
                                    }
                                }
                                ui.separator();
                                ui.menu_button("Categoria", |ui| {
                                    for c in Category::ALL {
                                        let cur = cat == c;
                                        if ui.selectable_label(cur, RichText::new(c.label()).color(c.color())).clicked() {
                                            set_cat = Some((p.name_lower.clone(), Some(c)));
                                            ui.close_menu();
                                        }
                                    }
                                    ui.separator();
                                    if ui.button("Regra automática").clicked() {
                                        set_cat = Some((p.name_lower.clone(), None));
                                        ui.close_menu();
                                    }
                                });
                                if p.ppid != 0 && self.proc(p.ppid).is_some() && ui.button("↑ Ir para o pai").clicked() {
                                    click_select = Some(p.ppid);
                                    ui.close_menu();
                                }
                                ui.separator();
                                if !p.cmdline.is_empty() && ui.button("Copiar linha de comando").clicked() {
                                    copy = Some(p.cmdline.clone());
                                    ui.close_menu();
                                }
                                if !p.exe_path.is_empty() {
                                    if ui.button("Copiar caminho").clicked() {
                                        copy = Some(p.exe_path.clone());
                                        ui.close_menu();
                                    }
                                    if ui.button("Abrir pasta do executável").clicked() {
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
            if !self.collapsed_apps.remove(&k) {
                self.collapsed_apps.insert(k);
            }
            self.rows_dirty = true;
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
        if let Some(s) = copy {
            ui.ctx().copy_text(s);
            self.toast("Copiado".into(), false);
        }
        if let Some(path) = open_folder {
            open_in_explorer(&path);
        }
    }

    // ---------- visão Térmico ----------

    /// Sensores + controle de fans + ESTABILIZAR — o TempHUD embutido no RamDog. Toda leitura
    /// e toda escrita de hardware acontecem no helper `hwtemp.exe` (a curva mora lá, por
    /// segurança); aqui é só UI: comandos saem pelo stdin dele, o estado volta no snapshot.
    fn ui_thermal(&mut self, ui: &mut egui::Ui) {
        let hw = self.hwtemp.clone();
        let cmd = self.sampler.hw_cmd.clone();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(8.0);
            if hw.sensors.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(40.0);
                    ui.label(RichText::new("Sem leitura de sensores").strong().size(15.0));
                    let why = if cfg!(target_os = "macos") {
                        "A visão Térmico no macOS ainda não está ligada."
                    } else if cfg!(target_os = "linux") {
                        "Nenhum sensor em /sys/class/hwmon. Sem coretemp/k10temp/zenpower o kernel não expôs temperatura."
                    } else if !cfg!(windows) {
                        "A visão Térmico ainda é só Windows: depende do helper hwtemp.exe (LibreHardwareMonitorLib)."
                    } else if cmd.is_none() {
                        "hwtemp.exe não foi achado ao lado do ramdog.exe — reinstale com o helper junto."
                    } else {
                        "O helper subiu mas ainda não reportou. Se persistir: .NET 8 Desktop Runtime ausente, ou placa-mãe sem Super I/O suportado pela LibreHardwareMonitor."
                    };
                    ui.label(RichText::new(why).color(MUTED));
                });
                return;
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
                        Self::thermal_card(col, hw_name, rows);
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
                    .fill(Color32::from_rgb(24, 27, 33))
                    .stroke(Stroke::new(1.0_f32, LINE))
                    .corner_radius(6.0)
                    .inner_margin(egui::Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("CONTROLE DE FANS").strong().color(ACCENT).size(11.0));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(
                                    RichText::new("arraste ou digite % · Auto = BIOS · proteção no modo manual: ≥80°C força 100%, solta abaixo de 72°C")
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
                                    ("ESTABILIZAR  ·  travar fans em 50%".to_owned(), ACCENT_BG, ACCENT)
                                } else if stab.held > 50.5 {
                                    let tag = if cpu >= 95.0 { "teto térmico" } else { "rampa linear" };
                                    (format!("FANS EM {:.0}%  ·  CPU {cpu:.0}°C ({tag})", stab.held), THERM_WARN_BG, THERM_WARN_FG)
                                } else {
                                    (format!("FANS TRAVADOS EM 50%  ·  CPU {cpu:.0}°C"), THERM_STAB_BG, THERM_STAB_FG)
                                };
                                let btn = egui::Button::new(RichText::new(label).strong().size(13.5).color(fg))
                                    .fill(bg)
                                    .stroke(Stroke::new(1.0_f32, fg.gamma_multiply(0.7)))
                                    .corner_radius(6.0)
                                    .min_size(Vec2::new(ui.available_width(), 34.0));
                                let resp = ui.add(btn).on_hover_text(
                                    "Liga/desliga a curva do TempHUD. Só fans SuperIO da placa-mãe — GPU fica no zero-fan dela. Clicar de novo (ou fechar o app) devolve tudo à BIOS.",
                                );
                                if resp.clicked() {
                                    self.toggle_stab();
                                }
                                ui.add_space(4.0);
                                Self::thermal_curve(ui);
                                ui.add_space(2.0);
                                ui.label(
                                    RichText::new("50% até 80°C, rampa linear até 100% aos 92°C, teto imediato a 95°C. Sobe/desce no máx. 3%/s. Soltar devolve tudo à BIOS.")
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
                                            let sl = ui.add_enabled(!stab_on, egui::Slider::new(&mut v, 0.0..=100.0).show_value(false).trailing_fill(true));
                                            let dv = ui.add_enabled(!stab_on, egui::DragValue::new(&mut v).range(0.0..=100.0).max_decimals(0).suffix("%"));
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
                                                r.on_hover_text("Proteção térmica: CPU ≥80°C — 100% forçado sobre o % manual até esfriar (72°C).");
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
                    "Sem controles de fan: a placa-mãe não expôs Super I/O suportado pela LibreHardwareMonitor."
                } else {
                    "Sem controles de fan: rode elevado (botão \"Reabrir como admin\" no topo) — o driver de sensores não sobe sem isso."
                };
                ui.label(RichText::new(why).color(MUTED));
            } else if cfg!(target_os = "linux") {
                ui.add_space(14.0);
                ui.label(RichText::new(
                    "No Linux a visão Térmico é só leitura (hwmon). O RamDog não escreve PWM — sem curva ESTABILIZAR, os fans ficam com o kernel/BIOS."
                ).color(MUTED));
            }
        });
    }

    /// Um cartão de hardware da visão Térmico: cabeçalho, número-herói (temperatura mais
    /// alta) e as demais leituras em linhas compactas; cargas ganham uma minibarra.
    fn thermal_card(ui: &mut egui::Ui, hw_name: &str, rows: &[&crate::hwtemp::SensorRow]) {
        egui::Frame::new()
            .fill(Color32::from_rgb(24, 27, 33))
            .stroke(Stroke::new(1.0_f32, LINE))
            .corner_radius(6.0)
            .inner_margin(egui::Margin::symmetric(14, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 3.0;
                ui.label(RichText::new(hw_name).strong().color(ACCENT).size(11.0));
                let hero = rows
                    .iter()
                    .filter(|s| s.kind == "temp")
                    .max_by(|a, b| a.value.total_cmp(&b.value))
                    .copied();
                if let Some(h) = hero {
                    let n_temps = rows.iter().filter(|s| s.kind == "temp").count();
                    ui.label(num(format!("{:.1} °C", h.value)).size(24.0).strong().color(Self::temp_color(h.value.round() as u32)));
                    let sub = if n_temps > 1 { format!("{} · sensor mais quente", h.name) } else { h.name.clone() };
                    ui.label(RichText::new(sub).color(MUTED).size(11.0));
                    ui.add_space(3.0);
                }
                for s in rows {
                    if hero.is_some_and(|h| h.name == s.name && h.kind == s.kind) {
                        continue;
                    }
                    let (text, color) = match s.kind.as_str() {
                        "temp" => (format!("{:>5.1} °C", s.value), Self::temp_color(s.value.round() as u32)),
                        "rpm" => (format!("{:>5.0} RPM", s.value), MUTED),
                        _ => (format!("{:>5.1} %", s.value), Self::load_color(s.value / 100.0)),
                    };
                    let is_load = !matches!(s.kind.as_str(), "temp" | "rpm");
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        let avail = ui.available_width();
                        ui.scope(|ui| {
                            ui.set_width((avail - 148.0).max(40.0));
                            ui.add(egui::Label::new(RichText::new(&s.name).color(MUTED).size(12.0)).truncate());
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
        p.line_segment([egui::pos2(x0, rect.top() + 6.0), egui::pos2(x0, yax)], Stroke::new(1.0_f32, LINE));
        p.line_segment([egui::pos2(x0, yax), egui::pos2(x1, yax)], Stroke::new(1.0_f32, LINE));
        p.line_segment([egui::pos2(x0, y50), egui::pos2(x80, y50)], Stroke::new(2.0_f32, ACCENT));
        p.line_segment([egui::pos2(x80, y50), egui::pos2(x92, y100)], Stroke::new(2.0_f32, ACCENT));
        p.line_segment([egui::pos2(x92, y100), egui::pos2(x1, y100)], Stroke::new(2.0_f32, ACCENT));
        p.circle_filled(egui::pos2(x80, y50), 3.0, ACCENT);
        p.circle_filled(egui::pos2(x92, y100), 3.0, Color32::from_rgb(226, 166, 72));
        let font = egui::FontId::monospace(9.5);
        p.text(egui::pos2(x0 - 4.0, y50), egui::Align2::RIGHT_CENTER, "50%", font.clone(), MUTED);
        p.text(egui::pos2(x0 - 4.0, y100), egui::Align2::RIGHT_CENTER, "100%", font.clone(), MUTED);
        p.text(egui::pos2(x80, yax + 4.0), egui::Align2::CENTER_TOP, "80°C", font.clone(), MUTED);
        p.text(egui::pos2(x92, yax + 4.0), egui::Align2::CENTER_TOP, "92°C", font, MUTED);
    }

    fn ui_details(&mut self, ui: &mut egui::Ui) {
        let Some(sel) = self.selected else {
            // Estado vazio útil: em vez de só instruir, já responde "quem está comendo minha RAM".
            let m = self.cfg.mem_metric;
            let mut top: Vec<&ProcInfo> = self.procs.iter().collect();
            top.sort_by_key(|p| std::cmp::Reverse(Self::metric_of(m, p)));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                ui.label(RichText::new("Maiores agora").color(MUTED).size(11.5));
                for p in top.iter().take(4) {
                    let cat = self.cat(p.pid);
                    ui.add_space(6.0);
                    ui.label(RichText::new("●").color(cat.color()).size(10.0));
                    ui.label(RichText::new(&p.name).size(12.0));
                    ui.label(num(fmt_bytes(Self::metric_of(m, p))).color(ram_color(Self::metric_of(m, p), MUTED)));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_space(10.0);
                    ui.label(RichText::new("clique numa linha para ver origem e ações · botão direito abre o menu").color(MUTED).size(11.0));
                });
            });
            return;
        };
        let alive = self.proc(sel).cloned();
        let Some((p, cat)) = alive.clone().map(|p| (p, self.cat(sel))).or_else(|| self.selected_keep.clone()) else {
            self.selected = None;
            return;
        };
        let locked = self.is_locked(&p);
        let critical = is_critical(&p.name_lower, p.pid);
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
            ui.label(RichText::new(&p.name).strong().size(15.0));
            ui.label(RichText::new(format!("PID {}", p.pid)).monospace().weak());
            ui.label(RichText::new(format!("● {}", cat.label())).color(cat.color()));
            if locked {
                ui.label(RichText::new(if critical { "🔒 crítico" } else { "🔒 protegido" }).color(Color32::from_rgb(120, 200, 255)));
            }
            if alive.is_none() {
                ui.label(RichText::new("(encerrado)").color(Color32::from_rgb(235, 90, 90)).strong());
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if alive.is_some() {
                    if !locked {
                        if ui.add(egui::Button::new(RichText::new("✖ Finalizar").color(Color32::WHITE)).fill(Color32::from_rgb(170, 50, 50))).clicked() {
                            kill = Some((p.pid, false));
                        }
                        let n = self.subtree_count.get(&p.pid).copied().unwrap_or(1);
                        if n > 1
                            && ui
                                .add(egui::Button::new(RichText::new(format!("✖ Finalizar árvore ({n})")).color(Color32::WHITE)).fill(Color32::from_rgb(140, 40, 40)))
                                .on_hover_text(format!("Encerra este processo e todos os {} descendentes — total {}", n - 1, fmt_bytes(self.subtree.get(&p.pid).copied().unwrap_or(0))))
                                .clicked()
                        {
                            kill = Some((p.pid, true));
                        }
                    }
                    if !critical {
                        let lt = if locked { "🔓 Desproteger" } else { "🔒 Proteger" };
                        if ui.button(lt).on_hover_text("Lock por nome de executável: vale para todas as instâncias e persiste").clicked() {
                            lock = Some(p.name_lower.clone());
                        }
                    }
                }
                let overridden = self.cfg.overrides.contains_key(&p.name_lower);
                let mut chosen = cat;
                egui::ComboBox::from_id_salt("cat_override")
                    .selected_text(RichText::new(chosen.label()).color(chosen.color()))
                    .width(120.0)
                    .show_ui(ui, |ui| {
                        for c in Category::ALL {
                            ui.selectable_value(&mut chosen, c, RichText::new(c.label()).color(c.color()));
                        }
                    });
                if chosen != cat {
                    set_cat = Some((p.name_lower.clone(), Some(chosen)));
                }
                if overridden && ui.small_button("auto").on_hover_text("Voltar para a regra automática").clicked() {
                    set_cat = Some((p.name_lower.clone(), None));
                }
                ui.label(RichText::new("Categoria:").weak());
            });
        });
        // Fora do closure: a verificação precisa de `&mut self` e dispara a thread na
        // primeira vez que este executável é selecionado.
        let sig = self.signature_of(&p.exe_path, &ui.ctx().clone());
        ui.add_space(2.0);
        egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
            egui::Grid::new("details_grid").num_columns(2).spacing([10.0, 3.0]).show(ui, |ui| {
                // "O que é isso" vem antes de tudo: é a pergunta que faz alguém abrir o painel.
                if let Some(k) = knowledge::lookup(&p.name_lower) {
                    ui.label(RichText::new("O que é").weak());
                    ui.vertical(|ui| {
                        ui.add(egui::Label::new(RichText::new(k.what).size(12.5)).wrap());
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 5.0;
                            ui.label(
                                RichText::new(format!("{} {}", k.risk.dot(), k.risk.label()))
                                    .color(k.risk.color())
                                    .strong()
                                    .size(12.0),
                            )
                            .on_hover_text(k.risk.tip());
                            ui.add(egui::Label::new(RichText::new(k.why).weak().size(11.5)).wrap());
                        });
                    });
                    ui.end_row();
                }

                let svcs = self.services_of(p.pid);
                if !svcs.is_empty() {
                    ui.label(RichText::new("Serviços").weak());
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
                            egui::CollapsingHeader::new(format!("mais {} serviço(s)", svcs.len() - 3))
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

                ui.label(RichText::new("Origem").weak());
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let chain = self.ancestry(p.pid);
                    if chain.is_empty() {
                        if p.raw_ppid != 0 {
                            // Com o nome guardado a linha para de ser um número sem sentido.
                            match self.seen_names.get(&p.raw_ppid) {
                                Some(n) => {
                                    ui.label(RichText::new(format!("{n} (PID {})", p.raw_ppid)).strong());
                                    ui.label(RichText::new("— já encerrado").weak());
                                }
                                None => {
                                    ui.label(RichText::new(format!("pai (PID {}) já encerrado", p.raw_ppid)).weak());
                                }
                            }
                        } else {
                            ui.label(RichText::new("sem pai conhecido").weak());
                        }
                    }
                    for a in chain {
                        if let Some(ap) = self.proc(a) {
                            let r = ui.add(egui::Label::new(RichText::new(format!("{} ({})", ap.name, ap.pid)).color(self.cat(a).color())).sense(egui::Sense::click()));
                            if r.on_hover_text("Selecionar").clicked() {
                                goto = Some(a);
                            }
                            ui.label(RichText::new("›").weak());
                        }
                    }
                    ui.label(RichText::new(format!("{} ({})", p.name, p.pid)).strong());
                });
                ui.end_row();

                if !p.launcher.is_empty() {
                    ui.label(RichText::new("Lançado por").weak());
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let l = &p.launcher;
                        let purple = Color32::from_rgb(200, 160, 255);
                        if let Some(a) = &l.agent {
                            let mut txt = a.clone();
                            if let Some(sid) = &l.session {
                                txt.push_str(&format!(" (sessão {sid})"));
                            }
                            ui.label(RichText::new(txt).color(purple).strong());
                            match l.agent_pid {
                                Some(apid) => match self.proc(apid) {
                                    Some(ap) => {
                                        let r = ui.add(egui::Label::new(RichText::new(format!("→ {} ({})", ap.name, apid)).color(self.cat(apid).color())).sense(egui::Sense::click()));
                                        if r.on_hover_text("Selecionar o processo do agente").clicked() {
                                            goto = Some(apid);
                                        }
                                    }
                                    None => { ui.label(RichText::new(format!("→ PID {apid} (já encerrado)")).weak()); }
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
                        ui.label(RichText::new("(via variáveis de ambiente herdadas)").weak().small());
                    });
                    ui.end_row();
                }

                ui.label(RichText::new("Executável").weak());
                ui.vertical(|ui| {
                    ui.add(egui::Label::new(RichText::new(if p.exe_path.is_empty() { "(sem acesso)" } else { &p.exe_path }).monospace().small()).wrap());
                    // Sem isto não há como distinguir o wininit.exe verdadeiro de um
                    // impostor de mesmo nome numa pasta qualquer.
                    match sig.as_ref() {
                        Some(s) => {
                            ui.label(RichText::new(s.label()).color(s.color()).size(11.5))
                                .on_hover_text(s.tip());
                        }
                        None => {
                            ui.label(RichText::new("verificando assinatura…").weak().size(11.5));
                        }
                    }
                });
                ui.end_row();

                ui.label(RichText::new("Comando").weak());
                ui.add(egui::Label::new(RichText::new(if p.cmdline.is_empty() { "(sem acesso)" } else { &p.cmdline }).monospace().small()).wrap());
                ui.end_row();

                ui.label(RichText::new("Memória").weak());
                // Próprio e subárvore em linhas separadas e rotuladas: ler "wininit 5,12 GB"
                // e concluir que o wininit é o vilão é o mal-entendido nº 1 da visão de árvore.
                // Ele usa 8 MB; os 5 GB são dos filhos.
                let sub = self.subtree.get(&p.pid).copied().unwrap_or(self.mem_of(&p));
                let n = self.subtree_count.get(&p.pid).copied().unwrap_or(1);
                ui.vertical(|ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 5.0;
                        ui.label(RichText::new("deste processo:").weak().size(11.5));
                        ui.label(RichText::new(fmt_bytes(p.working_set)).strong());
                        ui.label(
                            RichText::new(format!(
                                "(privada {} · commit {})",
                                fmt_bytes(p.private_ws),
                                fmt_bytes(p.commit)
                            ))
                            .weak()
                            .size(11.5),
                        );
                    });
                    if n > 1 {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 5.0;
                            ui.label(RichText::new("com os filhos:").weak().size(11.5));
                            ui.label(RichText::new(fmt_bytes(sub)).strong().color(Color32::from_rgb(230, 190, 80)));
                            ui.label(RichText::new(format!("em {n} processos")).weak().size(11.5))
                                .on_hover_text("É este o número que a coluna RAM mostra na visão de árvore — a soma da subárvore, não o consumo deste processo.");
                        });
                    }
                });
                ui.end_row();

                ui.label(RichText::new("Execução").weak());
                ui.label(format!(
                    "iniciado há {}   ·   CPU {:.1}% (último intervalo {:.1}%)   ·   {} threads   ·   {} handles   ·   sessão {}",
                    fmt_age(secs),
                    p.cpu_pct,
                    p.cpu_raw_pct,
                    p.threads,
                    p.handles,
                    p.session
                ));
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
    fn ui_accounting(&mut self, ui: &mut egui::Ui) {
        let b = self.breakdown();
        if b.used == 0 {
            return;
        }
        // "Medido" = privado dos processos + os dois pools, cada um lido de uma API. O resto
        // é um subtraendo, e o rótulo diz isso — chamá-lo de "atribuído" fingiria uma
        // medição que não existe, que é justamente o vício do Gerenciador de Tarefas.
        let measured = b.private.saturating_add(b.paged_pool).saturating_add(b.nonpaged_pool);
        let overflow = measured > b.used;
        let color = if b.kernel_ok && !overflow { MUTED } else { Color32::from_rgb(200, 150, 90) };
        let head = if b.shared_and_cache > 0 {
            format!(
                "RAM {}: {} medidos + {} por diferença",
                fmt_gb(b.used),
                fmt_gb(measured),
                fmt_gb(b.shared_and_cache)
            )
        } else {
            format!("RAM {}: {} medidos", fmt_gb(b.used), fmt_gb(measured.min(b.used)))
        };
        let mut tip = format!(
            "Composição dos {} em uso, sempre em memória privada — a única base que não conta \
             a mesma página física duas vezes:\n\n\
             Processos (privado)  {}\n",
            fmt_gb(b.used),
            fmt_bytes_short(b.private)
        );
        if b.kernel_ok {
            tip.push_str(&format!("{}  {}\n", SysRow::PagedPool.label(), fmt_bytes_short(b.paged_pool)));
            tip.push_str(&format!("{}  {}\n", SysRow::NonPagedPool.label(), fmt_bytes_short(b.nonpaged_pool)));
        } else {
            tip.push_str("Pools do kernel: indisponíveis (GetPerformanceInfo falhou)\n");
        }
        tip.push_str(&format!("{}  {}\n", SysRow::SharedAndCache.label(), fmt_bytes_short(b.shared_and_cache)));
        if overflow {
            tip.push_str(
                "\nAs parcelas medidas já passam do total em uso: o pool paginado inclui a \
                 fração que está no disco, e os processos são amostrados num instante \
                 diferente da leitura de memória. O resto foi zerado em vez de negativado.\n",
            );
        }
        if self.cfg.mem_metric != MemMetric::Private {
            let shown: u64 = self.procs.iter().map(|p| self.mem_of(p)).sum();
            tip.push_str(&format!(
                "\nA coluna RAM está em {} e soma {} — acima do privado porque cada página \
                 compartilhada conta em todo processo que a mapeia.",
                self.cfg.mem_metric.label().to_lowercase(),
                fmt_bytes_short(shown)
            ));
        }
        ui.label(RichText::new(head).color(color).small()).on_hover_text(tip);
    }

    fn save_cfg_if_dirty(&mut self) {
        if self.cfg_dirty {
            self.cfg_dirty = false;
            if let Err(e) = self.cfg.save() {
                self.toast(format!("Falha ao salvar config: {e}"), true);
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
                    let bg = if err { Color32::from_rgb(120, 40, 40) } else { Color32::from_rgb(40, 100, 60) };
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
        self.ingest(ctx);
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
        }
        if esc && !ctx.wants_keyboard_input() {
            self.selected = None;
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| self.ui_top(ui));
        // O painel de detalhes descreve a linha selecionada da tabela. Num addon não há
        // tabela nem seleção — ele ficaria como 150 px de espaço vazio.
        if !self.cfg.view.is_addon() {
            egui::TopBottomPanel::bottom("details")
                .resizable(true)
                .default_height(150.0)
                .min_height(40.0)
                .show(ctx, |ui| self.ui_details(ui));
        }
        egui::TopBottomPanel::bottom("statusbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let shown = self.procs.iter().filter(|p| self.passes(p, &self.search.trim().to_lowercase())).count();
                ui.label(RichText::new(format!("{} processos ({} exibidos)", self.procs.len(), shown)).weak().small());
                ui.separator();
                let locked_n = self.cfg.locked.len();
                ui.label(RichText::new(format!("{locked_n} protegidos")).weak().small())
                    .on_hover_text(self.cfg.locked.iter().cloned().collect::<Vec<_>>().join("\n"));
                ui.separator();
                ui.label(RichText::new(format!("amostra {:.0} ms", self.sample_ms)).weak().small());
                self.ui_sampling_controls(ui);
                ui.separator();
                self.ui_accounting(ui);
                if self.order_frozen {
                    ui.separator();
                    ui.label(RichText::new("ordem congelada (mouse sobre a tabela)").weak().small())
                        .on_hover_text("Enquanto o mouse está sobre a tabela a ordem das linhas não muda, para você não clicar no processo errado. Valores continuam atualizando.");
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    // Os atalhos agem sobre a linha selecionada da tabela. Anunciá-los
                    // dentro de um addon é prometer uma tecla que não faz nada ali.
                    if !self.cfg.view.is_addon() {
                        ui.label(RichText::new("Del: finalizar · Shift+Del: árvore · F5: atualizar · botão direito: menu").weak().small());
                    }
                });
            });
        });
        let view = self.cfg.view;
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(&ctx.style()).inner_margin(egui::Margin::symmetric(6, 2)))
            .show(ctx, |ui| {
                if view.is_addon() {
                    self.ui_addon_body(ui, view);
                } else {
                    self.ui_table(ui);
                }
            });

        self.ui_status(ctx);
        self.save_cfg_if_dirty();
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
        defs.font_data.insert("segoeui".into(), std::sync::Arc::new(FontData::from_owned(bytes)));
        defs.families.entry(FontFamily::Proportional).or_default().insert(0, "segoeui".into());
    }
    // Segoe UI Symbol: ● ▶ ▼ ⏸ e afins, como fallback
    if let Ok(bytes) = std::fs::read(format!("{fonts_dir}seguisym.ttf")) {
        defs.font_data.insert("seguisym".into(), std::sync::Arc::new(FontData::from_owned(bytes)));
        defs.families.entry(FontFamily::Proportional).or_default().push("seguisym".into());
        defs.families.entry(FontFamily::Monospace).or_default().push("seguisym".into());
    }
    // Consolas para monospace (comandos/caminhos)
    if let Ok(bytes) = std::fs::read(format!("{fonts_dir}consola.ttf")) {
        defs.font_data.insert("consolas".into(), std::sync::Arc::new(FontData::from_owned(bytes)));
        defs.families.entry(FontFamily::Monospace).or_default().insert(0, "consolas".into());
    }
    ctx.set_fonts(defs);
}

/// Paleta: cinza-azulado tintado (nunca preto puro), um acento frio para seleção/estado ativo;
/// as cores de categoria e o "calor" da RAM são semânticas e ficam de fora do acento.
pub const BG: Color32 = Color32::from_rgb(17, 19, 24);
pub const PANEL: Color32 = Color32::from_rgb(21, 24, 30);
pub const SURFACE: Color32 = Color32::from_rgb(28, 32, 39);
pub const SURFACE_HI: Color32 = Color32::from_rgb(36, 41, 50);
pub const LINE: Color32 = Color32::from_rgb(40, 45, 54);
pub const TEXT: Color32 = Color32::from_rgb(222, 226, 232);
pub const MUTED: Color32 = Color32::from_rgb(140, 148, 160);
pub const ACCENT: Color32 = Color32::from_rgb(96, 148, 214);
pub const ACCENT_BG: Color32 = Color32::from_rgb(38, 60, 92);
/// Verde "segurando" / laranja "rampa" do ESTABILIZAR — portados do TempHUD pra manter a
/// mesma leitura de estado entre os dois apps.
const THERM_STAB_BG: Color32 = Color32::from_rgb(10, 61, 50);
const THERM_STAB_FG: Color32 = Color32::from_rgb(105, 240, 174);
const THERM_WARN_BG: Color32 = Color32::from_rgb(74, 28, 11);
const THERM_WARN_FG: Color32 = Color32::from_rgb(255, 171, 145);

fn setup_style(ctx: &egui::Context) {
    setup_fonts(ctx);
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = SURFACE;
    v.extreme_bg_color = BG;
    v.faint_bg_color = Color32::from_rgb(24, 27, 33);
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
    v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, Color32::from_rgb(52, 58, 70));
    v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, Color32::from_rgb(200, 205, 212));
    v.widgets.hovered.bg_fill = SURFACE_HI;
    v.widgets.hovered.weak_bg_fill = SURFACE_HI;
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, Color32::from_rgb(58, 65, 78));
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0_f32, Color32::WHITE);
    v.widgets.active.bg_fill = ACCENT_BG;
    v.widgets.active.weak_bg_fill = ACCENT_BG;
    v.widgets.active.bg_stroke = egui::Stroke::new(1.0_f32, ACCENT);
    v.widgets.active.fg_stroke = egui::Stroke::new(1.0_f32, Color32::WHITE);
    v.widgets.open.bg_fill = SURFACE_HI;
    v.widgets.open.weak_bg_fill = SURFACE_HI;
    for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = 4.0.into();
    }
    v.striped = true;
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        use egui::{FontFamily, FontId, TextStyle};
        s.text_styles.insert(TextStyle::Body, FontId::new(13.0, FontFamily::Proportional));
        s.text_styles.insert(TextStyle::Button, FontId::new(13.0, FontFamily::Proportional));
        s.text_styles.insert(TextStyle::Small, FontId::new(11.0, FontFamily::Proportional));
        s.text_styles.insert(TextStyle::Monospace, FontId::new(12.0, FontFamily::Monospace));
        s.text_styles.insert(TextStyle::Heading, FontId::new(17.0, FontFamily::Proportional));
        s.spacing.item_spacing = egui::vec2(8.0, 4.0);
        s.spacing.button_padding = egui::vec2(8.0, 3.0);
        s.spacing.menu_margin = egui::Margin::same(8);
        s.interaction.selectable_labels = false;
        s.interaction.tooltip_delay = 0.35;
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
    if dark { Color32::from_gray(220) } else { Color32::from_gray(30) }
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

/// Fim do primeiro `.exe` na string (ASCII, case-insensitive) — sempre em fronteira de char.
fn exe_token_end(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    if b.len() < 4 {
        return None;
    }
    (0..=b.len() - 4).find(|&i| b[i..i + 4].eq_ignore_ascii_case(b".exe")).map(|i| i + 4)
}

/// A coluna de comando mostrava o caminho completo do exe e truncava antes de chegar nos
/// argumentos — justamente a parte que diferencia um `brave.exe` do outro. Aqui o caminho
/// sai (já está na coluna Nome) e sobram os argumentos.
fn cmd_args(p: &ProcInfo) -> String {
    let cmd = p.cmdline.trim();
    if cmd.is_empty() {
        return if p.exe_path.is_empty() { "(sem acesso)".to_string() } else { p.exe_path.clone() };
    }
    let rest = if let Some(stripped) = cmd.strip_prefix('"') {
        match stripped.find('"') {
            Some(i) => &stripped[i + 1..],
            None => cmd,
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

fn fmt_age(secs: u64) -> String {
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
    let _ = std::process::Command::new("cmd").args(["/c", "start", "", url]).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

fn open_in_explorer(path: &str) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer.exe").arg(format!("/select,{path}")).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").args(["-R", path]).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(dir) = std::path::Path::new(path).parent() {
            let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
        }
    }
}
