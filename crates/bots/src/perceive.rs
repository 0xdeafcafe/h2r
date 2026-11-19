//! Turn host candidates into the information one player can act on.
//! This runs before radio, objective planning, movement, and item memory.
use crate::brain::{Me, Perception, Vec3};
use crate::cache::MapData;
use crate::emote::Kill;
use crate::mind::{CHEST, EYE, NOTICE_CONE, NOTICE_NEAR};
use crate::vehicle::Sight;

pub(crate) fn in_view(me: Me, at: Vec3, height: f32, map: Option<&MapData>) -> bool {
    let Some(map) = map else { return false };
    let eye = [me.position[0], me.position[1], me.position[2] + EYE];
    let target = [at[0], at[1], at[2] + height];
    let delta: Vec3 = std::array::from_fn(|i| target[i] - eye[i]);
    let distance = crate::decisions::distance(target, eye);
    let dot: f32 = (0..3).map(|i| me.aim[i] * delta[i]).sum();
    (distance < NOTICE_NEAR || dot > NOTICE_CONE.cos() * distance) && map.walls.clear(eye, target)
}

pub(crate) fn observe(
    p: &mut Perception,
    vehicles: &mut Sight,
    kills: &mut [Kill],
    map: Option<&MapData>,
) {
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
    p.observations.contacts.retain(|c| {
        p.others
            .iter()
            .any(|o| o.id == c.id && visible(o.position, CHEST))
    });
    p.others.retain(|o| {
        (p.teams && p.me.is_some_and(|me| me.team == o.team)) || visible(o.position, CHEST)
    });
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
    vehicles.vehicles.retain_mut(|v| {
        if Some(v.on.unwrap_or(v.id)) == riding {
            return true;
        }
        if !visible(v.position, CHEST) {
            return false;
        }
        // Seeing a hull doesn't reveal its vitality or identify a rider
        // whose body is occluded. An attempted entry can still fail.
        v.health = 1.0;
        for seat in &mut v.seats {
            seat.rider = seat.rider.filter(|r| {
                p.others
                    .iter()
                    .any(|o| o.id == r.player && visible(o.position, CHEST))
            });
        }
        true
    });
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
                            powerup: true,
                        });
                    }
                    (a, b)
                }),
            );
        }
    }
}
