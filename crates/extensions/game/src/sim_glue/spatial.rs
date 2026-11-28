//! The spatial trace's reads (docs/runtime/sim.md, "The spatial trace"):
//! each tick's snapshot as [`Pose`]s, the scenario's starting locations
//! and netgame equipment from guest memory, and the bots' nav mesh and
//! walls as the trace's [`Ground`]. The recorder itself is pure
//! ([`sim_bench::spatial::Recorder`]); the observers in `sim_glue` feed it
//! beside the tally.

use std::sync::Arc;

use engine::objectives::check_span;
use guest_layout::{GuestMemory, MemFault, points};
use map::scenario_layout::{self, BlockDef, BlockKind};
use sim_bench::spatial::{Ground, Item, Mesh, Place, Pose, Start, V3, Weapon};

use crate::telemetry;

/// The most starting locations or equipment placements read.
const MAX_PLACES: u32 = 256;

/// The loaded map's bot data as the trace's ground: the polygon under a
/// point from the nav mesh, lines from the walls. Neither while no map
/// data is loaded.
pub struct MapGround(pub Option<Arc<bots::cache::MapData>>);

impl MapGround {
    pub fn current() -> MapGround {
        MapGround(bots::cache::current())
    }

    /// The current ground when the run traces; none (and no lock taken)
    /// when it does not.
    pub fn when(traces: bool) -> MapGround {
        if traces {
            MapGround::current()
        } else {
            MapGround(None)
        }
    }

    /// The mesh as the trace keeps it: polygon centres and links.
    pub fn mesh(&self) -> Option<Mesh> {
        let d = self.0.as_ref()?;
        Some(Mesh {
            centers: d.nav.polys.iter().map(|p| p.center).collect(),
            links: d.nav.edges.iter().map(|e| (e.from, e.to)).collect(),
        })
    }
}

impl Ground for MapGround {
    fn poly(&self, p: V3) -> Option<u32> {
        self.0.as_ref()?.nav.poly_near(p)
    }
    fn sees(&self) -> bool {
        self.0.as_ref().is_some_and(|d| !d.walls.tris.is_empty())
    }
    fn clear(&self, a: V3, b: V3) -> bool {
        self.0.as_ref().is_none_or(|d| d.walls.clear(a, b))
    }
}

fn v3(p: [f64; 3]) -> V3 {
    [p[0] as f32, p[1] as f32, p[2] as f32]
}

/// Each player of a tick's snapshot as the recorder takes them. Alive as
/// the tally's sample says it ([`super::tick_sample`]).
pub fn poses(s: &telemetry::Snapshot) -> Vec<Pose> {
    s.players
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let o = s.objects.get(i).and_then(Option::as_ref);
            let alive = o.is_some_and(|o| o.vitality.is_none_or(|(body, _)| body > 0.0));
            Pose {
                player: (p.player.index & 0xff) as u8,
                team: Some(p.player.team),
                alive,
                pos: o.map(|o| v3(o.pos)).unwrap_or_default(),
                facing: o.map(|o| v3(o.aim.unwrap_or(o.forward))).unwrap_or_default(),
                weapon: o.and_then(|o| o.weapon.as_ref()).map(|w| Weapon {
                    datum: w.datum,
                    definition: Some(w.definition).filter(|d| *d != u32::MAX),
                }),
                weapons: o.and_then(|o| o.motion.weapons),
                grenades: o.and_then(|o| o.motion.grenades),
            }
        })
        .collect()
}

/// A block's element count (at most [`MAX_PLACES`]) and address in the
/// scenario at `s`.
fn block(m: &dyn GuestMemory, s: u32, def: &BlockDef) -> Result<(u32, u32), MemFault> {
    check_span(s, def.offset + 8)?;
    let n = (m.i32(s + def.offset)?.max(0) as u32).min(MAX_PLACES);
    let at = m.u32(s + def.offset + 4)?;
    check_span(at, n * def.stride)?;
    Ok((n, at))
}

fn offset(def: &BlockDef, name: &str) -> u32 {
    def.field(name)
        .map(|f| f.offset)
        .unwrap_or_else(|| panic!("scenario_layout has no {name} in {}", def.label))
}

fn point(m: &dyn GuestMemory, at: u32) -> Result<Option<V3>, MemFault> {
    let p = [m.f32(at)?, m.f32(at + 4)?, m.f32(at + 8)?];
    Ok(p.iter().all(|c| c.is_finite()).then_some(p))
}

