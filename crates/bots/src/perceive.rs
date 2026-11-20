//! Turn host candidates into the information one player can act on.
//! This runs before radio, objective planning, movement, and item memory.
use crate::brain::{Me, Perception, Vec3};
use crate::cache::MapData;
use crate::emote::Kill;
use crate::mind::{CHEST, EYE, NOTICE_CONE, NOTICE_NEAR};
use crate::vehicle::{Sight, read::SeatBuffers};

use std::sync::{Arc, Weak};

pub(crate) fn player_bit(id: u32) -> u16 {
    1u16.checked_shl(id).unwrap_or(0)
}

#[derive(Clone, Copy)]
struct SightRay {
    unit: u32,
    tick: u32,
    eye: Vec3,
    chest: Vec3,
    clear: bool,
}

#[derive(Default)]
pub(crate) struct Visibility {
    map: Option<Weak<MapData>>,
    own_unit: Option<(u32, u32)>,
    tick: Option<u32>,
    players: [Option<SightRay>; 16],
}

impl Visibility {
    pub(crate) fn matches_map(&self, map: Option<&Arc<MapData>>) -> bool {
        match (&self.map, map) {
            (Some(old), Some(new)) => std::ptr::eq(old.as_ptr(), Arc::as_ptr(new)),
            (None, None) => true,
            _ => false,
        }
    }

    pub(crate) fn clear_life(&mut self) {
        self.own_unit = None;
        self.tick = None;
        self.players.fill(None);
    }

    /// Bind to an immutable loaded map so a recycled address cannot hit
    /// the old geometry. A restarted clock or a new body also drops sight.
    pub(crate) fn bind(
        &mut self,
        map: Option<&Arc<MapData>>,
        own_unit: Option<(u32, u32)>,
        tick: u32,
    ) -> bool {
        let changed = !self.matches_map(map);
        if changed {
            self.map = map.map(Arc::downgrade);
        }
        let rewound = self
            .tick
            .is_some_and(|old| tick.wrapping_sub(old) > i32::MAX as u32);
        if changed || rewound || own_unit != self.own_unit || own_unit.is_none() {
            self.players.fill(None);
        }
        self.own_unit = own_unit;
        self.tick = Some(tick);
        changed
    }

    #[allow(clippy::too_many_arguments)]
    fn player_clear(
        &mut self,
        slot: u32,
        id: u32,
        unit: Option<u32>,
        tick: u32,
        period: u32,
        eye: Vec3,
        chest: Vec3,
        cast: impl FnOnce() -> bool,
    ) -> bool {
        let (Some(entry), Some(unit)) = (self.players.get_mut(id as usize), unit) else {
            return cast();
        };
        if let Some(ray) = entry
            && ray.unit == unit
            && tick.wrapping_sub(ray.tick) < period
            && !crate::work::phase_due(tick, ray.tick, slot + id, period)
            && if ray.clear {
                // Never reveal a moving body based on an old clear ray.
                ray.eye == eye && ray.chest == chest
            } else {
                // Keep a recently occluded body hidden during small
                // movements, with a tight bound on acquisition delay.
                let near = |a: Vec3, b: Vec3| {
                    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f32>() <= 0.1f32.powi(2)
                };
                near(ray.eye, eye) && near(ray.chest, chest)
            }
        {
            return ray.clear;
        }

        let clear = cast();
        *entry = Some(SightRay {
            unit,
            tick,
            eye,
            chest,
            clear,
        });
        clear
    }
}

fn sight_line(me: Me, at: Vec3, height: f32) -> Option<(Vec3, Vec3)> {
    let eye = [me.position[0], me.position[1], me.position[2] + EYE];
    let target = [at[0], at[1], at[2] + height];
    let delta: Vec3 = std::array::from_fn(|i| target[i] - eye[i]);
    let distance = crate::decisions::distance(target, eye);
    let dot: f32 = (0..3).map(|i| me.aim[i] * delta[i]).sum();
    (distance < NOTICE_NEAR || dot > NOTICE_CONE.cos() * distance).then_some((eye, target))
}

pub(crate) fn in_view(me: Me, at: Vec3, height: f32, map: Option<&MapData>) -> bool {
    map.is_some_and(|m| sight_line(me, at, height).is_some_and(|(a, b)| m.walls.clear(a, b)))
}

#[cfg(test)]
pub(crate) fn observe(
    p: &mut Perception,
    vehicles: &mut Sight,
    kills: &mut [Kill],
    map: Option<&MapData>,
) {
    observe_cached(
        p,
        vehicles,
        kills,
        map,
        &mut Visibility::default(),
        1,
        &mut SeatBuffers::default(),
    );
}

