//! Stateful row-cache protocol, independent of pointer coordinates and drawing.

use super::Row;
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum RowCacheAction {
    Rebuild,
    ReconcileSnapshot,
    Reuse,
}

/// Owns both cached rows and the invalidation state. A snapshot may update values
/// while hover protects order. Leaving hover applies the deferred order once.
#[derive(Default)]
pub(in crate::app) struct RowCache {
    rows: Option<Vec<Row>>,
    key: u64,
    snapshot_dirty: bool,
    rows_dirty: bool,
    shown_count: usize,
    order_frozen: bool,
}

impl RowCache {
    pub(in crate::app) fn snapshot_changed(&mut self) {
        self.snapshot_dirty = true;
    }

    pub(in crate::app) fn invalidate(&mut self) {
        self.rows_dirty = true;
    }

    pub(in crate::app) fn thaw(&mut self) {
        self.order_frozen = false;
    }

    pub(in crate::app) fn clear(&mut self) {
        self.rows = None;
        self.invalidate();
        self.thaw();
    }

    pub(in crate::app) fn shown_count(&self) -> usize {
        self.shown_count
    }

    pub(in crate::app) fn order_frozen(&self) -> bool {
        self.order_frozen
    }

    pub(in crate::app) fn action(&self, key: u64, hovering: bool) -> RowCacheAction {
        if self.rows.is_none() || self.rows_dirty || key != self.key {
            RowCacheAction::Rebuild
        } else if self.snapshot_dirty {
            if hovering {
                RowCacheAction::ReconcileSnapshot
            } else {
                RowCacheAction::Rebuild
            }
        } else if self.order_frozen && !hovering {
            RowCacheAction::Rebuild
        } else {
            RowCacheAction::Reuse
        }
    }

    pub(in crate::app) fn rows(&self) -> Vec<Row> {
        self.rows.clone().unwrap_or_default()
    }

    pub(in crate::app) fn rebuilt(&mut self, key: u64, rows: Vec<Row>, shown_count: usize) {
        self.rows = Some(rows);
        self.key = key;
        self.shown_count = shown_count;
        self.snapshot_dirty = false;
        self.rows_dirty = false;
        self.order_frozen = false;
    }

    /// Keep positions, remove dead/filtered processes, and reduce groups that have
    /// lost members. New processes wait until the next rebuild. Group membership
    /// and totals are refreshed by the caller before supplying this read-only view.
    pub(in crate::app) fn reconcile<'a>(
        &mut self,
        keep: &HashSet<u32>,
        shown_count: usize,
        group_pids: impl Fn(usize) -> Option<&'a [u32]>,
    ) {
        let rows = self.rows.take().expect("reconcile requires cached rows");
        self.rows = Some(
            rows.into_iter()
                .flat_map(|r| match r {
                    Row::Proc { pid, .. } if !keep.contains(&pid) => Vec::new(),
                    Row::AppHeader { gi } => match group_pids(gi) {
                        Some([pid]) => vec![Row::Proc {
                            pid: *pid,
                            depth: 0,
                            has_children: false,
                            expanded: false,
                            dim: false,
                        }],
                        Some(pids) if pids.len() > 1 => vec![Row::AppHeader { gi }],
                        _ => Vec::new(),
                    },
                    other => vec![other],
                })
                .collect(),
        );
        self.shown_count = shown_count;
        self.order_frozen = true;
        self.snapshot_dirty = false;
    }
}