/// The loaded scenario's player starting locations and netgame equipment
/// placements, by index (a placement with a position that is not finite
/// is kept, at the origin, so indices stay the scenario's). No scenario:
/// none of either.
pub fn places(m: &dyn GuestMemory) -> Result<(Vec<Start>, Vec<Item>), MemFault> {
    let s = m.u32(points::SCENARIO.base)?;
    if s == 0 {
        return Ok(Default::default());
    }
    let at_origin = |p: Option<V3>| Place {
        pos: p.unwrap_or_default(),
        poly: None,
    };
    let def = scenario_layout::block(BlockKind::StartingLocations);
    let (pos, team) = (offset(def, "position"), offset(def, "team"));
    let (n, at) = block(m, s, def)?;
    let mut starts = Vec::with_capacity(n as usize);
    for i in 0..n {
        let e = at + i * def.stride;
        starts.push(Start {
            place: at_origin(point(m, e + pos)?),
            team: Some(m.u16(e + team)? as i16),
        });
    }
    let def = scenario_layout::block(BlockKind::NetgameEquipment);
    let (pos, respawn) = (offset(def, "position"), offset(def, "respawn time"));
    let (n, at) = block(m, s, def)?;
    let mut items = Vec::with_capacity(n as usize);
    for i in 0..n {
        let e = at + i * def.stride;
        let secs = m.u16(e + respawn)? as i16;
        items.push(Item {
            place: at_origin(point(m, e + pos)?),
            respawn_s: u16::try_from(secs).ok().filter(|s| *s > 0),
        });
    }
    Ok((starts, items))
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;
    use sim_bench::spatial::{Event, Header, NoGround, Recorder};

    const SCENARIO: u32 = 0x10_0000;

    /// A scenario with two starting locations and one equipment placement.
    fn scenario() -> FakeGuest {
        let mut m = FakeGuest::new();
        let starts = scenario_layout::block(BlockKind::StartingLocations);
        let items = scenario_layout::block(BlockKind::NetgameEquipment);
        m.put_u32(points::SCENARIO.base, SCENARIO)
            .put_u32(SCENARIO + starts.offset, 2)
            .put_u32(SCENARIO + starts.offset + 4, 0x11_0000)
            .put_u32(SCENARIO + items.offset, 1)
            .put_u32(SCENARIO + items.offset + 4, 0x12_0000);
        for (i, (x, team)) in [(1.0f32, 0u16), (12.0, 1)].into_iter().enumerate() {
            let e = 0x11_0000 + i as u32 * starts.stride;
            m.put_f32(e, x).put_f32(e + 4, 1.0).put_f32(e + 8, 2.0);
            m.put_u16(e + 0x10, team);
        }
        m.put_f32(0x12_0000 + 0x40, 5.0)
            .put_f32(0x12_0000 + 0x44, 1.5)
            .put_f32(0x12_0000 + 0x48, 2.0)
            .put_u16(0x12_0000 + 0x0e, 45);
        m
    }

    #[test]
    fn the_scenario_places_are_read_from_guest_memory() {
        let (starts, items) = places(&scenario()).unwrap();
        assert_eq!(starts.len(), 2);
        assert_eq!(starts[1].place.pos, [12.0, 1.0, 2.0]);
        assert_eq!(starts[1].team, Some(1));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].place.pos, [5.0, 1.5, 2.0]);
        assert_eq!(items[0].respawn_s, Some(45));
        // No scenario: no places. A block pointer into nothing: a fault.
        assert_eq!(places(&FakeGuest::new()).ok(), None);
        let mut none = FakeGuest::new();
        none.put_u32(points::SCENARIO.base, 0);
        assert_eq!(places(&none).unwrap(), (vec![], vec![]));
        let mut broken = scenario();
        let items = scenario_layout::block(BlockKind::NetgameEquipment);
        broken.put_u32(SCENARIO + items.offset + 4, 0x7700_0000);
        assert!(places(&broken).is_err());
    }

    fn player(index: u32, team: i32, unit: u32) -> telemetry::PlayerState {
        telemetry::PlayerState {
            datum: index,
            player: live_proto::game::Player {
                index,
                name: format!("P{index}"),
                xuid: None,
                team,
                local: Some(false),
                appearance: None,
                bot: None,
            },
            appearance: Default::default(),
            local_user: None,
            controller: None,
            machine: [0; 6],
            unit,
            stats: telemetry::PlayerStats {
                score: 0,
                kills: 0,
                deaths: 0,
                assists: 0,
                suicides: 0,
            },
        }
    }

    fn object(x: f64, body: f64, weapon: u32) -> telemetry::ObjectState {
        telemetry::ObjectState {
            pos: [x, 1.0, 2.0],
            forward: [1.0, 0.0, 0.0],
            weapon: Some(telemetry::Weapon {
                datum: weapon,
                definition: 0xd000 + weapon,
            }),
            vitality: Some((body, 1.0)),
            motion: telemetry::Motion {
                weapons: Some([weapon, u32::MAX, u32::MAX, u32::MAX]),
                grenades: Some(2),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn snapshots_become_poses() {
        let snap = telemetry::Snapshot {
            tick: 9,
            players: vec![player(0, 0, 0xe001), player(1, 1, 0xe002), player(2, 1, u32::MAX)],
            objects: vec![Some(object(3.0, 1.0, 7)), Some(object(4.0, 0.0, 8)), None],
            ..Default::default()
        };
        let p = poses(&snap);
        assert_eq!(p.len(), 3);
        assert!(p[0].alive && !p[1].alive && !p[2].alive);
        assert_eq!(p[0].pos, [3.0, 1.0, 2.0]);
        assert_eq!(p[0].facing, [1.0, 0.0, 0.0]);
        assert_eq!(
            p[0].weapon,
            Some(Weapon {
                datum: 7,
                definition: Some(0xd007)
            })
        );
        assert_eq!(p[1].team, Some(1));
        assert_eq!(p[2].weapons, None);
    }

    #[test]
    fn over_fake_guest_memory_each_event_is_written_once_with_its_places() {
        let (starts, items) = places(&scenario()).unwrap();
        let mut r = Recorder::new(Header {
            team_play: true,
            ..Default::default()
        });
        r.set_places(starts, items);
        r.start(30);
        let snap = |tick: u64, x0: f64, body1: f64, w0: u32| telemetry::Snapshot {
            tick,
            players: vec![player(0, 0, 0xe001), player(1, 1, 0xe002)],
            objects: vec![Some(object(x0, 1.0, w0)), Some(object(12.0, body1, 8))],
            ..Default::default()
        };
        r.tick(30, &poses(&snap(30, 1.0, 1.0, 7)), &NoGround);
        // Player 0 takes the weapon at the placement and kills player 1.
        let mut s = snap(31, 5.0, 1.0, 7);
        s.objects[0].as_mut().unwrap().motion.weapons = Some([7, 9, u32::MAX, u32::MAX]);
        r.tick(31, &poses(&s), &NoGround);
        r.damage(32, Some(0), 1, 1.0, &NoGround);
        r.kill(32, Some(0), 1, false, &NoGround);
        r.tick(32, &poses(&snap(32, 5.0, 0.0, 7)), &NoGround);
        let t = r.finish(&NoGround, None);
        let kinds: Vec<&str> = t
            .events
            .iter()
            .map(|e| match e {
                Event::Sample { .. } => "sample",
                Event::Spawn { .. } => "spawn",
                Event::Damage { .. } => "damage",
                Event::Kill { .. } => "kill",
                Event::Pickup { .. } => "pickup",
                Event::Stuck { .. } => "stuck",
                Event::Repath { .. } => "repath",
            })
            .collect();
        assert_eq!(
            kinds,
            ["spawn", "sample", "spawn", "sample", "pickup", "damage", "kill"]
        );
        let starts: Vec<_> = t
            .events
            .iter()
            .filter_map(|e| match e {
                Event::Spawn { start, .. } => Some(*start),
                _ => None,
            })
            .collect();
        assert_eq!(starts, [Some(0), Some(1)]);
        assert!(t.events.iter().any(|e| matches!(
            e,
            Event::Pickup {
                item: Some(0),
                weapon: Some(Weapon { datum: 9, .. }),
                ..
            }
        )));
        let Some(Event::Kill {
            killer: Some(k),
            victim,
            ..
        }) = t.events.last()
        else {
            panic!();
        };
        assert_eq!(k.at.unwrap().pos, [5.0, 1.0, 2.0]);
        assert_eq!(victim.at.unwrap().pos, [12.0, 1.0, 2.0]);
        assert_eq!(k.weapon.unwrap().definition, Some(0xd007));
    }
}
