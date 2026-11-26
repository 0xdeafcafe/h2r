//! Before and after (docs/editor/playtest.md, "Before and after"): two
//! results over the same seeds and match file, compared.
//!
//! An edit changes the map, so the matches differ from the first tick and
//! are compared by what they measure, not tick by tick:
//!
//! - each scalar measure is taken per seed (a seed's two legs, when sides
//!   were swapped, are one value) and a change is called out only when the
//!   means differ by more than the larger of the two sides' spread across
//!   seeds ([`change`]);
//! - each layer is a difference layer ([`Diff`]): its marks are the places
//!   whose count, per trace, moved by more than counting noise would
//!   ([`Z`] standard deviations of a Poisson count; of a share, for the
//!   spawns).

use std::collections::BTreeMap;

use serde::Serialize;

use super::{
    Bin, Cell, Measures, SAMPLE_EVERY, TICK_RATE, Trace, V3, flow::Flow, heat::Heat, items::Items,
    measure, spawns::Spawns, stuck::Stuck,
};

/// Standard deviations a layer's change must clear to be a mark.
pub const Z: f32 = 2.0;
/// Seeds a side needs for its spread to mean anything.
pub const MIN_SEEDS: usize = 2;

/// The scalar measures, by name.
pub const SCALARS: [&str; 8] = [
    "deaths_per_min",
    "kills_per_min",
    "spawn_kills_per_min",
    "mean_life_s",
    "mean_in_sight",
    "pickups_per_min",
    "stuck_ticks_per_min",
    "sight_median",
];

/// One scalar measure over the seeds of both sides.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Scalar {
    pub name: &'static str,
    /// The means over seeds; `None` where no seed could tell.
    pub a: Option<f32>,
    pub b: Option<f32>,
    /// The larger of the two sides' standard deviations across seeds;
    /// `None` when a side has fewer than [`MIN_SEEDS`] values.
    pub spread: Option<f32>,
    /// Seeds that gave a value, per side.
    pub n_a: usize,
    pub n_b: usize,
    /// `b - a` is larger than the spread.
    pub changed: bool,
}

fn mean(v: &[f32]) -> Option<f32> {
    (!v.is_empty()).then(|| v.iter().sum::<f32>() / v.len() as f32)
}

/// The sample standard deviation; `None` under [`MIN_SEEDS`] values.
fn deviation(v: &[f32]) -> Option<f32> {
    if v.len() < MIN_SEEDS {
        return None;
    }
    let m = mean(v)?;
    let var = v.iter().map(|x| (x - m) * (x - m)).sum::<f32>() / (v.len() - 1) as f32;
    Some(var.sqrt())
}

/// One scalar from each side's per-seed values.
pub fn change(name: &'static str, a: &[f32], b: &[f32]) -> Scalar {
    let (ma, mb) = (mean(a), mean(b));
    let spread = match (deviation(a), deviation(b)) {
        (Some(x), Some(y)) => Some(x.max(y)),
        _ => None,
    };
    let changed = match (ma, mb, spread) {
        (Some(x), Some(y), Some(s)) => (y - x).abs() > s,
        _ => false,
    };
    Scalar {
        name,
        a: ma,
        b: mb,
        spread,
        n_a: a.len(),
        n_b: b.len(),
        changed,
    }
}

/// The scalars of one seed's measures, in [`SCALARS`]' order.
fn scalars(m: &Measures) -> [Option<f32>; 8] {
    let minutes = m.seconds / 60.0;
    let per_min = |n: f32| (minutes > 0.0).then(|| n / minutes);
    let weighted = |rows: &[super::spawns::StartRow],
                    f: fn(&super::spawns::StartRow) -> Option<f32>| {
        let (sum, n) = rows
            .iter()
            .filter_map(|r| f(r).map(|v| (v * r.uses as f32, r.uses as f32)))
            .fold((0.0, 0.0), |(s, n), (v, w)| (s + v, n + w));
        (n > 0.0).then(|| sum / n)
    };
    let sum = |bins: &[Bin]| bins.iter().map(|b| b.count).sum::<u32>() as f32;
    [
        m.heat.as_ref().ok().and_then(|h| per_min(sum(&h.deaths))),
        m.heat.as_ref().ok().and_then(|h| per_min(sum(&h.kills))),
        m.spawns
            .as_ref()
            .ok()
            .and_then(|s| per_min(s.spawn_kills.len() as f32)),
        m.spawns
            .as_ref()
            .ok()
            .and_then(|s| weighted(&s.starts, |r| r.mean_life_s)),
        m.spawns
            .as_ref()
            .ok()
            .and_then(|s| weighted(&s.starts, |r| r.mean_in_sight)),
        m.items
            .as_ref()
            .ok()
            .and_then(|i| per_min(i.pickups as f32)),
        m.stuck
            .as_ref()
            .ok()
            .and_then(|s| per_min(s.spots.iter().map(|p| p.ticks).sum::<u64>() as f32)),
        m.sight.as_ref().ok().map(|s| s.median),
    ]
}

