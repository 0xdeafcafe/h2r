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

use std::collections::{BTreeMap, BTreeSet, HashMap};
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

/// A training-detail report's digest rolled over every tick, beside its
/// sparse `state_hashes_v1` (`detail = "training"`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateDigest {
    /// `state_hashes_v1` keeps its first row and then every `stride`th tick.
    pub stride: u32,
    /// Ticks the digest covers.
    pub ticks: u64,
    /// 16 hexadecimal digits.
    pub digest: String,
}

/// One tick's changed components, name to value. The first row of a series
/// has them all.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentTick {
    pub tick: u32,
    pub changed: BTreeMap<String, String>,
}

/// A component's value as the recorder holds it. An arena block is a digest
/// kept as its `u64` and printed (`{:016x}`) only when the report is built.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Held {
    Hex(u64),
    Text(Box<str>),
}

impl Held {
    fn text(&self) -> String {
        match self {
            Held::Hex(h) => format!("{h:016x}"),
            Held::Text(t) => t.to_string(),
        }
    }
}

/// A value on its way in.
#[derive(Clone, Copy, Debug)]
enum Value<'a> {
    Hex(u64),
    Text(&'a str),
}

impl Value<'_> {
    fn same_as(self, held: &Held) -> bool {
        match (self, held) {
            (Value::Hex(a), Held::Hex(b)) => a == *b,
            (Value::Text(a), Held::Text(b)) => a == &**b,
            // A name that changes kind compares by what it prints.
            (Value::Hex(a), Held::Text(b)) => format!("{a:016x}") == **b,
            (Value::Text(a), Held::Hex(b)) => a == format!("{b:016x}"),
        }
    }

    fn hold(self) -> Held {
        match self {
            Value::Hex(h) => Held::Hex(h),
            Value::Text(t) => Held::Text(t.into()),
        }
    }
}

fn put<'a>(next: &mut Vec<Option<Value<'a>>>, id: usize, value: Value<'a>) {
    if next.len() <= id {
        next.resize(id + 1, None);
    }
    next[id] = Some(value);
}

/// One recorded change: `None` is a component going away (printed as
/// [`ABSENT`]).
type Change = (u32, Option<Held>);

/// Builds a [`StateComponents`] tick by tick, storing deltas.
///
/// Component names are interned once (a row holds `u32` ids, not strings),
/// arena block digests stay `u64` until [`Recorder::finish`], and a value is
/// stored only when it changed. [`Recorder::finish`] prints the series
/// straight from those rows, so the text of a value is made once and there
/// is no second copy of the series.
#[derive(Clone, Debug)]
pub struct Recorder {
    ids: HashMap<Box<str>, u32>,
    names: Vec<Box<str>>,
    /// Ids of `arena.NN`, by block index.
    blocks: Vec<u32>,
    /// The latest value of each component, by id; `None`: absent. As long as
    /// `names`.
    current: Vec<Option<Held>>,
    /// What the last recorded tick replaced, to restore when it is observed
    /// again.
    undo: Vec<(u32, Option<Held>)>,
    rows: Vec<(u32, Vec<Change>)>,
    arena_base: Option<u32>,
    truncated_after: Option<u32>,
    entries: usize,
    limit: usize,
}

impl Default for Recorder {
    fn default() -> Self {
        Recorder::with_limit(MAX_ENTRIES)
    }
}

impl Recorder {
    /// A recorder that stops after `limit` recorded values.
    pub fn with_limit(limit: usize) -> Recorder {
        Recorder {
            ids: HashMap::new(),
            names: Vec::new(),
            blocks: Vec::new(),
            current: Vec::new(),
            undo: Vec::new(),
            rows: Vec::new(),
            arena_base: None,
            truncated_after: None,
            entries: 0,
            limit,
        }
    }

    fn intern(&mut self, name: &str) -> usize {
        if let Some(&id) = self.ids.get(name) {
            return id as usize;
        }
        let id = self.names.len();
        self.names.push(name.into());
        self.ids.insert(name.into(), id as u32);
        self.current.push(None);
        id
    }

    fn block_id(&mut self, index: usize) -> usize {
        while self.blocks.len() <= index {
            let name = format!("{ARENA_PREFIX}{:02}", self.blocks.len());
            let id = self.intern(&name);
            self.blocks.push(id as u32);
        }
        self.blocks[index] as usize
    }

