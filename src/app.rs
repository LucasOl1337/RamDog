//! Interface egui: lista / árvore / categorias, detalhes, kill, lock.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::time::Instant;

use egui::{Align, Color32, Layout, Rect, RichText, Stroke, StrokeKind, TextureHandle, Vec2};
use egui_extras::{Column, TableBuilder, TableRow};

use crate::categories::{self, classify, is_critical, Category};
use crate::config::{Config, ViewMode};
use crate::drains::{DrainOut, Drains};
use crate::hwtemp::HwTemp;
use crate::metrics::SysSample;
use crate::procs::{self, MemStatus, ProcInfo};
use crate::sampler::{self, SamplerHandle};

const MB: u64 = 1024 * 1024;
const GB: u64 = 1024 * 1024 * 1024;
const ROW_H: f32 = 24.0;
const ICON: f32 = 16.0;
/// Altura das barrinhas dos medidores do topo (CPU/RAM/GPU/Disco) — uma só constante pras
/// quatro pra elas ficarem realmente alinhadas, não só "parecidas".
const TOP_BAR_H: f32 = 8.0;
/// Altura alocada pra linha inteira do topo (rótulo + medidores + controles à direita).
/// Precisa ser explícita: sem isso o bloco de botões à direita centraliza contra a altura do
/// painel inteiro, não a da linha, e sai visualmente desalinhado dos medidores.
const TOP_ROW_H: f32 = 34.0;

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
}

struct KillReq {
    pids: Vec<(u32, String, u64)>,
    title: String,
    tree: bool,
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
    last_sample: Option<Instant>,
    sample_ms: f32,
    sys: SysSample,
    gpu_per_proc: bool,
    hwtemp: HwTemp,

    icons: HashMap<String, Option<TextureHandle>>,

