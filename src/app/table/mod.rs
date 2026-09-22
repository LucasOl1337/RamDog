//! Pure process-table model. No egui context, sampling, or process actions.
//!
//! `RowCache` owns frame transitions. `ProcessQuery` reads a snapshot and applies
//! the same filters/order for row construction and snapshot reconciliation.

mod cache;
mod query;

use crate::categories::Category;

pub(super) use cache::{RowCache, RowCacheAction};
pub(super) use query::{metric_of, ProcessQuery, ResourceFilter};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SortKey {
    Name,
    Ram,
    Cat,
    Pid,
    Cpu,
    Gpu,
    Vram,
    Disk,
    Age,
    Parent,
    State,
    /// Sobra / loop / CPU barata primeiro; depois % de CPU. É o que a faixa "Disputa" liga.
    Steal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Row {
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
    /// propósito — as somas são recalculadas a cada amostra, inclusive quando a ordem
    /// está congelada porque o mouse está em cima da tabela.
    AppHeader { gi: usize },
    /// Linha sintética: memória em uso que não pertence a processo nenhum.
    /// Sem PID, sem kill — existe para a soma da lista bater com o "em uso" do topo.
    System { kind: SysRow, bytes: u64 },
}

/// As parcelas do "em uso" que nunca aparecem numa lista de processos.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SysRow {
    PagedPool,
    NonPagedPool,
    SharedAndCache,
}

#[cfg(test)]
mod tests;
