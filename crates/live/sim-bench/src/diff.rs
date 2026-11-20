//! Which state differs first between two sim reports (docs/runtime/sim.md,
//! "Finding the first differing state").
//!
//! A report's `state_components_v1` is a per-tick breakdown of the state
//! behind the aggregate `state_hashes_v1` digests: named components (a
//! player's position, the RNG seed, a 64 KiB block of the checkpoint
//! arena), each as a value string. The first recorded tick lists every
//! component; later ticks list only those that changed, so a long match
//! stays small. [`diff`] replays two such series and names the components
//! that differ at the first differing tick. Reports without the series fall
//! back to the aggregate digests.
//!
//! Pure: nothing here reads the game.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::metrics::Report;

/// The checkpoint arena is hashed in blocks of this many guest bytes.
pub const ARENA_BLOCK: u32 = 0x10000;
/// The checkpoint arena's size, for the last block's address range.
pub const ARENA_LEN: u32 = 0x3f_e000;
/// Prefix of the arena block components (`arena.07`).
pub const ARENA_PREFIX: &str = "arena.";
/// A component's value once it has gone away (a dead player's pose).
pub const ABSENT: &str = "-";
/// Recorded component values across a whole report, a bound on its size. A
/// match that outgrows it keeps its first ticks (where a divergence is
/// cheapest to find) and says where the series stops.
pub const MAX_ENTRIES: usize = 250_000;

/// The component series in a report.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateComponents {
    /// Guest address of arena block 0, when the arena was read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arena_base: Option<u32>,
    /// Set once [`MAX_ENTRIES`] was reached: the last tick recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated_after: Option<u32>,
    pub ticks: Vec<ComponentTick>,
}

/// One tick's changed components, name to value. The first row of a series
/// has them all.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentTick {
    pub tick: u32,
    pub changed: BTreeMap<String, String>,
}

/// Builds a [`StateComponents`] tick by tick, storing deltas.
#[derive(Clone, Debug, Default)]
pub struct Recorder {
    current: BTreeMap<String, String>,
    /// The previous values of what the last recorded tick changed, to
    /// replace that tick when it is observed again.
    undo: Vec<(String, Option<String>)>,
    out: StateComponents,
    entries: usize,
}

impl Recorder {
    /// Record `components` for `tick`. The same tick again replaces the
    /// last row; the caller restarts the recorder on a rewind.
    pub fn record(
        &mut self,
        tick: u32,
        arena_base: Option<u32>,
        components: impl IntoIterator<Item = (String, String)>,
    ) {
        if self.out.truncated_after.is_some() {
            return;
        }
        if self.out.ticks.last().is_some_and(|t| t.tick == tick) {
            for (name, old) in self.undo.drain(..) {
                match old {
                    Some(v) => self.current.insert(name, v),
                    None => self.current.remove(&name),
                };
            }
            if let Some(t) = self.out.ticks.pop() {
                self.entries -= t.changed.len();
            }
        }
        let next: BTreeMap<String, String> = components.into_iter().collect();
        let mut changed = BTreeMap::new();
        let mut undo = Vec::new();
        for (name, value) in &next {
            if self.current.get(name) != Some(value) {
                changed.insert(name.clone(), value.clone());
                undo.push((name.clone(), self.current.get(name).cloned()));
            }
        }
        for (name, old) in &self.current {
            if !next.contains_key(name) {
                changed.insert(name.clone(), ABSENT.into());
                undo.push((name.clone(), Some(old.clone())));
            }
        }
        if self.entries + changed.len() > MAX_ENTRIES {
            self.out.truncated_after = self.out.ticks.last().map(|t| t.tick);
            return;
        }
        for (name, _) in &undo {
            match next.get(name) {
                Some(v) => self.current.insert(name.clone(), v.clone()),
                None => self.current.remove(name),
            };
        }
        self.undo = undo;
        self.entries += changed.len();
        if arena_base.is_some() {
            self.out.arena_base = arena_base;
        }
        self.out.ticks.push(ComponentTick { tick, changed });
    }