    search: String,
    sort: SortKey,
    sort_desc: bool,
    cat_enabled: HashSet<Category>,
    selected: Option<u32>,
    selected_keep: Option<(ProcInfo, Category)>,
    expanded: HashSet<u32>,
    collapsed_cats: HashSet<Category>,
    pending: Option<KillReq>,
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
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_style(&cc.egui_ctx);
        let cfg = Config::load();
        let is_admin = procs::is_admin();
        let sampler = sampler::spawn(cc.egui_ctx.clone(), cfg.refresh_ms, is_admin);
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
            last_sample: None,
            sample_ms: 0.0,
            sys: SysSample::default(),
            gpu_per_proc: true,
            hwtemp: HwTemp::default(),
            icons: HashMap::new(),
            search: String::new(),
            sort: SortKey::Ram,
            sort_desc: true,
            cat_enabled: Category::ALL.iter().copied().collect(),
            selected: None,
            selected_keep: None,
            expanded: HashSet::new(),
            collapsed_cats: HashSet::new(),
            pending: None,
            status: None,
            is_admin,
            scroll_to_selected: false,
            cached_rows: None,
            cached_key: 0,
            rows_dirty: true,
            table_rect: None,
            order_frozen: false,
            drains: Drains::new(),
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
        self.last_sample = Some(snap.taken);
        self.sample_ms = snap.sample_ms;
        self.sys = snap.sys;
        self.gpu_per_proc = snap.gpu_per_proc;
        self.hwtemp = snap.hwtemp;
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
        let own = self.by_pid.get(&pid).map(|&i| self.procs[i].private_ws).unwrap_or(0);
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
        if p.private_ws < self.cfg.min_mb as u64 * MB {
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
                        pa.private_ws.cmp(&pb.private_ws)
                    }
                }
                SortKey::Cat => self.cat(*a).cmp(&self.cat(*b)).then(pb.private_ws.cmp(&pa.private_ws)),
                SortKey::Pid => pa.pid.cmp(&pb.pid),
                SortKey::Cpu => pa.cpu_pct.partial_cmp(&pb.cpu_pct).unwrap_or(std::cmp::Ordering::Equal),
                SortKey::Gpu => pa.gpu_pct.partial_cmp(&pb.gpu_pct).unwrap_or(std::cmp::Ordering::Equal),
                SortKey::Disk => pa.disk_bps.partial_cmp(&pb.disk_bps).unwrap_or(std::cmp::Ordering::Equal),
                SortKey::Age => pb.create_time.cmp(&pa.create_time),
                SortKey::Parent => {
                    let na = self.proc(pa.ppid).map(|p| p.name_lower.as_str()).unwrap_or("");
                    let nb = self.proc(pb.ppid).map(|p| p.name_lower.as_str()).unwrap_or("");
                    na.cmp(nb).then(pb.private_ws.cmp(&pa.private_ws))
                }
            };
            if desc { ord.reverse() } else { ord }
        });
    }

    fn build_rows(&self) -> Vec<Row> {
        let search = self.search.trim().to_lowercase();
        let hits: Vec<u32> = self.procs.iter().filter(|p| self.passes(p, &search)).map(|p| p.pid).collect();
        match self.cfg.view {
            ViewMode::List | ViewMode::Drains => {
                let mut pids = hits;
                self.sort_pids(&mut pids, false);
                pids.into_iter()
                    .map(|pid| Row::Proc { pid, depth: 0, has_children: false, expanded: false, dim: false })
                    .collect()
            }
            ViewMode::Category => {
                let mut groups: HashMap<Category, Vec<u32>> = HashMap::new();
                for pid in hits {
                    groups.entry(self.cat(pid)).or_default().push(pid);
                }
                let mut cats: Vec<(Category, u64, Vec<u32>)> = groups
                    .into_iter()
                    .map(|(c, pids)| {
                        let total = pids.iter().map(|p| self.proc(*p).map(|x| x.private_ws).unwrap_or(0)).sum();
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
                let mut rows = Vec::new();
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
        h.finish()
    }

    fn rows_for_frame(&mut self, ctx: &egui::Context) -> Vec<Row> {
        let hovering = match (self.table_rect, ctx.pointer_latest_pos()) {
            (Some(r), Some(p)) => r.contains(p) && ctx.input(|i| i.pointer.has_pointer()),
            _ => false,
        };
        let key = self.rows_key();
        let can_freeze = hovering && !self.rows_dirty && key == self.cached_key && self.pending.is_none();
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
                let rows = self.cached_rows.as_mut().unwrap();
                rows.retain(|r| match r {
                    Row::Proc { pid, .. } => keep.contains(pid),
                    Row::CatHeader { .. } => true,
                });
                self.order_frozen = true;
                return rows.clone();
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

    fn request_kill(&mut self, pid: u32, tree: bool) {
        let Some(p) = self.proc(pid).cloned() else { return };
        let mut pids = vec![];
        let mut skipped_locked = 0;
        if self.is_locked(&p) {
            self.toast(format!("{} está protegido (lock)", p.name), true);
            return;
        }
        pids.push((p.pid, p.name.clone(), p.private_ws));
        if tree {
            for d in self.descendants(pid) {
                if let Some(c) = self.proc(d) {
                    if self.is_locked(c) {
                        skipped_locked += 1;
                    } else {
                        pids.push((c.pid, c.name.clone(), c.private_ws));
                    }
                }
            }
        }
        let total: u64 = pids.iter().map(|x| x.2).sum();
        let title = if tree {
            format!(
                "Finalizar árvore de {} (PID {}): {} processo(s), {}{}",
                p.name,
                p.pid,
                pids.len(),
                fmt_bytes(total),
                if skipped_locked > 0 { format!(" — {skipped_locked} protegido(s) serão poupados") } else { String::new() }
            )
        } else {
            format!("Finalizar {} (PID {}, {})?", p.name, p.pid, fmt_bytes(p.private_ws))
        };
        let req = KillReq { pids, title, tree };
        if self.cfg.confirm_kill {
            self.pending = Some(req);
        } else {
            self.execute_kill(req);
        }
    }

    fn execute_kill(&mut self, req: KillReq) {
        let mut ok = 0;
        let mut freed = 0u64;
        let mut errs: Vec<String> = Vec::new();
        for (pid, name, ram) in &req.pids {
            match procs::kill(*pid) {
                Ok(()) => {
                    ok += 1;
                    freed += ram;
                }
                Err(e) => errs.push(format!("{name} ({pid}): {e}")),
            }
        }
        self.sampler.force.store(true, Ordering::Relaxed);
        if errs.is_empty() {
            self.toast(
                format!("{} processo(s) finalizado(s), ~{} liberados", ok, fmt_bytes(freed)),
                false,
            );
        } else {
            let mut msg = format!("{ok} ok, {} falha(s): {}", errs.len(), errs[0]);
            if errs.len() > 1 {
                msg.push_str(&format!(" (+{})", errs.len() - 1));
            }
            self.toast(msg, true);
        }
        let _ = req.tree;
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

    // ---------- UI ----------

    /// Totais de RAM por categoria (usado no medidor do topo e nos chips de filtro).
    fn cat_totals(&self) -> HashMap<Category, (u64, usize)> {
        let mut totals: HashMap<Category, (u64, usize)> = HashMap::new();
        for p in &self.procs {
            let e = totals.entry(self.cat(p.pid)).or_default();
            e.0 += p.private_ws;
            e.1 += 1;
        }
        totals
    }

    /// Medidor empilhado: mostra *para onde* foi a RAM, não só quanto sobrou.
    /// Cada faixa é uma categoria (mesma cor dos chips); o cinza no fim é o que o
    /// kernel/drivers usam e não aparece como processo.
    fn ram_gauge(&self, ui: &mut egui::Ui, totals: &HashMap<Category, (u64, usize)>, width: f32) {
        let used = self.mem.used_phys();
        let total = self.mem.total_phys.max(1);
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, TOP_BAR_H), egui::Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, 3.0, Color32::from_rgb(30, 34, 41));
        let scale = rect.width() / total as f32;
        let mut segs: Vec<(Category, u64)> = totals
            .iter()
            .map(|(c, (t, _))| (*c, *t))
            .filter(|(_, t)| *t > 0)
            .collect();
        segs.sort_by_key(|(_, t)| std::cmp::Reverse(*t));
        let mut x = rect.left();
        for (c, t) in &segs {
            let w = *t as f32 * scale;
            if w < 0.5 {
                continue;
            }
            let seg = Rect::from_min_size(egui::pos2(x, rect.top() + 1.0), Vec2::new(w, rect.height() - 2.0));
            p.rect_filled(seg, 0.0, c.color().gamma_multiply(0.85));
            x += w;
        }
        // Resto do "em uso" que não é processo do usuário (kernel, drivers, cache não paginável).
        let used_x = rect.left() + used as f32 * scale;
        if used_x > x + 0.5 {
            let seg = Rect::from_min_max(egui::pos2(x, rect.top() + 1.0), egui::pos2(used_x.min(rect.right()), rect.bottom() - 1.0));
            p.rect_filled(seg, 0.0, Color32::from_rgb(74, 82, 95));
        }
        p.rect_stroke(rect, 3.0, Stroke::new(1.0_f32, LINE), StrokeKind::Inside);
        let mut tip = format!("{} em uso de {}\n", fmt_gb(used), fmt_gb(total));
        for (c, t) in &segs {
            tip.push_str(&format!("\n{}  {}", c.label(), fmt_bytes_short(*t)));
        }
        let proc_sum: u64 = segs.iter().map(|(_, t)| *t).sum();
        tip.push_str(&format!("\n\nKernel / drivers / cache  {}", fmt_bytes_short(used.saturating_sub(proc_sum))));
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

    /// Um medidor compacto: rótulo, barra fininha, percentual, temperatura opcional — mesma
    /// linguagem visual para CPU/GPU/Disco. `None` vira "–" cinza com o tooltip explicando por
    /// quê, nunca um 0 falso.
    fn mini_meter(ui: &mut egui::Ui, label: &str, pct: Option<f32>, temp_c: Option<u32>, width: f32, unavailable_tip: &str) -> egui::Response {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(label).color(MUTED).size(10.5));
                if let Some(p) = pct {
                    ui.label(num(Self::fmt_pct(p)).size(12.5).strong().color(Self::load_color(p / 100.0)));
                } else {
                    ui.label(RichText::new("–").color(Color32::from_gray(90)).size(12.5));
                }
                if let Some(t) = temp_c {
                    ui.label(RichText::new(format!("{t}°C")).size(11.0).strong().color(Self::temp_color(t)));
                }
            });
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
            resp
        })
        .inner
        .on_hover_text(if pct.is_some() { label.to_string() } else { unavailable_tip.to_string() })
    }

    fn ui_top(&mut self, ui: &mut egui::Ui) {
        let totals = self.cat_totals();
        ui.add_space(6.0);
        // Linha 1: os quatro medidores — CPU, RAM (com detalhe por categoria), GPU, Disco.
        // É a razão de ser desta versão: nunca mais abrir o Gerenciador de Tarefas para ver
        // "quem está pesando" — CPU e GPU já respondem isso de cara.
        // Altura fixa alocada explicitamente: sem isso o bloco da direita (botões) centraliza
        // contra a altura do painel inteiro em vez da altura da própria linha, e some
        // desalinhado dos medidores.
        let row_w = ui.available_width();
        ui.allocate_ui_with_layout(Vec2::new(row_w, TOP_ROW_H), Layout::left_to_right(Align::Center), |ui| {
            ui.label(RichText::new("RamDog").strong().size(16.0).color(MUTED));
            ui.add_space(10.0);

            let cpu_temp = self.hwtemp.cpu_temp.map(|t| t.round() as u32);
            Self::mini_meter(ui, "CPU", self.sys.cpu_pct, cpu_temp, 90.0, if self.is_admin {
                "Temperatura indisponível: hwtemp.exe não achado ao lado do ramdog.exe, ou placa-mãe sem sensor suportado."
            } else {
                "Temperatura de CPU precisa do RamDog rodando como admin (acesso a hardware, sem API pública do Windows)."
            });
            ui.add_space(14.0);

            let used = self.mem.used_phys();
            let total = self.mem.total_phys.max(1);
            let frac = used as f32 / total as f32;
            let color = Self::load_color(frac);
            let ram_temp = self.hwtemp.ram_max();
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("RAM").color(MUTED).size(10.5));
                    ui.label(num(format!("{:.0}%", frac * 100.0)).size(12.5).strong().color(color));
                    ui.label(RichText::new(format!("{} / {}", fmt_gb(used), fmt_gb(total))).color(MUTED).size(10.5));
                    if let Some(t) = ram_temp {
                        ui.label(RichText::new(format!("{:.0}°C", t)).size(11.0).strong().color(Self::temp_color(t.round() as u32)));
                    }
                });
                self.ram_gauge(ui, &totals, 200.0);
            });
            ui.add_space(14.0);

            if let Some(g) = self.sys.gpu.clone() {
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("GPU").color(MUTED).size(10.5));
                        match g.util_pct {
                            Some(p) => {
                                ui.label(num(Self::fmt_pct(p)).size(12.5).strong().color(Self::load_color(p / 100.0)));
                            }
                            None => {
                                ui.label(RichText::new("–").color(Color32::from_gray(90)).size(12.5));
                            }
                        }
                        if let Some(t) = g.temp_c {
                            ui.label(RichText::new(format!("{t}°C")).size(11.0).strong().color(Self::temp_color(t)));
                        }
                    });
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(90.0, TOP_BAR_H), egui::Sense::hover());
                    let p = ui.painter();
                    p.rect_filled(rect, 2.0, Color32::from_rgb(30, 34, 41));
                    if let Some(pct) = g.util_pct {
                        let frac = (pct / 100.0).clamp(0.0, 1.0);
                        if frac > 0.01 {
                            let bar = Rect::from_min_size(rect.min, Vec2::new(rect.width() * frac, rect.height()));
                            p.rect_filled(bar, 2.0, Self::load_color(frac));
                        }
                    }
                    let mut tip = g.name.clone();
                    if g.mem_total > 0 {
                        tip.push_str(&format!("\nVRAM: {} / {}", fmt_gb(g.mem_used), fmt_gb(g.mem_total)));
                    }
                    if let Some(w) = g.power_w {
                        tip.push_str(&format!("\nPotência: {w:.0} W"));
                    }
                    if let Some(f) = g.fan_pct {
                        tip.push_str(&format!("\nCooler: {f}%"));
                    }
                    resp.on_hover_text(tip);
                });
            } else {
                Self::mini_meter(ui, "GPU", None, None, 90.0, "Sem GPU NVIDIA detectada (nvml.dll não carregou) — sem essa leitura em placas AMD/Intel aqui ainda.");
            }
            ui.add_space(14.0);

            // Disco: % sozinho fica ilegível num SSD NVMe rápido (fica quase sempre <1%, e
            // arredondado sem casa decimal parece travado em "0%") — junto do throughput real
            // (bytes/s) fica claro que o contador está vivo, só que o disco está mesmo ocioso.
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("DISCO").color(MUTED).size(10.5));
                    match self.sys.disk_pct {
                        Some(p) => {
                            ui.label(num(Self::fmt_pct(p)).size(12.5).strong().color(Self::load_color(p / 100.0)));
                        }
                        None => {
                            ui.label(RichText::new("–").color(Color32::from_gray(90)).size(12.5));
                        }
                    }
                    if let Some(bps) = self.sys.disk_bps {
                        if bps >= 1024.0 {
                            ui.label(RichText::new(fmt_bps(bps)).color(MUTED).size(10.5));
                        }
                    }
                });
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(90.0, TOP_BAR_H), egui::Sense::hover());
                let p = ui.painter();
                p.rect_filled(rect, 2.0, Color32::from_rgb(30, 34, 41));
                if let Some(pct) = self.sys.disk_pct {
                    let frac = (pct / 100.0).clamp(0.0, 1.0);
                    if frac > 0.01 {
                        let bar = Rect::from_min_size(rect.min, Vec2::new(rect.width() * frac, rect.height()));
                        p.rect_filled(bar, 2.0, Self::load_color(frac));
                    }
                }
                resp.on_hover_text(if self.sys.disk_pct.is_some() {
                    "% de tempo ocupado do disco (todos os volumes) — igual ao Gerenciador de Tarefas".to_string()
                } else {
                    "Contador de disco indisponível neste host.".to_string()
                });
            });

            ui.allocate_ui_with_layout(Vec2::new(ui.available_width(), TOP_ROW_H), Layout::right_to_left(Align::Center), |ui| {
                if self.is_admin {
                    ui.label(RichText::new("ADMIN").color(Color32::from_rgb(90, 220, 130)).strong())
                        .on_hover_text("Rodando elevado: pode encerrar processos de outros usuários/serviços");
                } else if ui
                    .button("Reabrir como admin")
                    .on_hover_text("Necessário para encerrar serviços e processos de outros usuários")
                    .clicked()
                {
                    self.relaunch_as_admin();
                }
                ui.separator();
                let mut paused = self.sampler.paused.load(Ordering::Relaxed);
                if ui.selectable_label(paused, if paused { "▶ Retomar" } else { "⏸ Pausar" }).clicked() {
                    paused = !paused;
                    self.sampler.paused.store(paused, Ordering::Relaxed);
                }
                let mut iv = self.cfg.refresh_ms;
                egui::ComboBox::from_id_salt("refresh")
                    .selected_text(format!("{:.1}s", iv as f32 / 1000.0))
                    .width(64.0)
                    .show_ui(ui, |ui| {
                        for v in [500u64, 1000, 2000, 5000] {
                            ui.selectable_value(&mut iv, v, format!("{:.1}s", v as f32 / 1000.0));
                        }
                    });
                if iv != self.cfg.refresh_ms {
                    self.cfg.refresh_ms = iv;
                    self.sampler.interval_ms.store(iv, Ordering::Relaxed);
                    self.cfg_dirty = true;
                }
                ui.label(RichText::new("Atualizar").weak());
                let mut confirm = self.cfg.confirm_kill;
                if ui.checkbox(&mut confirm, "Confirmar kill").changed() {
                    self.cfg.confirm_kill = confirm;
                    self.cfg_dirty = true;
                }
            });
        });
        ui.add_space(3.0);
        ui.horizontal(|ui| {
            ui.add_space(46.0);
            ui.label(
                RichText::new(format!("compromisso {} / {}", fmt_gb(self.mem.used_commit()), fmt_gb(self.mem.total_commit)))
                    .color(MUTED)
                    .size(10.5),
            )
            .on_hover_text("Memória confirmada (RAM + arquivo de paginação)");
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let te = egui::TextEdit::singleline(&mut self.search)
                .hint_text("Buscar nome, PID, caminho ou comando…")
                .desired_width(240.0);
            let resp = ui.add(te);
            if resp.changed() {
                self.scroll_to_selected = false;
            }
            if !self.search.is_empty() && ui.small_button("✖").on_hover_text("Limpar busca").clicked() {
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
                    for (v, label, tip) in [
                        (ViewMode::List, "Lista", "Todos os processos, um por linha"),
                        (ViewMode::Tree, "Árvore", "Pai → filhos, com a RAM da subárvore"),
                        (ViewMode::Category, "Categorias", "Agrupado por categoria"),
                        (ViewMode::Drains, "Ralos", "Defender, serviços dispensáveis, apps de sistema e inicialização"),
                    ] {
                        let on = view == v;
                        let t = RichText::new(label)
                            .size(12.5)
                            .color(if on { Color32::WHITE } else { MUTED });
                        let b = if on {
                            egui::Button::new(t).fill(ACCENT_BG).stroke(Stroke::new(1.0_f32, ACCENT.gamma_multiply(0.7)))
                        } else {
                            egui::Button::new(t).stroke(Stroke::NONE)
                        }
                        .corner_radius(4.0);
                        if ui.add(b).on_hover_text(tip).clicked() {
                            view = v;
                        }
                    }
                });
            if view != self.cfg.view {
                self.cfg.view = view;
                self.cfg_dirty = true;
            }
            if self.cfg.view == ViewMode::Tree {
                if ui.small_button("Expandir tudo").clicked() {
                    self.expanded = self.children.keys().copied().collect();
                }
                if ui.small_button("Recolher").clicked() {
                    self.expanded.clear();
                }
            }
            ui.add_space(4.0);
            ui.label(RichText::new("mín.").color(MUTED).size(12.0));
            let mut min_mb = self.cfg.min_mb;
            if ui
                .add(egui::DragValue::new(&mut min_mb).range(0..=4096).speed(5).suffix(" MB"))
                .on_hover_text("Ocultar processos com menos RAM privada que isto")
                .changed()
            {
                self.cfg.min_mb = min_mb;
                self.cfg_dirty = true;
            }
        });
        // Chips em linha própria: antes eles vazavam para uma terceira linha e sobrava
        // "Sistema / Outros" órfãos embaixo.
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
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
            if self.cat_enabled.len() != Category::ALL.len() && ui.small_button("todas").clicked() {
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
        let mut click_select: Option<u32> = None;
        let mut toggle_expand: Option<u32> = None;
        let mut toggle_cat: Option<Category> = None;
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
                        self.subtree.get(pid).copied().or_else(|| self.proc(*pid).map(|p| p.private_ws))
                    } else {
                        self.proc(*pid).map(|p| p.private_ws)
                    }
                }
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
            .filter_map(|r| match r { Row::Proc { pid, .. } => self.proc(*pid).map(|p| p.gpu_pct), _ => None })
            .fold(1.0_f32, f32::max);
        let max_disk: f64 = rows
            .iter()
            .filter_map(|r| match r { Row::Proc { pid, .. } => self.proc(*pid).map(|p| p.disk_bps), _ => None })
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
                header.col(|ui| self.header_btn_right(ui, SortKey::Ram, if tree { "RAM (árvore)" } else { "RAM" }));
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
                                    (self.subtree.get(&pid).copied().unwrap_or(p.private_ws), p.private_ws)
                                } else {
                                    (p.private_ws, p.private_ws)
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
                                        r.on_hover_text(format!("Próprio: {}", fmt_bytes(own)));
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
                                    ui.label(num(if p.cpu_pct < 0.05 { "–".to_string() } else { format!("{:.1}%", p.cpu_pct) }).color(c));
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
                                let s = cmd_args(&p);
                                let full = if p.cmdline.is_empty() { p.exe_path.clone() } else { p.cmdline.clone() };
                                let r = ui.add(egui::Label::new(RichText::new(&s).color(MUTED).size(11.5)).truncate());
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

    fn ui_details(&mut self, ui: &mut egui::Ui) {
        let Some(sel) = self.selected else {
            // Estado vazio útil: em vez de só instruir, já responde "quem está comendo minha RAM".
            let mut top: Vec<&ProcInfo> = self.procs.iter().collect();
            top.sort_by_key(|p| std::cmp::Reverse(p.private_ws));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                ui.label(RichText::new("Maiores agora").color(MUTED).size(11.5));
                for p in top.iter().take(4) {
                    let cat = self.cat(p.pid);
                    ui.add_space(6.0);
                    ui.label(RichText::new("●").color(cat.color()).size(10.0));
                    ui.label(RichText::new(&p.name).size(12.0));
                    ui.label(num(fmt_bytes(p.private_ws)).color(ram_color(p.private_ws, MUTED)));
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
        ui.add_space(2.0);
        egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
            egui::Grid::new("details_grid").num_columns(2).spacing([10.0, 3.0]).show(ui, |ui| {
                ui.label(RichText::new("Origem").weak());
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let chain = self.ancestry(p.pid);
                    if chain.is_empty() {
                        if p.raw_ppid != 0 {
                            ui.label(RichText::new(format!("pai (PID {}) já encerrado", p.raw_ppid)).weak());
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
                ui.add(egui::Label::new(RichText::new(if p.exe_path.is_empty() { "(sem acesso)" } else { &p.exe_path }).monospace().small()).wrap());
                ui.end_row();

                ui.label(RichText::new("Comando").weak());
                ui.add(egui::Label::new(RichText::new(if p.cmdline.is_empty() { "(sem acesso)" } else { &p.cmdline }).monospace().small()).wrap());
                ui.end_row();

                ui.label(RichText::new("Memória").weak());
                let sub = self.subtree.get(&p.pid).copied().unwrap_or(p.private_ws);
                let n = self.subtree_count.get(&p.pid).copied().unwrap_or(1);
                let mut s = format!(
                    "privada {}   ·   working set {}   ·   commit {}",
                    fmt_bytes(p.private_ws),
                    fmt_bytes(p.working_set),
                    fmt_bytes(p.commit)
                );
                if n > 1 {
                    s.push_str(&format!("   ·   árvore: {} em {} processos", fmt_bytes(sub), n));
                }
                ui.label(s);
                ui.end_row();

                ui.label(RichText::new("Execução").weak());
                ui.label(format!(
                    "iniciado há {}   ·   CPU {:.1}%   ·   {} threads   ·   {} handles   ·   sessão {}",
                    fmt_age(secs),
                    p.cpu_pct,
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

    fn ui_modal(&mut self, ctx: &egui::Context) {
        let Some(req) = self.pending.as_ref() else { return };
        let title = req.title.clone();
        let list: Vec<String> = req.pids.iter().take(12).map(|(pid, name, ram)| format!("{name}  ({pid})  {}", fmt_bytes(*ram))).collect();
        let more = req.pids.len().saturating_sub(12);
        let mut confirm = false;
        let mut cancel = false;
        let modal = egui::Modal::new(egui::Id::new("kill_modal")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.heading("Confirmar encerramento");
            ui.add_space(6.0);
            ui.label(&title);
            if list.len() > 1 {
                ui.add_space(4.0);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    for l in &list {
                        ui.label(RichText::new(l).monospace().small());
                    }
                    if more > 0 {
                        ui.label(RichText::new(format!("… e mais {more}")).weak());
                    }
                });
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(RichText::new("Finalizar").color(Color32::WHITE)).fill(Color32::from_rgb(170, 50, 50))).clicked() {
                    confirm = true;
                }
                if ui.button("Cancelar").clicked() {
                    cancel = true;
                }
                ui.label(RichText::new("Enter confirma · Esc cancela").weak().small());
            });
        });
        if modal.should_close() {
            cancel = true;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
            confirm = true;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            cancel = true;
        }
        if confirm {
            if let Some(req) = self.pending.take() {
                self.execute_kill(req);
            }
        } else if cancel {
            self.pending = None;
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

        // atalhos
        if self.pending.is_none() {
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
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| self.ui_top(ui));
        if self.cfg.view != ViewMode::Drains {
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
                if self.order_frozen {
                    ui.separator();
                    ui.label(RichText::new("ordem congelada (mouse sobre a tabela)").weak().small())
                        .on_hover_text("Enquanto o mouse está sobre a tabela a ordem das linhas não muda, para você não clicar no processo errado. Valores continuam atualizando.");
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new("Del: finalizar · Shift+Del: árvore · F5: atualizar · botão direito: menu").weak().small());
                });
            });
        });
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(&ctx.style()).inner_margin(egui::Margin::symmetric(6, 2)))
            .show(ctx, |ui| {
                if self.cfg.view == ViewMode::Drains {
                    let is_admin = self.is_admin;
                    let procs = std::mem::take(&mut self.procs);
                    let evs = self.drains.ui(ui, &procs, is_admin);
                    self.procs = procs;
                    for ev in evs {
                        match ev {
                            DrainOut::Toast(m, err) => self.toast(m, err),
                            DrainOut::Kill(pids) => {
                                let list: Vec<(u32, String, u64)> = pids
                                    .iter()
                                    .filter_map(|pid| self.proc(*pid).map(|p| (p.pid, p.name.clone(), p.private_ws)))
                                    .collect();
                                if !list.is_empty() {
                                    let title = format!("Finalizar {} processo(s)?", list.len());
                                    let req = KillReq { pids: list, title, tree: false };
                                    if self.cfg.confirm_kill { self.pending = Some(req); } else { self.execute_kill(req); }
                                }
                            }
                        }
                    }
                } else {
                    self.ui_table(ui);
                }
            });

        self.ui_modal(ctx);
        self.ui_status(ctx);

        if self.cfg_dirty {
            self.cfg_dirty = false;
            if let Err(e) = self.cfg.save() {
                self.toast(format!("Falha ao salvar config: {e}"), true);
            }
        }
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
    let _ = std::process::Command::new("cmd").args(["/c", "start", "", url]).spawn();
}

fn open_in_explorer(path: &str) {
    let _ = std::process::Command::new("explorer.exe").arg(format!("/select,{path}")).spawn();
}
