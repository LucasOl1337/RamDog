use std::collections::{HashMap, HashSet};

use super::*;
use crate::config::MemMetric;
use crate::procs::ProcInfo;

const MB: u64 = 1024 * 1024;

fn process(pid: u32, name: &str, ram_mb: u64) -> ProcInfo {
    ProcInfo {
        pid,
        name: name.into(),
        name_lower: name.to_lowercase(),
        private_ws: ram_mb * MB,
        ..ProcInfo::default()
    }
}

fn row(pid: u32) -> Row {
    Row::Proc {
        pid,
        depth: 0,
        has_children: false,
        expanded: false,
        dim: false,
    }
}

struct Snapshot {
    procs: Vec<ProcInfo>,
    by_pid: HashMap<u32, usize>,
    cats: HashMap<u32, Category>,
    subtree: HashMap<u32, u64>,
    subtree_cpu: HashMap<u32, f32>,
    enabled: HashSet<Category>,
}

impl Snapshot {
    fn new(procs: Vec<ProcInfo>) -> Self {
        Self {
            by_pid: procs.iter().enumerate().map(|(i, p)| (p.pid, i)).collect(),
            procs,
            cats: HashMap::new(),
            subtree: HashMap::new(),
            subtree_cpu: HashMap::new(),
            enabled: Category::ALL.into_iter().collect(),
        }
    }

    fn query(&self) -> ProcessQuery<'_> {
        ProcessQuery {
            procs: &self.procs,
            by_pid: &self.by_pid,
            cats: &self.cats,
            subtree: &self.subtree,
            subtree_cpu: &self.subtree_cpu,
            cat_enabled: &self.enabled,
            metric: MemMetric::Private,
            resources: ResourceFilter::default(),
            grouped: false,
        }
    }
}

fn cached() -> RowCache {
    let mut cache = RowCache::default();
    cache.rebuilt(7, vec![row(1), row(2)], 2);
    cache
}

#[test]
fn stable_visual_frame_reuses_cached_rows_even_outside_the_table() {
    let cache = cached();
    for hovering in [false, true, false] {
        assert_eq!(cache.action(7, hovering), RowCacheAction::Reuse);
        assert_eq!(cache.rows(), vec![row(1), row(2)]);
    }
}

#[test]
fn new_snapshot_is_reconciled_once_while_hovering() {
    let mut cache = cached();
    cache.snapshot_changed();
    assert_eq!(cache.action(7, true), RowCacheAction::ReconcileSnapshot);
    cache.reconcile(&HashSet::from([1, 2]), 2, |_| None);
    assert_eq!(cache.action(7, true), RowCacheAction::Reuse);
    assert!(cache.order_frozen());
}

#[test]
fn new_snapshot_reorders_once_when_not_hovering() {
    let mut cache = cached();
    cache.snapshot_changed();
    assert_eq!(cache.action(7, false), RowCacheAction::Rebuild);
    cache.rebuilt(7, vec![row(2), row(1)], 2);
    assert_eq!(cache.action(7, false), RowCacheAction::Reuse);
    assert!(!cache.order_frozen());
    assert_eq!(cache.rows(), vec![row(2), row(1)]);
}

#[test]
fn leaving_a_frozen_table_applies_the_pending_order_once() {
    let mut cache = cached();
    cache.snapshot_changed();
    cache.reconcile(&HashSet::from([1, 2]), 2, |_| None);
    assert_eq!(cache.action(7, false), RowCacheAction::Rebuild);
    cache.rebuilt(7, vec![row(2), row(1)], 2);
    assert_eq!(cache.action(7, false), RowCacheAction::Reuse);
}

#[test]
fn hover_without_new_data_is_a_cache_hit() {
    let mut cache = cached();
    cache.snapshot_changed();
    cache.reconcile(&HashSet::from([1, 2]), 2, |_| None);
    for _ in 0..10 {
        assert_eq!(cache.action(7, true), RowCacheAction::Reuse);
    }
}

