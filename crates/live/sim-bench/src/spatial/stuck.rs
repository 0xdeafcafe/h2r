//! Stuck spots and nav holes (docs/editor/playtest.md, "What is
//! measured"): the bots' stuck spells and repaths by bin, and the islands
//! of the mesh no starting location reaches along its links.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::Serialize;

use super::{Cell, Event, Mean, Mesh, TooFew, Trace, V3, enough};

/// Fewer position samples than this (a minute of one player's play) say
/// too little about where nobody got stuck, and are refused.
pub const MIN_SAMPLES: usize = 300;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Spot {
    pub cell: Cell,
    pub at: V3,
    pub spells: u32,
    /// Ticks stuck, over its spells.
    pub ticks: u64,
    pub repaths: u32,
    /// Where the first bot stuck here was going.
    pub toward: Option<V3>,
}

/// Polygons joined by links that no starting location reaches.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Island {
    pub polys: Vec<u32>,
    /// The mean of their centres.
    pub at: V3,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Stuck {
    pub samples: usize,
    /// The most ticks stuck first.
    pub spots: Vec<Spot>,
    /// The first trace's mesh's unreached islands, the largest first;
    /// `None` without a mesh or a start on it.
    pub islands: Option<Vec<Island>>,
}

#[derive(Default)]
struct Acc {
    at: Mean,
    spells: u32,
    ticks: u64,
    repaths: u32,
    toward: Option<V3>,
}

pub fn stuck(traces: &[Trace]) -> Result<Stuck, TooFew> {
    let mut spots: BTreeMap<Cell, Acc> = BTreeMap::new();
    let mut samples = 0;
    for e in traces.iter().flat_map(|t| &t.events) {
        match e {
            Event::Sample { .. } => samples += 1,
            Event::Stuck {
                at, toward, ticks, ..
            } => {
                let s = spots.entry(at.cell()).or_default();
                s.at.add(at.pos);
                s.spells += 1;
                s.ticks += u64::from(*ticks);
                if s.toward.is_none() {
                    s.toward = *toward;
                }
            }
            Event::Repath { at, toward, .. } => {
                let s = spots.entry(at.cell()).or_default();
                s.at.add(at.pos);
                s.repaths += 1;
                if s.toward.is_none() {
                    s.toward = *toward;
                }
            }
            _ => {}
        }
    }
    enough("stuck", "position samples", samples, MIN_SAMPLES)?;
    let mut spots: Vec<Spot> = spots
        .into_iter()
        .map(|(cell, a)| Spot {
            cell,
            at: a.at.get(),
            spells: a.spells,
            ticks: a.ticks,
            repaths: a.repaths,
            toward: a.toward,
        })
        .collect();
    spots.sort_by(|a, b| {
        b.ticks
            .cmp(&a.ticks)
            .then(b.repaths.cmp(&a.repaths))
            .then(a.cell.cmp(&b.cell))
    });
    let islands = traces.iter().find(|t| t.header.mesh.is_some()).and_then(|t| {
        let seeds: Vec<u32> = t.header.starts.iter().filter_map(|s| s.place.poly).collect();
        islands(t.header.mesh.as_ref()?, &seeds)
    });
    Ok(Stuck {
        samples,
        spots,
        islands,
    })
}