    /// The series, or `None` when nothing was recorded.
    pub fn finish(&self) -> Option<StateComponents> {
        (!self.out.ticks.is_empty()).then(|| self.out.clone())
    }
}

/// One component that differs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Difference {
    pub component: String,
    pub a: Option<String>,
    pub b: Option<String>,
}

/// Where two component series first part.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Divergence {
    pub tick: u32,
    /// Sorted by component name.
    pub differences: Vec<Difference>,
    pub arena_base_a: Option<u32>,
    pub arena_base_b: Option<u32>,
}

/// An aggregate-only divergence: the first tick whose digests differ.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AggregateDivergence {
    pub tick: u32,
    pub gameplay: (Option<String>, Option<String>),
    pub arena: (Option<String>, Option<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Every recorded tick and component agrees.
    Identical { ticks: usize },
    /// Identical as far as one or both series go before being cut short.
    IdenticalTruncated { checked_through: Option<u32> },
    Components(Divergence),
    /// A report lacks the breakdown: today's aggregate comparison.
    Aggregate(AggregateDivergence),
    AggregateIdentical { ticks: usize },
    /// Neither the breakdown nor digests can be compared.
    Unverifiable(String),
}

/// Compare two reports: the component series when both carry one, else
/// the aggregate digests.
pub fn diff(a: &Report, b: &Report) -> Verdict {
    if let (Some(ca), Some(cb)) = (&a.state_components_v1, &b.state_components_v1) {
        return components(ca, cb);
    }
    aggregate(a, b)
}

fn components(a: &StateComponents, b: &StateComponents) -> Verdict {
    let mut state_a: BTreeMap<&str, &str> = BTreeMap::new();
    let mut state_b: BTreeMap<&str, &str> = BTreeMap::new();
    let (mut ia, mut ib) = (a.ticks.iter().peekable(), b.ticks.iter().peekable());
    let mut seen = 0;
    loop {
        let (ta, tb) = (ia.peek().map(|t| t.tick), ib.peek().map(|t| t.tick));
        let tick = match (ta, tb) {
            (None, None) => break,
            (Some(x), None) | (None, Some(x)) => x,
            (Some(x), Some(y)) => x.min(y),
        };
        let ra = (ta == Some(tick)).then(|| ia.next()).flatten();
        let rb = (tb == Some(tick)).then(|| ib.next()).flatten();
        let (Some(ra), Some(rb)) = (ra, rb) else {
            // One run never observed this tick (a gap or an early end).
            let a_has = ra.is_some();
            let cut = if a_has { b } else { a };
            if cut.truncated_after.is_some_and(|t| tick > t) {
                return Verdict::IdenticalTruncated {
                    checked_through: cut.truncated_after,
                };
            }
            let seen = |has: bool| Some(if has { "observed" } else { "missing" }.to_string());
            return Verdict::Components(Divergence {
                tick,
                differences: vec![Difference {
                    component: "tick".into(),
                    a: seen(a_has),
                    b: seen(!a_has),
                }],
                arena_base_a: a.arena_base,
                arena_base_b: b.arena_base,
            });
        };
        let mut touched = BTreeSet::new();
        for (row, state) in [(ra, &mut state_a), (rb, &mut state_b)] {
            for (name, value) in &row.changed {
                touched.insert(name.as_str());
                if value == ABSENT {
                    state.remove(name.as_str());
                } else {
                    state.insert(name, value);
                }
            }
        }
        let differences: Vec<_> = touched
            .into_iter()
            .filter(|n| state_a.get(n) != state_b.get(n))
            .map(|n| Difference {
                component: n.to_string(),
                a: state_a.get(n).map(|s| s.to_string()),
                b: state_b.get(n).map(|s| s.to_string()),
            })
            .collect();
        if !differences.is_empty() {
            return Verdict::Components(Divergence {
                tick,
                differences,
                arena_base_a: a.arena_base,
                arena_base_b: b.arena_base,
            });
        }
        seen += 1;
    }
    if a.truncated_after.is_some() || b.truncated_after.is_some() {
        return Verdict::IdenticalTruncated {
            checked_through: a.truncated_after.into_iter().chain(b.truncated_after).min(),
        };
    }
    Verdict::Identical { ticks: seen }
}

