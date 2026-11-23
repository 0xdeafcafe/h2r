//! Weapon use (docs/editor/playtest.md, "What is measured"): per netgame
//! equipment placement, its pickups a minute, the time to its first
//! pickup, and the kills made with what was taken from it; the placements
//! never picked from.

use std::collections::BTreeMap;

use serde::Serialize;

use super::{Event, TICK_RATE, TooFew, Trace, V3, enough};
use crate::metrics::KillKind;

/// Fewer pickups than this are refused.
pub const MIN_PICKUPS: usize = 3;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Placement {
    /// The placement's index in the first trace's header.
    pub item: u16,
    pub at: V3,
    pub respawn_s: Option<u16>,
    pub pickups: u32,
    /// Over every trace's game minutes.
    pub per_minute: f32,
    /// From the match's start, the mean over the traces it was picked in.
    pub first_pickup_s: Option<f32>,
    /// Kills by a player holding a weapon taken from it.
    pub kills: u32,
    pub never: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Items {
    pub pickups: usize,
    /// Pickups near no placement (a dropped weapon, a grenade off a body).
    pub loose: u32,
    pub placements: Vec<Placement>,
}

#[derive(Default)]
struct Tally {
    pickups: u32,
    first_sum: f64,
    first_n: u32,
    kills: u32,
}

pub fn items(traces: &[Trace]) -> Result<Items, TooFew> {
    let header_items = traces.first().map_or(&[][..], |t| &t.header.items[..]);
    enough("items", "equipment placements", header_items.len(), 1)?;
    let mut per: BTreeMap<u16, Tally> = BTreeMap::new();
    let mut loose = 0;
    let mut pickups = 0;
    let mut minutes = 0.0f32;
    for t in traces {
        minutes += t.header.seconds() / 60.0;
        // Weapon objects taken from a placement in this match.
        let mut from: BTreeMap<u32, u16> = BTreeMap::new();
        let mut first: BTreeMap<u16, u32> = BTreeMap::new();
        for e in &t.events {
            match e {
                Event::Pickup {
                    tick, weapon, item, ..
                } => {
                    pickups += 1;
                    let Some(i) = item else {
                        loose += 1;
                        continue;
                    };
                    per.entry(*i).or_default().pickups += 1;
                    first.entry(*i).or_insert(*tick);
                    if let Some(w) = weapon {
                        from.insert(w.datum, *i);
                    }
                }
                Event::Kill {
                    killer: Some(k),
                    kind: KillKind::Kill,
                    ..
                } => {
                    if let Some(i) = k.weapon.and_then(|w| from.get(&w.datum)) {
                        per.entry(*i).or_default().kills += 1;
                    }
                }
                _ => {}
            }
        }
        for (i, tick) in first {
            let p = per.entry(i).or_default();
            p.first_sum +=
                f64::from(tick.saturating_sub(t.header.start_tick)) / f64::from(TICK_RATE);
            p.first_n += 1;
        }
    }
    enough("items", "pickups", pickups, MIN_PICKUPS)?;
    let empty = Tally::default();
    let placements = header_items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| {
            let i = u16::try_from(i).ok()?;
            let p = per.get(&i).unwrap_or(&empty);
            Some(Placement {
                item: i,
                at: item.place.pos,
                respawn_s: item.respawn_s,
                pickups: p.pickups,
                per_minute: if minutes > 0.0 {
                    p.pickups as f32 / minutes
                } else {
                    0.0
                },
                first_pickup_s: (p.first_n > 0)
                    .then(|| (p.first_sum / f64::from(p.first_n)) as f32),
                kills: p.kills,
                never: p.pickups == 0,
            })
        })
        .collect();
    Ok(Items {
        pickups,
        loose,
        placements,
    })
}

#[cfg(test)]
mod tests {
    use super::super::invent::*;
    use super::super::{Item, Weapon};
    use super::*;

    fn pickup(tick: u32, player: u8, datum: Option<u32>, item: Option<u16>) -> Event {
        Event::Pickup {
            tick,
            player,
            at: on(0, 0.0),
            weapon: datum.map(|datum| Weapon {
                datum,
                definition: None,
            }),
            item,
        }
    }

    fn header_items(t: &mut Trace) {
        t.header.items = (0..3)
            .map(|i| Item {
                place: on(i, i as f32),
                respawn_s: Some(30),
            })
            .collect();
    }

    #[test]
    fn pickups_kills_and_unused_placements() {
        let mut events = vec![
            // The rocket launcher at placement 0, 30 s in, kills twice.
            pickup(900, 0, Some(0x77), Some(0)),
            kill(1000, party(0, 0, on(1, 1.0), Some(0x77)), party(1, 1, on(2, 2.0), None)),
            kill(1100, party(0, 0, on(1, 1.0), Some(0x77)), party(1, 1, on(2, 2.0), None)),
            // A kill with the starting rifle counts for no placement.
            kill(1200, party(0, 0, on(1, 1.0), Some(0x10)), party(1, 1, on(2, 2.0), None)),
            // Grenades at placement 1, 60 s in; a weapon off a body.
            pickup(1800, 1, None, Some(1)),
            pickup(1900, 1, Some(0x99), None),
        ];
        events.push(pickup(2700, 1, Some(0x78), Some(0)));
        let mut t = trace(3600, events);
        header_items(&mut t);
        let i = items(&[t]).unwrap();
        assert_eq!((i.pickups, i.loose), (4, 1));
        let p = &i.placements;
        assert_eq!(p.len(), 3);
        assert_eq!((p[0].pickups, p[0].kills), (2, 2));
        assert_eq!(p[0].first_pickup_s, Some(30.0));
        // Two pickups over two game minutes.
        assert_eq!(p[0].per_minute, 1.0);
        assert_eq!((p[1].pickups, p[1].first_pickup_s), (1, Some(60.0)));
        assert!(p[2].never && !p[0].never);
        assert_eq!(p[2].first_pickup_s, None);
        assert_eq!(p[0].respawn_s, Some(30));
    }

    #[test]
    fn too_few_pickups_or_no_placements_are_refused() {
        let mut t = trace(3600, vec![pickup(10, 0, Some(1), Some(0)); 2]);
        header_items(&mut t);
        let e = items(&[t]).unwrap_err();
        assert_eq!((e.what, e.have, e.need), ("pickups", 2, MIN_PICKUPS));
        let bare = trace(3600, vec![pickup(10, 0, Some(1), Some(0)); 5]);
        assert_eq!(items(&[bare]).unwrap_err().what, "equipment placements");
    }
}
