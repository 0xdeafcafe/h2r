//! Guest code the lift left out (docs/lifter/lift-pruning.md, "Boot
//! safety"): the module's pruned table, held by the dispatch tables. A
//! transfer nothing resolves that lands in one of its ranges stops with
//! [`Event::Pruned`](crate::Event::Pruned), naming the replaced entry the
//! range hangs from, instead of the generic missing-handler fault. The
//! entry fallback never gets to interpret it.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// One run of left-out code: `[start, end)`, owned by the replaced entry
/// `root` (0 when no replaced entry reaches it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub start: u32,
    pub end: u32,
    pub root: u32,
}

/// What a lookup found: the replaced entry and its point's name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub root: u32,
    pub point: Arc<str>,
}

/// The table: ranges sorted and disjoint, and each root's point name.
#[derive(Debug, Clone, Default)]
pub struct Pruned {
    ranges: Vec<Range>,
    points: Vec<(u32, Arc<str>)>,
}

impl Pruned {
    /// `ranges` in any order; overlaps keep the first range's root.
    /// `points`: each replaced entry's point name.
    pub fn new(mut ranges: Vec<Range>, points: impl IntoIterator<Item = (u32, String)>) -> Pruned {
        ranges.retain(|r| r.start < r.end);
        ranges.sort_by_key(|r| (r.start, r.end));
        let mut out: Vec<Range> = Vec::with_capacity(ranges.len());
        for mut r in ranges {
            if let Some(last) = out.last() {
                r.start = r.start.max(last.end);
            }
            if r.start < r.end {
                out.push(r);
            }
        }
        let mut points: Vec<(u32, Arc<str>)> = points.into_iter().map(|(e, p)| (e, Arc::from(p))).collect();
        points.sort_by_key(|p| p.0);
        points.dedup_by_key(|p| p.0);
        Pruned { ranges: out, points }
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// The range holding `pc`, if any.
    pub fn at(&self, pc: u32) -> Option<Hit> {
        let i = self.ranges.partition_point(|r| r.end <= pc);
        let r = self.ranges.get(i).filter(|r| r.start <= pc)?;
        let point = match self.points.binary_search_by_key(&r.root, |p| p.0) {
            Ok(k) => self.points[k].1.clone(),
            Err(_) => Arc::from("no replaced entry reaches it"),
        };
        Some(Hit { root: r.root, point })
    }
}

/// Transfers into left-out code, process-wide (`lift_pruned_hits`).
static HITS: AtomicU64 = AtomicU64::new(0);

/// How many transfers have stopped in left-out code.
pub fn hits() -> u64 {
    HITS.load(Ordering::Relaxed)
}

/// Count one: the runner's and the boot interpreter's stops call it.
pub fn count_hit() {
    HITS.fetch_add(1, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lookup_finds_the_range_and_names_its_root() {
        let p = Pruned::new(
            vec![
                Range { start: 0x2000, end: 0x2010, root: 0x2000 },
                Range { start: 0x1000, end: 0x1008, root: 0x2000 },
                // Overlaps the first: the earlier range keeps its bytes.
                Range { start: 0x2008, end: 0x2020, root: 0 },
            ],
            [(0x2000, "B".to_owned())],
        );
        assert_eq!(p.at(0x0fff), None);
        assert_eq!(p.at(0x1000).map(|h| (h.root, h.point.to_string())), Some((0x2000, "B".into())));
        assert_eq!(p.at(0x1008), None);
        assert_eq!(p.at(0x200f).map(|h| h.root), Some(0x2000));
        assert_eq!(p.at(0x2010).map(|h| h.root), Some(0));
        assert_eq!(p.at(0x2020), None);
        assert!(p.at(0x2010).unwrap().point.contains("no replaced entry"));
    }
}
