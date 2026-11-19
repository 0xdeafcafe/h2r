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
    vehicles
        .vehicles
        .retain(|v| Some(v.on.unwrap_or(v.id)) == riding || visible(v.position, CHEST));
}

#[cfg(test)]
pub(crate) mod honesty {
    use super::*;
    use crate::brain::{Observations, Seen};
    use crate::los::Walls;
    use crate::mind::{Mind, Skill};

    /// Replay two host worlds on independent, identically seeded minds.
    /// Check inputs as well as outputs so an inert leak fails immediately.
    pub(crate) fn paired_worlds(
        map: Option<&MapData>,
        frames: impl IntoIterator<Item = (Perception, Perception)>,
    ) {
        let mut minds = [
            Mind::new(Skill::Legendary, 41),
            Mind::new(Skill::Legendary, 41),
        ];
        for (mut left, mut right) in frames {
            for frame in [&mut left, &mut right] {
                observe(frame, &mut Sight::default(), &mut [], map);
            }
            assert_eq!(left, right, "perceptions at tick {}", left.tick);
            assert_eq!(minds[0].step(&left, map), minds[1].step(&right, map));
            assert_eq!(minds[0].doing, minds[1].doing);
            assert_eq!(minds[0].kit, minds[1].kit);
            assert_eq!(minds[0].decisions.report, minds[1].decisions.report);
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