pub(crate) fn observe_cached(
    p: &mut Perception,
    vehicles: &mut Sight,
    kills: &mut [Kill],
    map: Option<&MapData>,
    sight: &mut Visibility,
    period: u32,
    seats: &mut SeatBuffers,
) -> u16 {
    let own = p.observations.player.map(|(id, _)| id);
    let visible = |at, h| p.me.is_some_and(|me| in_view(me, at, h, map));
    for kill in kills {
        if Some(kill.victim) != own {
            kill.at = kill
                .at
                .filter(|&at| p.tick.wrapping_sub(kill.tick) <= 1 && visible(at, 0.2));
        }
    }
    // Allies have teammate HUD markers. Their aim and carried weapon
    // still require personal sight.
    let mut seen = 0;
    let mut units = [None; 16];
    for player in &p.observations.supply_players {
        if let Some(unit) = units.get_mut(player.id as usize) {
            *unit = Some(player.unit);
        }
    }
    if let (Some(me), Some(map)) = (p.me, map) {
        for player in &p.others {
            let Some((eye, chest)) = sight_line(me, player.position, CHEST) else {
                if let Some(entry) = sight.players.get_mut(player.id as usize) {
                    *entry = None;
                }
                continue;
            };
            let unit = units.get(player.id as usize).copied().flatten();
            let clear = sight.player_clear(
                own.unwrap_or(0),
                player.id,
                unit,
                p.tick,
                period,
                eye,
                chest,
                || map.walls.clear(eye, chest),
            );
            if clear {
                seen |= player_bit(player.id);
            }
        }
    }
    for (entry, unit) in sight.players.iter_mut().zip(units) {
        if unit.is_none() {
            *entry = None;
        }
    }
    let seen_player = |id| seen & player_bit(id) != 0;
    p.observations
        .supply_players
        .retain(|player| Some(player.id) == own || seen_player(player.id));
    let players = &p.observations.supply_players;
    p.observations.held_power.retain(|object| {
        players
            .iter()
            .any(|player| player.hands.contains(&Some(*object)))
    });
    p.observations.takes.retain(|e| {
        Some(e.player) == own
            || (visible(e.at, 0.2)
                && players
                    .iter()
                    .any(|player| player.id == e.player && player.unit == e.unit))
    });
    p.observations.contacts.retain(|c| seen_player(c.id));
    p.others
        .retain(|o| (p.teams && p.me.is_some_and(|me| me.team == o.team)) || seen_player(o.id));
    p.pickups.retain(|pk| visible(pk.position, 0.2));
    for pk in &mut p.pickups {
        // Someone looking at a dropped gun knows its model, not its rounds.
        if pk.gun.ammo.gun() {
            pk.gun.ammo = crate::arms::Ammo::Battery(1.0);
        }
    }
    if map.is_none() {
        p.observations.supplies = None;
    } else if let Some(items) = &mut p.observations.supplies {
        items.retain(|s| visible(s.at, 0.2));
    }
    if let Some(o) = &mut p.objective {
        let known = p
            .others
            .iter()
            .map(|p| p.id)
            .chain(own)
            .filter(|&id| id < 16)
            .fold(0u16, |mask, id| mask | (1u16 << id));
        match &mut o.state {
            crate::objective::State::Hill { occupants } => *occupants &= known,
            crate::objective::State::Territories(sites) => {
                for site in sites {
                    site.occupants &= known;
                }
            }
            _ => {}
        }
        o.items.retain(|item| {
            item.carrier.is_some_and(|id| Some(id) == own)
                || visible(item.position, CHEST)
                || item.carrier.is_some_and(|id| {
                    p.others
                        .iter()
                        .any(|s| s.id == id && p.teams && p.me.is_some_and(|me| s.team == me.team))
                })
        });
    }
    let riding = vehicles
        .riding
        .and_then(|r| vehicles.get(r.vehicle))
        .map(|v| vehicles.root(v).id);
    for hidden in vehicles.vehicles.extract_if(.., |v| {
        if Some(v.on.unwrap_or(v.id)) == riding {
            return false;
        }
        if !visible(v.position, CHEST) {
            return true;
        }
        // Seeing a hull doesn't reveal its vitality or identify a rider
        // whose body is occluded. An attempted entry can still fail.
        v.health = 1.0;
        for seat in &mut v.seats {
            seat.rider = seat.rider.filter(|r| seen_player(r.player));
        }
        false
    }) {
        seats.save(hidden.seats);
    }
    seen
}

