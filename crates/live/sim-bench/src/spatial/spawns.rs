//! Spawn fairness and time to first engagement (docs/editor/playtest.md,
//! "What is measured"): per starting location its uses, the seconds its
//! players lived, the deaths within [`SPAWN_KILL_TICKS`], the enemies in
//! sight at the spawn and the time to the first fight; the spawn kills as
//! marks; and each team's opening, the time to its first engagement.

use std::collections::BTreeMap;

use serde::Serialize;

use super::{Event, Mean, TICK_RATE, TooFew, Trace, V3, enough};

/// Fewer spawns than this are refused.
pub const MIN_SPAWNS: usize = 8;
/// A death this soon after its spawn is a spawn kill (three seconds).
pub const SPAWN_KILL_TICKS: u32 = 90;

/// One starting location's numbers; `start: None` gathers the spawns
/// near none.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StartRow {
    pub start: Option<u16>,
    /// The location, or the mean of its spawns.
    pub at: V3,
    pub uses: u32,
    pub mean_life_s: Option<f32>,
    /// Deaths within [`SPAWN_KILL_TICKS`] of the spawn.
    pub spawn_deaths: u32,
    /// Living enemies with a line to the spawn, on average; `None` where
    /// no spawn could tell.
    pub mean_in_sight: Option<f32>,
    /// The share of spawns with at least one enemy in sight.
    pub seen_share: Option<f32>,
    pub mean_nearest_enemy: Option<f32>,
    /// From the spawn to the first damage given or taken, over the lives
    /// that had one.
    pub mean_first_engagement_s: Option<f32>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Spawns {
    pub spawns: usize,
    /// Every starting location of the first trace's header, used or not,
    /// then the spawns near none.
    pub starts: Vec<StartRow>,
    /// Where the spawn kills fell.
    pub spawn_kills: Vec<V3>,
    /// Per team, the mean over traces of the seconds from the match's
    /// start to the team's first engagement.
    pub opening: BTreeMap<i8, f32>,
}

/// One life, from its spawn.
struct Life {
    start: Option<u16>,
    at: V3,
    spawn: u32,
    end: Option<u32>,
    death_at: Option<V3>,
    engaged: Option<u32>,
    in_sight: Option<u8>,
    nearest: Option<f32>,
}

#[derive(Default)]
struct Row {
    at: Mean,
    uses: u32,
    life_ticks: u64,
    lives_ended: u32,
    spawn_deaths: u32,
    sight_sum: u32,
    sight_n: u32,
    seen: u32,
    nearest_sum: f64,
    nearest_n: u32,
    engaged_ticks: u64,
    engaged_n: u32,
}

fn mean(sum: f64, n: u32) -> Option<f32> {
    (n > 0).then(|| (sum / f64::from(n)) as f32)
}