/// One mark of a difference layer.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Delta {
    /// Where (a flow line's start; a polygon's centre).
    pub at: V3,
    /// A flow line's end.
    pub to: Option<V3>,
    /// The mesh polygon, for the polygon layer.
    pub poly: Option<u32>,
    /// The value per trace before and after: a count, seconds, or a share.
    pub a: f32,
    pub b: f32,
}

impl Delta {
    pub fn change(&self) -> f32 {
        self.b - self.a
    }
}

/// A difference layer's marks, or why it has none.
pub type Marks = Result<Vec<Delta>, String>;

/// Every layer that has a difference (the sightlines have none), the most
/// changed mark first.
#[derive(Clone, Debug, PartialEq)]
pub struct Diff {
    pub a_traces: usize,
    pub b_traces: usize,
    pub kills: Marks,
    pub deaths: Marks,
    pub traffic: Marks,
    pub flow: Marks,
    pub unvisited: Marks,
    pub spawns: Marks,
    pub weapon_use: Marks,
    pub stuck: Marks,
}

/// What `playtest.compare` answers.
#[derive(Clone, Debug, PartialEq)]
pub struct Comparison {
    pub scalars: Vec<Scalar>,
    pub diff: Diff,
}

impl Comparison {
    /// The names of the scalars that changed.
    pub fn changed(&self) -> Vec<&'static str> {
        self.scalars
            .iter()
            .filter(|s| s.changed)
            .map(|s| s.name)
            .collect()
    }
}

/// Traces grouped by seed (a seed's swapped leg joins it), seeds in order.
fn by_seed(traces: &[Trace]) -> Vec<Vec<Trace>> {
    let mut groups: BTreeMap<u64, Vec<Trace>> = BTreeMap::new();
    for t in traces {
        groups.entry(t.header.seed).or_default().push(t.clone());
    }
    groups.into_values().collect()
}

/// Compares `a`'s traces with `b`'s.
pub fn compare(a: &[Trace], b: &[Trace]) -> Result<Comparison, String> {
    if a.is_empty() || b.is_empty() {
        return Err("a result with no match that ended cannot be compared".into());
    }
    let per_seed = |traces: &[Trace]| -> Vec<[Option<f32>; 8]> {
        by_seed(traces)
            .iter()
            .map(|g| scalars(&measure(g)))
            .collect()
    };
    let (sa, sb) = (per_seed(a), per_seed(b));
    let scalars = SCALARS
        .iter()
        .enumerate()
        .map(|(i, &name)| {
            let col = |s: &[[Option<f32>; 8]]| s.iter().filter_map(|r| r[i]).collect::<Vec<f32>>();
            change(name, &col(&sa), &col(&sb))
        })
        .collect();
    Ok(Comparison {
        scalars,
        diff: diff(&measure(a), &measure(b), a, b),
    })
}

/// The difference layers of two pooled measures; `a` and `b` are the
/// traces they came from, for the meshes.
pub fn diff(ma: &Measures, mb: &Measures, a: &[Trace], b: &[Trace]) -> Diff {
    let (na, nb) = (ma.traces.max(1) as f32, mb.traces.max(1) as f32);
    let n = Pool { na, nb };
    let heat = both_ok(&ma.heat, &mb.heat);
    let flow = both_ok(&ma.flow, &mb.flow);
    let take = |r: Result<(&Heat, &Heat), String>, f: fn(&Heat) -> &[Bin]| -> Marks {
        r.map(|(x, y)| n.counts(bins(f(x)), bins(f(y)), 1.0))
    };
    Diff {
        a_traces: ma.traces,
        b_traces: mb.traces,
        kills: take(heat.clone(), |h| h.kills.as_slice()),
        deaths: take(heat, |h| h.deaths.as_slice()),
        traffic: flow.clone().map(|(x, y)| {
            // A sample is one `SAMPLE_EVERY` ticks of a player.
            let per_second = TICK_RATE / SAMPLE_EVERY as f32;
            n.counts(
                x.cells.iter().map(|c| (c.cell, c.at, c.seconds)),
                y.cells.iter().map(|c| (c.cell, c.at, c.seconds)),
                per_second,
            )
        }),
        flow: flow.clone().map(|(x, y)| crossings(&n, x, y)),
        unvisited: flow.and_then(|(x, y)| unvisited(x, y, a, b)),
        spawns: both_ok(&ma.spawns, &mb.spawns).map(|(x, y)| spawns(x, y)),
        weapon_use: both_ok(&ma.items, &mb.items).map(|(x, y)| items(&n, x, y)),
        stuck: both_ok(&ma.stuck, &mb.stuck).map(|(x, y)| stuck(&n, x, y)),
    }
}