#[cfg(test)]
pub(crate) mod honesty {
    use super::*;
    use crate::brain::{Observations, Seen};
    use crate::los::Walls;
    use crate::mind::Skill;
    use crate::{Brain, radio};
    use std::sync::Mutex;

    /// Replay two host worlds on independent, identically seeded minds.
    /// Check inputs as well as outputs so an inert leak fails immediately.
    pub(crate) fn paired_worlds(
        map: Option<&MapData>,
        frames: impl IntoIterator<Item = (Perception, Perception)>,
    ) {
        let mut brains = std::array::from_fn::<_, 2, _>(|_| {
            let mut brain = Brain::new(Skill::Legendary, 41);
            brain.epoch = 0;
            brain
        });
        let radios = [Mutex::new(radio::Radio::default()), Mutex::default()];
        for (mut left, mut right) in frames {
            for frame in [&mut left, &mut right] {
                observe(frame, &mut Sight::default(), &mut [], map);
            }
            assert_eq!(left, right, "perceptions at tick {}", left.tick);
            let a = brains[0].play(&left, map, &[], &radios[0]);
            let b = brains[1].play(&right, map, &[], &radios[1]);
            assert_eq!(a, b);
            assert_eq!(brains[0].mind.doing, brains[1].mind.doing);
            assert_eq!(brains[0].mind.kit, brains[1].mind.kit);
            assert_eq!(
                brains[0].mind.decisions.report,
                brains[1].mind.decisions.report
            );
        }
    }

    #[test]
    fn a_visible_vehicle_does_not_reveal_a_hidden_driver_or_vitality() {
        use crate::vehicle::{Class, Rider, SeatRole, tests::vehicle};
        let map = MapData {
            walls: Walls::new(vec![[[2., -10., -5.], [2., 10., -5.], [2., 0., 10.]]]),
            ..Default::default()
        };
        let model = vehicle(5, Class::Scout, [1., 0., 0.], &[SeatRole::Driver]);
        let mut p = Perception {
            me: Some(Me::default()),
            others: vec![Seen {
                id: 1,
                team: 1,
                position: [3., 0., 0.],
            }],
            ..Default::default()
        };
        let mut sight = Sight {
            riding: None,
            vehicles: vec![model.clone()],
        };
        sight.vehicles[0].health = 0.2;
        sight.vehicles[0].seats[0].rider = Some(Rider { player: 1, team: 1 });
        observe(&mut p, &mut sight, &mut [], Some(&map));
        assert_eq!(sight.vehicles, [model]);
    }

    #[test]
    fn sight_reveals_a_body_and_objective_but_the_public_kill_has_no_hidden_location() {
        use crate::objective::{Game, Item, Objective};
        let map = MapData {
            walls: Walls::new(vec![[[2., -10., -5.], [2., 10., -5.], [2., 0., 10.]]]),
            ..Default::default()
        };
        let frame = Perception {
            tick: 3,
            me: Some(Me::default()),
            ..Default::default()
        };
        for (x, seen) in [(1., true), (5., false)] {
            let at = [x, 0., 0.];
            let mut kills = [Kill {
                seq: 1,
                tick: 3,
                victim: 1,
                attacker: Some(2),
                betrayal: false,
                at: Some(at),
            }];
            let mut p = frame.clone();
            p.others.push(Seen {
                id: 2,
                team: 1,
                position: at,
            });
            p.objective = Some(Objective {
                me: 0,
                game: Game::Oddball,
                state: Default::default(),
                hill: None,
                spots: vec![],
                items: vec![Item {
                    id: 7,
                    team: 0,
                    carrier: Some(2),
                    position: at,
                }],
            });
            observe(&mut p, &mut Sight::default(), &mut kills, Some(&map));
            assert_eq!(p.others.len(), usize::from(seen));
            assert_eq!(p.objective.unwrap().items.len(), usize::from(seen));
            assert_eq!(kills[0].at, seen.then_some(at));
            assert_eq!(kills[0].attacker, Some(2));
        }
    }

