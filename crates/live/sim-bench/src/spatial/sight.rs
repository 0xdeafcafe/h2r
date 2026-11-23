//! Sightlines and engagement distance (docs/editor/playtest.md, "What is
//! measured"): the segments from each damage's and kill's attacker to its
//! victim. The longest lines and the commonest (by the pair of bins they
//! join, either way round), and a distance histogram per weapon.

use std::collections::BTreeMap;

use serde::Serialize;

use super::{Cell, Event, Mean, TooFew, Trace, V3, enough};

/// Fewer segments than this are refused.
pub const MIN_SEGMENTS: usize = 10;
/// The histogram's bucket width, world units; the last bucket holds
/// everything from `BUCKET * (BUCKETS - 1)` on.
pub const BUCKET: f32 = 5.0;
pub const BUCKETS: usize = 11;
/// Lines kept in each list.
pub const TOP: usize = 10;
/// A weapon key for a segment whose attacker's weapon is not known.
pub const UNKNOWN_WEAPON: u32 = u32::MAX;

/// A line drawn over the map.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Line {
    pub a: V3,
    pub b: V3,
    /// Its length (for a commonest line, the mean of its segments').
    pub distance: f32,
    /// Segments along it.
    pub count: u32,
    /// Of those, kills.
    pub kills: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Sight {
    pub segments: usize,
    pub median: f32,
    /// The longest segments, longest first.
    pub longest: Vec<Line>,
    /// The pairs of bins most fought across, most first.
    pub commonest: Vec<Line>,
    /// Per weapon definition, segments per [`BUCKET`] of distance.
    pub by_weapon: BTreeMap<u32, Vec<u32>>,
}

fn bucket(d: f32) -> usize {
    ((d / BUCKET).floor().max(0.0) as usize).min(BUCKETS - 1)
}

#[derive(Default)]
struct Pair {
    a: Mean,
    b: Mean,
    distance: f64,
    count: u32,
    kills: u32,
}

pub fn sight(traces: &[Trace]) -> Result<Sight, TooFew> {
    let mut lines: Vec<Line> = Vec::new();
    let mut pairs: BTreeMap<(Cell, Cell), Pair> = BTreeMap::new();
    let mut by_weapon: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for e in traces.iter().flat_map(|t| &t.events) {
        let Some((from, to)) = e.segment() else {
            continue;
        };
        let (attacker, kill) = match e {
            Event::Damage { attacker, .. } => (attacker, false),
            Event::Kill { killer, .. } => (killer, true),
            _ => continue,
        };
        let distance = from.distance(&to);
        lines.push(Line {
            a: from.pos,
            b: to.pos,
            distance,
            count: 1,
            kills: u32::from(kill),
        });
        // Either way round: the cells in order, the places with them.
        let (ca, cb) = (from.cell(), to.cell());
        let (key, pa, pb) = if ca <= cb {
            ((ca, cb), from.pos, to.pos)
        } else {
            ((cb, ca), to.pos, from.pos)
        };
        let p = pairs.entry(key).or_default();
        p.a.add(pa);
        p.b.add(pb);
        p.distance += f64::from(distance);
        p.count += 1;
        p.kills += u32::from(kill);
        let weapon = attacker
            .and_then(|a| a.weapon)
            .and_then(|w| w.definition)
            .unwrap_or(UNKNOWN_WEAPON);
        by_weapon.entry(weapon).or_insert_with(|| vec![0; BUCKETS])[bucket(distance)] += 1;
    }
    let segments = lines.len();
    enough("sight", "segments", segments, MIN_SEGMENTS)?;
    lines.sort_by(|a, b| b.distance.total_cmp(&a.distance));
    let median = lines[segments / 2].distance;
    lines.truncate(TOP);
    let mut commonest: Vec<Line> = pairs
        .into_values()
        .map(|p| Line {
            a: p.a.get(),
            b: p.b.get(),
            distance: (p.distance / f64::from(p.count)) as f32,
            count: p.count,
            kills: p.kills,
        })
        .collect();
    commonest.sort_by(|a, b| b.count.cmp(&a.count).then(b.distance.total_cmp(&a.distance)));
    commonest.truncate(TOP);
    Ok(Sight {
        segments,
        median,
        longest: lines,
        commonest,
        by_weapon,
    })
}

#[cfg(test)]
mod tests {
    use super::super::invent::*;
    use super::*;

    #[test]
    fn lines_are_ranked_and_weapons_histogrammed() {
        let mut events = Vec::new();
        // Nine short hits from polygon 1 to 2 (2 units), with a rifle.
        for i in 0..9 {
            events.push(damage(i, party(0, 0, on(1, 1.0), Some(1)), party(1, 1, on(2, 3.0), None)));
        }
        // The same pair the other way round, as a kill.
        events.push(kill(9, party(1, 1, on(2, 3.0), Some(1)), party(0, 0, on(1, 1.0), None)));
        // One long sniper kill, 60 units, from polygon 7.
        events.push(kill(10, party(0, 0, on(7, 61.0), Some(5)), party(1, 1, on(9, 1.0), None)));
        // A suicide is no segment.
        events.push(kill(11, party(1, 1, on(2, 3.0), None), party(1, 1, on(2, 3.0), None)));
        let s = sight(&[trace(900, events)]).unwrap();
        assert_eq!(s.segments, 11);
        assert_eq!(s.longest[0].distance, 60.0);
        assert_eq!(s.longest[0].kills, 1);
        assert_eq!(s.median, 2.0);
        let c = &s.commonest[0];
        assert_eq!((c.count, c.kills, c.distance), (10, 1, 2.0));
        assert_eq!(c.a, [1.0, 0.0, 0.0]);
        assert_eq!(s.by_weapon[&0xd001][0], 10);
        assert_eq!(s.by_weapon[&0xd005][BUCKETS - 1], 1);
        assert_eq!(bucket(4.99), 0);
        assert_eq!(bucket(5.0), 1);
    }

    #[test]
    fn segments_without_both_places_are_not_counted() {
        let mut events = Vec::new();
        for i in 0..20 {
            let mut v = party(1, 1, on(2, 3.0), None);
            v.at = None;
            events.push(damage(i, party(0, 0, on(1, 1.0), None), v));
        }
        let e = sight(&[trace(900, events)]).unwrap_err();
        assert_eq!((e.have, e.need), (0, MIN_SEGMENTS));
    }
}
