//! Leaving replaced guest code out of the lift (docs/lifter/lift-pruning.md):
//! the replaced list the composition root registers, the keep set both
//! tiers' builds lift, and the pruned table a loaded module carries.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use anyhow::Result;
use emit::pruned::{Replaced, Table};

/// What the composition root says the lift may leave out.
pub type ReplacedFn = fn(&BTreeSet<u32>) -> Replaced;

static REPLACED: OnceLock<ReplacedFn> = OnceLock::new();

/// Register the replaced list (`boot::registration::lift_replaced`), as
/// [`crate::register_retail_entries`] registers the instruction entries.
/// Idempotent; with none registered the build lifts everything.
pub fn register_lift_replaced(f: ReplacedFn) {
    let _ = REPLACED.set(f);
}

/// A pruned lift's plan: the keep set and what it was cut by.
pub struct Plan {
    pub keep: analyze::prune::Keep,
    pub replaced: Replaced,
}

/// The plan for `analysis`, or `None` to lift everything: `H2_LIFT_PRUNE=0`,
/// no list registered, or nothing replaced.
pub fn plan(x: &xbe::Xbe, analysis: &analyze::Analysis) -> Result<Option<Plan>> {
    if !emit::pruned::enabled().map_err(anyhow::Error::msg)? {
        return Ok(None);
    }
    let Some(f) = REPLACED.get() else { return Ok(None) };
    let entries: BTreeSet<u32> = analysis.functions.keys().copied().collect();
    let replaced = f(&entries);
    if replaced.entries.is_empty() {
        return Ok(None);
    }
    let keep = replaced.keep(x, analysis);
    Ok(Some(Plan { keep, replaced }))
}

impl Plan {
    /// The module's table, once `owners` are planned from the kept entries.
    pub fn table(&self, x: &xbe::Xbe, analysis: &analyze::Analysis, owners: &[emit::Owner]) -> Table {
        Table::new(x, analysis, &self.keep, &self.replaced, owners)
    }

    /// One line for the build log.
    pub fn summary(&self, table: &Table) -> String {
        let bytes: u64 = self.keep.dropped.values().map(|d| u64::from(d.bytes)).sum();
        format!(
            "lift pruning: {} replaced, {} functions ({:.1} KB) left out in {} ranges, digest {:016x}",
            table.roots.len(),
            self.keep.dropped.len(),
            bytes as f64 / 1e3,
            table.ranges.len(),
            self.keep.digest
        )
    }
}

/// The runtime's form of a module's table.
pub fn runtime_table(table: &Table) -> runtime::pruned::Pruned {
    runtime::pruned::Pruned::new(
        table
            .ranges
            .iter()
            .map(|r| runtime::pruned::Range { start: r.start, end: r.end, root: r.root })
            .collect(),
        table.roots.iter().map(|r| (r.entry, r.point.clone())),
    )
}