fn aggregate(a: &Report, b: &Report) -> Verdict {
    if a.state_hashes_v1.is_empty() || b.state_hashes_v1.is_empty() {
        return Verdict::Unverifiable(
            "a report has neither state_components_v1 nor state_hashes_v1".into(),
        );
    }
    let ra: BTreeMap<u32, _> = a.state_hashes_v1.iter().map(|h| (h.tick, h)).collect();
    let rb: BTreeMap<u32, _> = b.state_hashes_v1.iter().map(|h| (h.tick, h)).collect();
    let ticks: BTreeSet<u32> = ra.keys().chain(rb.keys()).copied().collect();
    for &tick in &ticks {
        let (x, y) = (ra.get(&tick), rb.get(&tick));
        let same = match (x, y) {
            (Some(x), Some(y)) => x.gameplay == y.gameplay && x.arena == y.arena,
            _ => false,
        };
        if !same {
            return Verdict::Aggregate(AggregateDivergence {
                tick,
                gameplay: (x.map(|h| h.gameplay.clone()), y.map(|h| h.gameplay.clone())),
                arena: (
                    x.and_then(|h| h.arena.clone()),
                    y.and_then(|h| h.arena.clone()),
                ),
            });
        }
    }
    Verdict::AggregateIdentical { ticks: ticks.len() }
}

/// The guest address range of arena block `index`, from the arena base.
pub fn arena_range(base: u32, index: u32) -> (u32, u32) {
    let start = base.wrapping_add(index * ARENA_BLOCK);
    let end = base.wrapping_add(((index + 1) * ARENA_BLOCK).min(ARENA_LEN));
    (start, end)
}

impl Verdict {
    /// Whether the two reports were found to differ.
    pub fn differs(&self) -> bool {
        matches!(self, Verdict::Components(_) | Verdict::Aggregate(_))
    }

    /// The text `h2 sim diff` prints.
    pub fn render(&self) -> String {
        let mut out = String::new();
        match self {
            Verdict::Identical { ticks } => {
                let _ = writeln!(out, "identical: {ticks} ticks, every component agrees");
            }
            Verdict::IdenticalTruncated { checked_through } => {
                let _ = writeln!(
                    out,
                    "identical through tick {}; a component series was cut short at its size bound, so later ticks are unverified",
                    checked_through.map_or("-".into(), |t| t.to_string())
                );
            }
            Verdict::AggregateIdentical { ticks } => {
                let _ = writeln!(
                    out,
                    "identical: {ticks} aggregate digests agree (no component breakdown in one report)"
                );
            }
            Verdict::Unverifiable(why) => {
                let _ = writeln!(out, "cannot compare: {why}");
            }
            Verdict::Aggregate(d) => {
                let _ = writeln!(
                    out,
                    "first differing tick: {} (aggregate digests only: a report has no state_components_v1)",
                    d.tick
                );
                let show = |v: &Option<String>| v.clone().unwrap_or_else(|| "(none)".into());
                if d.gameplay.0 != d.gameplay.1 {
                    let _ = writeln!(
                        out,
                        "  gameplay  a={}  b={}",
                        show(&d.gameplay.0),
                        show(&d.gameplay.1)
                    );
                }
                if d.arena.0 != d.arena.1 {
                    let _ = writeln!(
                        out,
                        "  arena     a={}  b={}",
                        show(&d.arena.0),
                        show(&d.arena.1)
                    );
                }
            }
            Verdict::Components(d) => {
                let _ = writeln!(
                    out,
                    "first differing tick: {} ({} component{})",
                    d.tick,
                    d.differences.len(),
                    if d.differences.len() == 1 { "" } else { "s" }
                );
                for diff in &d.differences {
                    let show = |v: &Option<String>| v.clone().unwrap_or_else(|| "(absent)".into());
                    let label = arena_label(&diff.component, d.arena_base_a, d.arena_base_b);
                    let _ = writeln!(
                        out,
                        "  {}{}  a={}  b={}",
                        diff.component,
                        label,
                        show(&diff.a),
                        show(&diff.b)
                    );
                }
            }
        }
        out
    }
}