#[test]
fn thaw_clear_and_invalidate_preserve_their_distinct_protocols() {
    let mut cache = cached();
    cache.reconcile(&HashSet::from([1, 2]), 2, |_| None);
    cache.thaw();
    assert!(!cache.order_frozen());
    assert_eq!(cache.action(7, false), RowCacheAction::Reuse);
    cache.invalidate();
    cache.thaw();
    assert_eq!(cache.action(7, true), RowCacheAction::Rebuild);
    cache.rebuilt(7, vec![row(2)], 1);
    cache.reconcile(&HashSet::from([2]), 1, |_| None);
    assert!(cache.order_frozen());
    cache.clear();
    assert!(!cache.order_frozen());
    assert!(cache.rows().is_empty());
    assert_eq!(cache.action(7, true), RowCacheAction::Rebuild);
    // Clear historically retains the shown counter until the next rebuild.
    assert_eq!(cache.shown_count(), 1);
}

#[test]
fn group_membership_two_to_one_to_zero_is_reconciled_across_snapshots() {
    let mut cache = RowCache::default();
    cache.rebuilt(7, vec![Row::AppHeader { gi: 0 }], 2);
    for members in [vec![1, 2], vec![2], vec![]] {
        cache.snapshot_changed();
        assert_eq!(cache.action(7, true), RowCacheAction::ReconcileSnapshot);
        cache.reconcile(&members.iter().copied().collect(), members.len(), |_| {
            Some(members.as_slice())
        });
        let expected = match members.len() {
            2 => vec![Row::AppHeader { gi: 0 }],
            1 => vec![row(2)],
            _ => vec![],
        };
        assert_eq!(cache.rows(), expected);
        assert_eq!(cache.action(7, true), RowCacheAction::Reuse);
    }
}

#[test]
fn initial_cache_and_explicit_changes_override_hover_and_pending_snapshots() {
    assert_eq!(RowCache::default().action(0, true), RowCacheAction::Rebuild);
    let mut cache = cached();
    cache.snapshot_changed();
    assert_eq!(cache.action(8, true), RowCacheAction::Rebuild);
    cache.invalidate();
    assert_eq!(cache.action(7, true), RowCacheAction::Rebuild);
    cache.rebuilt(8, vec![], 0);
    assert_eq!(cache.action(8, true), RowCacheAction::Reuse);
    cache.clear();
    assert_eq!(cache.action(8, true), RowCacheAction::Rebuild);
    assert!(!cache.order_frozen());
    assert!(cache.rows().is_empty());
}

#[test]
fn repeated_snapshots_remove_rows_without_adding_or_reordering_under_hover() {
    let mut cache = cached();
    cache.snapshot_changed();
    cache.reconcile(&HashSet::from([2, 3]), 2, |_| None);
    assert_eq!(cache.rows(), vec![row(2)]);
    assert_eq!(cache.shown_count(), 2); // includes a new hit not inserted until thaw
    cache.snapshot_changed();
    assert_eq!(cache.action(7, true), RowCacheAction::ReconcileSnapshot);
    cache.reconcile(&HashSet::from([3]), 1, |_| None);
    assert!(cache.rows().is_empty());
    assert_eq!(cache.shown_count(), 1);
    assert_eq!(cache.action(7, true), RowCacheAction::Reuse);
    assert_eq!(cache.action(7, false), RowCacheAction::Rebuild);
}

#[test]
fn reconcile_preserves_tree_metadata_and_non_process_rows() {
    let ancestor = Row::Proc {
        pid: 1,
        depth: 3,
        has_children: true,
        expanded: true,
        dim: true,
    };
    let category = Row::CatHeader {
        cat: Category::Dev,
        count: 2,
        total: 123,
        collapsed: false,
    };
    let system = Row::System {
        kind: SysRow::PagedPool,
        bytes: 321,
    };
    let mut cache = RowCache::default();
    cache.rebuilt(7, vec![system, category, ancestor, row(2)], 1);
    cache.snapshot_changed();
    // Ancestors are retained by App even when only their child matches the filter.
    cache.reconcile(&HashSet::from([1, 2]), 1, |_| None);
    assert_eq!(cache.rows(), vec![system, category, ancestor, row(2)]);
    assert_eq!(cache.shown_count(), 1);
}

