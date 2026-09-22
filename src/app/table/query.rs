//! Snapshot queries. The caller supplies indices, category decisions and steal
//! ranking, so this module never collects data, reads a clock or changes the OS.

use super::SortKey;
use crate::categories::Category;
use crate::config::MemMetric;
use crate::identity;
use crate::procs::ProcInfo;
use std::collections::{HashMap, HashSet};

const MB: u64 = 1024 * 1024;

#[derive(Clone, Copy, Default)]
pub(in crate::app) struct ResourceFilter {
    pub min_mb: u32,
    pub min_cpu: f32,
    pub min_gpu: f32,
    pub min_vram_mb: u32,
}

/// Borrowed, already indexed snapshot. All fields are read-only query inputs.
pub(in crate::app) struct ProcessQuery<'a> {
    pub procs: &'a [ProcInfo],
    pub by_pid: &'a HashMap<u32, usize>,
    pub cats: &'a HashMap<u32, Category>,
    pub subtree: &'a HashMap<u32, u64>,
    pub subtree_cpu: &'a HashMap<u32, f32>,
    pub cat_enabled: &'a HashSet<Category>,
    pub metric: MemMetric,
    pub resources: ResourceFilter,
    pub grouped: bool,
}

impl ProcessQuery<'_> {
    fn proc(&self, pid: u32) -> Option<&ProcInfo> {
        self.by_pid.get(&pid).map(|&i| &self.procs[i])
    }

    fn cat(&self, pid: u32) -> Category {
        self.cats.get(&pid).copied().unwrap_or(Category::Other)
    }

    fn mem_of(&self, p: &ProcInfo) -> u64 {
        metric_of(self.metric, p)
    }

    /// Search normalization belongs here so rebuild and reconciliation agree.
    pub(in crate::app) fn matching_pids(&self, search: &str) -> Vec<u32> {
        let search = search.trim().to_lowercase();
        self.procs
            .iter()
            .filter(|p| self.passes(p, &search))
            .map(|p| p.pid)
            .collect()
    }

    pub(in crate::app) fn count_matches(&self, search: &str) -> usize {
        let search = search.trim().to_lowercase();
        self.procs
            .iter()
            .filter(|p| self.passes(p, &search))
            .count()
    }

    pub(in crate::app) fn passes_resources(
        &self,
        ram: u64,
        cpu: f32,
        gpu: Option<f32>,
        vram: Option<u64>,
    ) -> bool {
        if ram < self.resources.min_mb as u64 * MB {
            return false;
        }
        if self.resources.min_cpu > 0.0 && cpu < self.resources.min_cpu {
            return false;
        }
        if self.resources.min_gpu > 0.0 && gpu.unwrap_or(0.0) < self.resources.min_gpu {
            return false;
        }
        if self.resources.min_vram_mb > 0
            && vram.unwrap_or(0) < self.resources.min_vram_mb as u64 * MB
        {
            return false;
        }
        true
    }

    fn passes(&self, p: &ProcInfo, search: &str) -> bool {
        if !self.cat_enabled.contains(&self.cat(p.pid)) {
            return false;
        }
        if !self.grouped
            && !self.passes_resources(self.mem_of(p), p.cpu_pct, p.gpu_load, p.gpu_vram)
        {
            return false;
        }
        if !search.is_empty() {
            let pid_s = p.pid.to_string();
            let id = identity::of(p);
            let hit = p.name_lower.contains(search)
                || id.label.to_lowercase().contains(search)
                || pid_s == search
                || p.exe_path.to_lowercase().contains(search)
                || p.cmdline.to_lowercase().contains(search)
                || p.launcher.short().to_lowercase().contains(search)
                || p.window_title
                    .as_deref()
                    .map(|t| t.to_lowercase().contains(search))
                    .unwrap_or(false);
            if !hit {
                return false;
            }
        }
        true
    }

    pub(in crate::app) fn sort_pids(
        &self,
        pids: &mut [u32],
        tree: bool,
        key: SortKey,
        desc: bool,
        steal_rank: impl Fn(&ProcInfo) -> u8,
    ) {
        pids.sort_by(|a, b| {
            let (pa, pb) = match (self.proc(*a), self.proc(*b)) {
                (Some(x), Some(y)) => (x, y),
                _ => return std::cmp::Ordering::Equal,
            };
            let ord = match key {
                SortKey::Name => pa.name_lower.cmp(&pb.name_lower).then(pa.pid.cmp(&pb.pid)),
                SortKey::Ram => {
                    if tree {
                        self.subtree
                            .get(a)
                            .unwrap_or(&0)
                            .cmp(self.subtree.get(b).unwrap_or(&0))
                    } else {
                        self.mem_of(pa).cmp(&self.mem_of(pb))
                    }
                }
                SortKey::Cat => self
                    .cat(*a)
                    .cmp(&self.cat(*b))
                    .then(self.mem_of(pb).cmp(&self.mem_of(pa))),
                SortKey::Pid => pa.pid.cmp(&pb.pid),
                SortKey::Cpu => {
                    // Na Árvore compara a subárvore, como a RAM: o pai sobe junto com
                    // os filhos que estão comendo núcleo.
                    let (ca, cb) = if tree {
                        (
                            self.subtree_cpu.get(a).copied().unwrap_or(pa.cpu_pct),
                            self.subtree_cpu.get(b).copied().unwrap_or(pb.cpu_pct),
                        )
                    } else {
                        // Filhos já encerrados contam: o shell que disparou dez `rg` de
                        // 0,3 s é quem explica o CPU, e sem isso ele ficava no fim da lista.
                        (
                            pa.cpu_pct + pa.cpu_children_pct,
                            pb.cpu_pct + pb.cpu_children_pct,
                        )
                    };
                    ca.partial_cmp(&cb).unwrap_or(std::cmp::Ordering::Equal)
                }
                SortKey::Gpu => pa
                    .gpu_load
                    .unwrap_or(-1.0)
                    .partial_cmp(&pb.gpu_load.unwrap_or(-1.0))
                    .unwrap_or(std::cmp::Ordering::Equal),
                SortKey::Vram => pa.gpu_vram.unwrap_or(0).cmp(&pb.gpu_vram.unwrap_or(0)),
                SortKey::State => {
                    let sa = (pa.focused, pa.has_window, pa.kernel_state == Some('Z'));
                    let sb = (pb.focused, pb.has_window, pb.kernel_state == Some('Z'));
                    sa.cmp(&sb)
                }
                SortKey::Disk => pa
                    .disk_bps
                    .partial_cmp(&pb.disk_bps)
                    .unwrap_or(std::cmp::Ordering::Equal),
                SortKey::Age => pb.create_time.cmp(&pa.create_time),
                SortKey::Parent => {
                    let na = self
                        .proc(pa.ppid)
                        .map(|p| p.name_lower.as_str())
                        .unwrap_or("");
                    let nb = self
                        .proc(pb.ppid)
                        .map(|p| p.name_lower.as_str())
                        .unwrap_or("");
                    na.cmp(nb).then(self.mem_of(pb).cmp(&self.mem_of(pa)))
                }
                SortKey::Steal => steal_rank(pa).cmp(&steal_rank(pb)).then(
                    pa.cpu_pct
                        .partial_cmp(&pb.cpu_pct)
                        .unwrap_or(std::cmp::Ordering::Equal),
                ),
            };
            // Desempate por PID depois da inversão. Sem ele, as dezenas de processos
            // empatados em "–" na coluna CPU trocavam de lugar a cada amostra e a lista
            // inteira piscava embaixo do que você estava tentando ler.
            let ord = if desc { ord.reverse() } else { ord };
            ord.then(pa.pid.cmp(&pb.pid))
        });
    }
}

pub(in crate::app) fn metric_of(m: MemMetric, p: &ProcInfo) -> u64 {
    match m {
        #[cfg(target_os = "linux")]
        MemMetric::Proportional => p.linux_memory.map(|m| m.1).unwrap_or(0),
        MemMetric::WorkingSet => p.working_set,
        MemMetric::Private => p.private_ws,
        MemMetric::Commit => p.commit,
    }
}