fn arena_label(component: &str, a: Option<u32>, b: Option<u32>) -> String {
    let Some(index) = component
        .strip_prefix(ARENA_PREFIX)
        .and_then(|i| i.parse::<u32>().ok())
    else {
        return String::new();
    };
    let range = |base: u32| {
        let (s, e) = arena_range(base, index);
        format!("{s:#010x}..{e:#010x}")
    };
    match (a, b) {
        (Some(x), Some(y)) if x != y => {
            format!(" [block {index}, a {} b {}]", range(x), range(y))
        }
        (Some(x), _) | (None, Some(x)) => format!(" [block {index}, {}]", range(x)),
        (None, None) => format!(" [block {index}]"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::{End, Finish, Tally, TickHash};

    fn series(rows: &[(u32, &[(&str, &str)])]) -> StateComponents {
        StateComponents {
            arena_base: Some(0x1000_0000),
            truncated_after: None,
            ticks: rows
                .iter()
                .map(|(tick, c)| ComponentTick {
                    tick: *tick,
                    changed: c.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
                })
                .collect(),
        }
    }

    fn report(c: Option<StateComponents>) -> Report {
        let mut r = Tally::new().report(Finish::bare(End::Stopped));
        r.state_components_v1 = c;
        r
    }

    #[test]
    fn names_the_components_that_differ_at_the_first_tick() {
        let a = series(&[
            (4, &[("player.00.pos", "1,2,3"), ("rng.seed", "aa"), ("arena.02", "1")]),
            (5, &[("player.00.pos", "1,2,4")]),
            (6, &[("player.01.health", "1,1"), ("arena.03", "7")]),
            (7, &[("rng.seed", "x")]),
        ]);
        let b = series(&[
            (4, &[("player.00.pos", "1,2,3"), ("rng.seed", "aa"), ("arena.02", "1")]),
            (5, &[("player.00.pos", "1,2,4")]),
            (6, &[("player.01.health", "1,0.5"), ("arena.03", "8")]),
            (7, &[("rng.seed", "y")]),
        ]);
        let Verdict::Components(d) = diff(&report(Some(a)), &report(Some(b))) else {
            panic!("expected a divergence");
        };
        assert_eq!(d.tick, 6);
        let names: Vec<_> = d.differences.iter().map(|x| x.component.as_str()).collect();
        assert_eq!(names, ["arena.03", "player.01.health"]);
        assert_eq!(d.differences[1].a.as_deref(), Some("1,1"));
        assert_eq!(d.differences[1].b.as_deref(), Some("1,0.5"));
        let text = Verdict::Components(d).render();
        assert!(text.contains("first differing tick: 6 (2 components)"));
        assert!(text.contains("arena.03 [block 3, 0x10030000..0x10040000]  a=7  b=8"));
    }

    #[test]
    fn an_unchanged_component_that_differs_earlier_is_not_reported_later() {
        // The component is equal in both until tick 3, where only b changes.
        let a = series(&[(1, &[("rng.seed", "1")]), (2, &[]), (3, &[])]);
        let b = series(&[(1, &[("rng.seed", "1")]), (2, &[]), (3, &[("rng.seed", "2")])]);
        let Verdict::Components(d) = diff(&report(Some(a)), &report(Some(b))) else {
            panic!();
        };
        assert_eq!(d.tick, 3);
        assert_eq!(d.differences[0].a.as_deref(), Some("1"));
        assert_eq!(d.differences[0].b.as_deref(), Some("2"));
    }

    #[test]
    fn absent_components_and_missing_ticks() {
        let a = series(&[(1, &[("player.00.pos", "1")]), (2, &[("player.00.pos", ABSENT)])]);
        let b = series(&[(1, &[("player.00.pos", "1")]), (2, &[])]);
        let Verdict::Components(d) = diff(&report(Some(a.clone())), &report(Some(b))) else {
            panic!();
        };
        assert_eq!(d.differences[0].a, None);
        assert_eq!(d.differences[0].b.as_deref(), Some("1"));

        let short = series(&[(1, &[("player.00.pos", "1")])]);
        let Verdict::Components(d) = diff(&report(Some(a)), &report(Some(short))) else {
            panic!();
        };
        assert_eq!((d.tick, d.differences[0].component.as_str()), (2, "tick"));
    }

    #[test]
    fn identical_series_and_truncation() {
        let a = series(&[(1, &[("x", "1")]), (2, &[("x", "2")])]);
        assert_eq!(
            diff(&report(Some(a.clone())), &report(Some(a.clone()))),
            Verdict::Identical { ticks: 2 }
        );
        let mut cut = series(&[(1, &[("x", "1")])]);
        cut.truncated_after = Some(1);
        assert_eq!(
            diff(&report(Some(a)), &report(Some(cut))),
            Verdict::IdenticalTruncated {
                checked_through: Some(1)
            }
        );
    }

    #[test]
    fn falls_back_to_the_aggregate_digests() {
        let hashes = |g: &[&str]| {
            let mut r = report(None);
            r.state_hashes_v1 = g
                .iter()
                .enumerate()
                .map(|(i, h)| TickHash {
                    tick: i as u32 + 1,
                    gameplay: (*h).into(),
                    arena: None,
                })
                .collect();
            r
        };
        // One side has components and the other does not: still aggregate.
        let mut with = hashes(&["a", "b", "c"]);
        with.state_components_v1 = Some(series(&[(1, &[("x", "1")])]));
        let Verdict::Aggregate(d) = diff(&with, &hashes(&["a", "b", "z"])) else {
            panic!();
        };
        assert_eq!(d.tick, 3);
        assert_eq!(d.gameplay, (Some("c".into()), Some("z".into())));
        assert_eq!(
            diff(&hashes(&["a"]), &hashes(&["a"])),
            Verdict::AggregateIdentical { ticks: 1 }
        );
        assert!(matches!(
            diff(&report(None), &hashes(&["a"])),
            Verdict::Unverifiable(_)
        ));
    }

    #[test]
    fn recorder_stores_deltas_and_replaces_a_repeated_tick() {
        let c = |v: &[(&str, &str)]| {
            v.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<Vec<_>>()
        };
        let mut r = Recorder::default();
        r.record(1, Some(0x100), c(&[("a", "1"), ("b", "1")]));
        r.record(2, None, c(&[("a", "1"), ("b", "2")]));
        r.record(2, None, c(&[("a", "2"), ("b", "2")]));
        r.record(3, None, c(&[("b", "2")]));
        let s = r.finish().unwrap();
        assert_eq!(s.arena_base, Some(0x100));
        assert_eq!(s.ticks[0].changed.len(), 2);
        assert_eq!(s.ticks[1].changed.get("a").map(String::as_str), Some("2"));
        assert_eq!(s.ticks[1].changed.get("b").map(String::as_str), Some("2"));
        assert_eq!(s.ticks[2].changed.get("a").map(String::as_str), Some(ABSENT));
        assert_eq!(s.ticks.len(), 3);
    }

    #[test]
    fn arena_block_ranges_end_at_the_arena() {
        assert_eq!(arena_range(0x1000, 0), (0x1000, 0x11000));
        assert_eq!(arena_range(0x1000, 63), (0x1000 + 0x3f0000, 0x1000 + 0x3fe000));
    }
}