#[test]
fn reconcile_converts_single_member_groups_and_drops_empty_or_missing_groups() {
    let mut cache = RowCache::default();
    cache.rebuilt(7, (0..4).map(|gi| Row::AppHeader { gi }).collect(), 5);
    let groups = [vec![8, 9], vec![10], vec![]];
    cache.reconcile(&HashSet::from([8, 9, 10]), 3, |gi| {
        groups.get(gi).map(Vec::as_slice)
    });
    assert_eq!(cache.rows(), vec![Row::AppHeader { gi: 0 }, row(10)]);
    assert_eq!(cache.shown_count(), 3);
}

#[test]
fn filters_use_inclusive_thresholds_and_reject_unknown_gpu_when_required() {
    let snap = Snapshot::new(vec![]);
    let mut query = snap.query();
    query.resources = ResourceFilter {
        min_mb: 10,
        min_cpu: 3.0,
        min_gpu: 2.0,
        min_vram_mb: 4,
    };
    assert!(query.passes_resources(10 * MB, 3.0, Some(2.0), Some(4 * MB)));
    assert!(!query.passes_resources(10 * MB - 1, 3.0, Some(2.0), Some(4 * MB)));
    assert!(!query.passes_resources(10 * MB, 2.99, Some(2.0), Some(4 * MB)));
    assert!(!query.passes_resources(10 * MB, 3.0, None, Some(4 * MB)));
    assert!(!query.passes_resources(10 * MB, 3.0, Some(1.99), Some(4 * MB)));
    assert!(!query.passes_resources(10 * MB, 3.0, Some(2.0), None));
    assert!(!query.passes_resources(10 * MB, 3.0, Some(2.0), Some(4 * MB - 1)));
    query.resources = ResourceFilter::default();
    assert!(query.passes_resources(0, 0.0, None, None));
}

#[test]
fn grouped_queries_defer_resource_thresholds_but_keep_category_and_search_filters() {
    let mut snap = Snapshot::new(vec![process(1, "worker", 4), process(2, "helper", 4)]);
    snap.cats.insert(1, Category::Dev);
    snap.cats.insert(2, Category::Other);
    snap.enabled = HashSet::from([Category::Dev]);
    let mut query = snap.query();
    query.resources.min_mb = 8;
    assert!(query.matching_pids("").is_empty());
    query.grouped = true;
    assert_eq!(query.matching_pids(" WORKER "), vec![1]);
    assert_eq!(query.count_matches(" WORKER "), 1);
    assert!(query.matching_pids("helper").is_empty());
    assert!(!query.passes_resources(4 * MB, 0.0, None, None));
    assert!(query.passes_resources(8 * MB, 0.0, None, None));
}

#[test]
fn search_matches_each_source_and_only_exact_pid() {
    let mut p = process(1234, "worker", 0);
    p.exe_path = "/opt/Vendor/tool".into();
    p.cmdline = "worker --Project=Alpha".into();
    p.launcher.host = Some("TerminalHost".into());
    p.window_title = Some("Unpublished document".into());
    let label = crate::identity::of(&p).label;
    let snap = Snapshot::new(vec![p]);
    for search in [
        "WORKER",
        "vendor",
        "project=alpha",
        "terminalhost",
        "unpublished",
        "1234",
        label.as_str(),
    ] {
        assert_eq!(snap.query().matching_pids(search), vec![1234], "{search}");
    }
    assert!(snap.query().matching_pids("123").is_empty());
    assert!(snap.query().matching_pids("absent").is_empty());
}

#[test]
fn unknown_categories_default_to_other_and_can_be_disabled() {
    let mut snap = Snapshot::new(vec![process(1, "worker", 1)]);
    assert_eq!(snap.query().matching_pids(""), vec![1]);
    snap.enabled.remove(&Category::Other);
    assert!(snap.query().matching_pids("").is_empty());
}