    /// Record `components` for `tick`. The same tick again replaces the
    /// last row; the caller restarts the recorder on a rewind.
    pub fn record(
        &mut self,
        tick: u32,
        arena_base: Option<u32>,
        components: impl IntoIterator<Item = (String, String)>,
    ) {
        self.record_split(tick, arena_base, components, &[]);
    }

    /// [`Recorder::record`] with the arena's block digests as `u64`s, block
    /// `i` being component `arena.<i:02>`: no name or value text is made for
    /// a block until it changes.
    pub fn record_split<N: AsRef<str>, V: AsRef<str>>(
        &mut self,
        tick: u32,
        arena_base: Option<u32>,
        text: impl IntoIterator<Item = (N, V)>,
        arena_blocks: &[u64],
    ) {
        if self.truncated_after.is_some() {
            return;
        }
        let items: Vec<(N, V)> = text.into_iter().collect();
        // The incoming tick by id; a name given twice keeps its last value,
        // as a map would.
        let mut next: Vec<Option<Value<'_>>> = Vec::new();
        for (name, value) in &items {
            let id = self.intern(name.as_ref());
            put(&mut next, id, Value::Text(value.as_ref()));
        }
        for (i, &h) in arena_blocks.iter().enumerate() {
            let id = self.block_id(i);
            put(&mut next, id, Value::Hex(h));
        }
        self.replace_last_if(tick);
        let mut changed: Vec<Change> = Vec::new();
        let mut undo = Vec::new();
        for (id, old) in self.current.iter().enumerate() {
            match (next.get(id).copied().flatten(), old) {
                (Some(v), Some(o)) if v.same_as(o) => {}
                (Some(v), _) => {
                    changed.push((id as u32, Some(v.hold())));
                    undo.push((id as u32, old.clone()));
                }
                (None, Some(_)) => {
                    changed.push((id as u32, None));
                    undo.push((id as u32, old.clone()));
                }
                (None, None) => {}
            }
        }
        if self.entries + changed.len() > self.limit {
            self.truncated_after = self.rows.last().map(|r| r.0);
            return;
        }
        for (id, value) in &changed {
            self.current[*id as usize] = value.clone();
        }
        self.undo = undo;
        self.entries += changed.len();
        if arena_base.is_some() {
            self.arena_base = arena_base;
        }
        self.rows.push((tick, changed));
    }

    /// Observing the last recorded tick again takes it back first.
    fn replace_last_if(&mut self, tick: u32) {
        if self.rows.last().is_some_and(|r| r.0 == tick) {
            for (id, old) in self.undo.drain(..) {
                self.current[id as usize] = old;
            }
            if let Some((_, changed)) = self.rows.pop() {
                self.entries -= changed.len();
            }
        }
    }

    /// The series, or `None` when nothing was recorded.
    pub fn finish(&self) -> Option<StateComponents> {
        if self.rows.is_empty() {
            return None;
        }
        Some(StateComponents {
            arena_base: self.arena_base,
            truncated_after: self.truncated_after,
            ticks: self
                .rows
                .iter()
                .map(|(tick, changed)| ComponentTick {
                    tick: *tick,
                    changed: changed
                        .iter()
                        .map(|(id, v)| {
                            (
                                self.names[*id as usize].to_string(),
                                v.as_ref().map_or_else(|| ABSENT.to_string(), Held::text),
                            )
                        })
                        .collect(),
                })
                .collect(),
        })
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

/// The first sampled tick (one both reports have) whose digests differ. The
/// state first differs after `after` and no later than `tick`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SparseDivergence {
    pub tick: u32,
    /// The last sampled tick the reports agreed at.
    pub after: Option<u32>,
    pub stride: u32,
    pub gameplay: (String, String),
    pub arena: (Option<String>, Option<String>),
}

/// What the rolling digests of two reports say when every sampled tick
/// agrees.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DigestCheck {
    /// Both cover the same ticks and agree.
    Equal { ticks: u64 },
    /// Both cover the same ticks and differ: the reports part between two
    /// sampled ticks.
    Differ { ticks: u64, a: String, b: String },
    /// They cover different numbers of ticks.
    Unequal { a: u64, b: u64 },
    /// One report has none (it is not a training report).
    Missing,
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
    /// A training report is in the comparison: only the ticks both reports
    /// sampled, and the rolling digests.
    Sparse(SparseDivergence),
    SparseIdentical { common: usize, digest: DigestCheck },
    /// Neither the breakdown nor digests can be compared.
    Unverifiable(String),
}