/// Both measures, or the refusal of whichever was refused.
fn both_ok<'a, T>(
    a: &'a Result<T, super::TooFew>,
    b: &'a Result<T, super::TooFew>,
) -> Result<(&'a T, &'a T), String> {
    match (a, b) {
        (Ok(x), Ok(y)) => Ok((x, y)),
        (Err(e), _) => Err(format!("a: {e}")),
        (_, Err(e)) => Err(format!("b: {e}")),
    }
}

fn bins(b: &[Bin]) -> impl Iterator<Item = (Cell, V3, f32)> + '_ {
    b.iter().map(|b| (b.cell, b.at, b.count as f32))
}

/// How many traces each side has.
struct Pool {
    na: f32,
    nb: f32,
}

impl Pool {
    /// The marks of two counted layers. Values are in units of which
    /// `per_unit` make one counted event (1 for a count, the sample rate
    /// for seconds), so the counting noise is that of events.
    fn counts<K: Ord>(
        &self,
        a: impl Iterator<Item = (K, V3, f32)>,
        b: impl Iterator<Item = (K, V3, f32)>,
        per_unit: f32,
    ) -> Vec<Delta> {
        let mut m: BTreeMap<K, (Option<(V3, f32)>, Option<(V3, f32)>)> = BTreeMap::new();
        for (k, at, v) in a {
            m.entry(k).or_default().0 = Some((at, v));
        }
        for (k, at, v) in b {
            m.entry(k).or_default().1 = Some((at, v));
        }
        let mut out: Vec<Delta> = m
            .into_values()
            .filter_map(|(x, y)| {
                let (va, vb) = (x.map_or(0.0, |p| p.1), y.map_or(0.0, |p| p.1));
                let at = y.or(x)?.0;
                self.mark(at, va, vb, per_unit)
            })
            .collect();
        sort(&mut out);
        out
    }

    fn mark(&self, at: V3, va: f32, vb: f32, per_unit: f32) -> Option<Delta> {
        let (ra, rb) = (va / self.na, vb / self.nb);
        // Events are counted, so their variance is their number.
        let sd = (va * per_unit / (self.na * self.na) + vb * per_unit / (self.nb * self.nb)).sqrt()
            / per_unit;
        let d = rb - ra;
        (d != 0.0 && d.abs() > Z * sd).then_some(Delta {
            at,
            to: None,
            poly: None,
            a: ra,
            b: rb,
        })
    }
}

fn sort(v: &mut [Delta]) {
    v.sort_by(|x, y| y.change().abs().total_cmp(&x.change().abs()));
}

fn crossings(n: &Pool, a: &Flow, b: &Flow) -> Vec<Delta> {
    let key = |c: &super::flow::Crossing| ((c.from, c.to), c.a, c.count as f32);
    let mut out = n.counts(
        a.crossings.iter().map(key),
        b.crossings.iter().map(key),
        1.0,
    );
    // The line's far end, from whichever side has it.
    for d in &mut out {
        let found = b
            .crossings
            .iter()
            .chain(&a.crossings)
            .find(|c| c.a == d.at)
            .map(|c| c.b);
        d.to = found;
    }
    out
}

/// A spawn's unfairness: the larger of its spawn-kill share and the share
/// of spawns with an enemy in sight; `None` when it was never used.
fn unfair(r: &super::spawns::StartRow) -> Option<f32> {
    (r.uses > 0).then(|| {
        let died = r.spawn_deaths as f32 / r.uses as f32;
        died.max(r.seen_share.unwrap_or(0.0)).clamp(0.0, 1.0)
    })
}

