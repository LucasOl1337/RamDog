//! Pure process-table model. No egui context, sampling, or process actions.
//!
//! `RowCache` owns frame transitions. `ProcessQuery` reads a snapshot and applies
//! the same filters/order for row construction and snapshot reconciliation.

mod cache;
mod query;

use crate::categories::Category;
use std::collections::HashSet;

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
    /// Linha sintética da coluna CPU: o que o medidor do topo cobrou e nenhuma outra linha
    /// mostra (processo que nasceu e morreu entre duas amostras, kernel sem PID) mais os
    /// processos que nasceram com a ordem congelada. O valor é lido na hora de desenhar,
    /// então acompanha a amostra mesmo com o mouse em cima da tabela.
    CpuGap,
}

/// Põe `row` antes da primeira linha de topo que ela vence em `value`. Só linha de topo
/// (`value_of` devolve `Some`) é fronteira: filho de app aberto fica com o cabeçalho.
pub(super) fn insert_by_value(
    rows: &mut Vec<Row>,
    row: Row,
    value: f32,
    desc: bool,
    value_of: impl Fn(&Row) -> Option<f32>,
) {
    let at = rows
        .iter()
        .position(|r| value_of(r).is_some_and(|v| if desc { value > v } else { value < v }))
        .unwrap_or(rows.len());
    rows.insert(at, row);
}

/// Processos que passam no filtro agora e não existiam na última montagem da lista. Com a
/// ordem congelada eles não ganham linha; sem esta conta somem da tabela até o mouse sair.
pub(super) fn born_since_rebuild(keep: &HashSet<u32>, at_rebuild: &HashSet<u32>) -> Vec<u32> {
    let mut out: Vec<u32> = keep.difference(at_rebuild).copied().collect();
    out.sort_unstable();
    out
}

/// As parcelas do "em uso" que nunca aparecem numa lista de processos.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SysRow {
    PagedPool,
    NonPagedPool,
    /// Linux: arquivos em tmpfs (`/tmp`, `/dev/shm`), residentes ou já comprimidos no zram.
    RamFiles,
    /// Linux: parte do zram que guarda páginas de processos trocadas.
    ZramSwap,
    /// Linux: slab não-recuperável, pilhas, tabelas de página, vmalloc.
    LinuxKernel,
    SharedAndCache,
}

#[cfg(test)]
mod tests;