#[test]
fn selected_memory_metric_drives_filtering_and_sorting() {
    let mut p = process(1, "one", 4);
    p.working_set = 8 * MB;
    p.commit = 16 * MB;
    let mut q = process(2, "two", 6);
    q.working_set = 7 * MB;
    q.commit = 12 * MB;
    let snap = Snapshot::new(vec![p, q]);
    for (metric, values, expected) in [
        (MemMetric::Private, [4, 6], vec![2, 1]),
        (MemMetric::WorkingSet, [8, 7], vec![1, 2]),
        (MemMetric::Commit, [16, 12], vec![1, 2]),
    ] {
        let mut query = snap.query();
        query.metric = metric;
        assert_eq!(metric_of(metric, &snap.procs[0]), values[0] * MB);
        assert_eq!(metric_of(metric, &snap.procs[1]), values[1] * MB);
        let mut pids = vec![1, 2];
        query.sort_pids(&mut pids, false, SortKey::Ram, true, |_| 0);
        assert_eq!(pids, expected);
        query.resources.min_mb = 8;
        assert_eq!(
            query.count_matches(""),
            if metric == MemMetric::Private {
                0
            } else if metric == MemMetric::WorkingSet {
                1
            } else {
                2
            }
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn proportional_memory_keeps_unavailable_distinct_from_measured_values() {
    let mut p = process(1, "one", 9);
    assert_eq!(metric_of(MemMetric::Proportional, &p), 0);
    p.linux_memory = Some((3 * MB, 5 * MB));
    assert_eq!(metric_of(MemMetric::Proportional, &p), 5 * MB);
}

#[test]
fn every_sort_column_preserves_ascending_and_descending_semantics() {
    let mut a = process(10, "alpha", 1);
    let mut b = process(20, "beta", 2);
    a.cpu_pct = 1.0;
    b.cpu_pct = 2.0;
    a.gpu_load = Some(1.0);
    b.gpu_load = Some(2.0);
    a.gpu_vram = Some(1);
    b.gpu_vram = Some(2);
    a.disk_bps = 1.0;
    b.disk_bps = 2.0;
    a.create_time = 200;
    b.create_time = 100;
    b.focused = true;
    a.ppid = 10;
    b.ppid = 20;
    let mut snap = Snapshot::new(vec![b, a]);
    snap.cats.insert(10, Category::Ai);
    snap.cats.insert(20, Category::Dev);
    for key in [
        SortKey::Name,
        SortKey::Ram,
        SortKey::Cat,
        SortKey::Pid,
        SortKey::Cpu,
        SortKey::Gpu,
        SortKey::Vram,
        SortKey::Disk,
        SortKey::Age,
        SortKey::Parent,
        SortKey::State,
        SortKey::Steal,
    ] {
        for desc in [false, true] {
            let mut pids = vec![20, 10];
            snap.query().sort_pids(
                &mut pids,
                false,
                key,
                desc,
                |p| if p.pid == 20 { 2 } else { 1 },
            );
            assert_eq!(
                pids,
                if desc { vec![20, 10] } else { vec![10, 20] },
                "{key:?}, desc={desc}"
            );
        }
    }
}

#[test]
fn tied_numeric_columns_keep_pid_ascending_even_when_descending() {
    let snap = Snapshot::new(vec![process(2, "same", 0), process(1, "same", 0)]);
    for key in [
        SortKey::Ram,
        SortKey::Cat,
        SortKey::Cpu,
        SortKey::Gpu,
        SortKey::Vram,
        SortKey::Disk,
        SortKey::Age,
        SortKey::Parent,
        SortKey::State,
        SortKey::Steal,
    ] {
        let mut pids = vec![2, 1];
        snap.query().sort_pids(&mut pids, false, key, true, |_| 0);
        assert_eq!(pids, vec![1, 2], "{key:?}");
    }
    // Name deliberately includes PID before reversal, unlike numeric columns.
    let mut pids = vec![1, 2];
    snap.query()
        .sort_pids(&mut pids, false, SortKey::Name, true, |_| 0);
    assert_eq!(pids, vec![2, 1]);
}

#[test]
fn cpu_sort_credits_exited_children_but_tree_uses_subtree_totals() {
    let mut a = process(1, "shell", 1);
    a.cpu_pct = 1.0;
    a.cpu_children_pct = 10.0;
    let mut b = process(2, "worker", 2);
    b.cpu_pct = 5.0;
    let mut snap = Snapshot::new(vec![a, b]);
    snap.subtree_cpu.insert(2, 20.0);
    snap.subtree.insert(1, 100 * MB);
    for (key, tree, expected) in [
        (SortKey::Cpu, false, vec![1, 2]),
        (SortKey::Cpu, true, vec![2, 1]),
        (SortKey::Ram, false, vec![2, 1]),
        (SortKey::Ram, true, vec![1, 2]),
    ] {
        let mut pids = vec![1, 2];
        snap.query().sort_pids(&mut pids, tree, key, true, |_| 0);
        assert_eq!(pids, expected);
    }
}

#[test]
fn unknown_gpu_sorts_below_measured_zero_and_nan_falls_back_to_pid() {
    let mut a = process(1, "one", 0);
    let mut b = process(2, "two", 0);
    a.cpu_pct = f32::NAN;
    a.disk_bps = f64::NAN;
    b.gpu_load = Some(0.0);
    let snap = Snapshot::new(vec![b, a]);
    let mut pids = vec![1, 2];
    snap.query()
        .sort_pids(&mut pids, false, SortKey::Gpu, true, |_| 0);
    assert_eq!(pids, vec![2, 1]);
    for key in [SortKey::Cpu, SortKey::Disk] {
        snap.query().sort_pids(&mut pids, false, key, true, |_| 0);
        assert_eq!(pids, vec![1, 2]);
    }
}

#[test]
fn snapshot_query_and_cache_preserve_hover_then_apply_filter_and_new_order() {
    let old = Snapshot::new(vec![process(1, "one", 30), process(2, "two", 20)]);
    let mut query = old.query();
    query.resources.min_mb = 10;
    let mut pids = query.matching_pids("");
    query.sort_pids(&mut pids, false, SortKey::Ram, true, |_| 0);
    let mut cache = RowCache::default();
    cache.rebuilt(
        7,
        pids.into_iter().map(row).collect(),
        query.count_matches(""),
    );
    let new = Snapshot::new(vec![
        process(1, "one", 5),
        process(2, "two", 40),
        process(3, "three", 50),
    ]);
    let mut query = new.query();
    query.resources.min_mb = 10;
    cache.snapshot_changed();
    assert_eq!(cache.action(7, true), RowCacheAction::ReconcileSnapshot);
    let keep: HashSet<_> = query.matching_pids("").into_iter().collect();
    cache.reconcile(&keep, keep.len(), |_| None);
    assert_eq!(cache.rows(), vec![row(2)]);
    assert_eq!(cache.action(7, true), RowCacheAction::Reuse);
    assert_eq!(cache.action(7, false), RowCacheAction::Rebuild);
    let mut pids = query.matching_pids("");
    query.sort_pids(&mut pids, false, SortKey::Ram, true, |_| 0);
    cache.rebuilt(
        7,
        pids.into_iter().map(row).collect(),
        query.count_matches(""),
    );
    assert_eq!(cache.rows(), vec![row(3), row(2)]);
    assert_eq!(cache.shown_count(), 2);
    assert_eq!(cache.action(7, false), RowCacheAction::Reuse);
    // An explicit search/filter key change is immediate, even with the pointer inside.
    assert_eq!(cache.action(8, true), RowCacheAction::Rebuild);
    cache.rebuilt(
        8,
        query.matching_pids("three").into_iter().map(row).collect(),
        1,
    );
    assert_eq!(cache.rows(), vec![row(3)]);
}