fn spawns(a: &Spawns, b: &Spawns) -> Vec<Delta> {
    let rows = |s: &Spawns| -> BTreeMap<Option<u16>, (V3, f32, f32)> {
        s.starts
            .iter()
            .filter_map(|r| unfair(r).map(|u| (r.start, (r.at, u, r.uses as f32))))
            .collect()
    };
    let (ra, rb) = (rows(a), rows(b));
    let mut out: Vec<Delta> = rb
        .iter()
        .filter_map(|(k, &(at, pb, nb))| {
            let &(_, pa, na) = ra.get(k)?;
            let sd = (pa * (1.0 - pa) / na + pb * (1.0 - pb) / nb).sqrt();
            let d = pb - pa;
            (d != 0.0 && d.abs() > Z * sd).then_some(Delta {
                at,
                to: None,
                poly: None,
                a: pa,
                b: pb,
            })
        })
        .collect();
    sort(&mut out);
    out
}

fn items(n: &Pool, a: &Items, b: &Items) -> Vec<Delta> {
    let rows = |i: &Items| -> Vec<(u16, V3, f32)> {
        i.placements
            .iter()
            .map(|p| (p.item, p.at, p.pickups as f32))
            .collect()
    };
    n.counts(rows(a).into_iter(), rows(b).into_iter(), 1.0)
}

fn stuck(n: &Pool, a: &Stuck, b: &Stuck) -> Vec<Delta> {
    let rows = |s: &Stuck| -> Vec<(Cell, V3, f32)> {
        s.spots
            .iter()
            .map(|p| (p.cell, p.at, p.spells as f32))
            .collect()
    };
    n.counts(rows(a).into_iter(), rows(b).into_iter(), 1.0)
}

/// The first trace's mesh.
fn mesh_of(t: &[Trace]) -> Option<&super::Mesh> {
    t.first().and_then(|t| t.header.mesh.as_ref())
}