/// Compare two reports: the component series when both carry one, else
/// the aggregate digests.
pub fn diff(a: &Report, b: &Report) -> Verdict {
    if let (Some(ca), Some(cb)) = (&a.state_components_v1, &b.state_components_v1) {
        return components(ca, cb);
    }
    if a.state_digest_v1.is_some() || b.state_digest_v1.is_some() {
        return sparse(a, b);
    }
    aggregate(a, b)
}

/// A comparison with a training report in it: its digests are sparse, so
/// only the ticks both reports recorded are compared, then the rolling
/// digests when both have one and cover the same ticks.
fn sparse(a: &Report, b: &Report) -> Verdict {
    if a.state_hashes_v1.is_empty() || b.state_hashes_v1.is_empty() {
        return Verdict::Unverifiable("a report has no state_hashes_v1".into());
    }
    let rb: BTreeMap<u32, _> = b.state_hashes_v1.iter().map(|h| (h.tick, h)).collect();
    let (mut common, mut after) = (0, None);
    for x in &a.state_hashes_v1 {
        let Some(y) = rb.get(&x.tick) else { continue };
        if x.gameplay != y.gameplay || x.arena != y.arena {
            let stride = [&a.state_digest_v1, &b.state_digest_v1]
                .into_iter()
                .flatten()
                .map(|d| d.stride)
                .next()
                .unwrap_or(0);
            return Verdict::Sparse(SparseDivergence {
                tick: x.tick,
                after,
                stride,
                gameplay: (x.gameplay.clone(), y.gameplay.clone()),
                arena: (x.arena.clone(), y.arena.clone()),
            });
        }
        common += 1;
        after = Some(x.tick);
    }
    if common == 0 {
        return Verdict::Unverifiable(
            "the reports have no sampled tick in common (a training report keeps its first tick and every Nth)"
                .into(),
        );
    }
    let digest = match (&a.state_digest_v1, &b.state_digest_v1) {
        (Some(x), Some(y)) if x.ticks != y.ticks => DigestCheck::Unequal { a: x.ticks, b: y.ticks },
        (Some(x), Some(y)) if x.digest != y.digest => DigestCheck::Differ {
            ticks: x.ticks,
            a: x.digest.clone(),
            b: y.digest.clone(),
        },
        (Some(x), Some(_)) => DigestCheck::Equal { ticks: x.ticks },
        _ => DigestCheck::Missing,
    };
    Verdict::SparseIdentical { common, digest }
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
        matches!(
            self,
            Verdict::Components(_)
                | Verdict::Aggregate(_)
                | Verdict::Sparse(_)
                | Verdict::SparseIdentical {
                    digest: DigestCheck::Differ { .. },
                    ..
                }
        )
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
            Verdict::SparseIdentical { common, digest } => match digest {
                DigestCheck::Equal { ticks } => {
                    let _ = writeln!(
                        out,
                        "identical: {common} sampled digests and the rolling digest over {ticks} ticks agree (a training report has no component breakdown)"
                    );
                }
                DigestCheck::Differ { ticks, a, b } => {
                    let _ = writeln!(
                        out,
                        "differ: the {common} sampled digests agree, but the rolling digests over {ticks} ticks do not (a={a}  b={b})"
                    );
                    let _ = writeln!(
                        out,
                        "  the state parts between sampled ticks; a training report cannot say where: rerun both with detail = \"full\""
                    );
                }
                DigestCheck::Unequal { a, b } => {
                    let _ = writeln!(
                        out,
                        "sampled digests agree at {common} ticks; the rolling digests were not compared: a covers {a} ticks, b covers {b}"
                    );
                }
                DigestCheck::Missing => {
                    let _ = writeln!(
                        out,
                        "sampled digests agree at {common} ticks; the rolling digests were not compared: a report has none (it is not a training report)"
                    );
                }
            },
            Verdict::Sparse(d) => {
                let window = match d.after {
                    Some(t) => format!("the state first differs after tick {t}, at or before tick {}", d.tick),
                    None => format!("the state differs at or before tick {}", d.tick),
                };
                let _ = writeln!(
                    out,
                    "first differing sampled tick: {} (a training report keeps one tick in {}: {window}; no component breakdown)",
                    d.tick, d.stride
                );
                let _ = writeln!(out, "  gameplay  a={}  b={}", d.gameplay.0, d.gameplay.1);
                if d.arena.0 != d.arena.1 {
                    let show = |v: &Option<String>| v.clone().unwrap_or_else(|| "(none)".into());
                    let _ = writeln!(out, "  arena     a={}  b={}", show(&d.arena.0), show(&d.arena.1));
                }
                let _ = writeln!(out, "  rerun both with detail = \"full\" to name the component");
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
    fn a_report_without_components_is_named_in_the_text() {
        let v = Verdict::AggregateIdentical { ticks: 3 };
        assert!(v.render().contains("no component breakdown"));
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

    /// The recorder as it was before interned ids and `u64` values: the
    /// reference the new one must match byte for byte.
    #[derive(Default)]
    struct OldRecorder {
        current: BTreeMap<String, String>,
        undo: Vec<(String, Option<String>)>,
        out: StateComponents,
        entries: usize,
        limit: usize,
    }

    impl OldRecorder {
        fn with_limit(limit: usize) -> Self {
            OldRecorder { limit, ..Default::default() }
        }

        fn record(&mut self, tick: u32, arena_base: Option<u32>, components: Vec<(String, String)>) {
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
            if self.entries + changed.len() > self.limit {
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

        fn finish(&self) -> Option<StateComponents> {
            (!self.out.ticks.is_empty()).then(|| self.out.clone())
        }
    }

    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self, below: u64) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 33) % below
        }
    }

    /// One tick as both recorders take it: text components and arena blocks.
    struct Step {
        tick: u32,
        base: Option<u32>,
        text: Vec<(String, String)>,
        blocks: Vec<u64>,
    }

    fn scenario(seed: u64) -> Vec<Step> {
        let mut rng = Lcg(seed);
        let mut steps = Vec::new();
        let mut tick = 10;
        for _ in 0..80 {
            // One tick in five is observed twice, the second time with
            // different content.
            let repeats = if rng.next(5) == 0 { 2 } else { 1 };
            for _ in 0..repeats {
                let mut text = Vec::new();
                for i in 0..14 {
                    if rng.next(4) != 0 {
                        text.push((
                            format!("player.{:02}.{}", i % 3, ["pos", "aim", "health", "id"][i % 4]),
                            format!("v{}", rng.next(3)),
                        ));
                    }
                }
                // A name given twice keeps the last value.
                if rng.next(6) == 0 {
                    text.push(("rng.seed".into(), "1".into()));
                    text.push(("rng.seed".into(), format!("{}", rng.next(2))));
                }
                let blocks = match rng.next(8) {
                    0 => Vec::new(),
                    _ => (0..5).map(|_| rng.next(3)).collect(),
                };
                steps.push(Step {
                    tick,
                    base: (rng.next(4) == 0).then(|| 0x1000_0000 + rng.next(4) as u32),
                    text,
                    blocks,
                });
            }
            tick += 1 + rng.next(2) as u32;
        }
        steps
    }

    fn json(c: &Option<StateComponents>) -> String {
        serde_json::to_string(c).unwrap()
    }

    #[test]
    fn the_recorder_matches_the_one_it_replaced_byte_for_byte() {
        for seed in 1..=12 {
            // Roomy, tight (truncated mid-series) and tiny limits.
            for limit in [MAX_ENTRIES, 120, 37, 5, 0] {
                let (mut old, mut new) = (OldRecorder::with_limit(limit), Recorder::with_limit(limit));
                let mut via_text = Recorder::with_limit(limit);
                for step in scenario(seed) {
                    let mut all = step.text.clone();
                    all.extend(
                        step.blocks
                            .iter()
                            .enumerate()
                            .map(|(i, h)| (format!("{ARENA_PREFIX}{i:02}"), format!("{h:016x}"))),
                    );
                    old.record(step.tick, step.base, all.clone());
                    new.record_split(step.tick, step.base, step.text.clone(), &step.blocks);
                    via_text.record(step.tick, step.base, all);
                }
                let want = json(&old.finish());
                assert_eq!(json(&new.finish()), want, "seed {seed} limit {limit}");
                assert_eq!(json(&via_text.finish()), want, "seed {seed} limit {limit} as text");
                // The cut-off is the same one.
                assert_eq!(
                    new.finish().and_then(|s| s.truncated_after),
                    old.finish().and_then(|s| s.truncated_after)
                );
            }
        }
    }

    #[test]
    fn a_tick_observed_again_after_the_series_is_cut_changes_nothing() {
        let mut r = Recorder::with_limit(2);
        r.record_split(1, None, [("a", "1"), ("b", "1")], &[]);
        r.record_split(2, None, [("a", "2")], &[]);
        r.record_split(2, None, [("a", "3")], &[]);
        r.record_split(3, None, [("a", "4")], &[]);
        let s = r.finish().unwrap();
        assert_eq!(s.truncated_after, Some(1));
        assert_eq!(s.ticks.len(), 1);
    }

    #[test]
    fn arena_blocks_print_as_sixteen_hex_digits_only_when_they_change() {
        let mut r = Recorder::default();
        r.record_split(1, Some(0x100), [("time.tick", "1")], &[0xab, 0]);
        r.record_split(2, None, [("time.tick", "2")], &[0xab, 7]);
        let s = r.finish().unwrap();
        assert_eq!(s.ticks[0].changed["arena.00"], "00000000000000ab");
        assert_eq!(s.ticks[1].changed.len(), 2, "{:?}", s.ticks[1].changed);
        assert_eq!(s.ticks[1].changed["arena.01"], "0000000000000007");
        assert!(!s.ticks[1].changed.contains_key("arena.00"));
    }

    fn training(hashes: &[(u32, &str)], digest: Option<(u64, &str)>) -> Report {
        let mut r = report(None);
        r.state_hashes_v1 = hashes
            .iter()
            .map(|(tick, h)| TickHash { tick: *tick, gameplay: (*h).into(), arena: None })
            .collect();
        r.state_digest_v1 = digest.map(|(ticks, d)| StateDigest {
            stride: 30,
            ticks,
            digest: d.into(),
        });
        r
    }

    #[test]
    fn two_training_reports_compare_at_their_common_ticks_then_by_digest() {
        let rows = [(5, "a"), (30, "b"), (60, "c")];
        let same = training(&rows, Some((90, "dd")));
        let v = diff(&same, &same.clone());
        assert_eq!(
            v,
            Verdict::SparseIdentical { common: 3, digest: DigestCheck::Equal { ticks: 90 } }
        );
        assert!(!v.differs());
        assert!(v.render().contains("rolling digest over 90 ticks agree"));

        // The sampled rows agree and the rolling digests do not: the report
        // says it cannot say where.
        let v = diff(&same, &training(&rows, Some((90, "ee"))));
        assert!(v.differs());
        let text = v.render();
        assert!(text.contains("rolling digests over 90 ticks do not"), "{text}");
        assert!(text.contains("detail = \"full\""));

        // A sampled row differs: the window between the two sampled ticks.
        let other = training(&[(5, "a"), (30, "x"), (60, "c")], Some((90, "ee")));
        let Verdict::Sparse(d) = diff(&same, &other) else { panic!() };
        assert_eq!((d.tick, d.after, d.stride), (30, Some(5), 30));
        let text = Verdict::Sparse(d).render();
        assert!(text.contains("differs after tick 5, at or before tick 30"), "{text}");
    }

    #[test]
    fn a_training_report_against_other_shapes_says_what_it_cannot_compare() {
        let rows = [(5, "a"), (30, "b")];
        let t = training(&rows, Some((60, "dd")));
        // Against a full report with a digest every tick: common ticks only,
        // and no rolling digest to compare.
        let full = training(&[(5, "a"), (6, "q"), (30, "b")], None);
        let v = diff(&t, &full);
        assert_eq!(v, Verdict::SparseIdentical { common: 2, digest: DigestCheck::Missing });
        assert!(v.render().contains("rolling digests were not compared"));
        // Runs of different lengths.
        let v = diff(&t, &training(&rows, Some((45, "dd"))));
        assert_eq!(
            v,
            Verdict::SparseIdentical { common: 2, digest: DigestCheck::Unequal { a: 60, b: 45 } }
        );
        assert!(v.render().contains("a covers 60 ticks, b covers 45"));
        // No tick in common, or none recorded.
        let apart = training(&[(7, "a")], Some((60, "dd")));
        assert!(matches!(diff(&t, &apart), Verdict::Unverifiable(m) if m.contains("no sampled tick in common")));
        assert!(matches!(diff(&t, &training(&[], None)), Verdict::Unverifiable(_)));
    }
}