    #[test]
    fn a_hidden_respawn_and_enemy_cannot_change_a_pad_or_future_memory() {
        use crate::arms::{Ammo, Arms, Gun, Pickup};
        use crate::decisions::supplies::Supply;
        let map = MapData {
            walls: Walls::new(vec![[[2., -10., -5.], [2., 10., -5.], [2., 0., 10.]]]),
            ..Default::default()
        };
        let at = [5., 0., 0.];
        let frame = Perception {
            me: Some(Me {
                arms: Some(Arms::default()),
                ..Default::default()
            }),
            observations: Observations {
                player: Some((0, 0)),
                supplies: Some(vec![]),
                ..Default::default()
            },
            ..Default::default()
        };
        for geometry in [Some(&map), None] {
            paired_worlds(
                geometry,
                (0..100).map(|tick| {
                    let mut a = frame.clone();
                    a.tick = tick;
                    if (10..40).contains(&tick) {
                        a.others.push(Seen {
                            id: 1,
                            team: 1,
                            position: [1., 0., 0.],
                        });
                    }
                    let mut b = a.clone();
                    if tick < 50 {
                        b.others.push(Seen {
                            id: 3,
                            team: 2,
                            position: at,
                        });
                        b.observations
                            .supply_players
                            .push(crate::decisions::supplies::Player {
                                id: 3,
                                unit: 37,
                                position: at,
                                hands: [Some(18), None],
                                overshield: true,
                                camo: false,
                            });
                        b.observations.held_power.push(18);
                        b.observations.takes.push(crate::decisions::supplies::Take {
                            at,
                            site: 2,
                            object: 18,
                            player: 3,
                            unit: 37,
                            tick,
                        });
                        b.pickups.push(Pickup {
                            id: 18,
                            position: at,
                            gun: Gun {
                                ammo: Ammo::Battery(1.),
                                ..Default::default()
                            },
                        });
                        b.observations.supplies.as_mut().unwrap().push(Supply {
                            site: 2,
                            object: 18,
                            spawn: at,
                            at,
                            respawn: Some(900),
                            powerup: Some(crate::decisions::supplies::Powerup::Camo),
                            on_disturbance: true,
                            disabled: false,
                            role: crate::arms::Role::Unknown,
                        });
                    }
                    (a, b)
                }),
            );
        }
    }
}

#[cfg(test)]
mod visibility_tests {
    use super::*;

    const EYE_AT: Vec3 = [0., 0., 0.6];
    const CHEST_AT: Vec3 = [4., 0., 0.4];

    #[test]
    fn a_blocked_ray_survives_small_motion_until_its_refresh_phase() {
        let mut cache = Visibility::default();
        // Slot 1 / target 2 has phase zero, so the next refresh is tick 12.
        assert!(!cache.player_clear(1, 2, Some(30), 10, 3, EYE_AT, CHEST_AT, || false));
        let moved = [4.05, 0., 0.4];
        assert!(
            !cache.player_clear(1, 2, Some(30), 11, 3, EYE_AT, moved, || {
                panic!("the blocked ray fits the age and movement limits")
            })
        );
        assert!(cache.player_clear(1, 2, Some(30), 12, 3, EYE_AT, moved, || true));
        assert!(!cache.player_clear(1, 2, Some(30), 13, 3, EYE_AT, CHEST_AT, || false));
        assert!(cache.player_clear(1, 2, Some(30), 14, 3, EYE_AT, [4.5, 0., 0.4], || true,));
    }

    #[test]
    fn clear_rays_are_reused_only_at_identical_endpoints_and_for_the_same_unit() {
        let mut cache = Visibility::default();
        assert!(cache.player_clear(1, 2, Some(30), 10, 3, EYE_AT, CHEST_AT, || true));
        assert!(
            cache.player_clear(1, 2, Some(30), 11, 3, EYE_AT, CHEST_AT, || {
                panic!("the same endpoints are already clear")
            })
        );
        assert!(!cache.player_clear(1, 2, Some(31), 11, 3, EYE_AT, CHEST_AT, || false));
    }

    #[test]
    fn changing_map_or_observer_life_and_rewinding_drop_player_rays() {
        let map = Arc::new(MapData::default());
        let other = Arc::new(MapData::default());
        let mut cache = Visibility::default();
        assert!(cache.bind(Some(&map), Some((1, 1)), 10));
        assert!(!cache.player_clear(1, 2, Some(30), 10, 3, EYE_AT, CHEST_AT, || false));
        assert!(!cache.bind(Some(&map), Some((1, 2)), 11));
        assert!(cache.player_clear(1, 2, Some(30), 11, 3, EYE_AT, CHEST_AT, || true));
        assert!(cache.bind(Some(&other), Some((1, 2)), 11));
        assert!(!cache.player_clear(1, 2, Some(30), 11, 3, EYE_AT, CHEST_AT, || false));
        assert!(!cache.bind(Some(&other), Some((1, 2)), 4));
        assert!(cache.players.iter().all(Option::is_none));
        assert!(cache.bind(None, None, 4));
    }
}