/// The polygons `seeds` do not reach along `mesh`'s links, grouped by the
/// links among them either way; `None` with no seed on the mesh.
pub fn islands(mesh: &Mesh, seeds: &[u32]) -> Option<Vec<Island>> {
    let n = mesh.centers.len();
    let mut out: Vec<Vec<u32>> = vec![Vec::new(); n];
    let mut both: Vec<Vec<u32>> = vec![Vec::new(); n];
    for &(a, b) in &mesh.links {
        if (a as usize) < n && (b as usize) < n {
            out[a as usize].push(b);
            both[a as usize].push(b);
            both[b as usize].push(a);
        }
    }
    let mut reached = vec![false; n];
    let mut queue: VecDeque<u32> = seeds.iter().copied().filter(|&s| (s as usize) < n).collect();
    if queue.is_empty() {
        return None;
    }
    for &s in &queue {
        reached[s as usize] = true;
    }
    while let Some(p) = queue.pop_front() {
        for &q in &out[p as usize] {
            if !std::mem::replace(&mut reached[q as usize], true) {
                queue.push_back(q);
            }
        }
    }
    let mut seen: BTreeSet<u32> = BTreeSet::new();
    let mut islands = Vec::new();
    for p in 0..n as u32 {
        if reached[p as usize] || !seen.insert(p) {
            continue;
        }
        let mut polys = vec![p];
        let mut queue = VecDeque::from([p]);
        while let Some(a) = queue.pop_front() {
            for &b in &both[a as usize] {
                if !reached[b as usize] && seen.insert(b) {
                    polys.push(b);
                    queue.push_back(b);
                }
            }
        }
        polys.sort_unstable();
        let mut at = Mean::default();
        for &q in &polys {
            at.add(mesh.centers[q as usize]);
        }
        islands.push(Island { polys, at: at.get() });
    }
    islands.sort_by(|a, b| b.polys.len().cmp(&a.polys.len()).then(a.polys.cmp(&b.polys)));
    Some(islands)
}

#[cfg(test)]
mod tests {
    use super::super::invent::*;
    use super::super::{Mesh, Start};
    use super::*;

    fn samples(n: u32) -> Vec<Event> {
        (0..n).map(|i| sample(i * 6, 0, on(0, 0.0))).collect()
    }

    #[test]
    fn spells_and_repaths_gather_at_their_spots() {
        let mut events = samples(300);
        for (tick, ticks) in [(100, 40), (400, 20)] {
            events.push(Event::Stuck {
                tick,
                player: 2,
                at: on(5, 5.0),
                toward: Some([9.0, 0.0, 0.0]),
                ticks,
            });
        }
        events.push(Event::Repath {
            tick: 500,
            player: 2,
            at: on(5, 5.2),
            toward: None,
        });
        events.push(Event::Repath {
            tick: 600,
            player: 3,
            at: on(8, 8.0),
            toward: None,
        });
        let s = stuck(&[trace(2000, events)]).unwrap();
        assert_eq!(s.spots.len(), 2);
        let a = &s.spots[0];
        assert_eq!((a.cell, a.spells, a.ticks, a.repaths), (Cell::Poly(5), 2, 60, 1));
        assert_eq!(a.toward, Some([9.0, 0.0, 0.0]));
        assert_eq!((s.spots[1].cell, s.spots[1].ticks), (Cell::Poly(8), 0));
        assert_eq!(s.islands, None);
    }

    #[test]
    fn islands_no_start_reaches_are_found() {
        // 0 ↔ 1 → 2; 3 ↔ 4 apart; 5 drops into 0 but is never reached.
        let mesh = Mesh {
            centers: (0..6).map(|i| [i as f32, 0.0, 0.0]).collect(),
            links: vec![(0, 1), (1, 0), (1, 2), (3, 4), (4, 3), (5, 0)],
        };
        let found = islands(&mesh, &[0]).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].polys, [3, 4]);
        assert_eq!(found[0].at, [3.5, 0.0, 0.0]);
        assert_eq!(found[1].polys, [5]);
        // From 2 nothing else is reached: 0, 1 and 5 join one island.
        let from_two = islands(&mesh, &[2]).unwrap();
        assert_eq!(from_two[0].polys, [0, 1, 5]);
        assert_eq!(islands(&mesh, &[]), None);
        assert_eq!(islands(&mesh, &[99]), None);
        // Through a trace: its starts are the seeds.
        let mut t = trace(2000, samples(300));
        t.header.mesh = Some(mesh);
        t.header.starts = vec![Start {
            place: on(0, 0.0),
            team: None,
        }];
        assert_eq!(stuck(&[t]).unwrap().islands.unwrap().len(), 2);
    }

    #[test]
    fn too_little_play_is_refused() {
        let e = stuck(&[trace(2000, samples(10))]).unwrap_err();
        assert_eq!((e.measure, e.have), ("stuck", 10));
    }
}
