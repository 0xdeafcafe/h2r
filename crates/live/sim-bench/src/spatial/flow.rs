//! Traffic and flow (docs/editor/playtest.md, "What is measured"): the
//! time players spend in each bin, from the position samples; the
//! crossings from bin to bin with their direction; and the polygons of the
//! mesh no sample entered.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use super::{Cell, Event, Mean, SAMPLE_EVERY, TICK_RATE, TooFew, Trace, V3, enough};

/// Fewer samples than this (a minute of one player) are refused.
pub const MIN_SAMPLES: usize = 300;

/// Time spent in one bin.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Dwell {
    pub cell: Cell,
    pub at: V3,
    /// Player-seconds.
    pub seconds: f32,
}

/// Players seen in `from` at one sample and in `to` at the next of the
/// same life.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Crossing {
    pub from: Cell,
    pub to: Cell,
    /// The mean places on each side.
    pub a: V3,
    pub b: V3,
    pub count: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Flow {
    pub samples: usize,
    /// The most visited first.
    pub cells: Vec<Dwell>,
    /// The most crossed first.
    pub crossings: Vec<Crossing>,
    /// The polygons of the first trace's mesh no sample was on; `None`
    /// without a mesh.
    pub never: Option<Vec<u32>>,
}

pub fn flow(traces: &[Trace]) -> Result<Flow, TooFew> {
    let mut dwell: BTreeMap<Cell, (Mean, u32)> = BTreeMap::new();
    let mut crossed: BTreeMap<(Cell, Cell), (Mean, Mean, u32)> = BTreeMap::new();
    let mut samples = 0;
    for t in traces {
        // Each player's last sampled bin and place in this life.
        let mut last: BTreeMap<u8, (Cell, V3)> = BTreeMap::new();
        for e in &t.events {
            match e {
                Event::Sample { player, at, .. } => {
                    samples += 1;
                    let cell = at.cell();
                    let d = dwell.entry(cell).or_default();
                    d.0.add(at.pos);
                    d.1 += 1;
                    if let Some((from, a)) = last.insert(*player, (cell, at.pos))
                        && from != cell
                    {
                        let c = crossed.entry((from, cell)).or_default();
                        c.0.add(a);
                        c.1.add(at.pos);
                        c.2 += 1;
                    }
                }
                Event::Spawn { player, .. } => {
                    last.remove(player);
                }
                Event::Kill { victim, .. } => {
                    last.remove(&victim.player);
                }
                _ => {}
            }
        }
    }
    enough("flow", "position samples", samples, MIN_SAMPLES)?;
    let per_sample = SAMPLE_EVERY as f32 / TICK_RATE;
    let never = traces.iter().find_map(|t| t.header.mesh.as_ref()).map(|m| {
        let seen: BTreeSet<u32> = dwell
            .keys()
            .filter_map(|c| match c {
                Cell::Poly(p) => Some(*p),
                Cell::Grid(..) => None,
            })
            .collect();
        (0..m.centers.len() as u32)
            .filter(|p| !seen.contains(p))
            .collect()
    });
    let mut cells: Vec<Dwell> = dwell
        .into_iter()
        .map(|(cell, (m, n))| Dwell {
            cell,
            at: m.get(),
            seconds: n as f32 * per_sample,
        })
        .collect();
    cells.sort_by(|a, b| b.seconds.total_cmp(&a.seconds).then(a.cell.cmp(&b.cell)));
    let mut crossings: Vec<Crossing> = crossed
        .into_iter()
        .map(|((from, to), (a, b, count))| Crossing {
            from,
            to,
            a: a.get(),
            b: b.get(),
            count,
        })
        .collect();
    crossings.sort_by(|a, b| b.count.cmp(&a.count).then((a.from, a.to).cmp(&(b.from, b.to))));
    Ok(Flow {
        samples,
        cells,
        crossings,
        never,
    })
}

#[cfg(test)]
mod tests {
    use super::super::Mesh;
    use super::super::invent::*;
    use super::*;

    /// Player 0 walks polygons 0 → 1 → 2 and back, 50 samples a leg, for
    /// `legs` legs; a death and respawn between legs breaks no crossing in.
    fn walk(legs: u32) -> Vec<Event> {
        let mut events = Vec::new();
        let mut tick = 0;
        for leg in 0..legs {
            let polys: [u32; 3] = if leg % 2 == 0 { [0, 1, 2] } else { [2, 1, 0] };
            for p in polys {
                for _ in 0..50 / 3 + 1 {
                    events.push(sample(tick, 0, on(p, p as f32)));
                    tick += SAMPLE_EVERY;
                }
            }
        }
        events
    }

    #[test]
    fn time_and_crossings_follow_the_samples() {
        let mut t = trace(10_000, walk(6));
        t.header.mesh = Some(Mesh {
            centers: vec![[0.0; 3]; 4],
            links: vec![],
        });
        let f = flow(&[t]).unwrap();
        // 17 samples a polygon a leg, six legs.
        assert_eq!(f.samples, 17 * 3 * 6);
        let one = f.cells.iter().find(|d| d.cell == Cell::Poly(1)).unwrap();
        assert!((one.seconds - 17.0 * 6.0 * 0.2).abs() < 1e-3);
        let count = |a, b| {
            f.crossings
                .iter()
                .find(|c| (c.from, c.to) == (Cell::Poly(a), Cell::Poly(b)))
                .map_or(0, |c| c.count)
        };
        // Out 0→1→2 on even legs, back 2→1→0 on odd; the turn at each end
        // is no crossing (same polygon).
        assert_eq!((count(0, 1), count(1, 2)), (3, 3));
        assert_eq!((count(2, 1), count(1, 0)), (3, 3));
        assert_eq!(count(0, 2), 0);
        assert_eq!(f.never, Some(vec![3]));
    }

    #[test]
    fn a_death_breaks_the_path() {
        let mut events = walk(2);
        // Teleport: die on polygon 0, spawn on polygon 2, sampled there.
        let tick = 10_000;
        events.push(kill(tick, party(1, 1, on(5, 5.0), None), party(0, 0, on(0, 0.0), None)));
        events.push(spawn(tick + 60, 0, 0, None, on(2, 2.0)));
        events.push(sample(tick + 60, 0, on(2, 2.0)));
        let mut events_more = walk(4);
        events.append(&mut events_more);
        let f = flow(&[trace(20_000, events)]).unwrap();
        let count = |a, b| {
            f.crossings
                .iter()
                .find(|c| (c.from, c.to) == (Cell::Poly(a), Cell::Poly(b)))
                .map_or(0, |c| c.count)
        };
        // No 0→2 jump from the respawn; walk(4) after it starts on 0,
        // which is a 2→0 jump in the same life.
        assert_eq!(count(0, 2), 0);
        assert_eq!(count(2, 0), 1);
        assert_eq!(f.never, None);
    }

    #[test]
    fn too_few_samples_are_refused() {
        let events = (0..299).map(|i| sample(i * 6, 0, on(0, 0.0))).collect();
        let e = flow(&[trace(2000, events)]).unwrap_err();
        assert_eq!((e.what, e.have), ("position samples", 299));
    }
}
