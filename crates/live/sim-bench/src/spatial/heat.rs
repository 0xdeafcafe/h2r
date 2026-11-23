//! Kills and deaths, binned on the nav mesh's polygons
//! (docs/editor/playtest.md, "What is measured"): two heat layers, and the
//! same split by team and by weapon.

use std::collections::BTreeMap;

use serde::Serialize;

use super::{Bin, Bins, Event, TooFew, Trace, enough};
use crate::metrics::KillKind;

/// Fewer deaths than this draw no layer.
pub const MIN_DEATHS: usize = 10;

/// A weapon key for a kill whose killer's weapon is not known.
pub const UNKNOWN_WEAPON: u32 = u32::MAX;

/// One pair of layers.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Layers {
    /// Where killers stood.
    pub kills: Vec<Bin>,
    /// Where victims fell.
    pub deaths: Vec<Bin>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Heat {
    /// Where killers stood: kills and betrayals (not suicides or the
    /// world's).
    pub kills: Vec<Bin>,
    /// Where every death fell.
    pub deaths: Vec<Bin>,
    /// By the team of the killer (kills) and of the victim (deaths).
    pub by_team: BTreeMap<i8, Layers>,
    /// By the definition of the killer's weapon, both layers.
    pub by_weapon: BTreeMap<u32, Layers>,
}

#[derive(Default)]
struct Pair {
    kills: Bins,
    deaths: Bins,
}

impl Pair {
    fn done(self) -> Layers {
        Layers {
            kills: self.kills.into_vec(),
            deaths: self.deaths.into_vec(),
        }
    }
}

pub fn heat(traces: &[Trace]) -> Result<Heat, TooFew> {
    let mut all = Pair::default();
    let mut teams: BTreeMap<i8, Pair> = BTreeMap::new();
    let mut weapons: BTreeMap<u32, Pair> = BTreeMap::new();
    let mut deaths = 0;
    for e in traces.iter().flat_map(|t| &t.events) {
        let Event::Kill {
            killer,
            victim,
            kind,
            ..
        } = e
        else {
            continue;
        };
        let weapon = killer
            .and_then(|k| k.weapon)
            .and_then(|w| w.definition)
            .unwrap_or(UNKNOWN_WEAPON);
        if let Some(at) = &victim.at {
            deaths += 1;
            all.deaths.add(at);
            if let Some(t) = victim.team {
                teams.entry(t).or_default().deaths.add(at);
            }
            weapons.entry(weapon).or_default().deaths.add(at);
        }
        if matches!(kind, KillKind::Kill | KillKind::Betrayal)
            && let Some(k) = killer
            && let Some(at) = &k.at
        {
            all.kills.add(at);
            if let Some(t) = k.team {
                teams.entry(t).or_default().kills.add(at);
            }
            weapons.entry(weapon).or_default().kills.add(at);
        }
    }
    enough("heat", "deaths", deaths, MIN_DEATHS)?;
    let all = all.done();
    Ok(Heat {
        kills: all.kills,
        deaths: all.deaths,
        by_team: teams.into_iter().map(|(t, p)| (t, p.done())).collect(),
        by_weapon: weapons.into_iter().map(|(w, p)| (w, p.done())).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::super::Cell;
    use super::super::invent::*;
    use super::*;

    #[test]
    fn deaths_and_kills_are_binned_by_polygon_team_and_weapon() {
        let mut events = Vec::new();
        // Seven deaths on polygon 3 by team 0's rifle from polygon 1, and
        // three on polygon 4 by team 1's shotgun from polygon 4.
        for i in 0..7 {
            events.push(kill(
                i,
                party(0, 0, on(1, 1.0), Some(1)),
                party(1, 1, on(3, 3.0 + i as f32 * 0.1), None),
            ));
        }
        for i in 0..3 {
            events.push(kill(
                10 + i,
                party(1, 1, on(4, 4.0), Some(2)),
                party(0, 0, on(4, 4.5), None),
            ));
        }
        // A fall: a death with no kill.
        events.push(Event::Kill {
            tick: 20,
            killer: None,
            victim: party(0, 0, on(9, 9.0), None),
            kind: KillKind::World,
        });
        let h = heat(&[trace(900, events)]).unwrap();
        assert_eq!(h.deaths[0].cell, Cell::Poly(3));
        assert_eq!(h.deaths[0].count, 7);
        assert!((h.deaths[0].at[0] - 3.3).abs() < 1e-4);
        assert_eq!(h.deaths.iter().map(|b| b.count).sum::<u32>(), 11);
        assert_eq!(h.kills.iter().map(|b| b.count).sum::<u32>(), 10);
        assert_eq!(h.by_team[&0].kills[0].count, 7);
        assert_eq!(h.by_team[&0].deaths.iter().map(|b| b.count).sum::<u32>(), 4);
        assert_eq!(h.by_weapon[&0xd002].kills[0].cell, Cell::Poly(4));
        assert_eq!(h.by_weapon[&UNKNOWN_WEAPON].deaths[0].cell, Cell::Poly(9));
        assert!(h.by_weapon[&UNKNOWN_WEAPON].kills.is_empty());
    }

    #[test]
    fn a_place_off_the_mesh_is_binned_on_the_grid() {
        let mut events = Vec::new();
        for i in 0..10 {
            let mut victim = party(1, 1, on(0, 5.0), None);
            victim.at.as_mut().unwrap().poly = None;
            events.push(kill(i, party(0, 0, on(1, 1.0), None), victim));
        }
        let h = heat(&[trace(900, events)]).unwrap();
        assert_eq!(h.deaths[0].cell, Cell::Grid(2, 0));
    }

    #[test]
    fn too_few_deaths_are_refused() {
        let events = (0..9)
            .map(|i| kill(i, party(0, 0, on(1, 1.0), None), party(1, 1, on(2, 2.0), None)))
            .collect();
        let e = heat(&[trace(900, events)]).unwrap_err();
        assert_eq!((e.have, e.need), (9, MIN_DEATHS));
        assert_eq!(e.to_string(), "heat: too few deaths (9, at least 10 needed)");
    }
}