/// Polygons some sample entered on one side and none on the other.
/// Polygon numbers are the build's own, so the sides must share a mesh of
/// one size.
fn unvisited(a: &Flow, b: &Flow, ta: &[Trace], tb: &[Trace]) -> Marks {
    let (Some(x), Some(y)) = (mesh_of(ta), mesh_of(tb)) else {
        return Err("a trace has no bots' mesh".into());
    };
    if x.centers.len() != y.centers.len() {
        return Err(format!(
            "the bots' meshes differ in size (a {} polygons, b {})",
            x.centers.len(),
            y.centers.len()
        ));
    }
    let (Some(na), Some(nb)) = (&a.never, &b.never) else {
        return Err("no bots' mesh to say which polygons were never entered".into());
    };
    let mark = |i: u32, a: f32, b: f32| {
        y.centers.get(i as usize).map(|&at| Delta {
            at,
            to: None,
            poly: Some(i),
            a,
            b,
        })
    };
    // Opened: never entered before, entered now. Closed: the reverse.
    let opened = na
        .iter()
        .filter(|i| !nb.contains(i))
        .filter_map(|&i| mark(i, 1.0, 0.0));
    let closed = nb
        .iter()
        .filter(|i| !na.contains(i))
        .filter_map(|&i| mark(i, 0.0, 1.0));
    Ok(opened.chain(closed).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::KillKind;
    use crate::spatial::{Event, Header, Mesh, Party, Place};

    #[test]
    fn a_change_inside_the_spread_is_not_called_out() {
        let s = change("x", &[10.0, 12.0, 11.0, 13.0], &[11.0, 12.5, 10.0, 13.0]);
        assert!(!s.changed, "{s:?}");
        assert!(s.spread.unwrap() > 1.0);
    }

    #[test]
    fn a_change_outside_the_spread_is() {
        let s = change("x", &[10.0, 12.0, 11.0, 13.0], &[30.0, 32.0, 31.0, 33.0]);
        assert!(s.changed, "{s:?}");
        assert_eq!((s.n_a, s.n_b), (4, 4));
    }

    #[test]
    fn a_side_with_one_seed_has_no_spread_and_calls_nothing_out() {
        let s = change("x", &[10.0], &[40.0, 41.0]);
        assert_eq!(s.spread, None);
        assert!(!s.changed);
        assert_eq!(change("x", &[], &[1.0, 2.0]).a, None);
    }

    fn at(x: f32, poly: u32) -> Place {
        Place {
            pos: [x, 0.0, 0.0],
            poly: Some(poly),
        }
    }

    fn party(player: u8, at: Option<Place>) -> Party {
        Party {
            player,
            team: None,
            at,
            weapon: None,
        }
    }

    /// A two-minute match of `seed` with `deaths[i]` deaths on polygon `i`.
    fn trace(seed: u64, deaths: &[u32]) -> Trace {
        let mut events = Vec::new();
        for (poly, &n) in deaths.iter().enumerate() {
            for k in 0..n {
                events.push(Event::Kill {
                    tick: 100 + k,
                    killer: None,
                    victim: party(1, Some(at(poly as f32, poly as u32))),
                    kind: KillKind::World,
                });
            }
        }
        Trace {
            header: Header {
                seed,
                start_tick: 0,
                end_tick: 3600,
                mesh: Some(Mesh {
                    centers: vec![[0.0; 3], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]],
                    links: Vec::new(),
                }),
                ..Header::default()
            },
            events,
        }
    }

    fn side(counts: &[&[u32]]) -> Vec<Trace> {
        counts
            .iter()
            .enumerate()
            .map(|(i, d)| trace(i as u64 + 1, d))
            .collect()
    }

    #[test]
    fn two_results_with_a_moved_hot_spot_call_out_the_spot_and_the_rate() {
        // Seeds' deaths per two minutes: polygon 0 holds the fights before;
        // after, polygon 1 does and the total rises.
        let a = side(&[&[12, 1], &[14, 0], &[13, 2], &[15, 1]]);
        let b = side(&[&[1, 40], &[0, 42], &[2, 41], &[1, 43]]);
        let c = compare(&a, &b).unwrap();
        let deaths = c
            .scalars
            .iter()
            .find(|s| s.name == "deaths_per_min")
            .unwrap();
        assert!(deaths.changed, "{deaths:?}");
        let marks = c.diff.deaths.as_ref().unwrap();
        // Polygon 1 gained, polygon 0 lost; the biggest change first.
        assert_eq!(marks[0].at, [1.0, 0.0, 0.0]);
        assert!(marks[0].change() > 0.0);
        assert!(marks.iter().any(|d| d.at == [0.0; 3] && d.change() < 0.0));
        // Nothing was refused in what has no events: the layer has no marks.
        assert!(c.diff.traffic.is_err());
        assert_eq!(c.changed(), vec!["deaths_per_min"]);
    }

    #[test]
    fn two_alike_results_have_no_marks_and_no_change() {
        let a = side(&[&[12, 1], &[14, 0], &[13, 2], &[15, 1]]);
        let b = side(&[&[13, 1], &[12, 0], &[14, 2], &[13, 1]]);
        let c = compare(&a, &b).unwrap();
        assert!(c.changed().is_empty(), "{:?}", c.scalars);
        assert!(c.diff.deaths.unwrap().is_empty());
    }

    #[test]
    fn a_seeds_swapped_leg_is_one_value() {
        let mut legs = side(&[&[12], &[14]]);
        legs.push(trace(1, &[10]));
        assert_eq!(by_seed(&legs).len(), 2);
        assert_eq!(by_seed(&legs)[0].len(), 2);
    }

    #[test]
    fn an_empty_side_is_refused() {
        assert!(compare(&[], &side(&[&[12]])).is_err());
    }

    #[test]
    fn polygons_opened_and_closed_are_marked_and_a_resized_mesh_is_refused() {
        let flow = |never: &[u32]| Flow {
            never: Some(never.to_vec()),
            ..Flow::default()
        };
        let t = trace(1, &[]);
        let m = unvisited(&flow(&[0, 1]), &flow(&[1, 2]), std::slice::from_ref(&t), std::slice::from_ref(&t)).unwrap();
        let polys: Vec<(u32, f32, f32)> = m.iter().map(|d| (d.poly.unwrap(), d.a, d.b)).collect();
        assert_eq!(polys, vec![(0, 1.0, 0.0), (2, 0.0, 1.0)]);
        let mut small = t.clone();
        small.header.mesh.as_mut().unwrap().centers.pop();
        let err = unvisited(&flow(&[]), &flow(&[]), &[t], &[small]).unwrap_err();
        assert!(err.contains("differ in size"), "{err}");
    }

    #[test]
    fn a_spawn_that_became_unfair_is_marked() {
        use crate::spatial::spawns::StartRow;
        let row = |deaths| StartRow {
            start: Some(0),
            at: [5.0, 0.0, 0.0],
            uses: 20,
            mean_life_s: None,
            spawn_deaths: deaths,
            mean_in_sight: None,
            seen_share: Some(0.0),
            mean_nearest_enemy: None,
            mean_first_engagement_s: None,
        };
        let sp = |r| Spawns {
            starts: vec![r],
            ..Spawns::default()
        };
        let m = spawns(&sp(row(0)), &sp(row(12)));
        assert_eq!(m.len(), 1);
        assert_eq!((m[0].a, m[0].b), (0.0, 0.6));
        assert!(spawns(&sp(row(2)), &sp(row(3))).is_empty());
    }
}