pub fn spawns(traces: &[Trace]) -> Result<Spawns, TooFew> {
    let mut rows: BTreeMap<Option<u16>, Row> = BTreeMap::new();
    let mut spawn_kills = Vec::new();
    let mut openings: BTreeMap<i8, (f64, u32)> = BTreeMap::new();
    let mut count = 0;
    for t in traces {
        let mut lives: Vec<Life> = Vec::new();
        let mut open: BTreeMap<u8, usize> = BTreeMap::new();
        let mut first: BTreeMap<i8, u32> = BTreeMap::new();
        for e in &t.events {
            match e {
                Event::Spawn {
                    tick,
                    player,
                    at,
                    start,
                    nearest,
                    in_sight,
                    ..
                } => {
                    open.insert(*player, lives.len());
                    lives.push(Life {
                        start: *start,
                        at: at.pos,
                        spawn: *tick,
                        end: None,
                        death_at: None,
                        engaged: None,
                        in_sight: *in_sight,
                        nearest: nearest.map(|n| n.distance),
                    });
                }
                Event::Damage {
                    tick,
                    attacker: Some(a),
                    victim,
                    ..
                }
                | Event::Kill {
                    tick,
                    killer: Some(a),
                    victim,
                    ..
                } if a.player != victim.player => {
                    for p in [a.player, victim.player] {
                        if let Some(&i) = open.get(&p) {
                            lives[i].engaged.get_or_insert(*tick);
                        }
                    }
                    for team in [a.team, victim.team].into_iter().flatten() {
                        first.entry(team).or_insert(*tick);
                    }
                }
                _ => {}
            }
            if let Event::Kill { tick, victim, .. } = e
                && let Some(i) = open.remove(&victim.player)
            {
                lives[i].end = Some(*tick);
                lives[i].death_at = victim.at.map(|p| p.pos);
            }
        }
        count += lives.len();
        for l in &lives {
            let r = rows.entry(l.start).or_default();
            r.at.add(l.at);
            r.uses += 1;
            let end = l.end.unwrap_or(t.header.end_tick);
            r.life_ticks += u64::from(end.saturating_sub(l.spawn));
            r.lives_ended += 1;
            if let Some(d) = l.end
                && d.saturating_sub(l.spawn) <= SPAWN_KILL_TICKS
            {
                r.spawn_deaths += 1;
                if let Some(p) = l.death_at {
                    spawn_kills.push(p);
                }
            }
            if let Some(n) = l.in_sight {
                r.sight_sum += u32::from(n);
                r.sight_n += 1;
                r.seen += u32::from(n > 0);
            }
            if let Some(d) = l.nearest {
                r.nearest_sum += f64::from(d);
                r.nearest_n += 1;
            }
            if let Some(e) = l.engaged {
                r.engaged_ticks += u64::from(e.saturating_sub(l.spawn));
                r.engaged_n += 1;
            }
        }
        for (team, tick) in first {
            let o = openings.entry(team).or_default();
            o.0 += f64::from(tick.saturating_sub(t.header.start_tick)) / f64::from(TICK_RATE);
            o.1 += 1;
        }
    }
    enough("spawns", "spawns", count, MIN_SPAWNS)?;
    let ticks = |t: u64, n: u32| mean(t as f64 / f64::from(TICK_RATE), n);
    let row = |start: Option<u16>, at: V3, r: &Row| StartRow {
        start,
        at,
        uses: r.uses,
        mean_life_s: ticks(r.life_ticks, r.lives_ended),
        spawn_deaths: r.spawn_deaths,
        mean_in_sight: mean(f64::from(r.sight_sum), r.sight_n),
        seen_share: mean(f64::from(r.seen), r.sight_n),
        mean_nearest_enemy: mean(r.nearest_sum, r.nearest_n),
        mean_first_engagement_s: ticks(r.engaged_ticks, r.engaged_n),
    };
    let empty = Row::default();
    let header_starts = traces.first().map_or(&[][..], |t| &t.header.starts[..]);
    let mut starts: Vec<StartRow> = header_starts
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let i = u16::try_from(i).ok();
            row(i, s.place.pos, rows.get(&i).unwrap_or(&empty))
        })
        .collect();
    // Spawns on starts past the first header's (another map's trace),
    // then those near none.
    for (k, r) in &rows {
        if !starts.iter().any(|s| s.start == *k) {
            starts.push(row(*k, r.at.get(), r));
        }
    }
    Ok(Spawns {
        spawns: count,
        starts,
        spawn_kills,
        opening: openings
            .into_iter()
            .map(|(t, (s, n))| (t, (s / f64::from(n)) as f32))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::super::invent::*;
    use super::super::{Nearest, Start};
    use super::*;

    fn spawn_seen(tick: u32, player: u8, team: i8, start: u16, seen: u8, near: f32) -> Event {
        Event::Spawn {
            tick,
            player,
            team: Some(team),
            at: on(start.into(), f32::from(start)),
            start: Some(start),
            nearest: Some(Nearest {
                player: 9,
                distance: near,
            }),
            in_sight: Some(seen),
        }
    }

    #[test]
    fn a_start_in_sight_of_the_enemy_dies_early() {
        let mut events = Vec::new();
        // Start 0: four spawns of player 1, each killed a second later,
        // with two enemies in sight. Start 1: four of player 0, who lives
        // to the end and first fights 10 s in.
        for i in 0..4u32 {
            let t = i * 300;
            events.push(spawn_seen(t, 1, 1, 0, 2, 6.0));
            events.push(spawn_seen(t, 0, 0, 1, 0, 30.0));
            events.push(damage(
                t + 30,
                party(0, 0, on(1, 1.0), Some(1)),
                party(1, 1, on(0, 0.0), None),
            ));
            events.push(kill(
                t + 30,
                party(0, 0, on(1, 1.0), Some(1)),
                party(1, 1, on(0, 0.5), None),
            ));
            // Player 0 dies to the world before the next round's spawn.
            events.push(Event::Kill {
                tick: t + 299,
                killer: None,
                victim: party(0, 0, on(1, 1.0), None),
                kind: crate::metrics::KillKind::World,
            });
        }
        let mut tr = trace(1200, events);
        tr.header.starts = vec![
            Start {
                place: on(0, 0.0),
                team: Some(1),
            },
            Start {
                place: on(1, 1.0),
                team: Some(0),
            },
            Start {
                place: on(2, 2.0),
                team: Some(0),
            },
        ];
        let s = spawns(&[tr]).unwrap();
        assert_eq!(s.spawns, 8);
        let a = &s.starts[0];
        assert_eq!((a.start, a.uses, a.spawn_deaths), (Some(0), 4, 4));
        assert_eq!(a.mean_life_s, Some(1.0));
        assert_eq!(a.mean_in_sight, Some(2.0));
        assert_eq!(a.seen_share, Some(1.0));
        assert_eq!(a.mean_nearest_enemy, Some(6.0));
        assert_eq!(a.mean_first_engagement_s, Some(1.0));
        let b = &s.starts[1];
        assert_eq!((b.uses, b.spawn_deaths), (4, 0));
        assert_eq!(b.seen_share, Some(0.0));
        assert!((b.mean_life_s.unwrap() - 299.0 / 30.0).abs() < 1e-4);
        // An unused start is listed with no numbers.
        let c = &s.starts[2];
        assert_eq!((c.start, c.uses, c.mean_life_s), (Some(2), 0, None));
        assert_eq!(s.spawn_kills.len(), 4);
        assert_eq!(s.spawn_kills[0], [0.5, 0.0, 0.0]);
        // Both teams first fought a second in.
        assert_eq!(s.opening[&0], 1.0);
        assert_eq!(s.opening[&1], 1.0);
    }

    #[test]
    fn too_few_spawns_are_refused() {
        let events = (0..7)
            .map(|i| spawn(i * 30, 0, 0, Some(0), on(0, 0.0)))
            .collect();
        let e = spawns(&[trace(900, events)]).unwrap_err();
        assert_eq!((e.measure, e.have, e.need), ("spawns", 7, MIN_SPAWNS));
    }
}
