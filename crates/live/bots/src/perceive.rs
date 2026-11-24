//! Turn host candidates into the information one player can act on.
//! This runs before radio, objective planning, movement, and item memory.
use crate::brain::{Me, Perception, Vec3};
use crate::cache::MapData;
use crate::emote::Kill;
use crate::mind::{CHEST, EYE, HEAD, NOTICE_CONE, NOTICE_NEAR, Skill};
use crate::vehicle::{Sight, read::SeatBuffers};

pub(crate) fn player_bit(id: u32) -> u16 {
    1u16.checked_shl(id).unwrap_or(0)
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

// ---- visual range (docs/game/bots.md section 3.1, "Visual range") ----

/// The furthest (world units, about 3 m each) a bot of `skill` notices an
/// uncloaked player in the centre of its view: past it a person is a few
/// pixels on the screen.
pub(crate) fn sight_range(skill: Skill) -> f32 {
    match skill {
        Skill::Easy => 25.0,
        Skill::Normal => 35.0,
        Skill::Heroic => 45.0,
        Skill::Legendary => 55.0,
    }
}

/// Within this angle of the aim (radians) a player is in the centre of
/// the view and noticed to the full [`sight_range`]...
pub(crate) const CENTRE: f32 = 15.0 * std::f32::consts::PI / 180.0;
/// ...and at the edge of the view ([`NOTICE_CONE`]) only this fraction as
/// far: the corner of the eye sees what is near.
pub(crate) const EDGE_SIGHT: f32 = 0.4;
/// A held gun with a scope reaches this much further per zoom level for a
/// player within [`SCOPE_CONE`] of the aim: a person scans far through
/// a scope, never around it.
pub(crate) const SCOPE_SIGHT: f32 = 0.5;
pub(crate) const SCOPE_CONE: f32 = 6.0 * std::f32::consts::PI / 180.0;

/// An active-camouflaged player that neither fires nor is hurt is noticed
/// no further than this: the shimmer of a still one at arm's length.
pub(crate) fn camo_range(skill: Skill) -> f32 {
    match skill {
        Skill::Easy => 1.5,
        Skill::Normal => 2.0,
        Skill::Heroic => 2.5,
        Skill::Legendary => 3.0,
    }
}
/// A camouflaged player that moves shimmers this many times as far.
pub(crate) const CAMO_MOVING: f32 = 2.0;
/// A crouched player is a smaller shape: noticed this fraction as far.
pub(crate) const CROUCHED_SIGHT: f32 = 0.75;

/// What a player shows of itself beyond its body (from the host's read,
/// `brain::Observations`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Cues {
    /// Active camouflage is on.
    pub camo: bool,
    /// Moving faster than the motion tracker's threshold.
    pub moving: bool,
    /// Fired within `sound::HEARD_TICKS`: camouflage flickers on fire.
    pub fired: bool,
    /// Its shield is broken: camouflage flickers under damage.
    pub hurt: bool,
    /// Crouched: a smaller shape to pick out.
    pub crouching: bool,
}

impl Cues {
    pub(crate) fn of(p: &Perception, id: u32) -> Cues {
        let bit = player_bit(id);
        let o = &p.observations;
        let contact = o.contacts.iter().find(|c| c.id == id);
        Cues {
            camo: o.supply_players.iter().any(|s| s.id == id && s.camo),
            moving: o.moving & bit != 0,
            fired: o.firing & bit != 0,
            hurt: contact.is_some_and(|c| c.shield.is_some_and(|s| s <= 0.0)),
            crouching: contact.is_some_and(|c| c.crouching),
        }
    }

    /// Cloaked and showing nothing: neither firing nor hurt.
    pub(crate) fn cloaked(self) -> bool {
        self.camo && !self.fired && !self.hurt
    }
}

/// How far (world units) a bot of `skill` notices a player whose `part`
/// is at `to` from its eye: the visual range by where it is in the view,
/// or the camouflage's when cloaked.
pub(crate) fn notice_range(me: Me, skill: Skill, to: Vec3, cues: Cues) -> f32 {
    if cues.cloaked() {
        return camo_range(skill) * if cues.moving { CAMO_MOVING } else { 1.0 };
    }
    let distance = to.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-3);
    let dot: f32 = (0..3).map(|i| me.aim[i] * to[i]).sum();
    let off = (dot / distance).clamp(-1.0, 1.0).acos();
    let range = sight_range(skill) * if cues.crouching { CROUCHED_SIGHT } else { 1.0 };
    if off <= CENTRE {
        let zoom = me
            .arms
            .and_then(|a| a.held)
            .map_or(0, |g| g.profile.zoom.max(0));
        if off <= SCOPE_CONE && zoom > 0 {
            return range * (1.0 + SCOPE_SIGHT * f32::from(zoom));
        }
        return range;
    }
    if off >= NOTICE_CONE {
        // Out of the view: only what is close enough to feel.
        return NOTICE_NEAR;
    }
    let t = (off - CENTRE) / (NOTICE_CONE - CENTRE);
    range * (1.0 + t * (EDGE_SIGHT - 1.0))
}

/// A player standing at `at` is in view when its chest or its head is
/// (docs/game/bots.md, section 3.1): a head over a parapet is seen. The
/// part must be within the bot's visual range ([`notice_range`]).
pub(crate) fn body_in_view(
    me: Me,
    skill: Skill,
    at: Vec3,
    cues: Cues,
    map: Option<&MapData>,
) -> bool {
    [CHEST, HEAD]
        .into_iter()
        .any(|h| in_view(me, at, h, map) && within_sight(me, skill, at, h, cues))
}

fn within_sight(me: Me, skill: Skill, at: Vec3, height: f32, cues: Cues) -> bool {
    let to = [
        at[0] - me.position[0],
        at[1] - me.position[1],
        at[2] + height - me.position[2] - EYE,
    ];
    let distance = to.iter().map(|v| v * v).sum::<f32>().sqrt();
    distance <= notice_range(me, skill, to, cues)
}

#[cfg(test)]
pub(crate) fn observe(
    p: &mut Perception,
    vehicles: &mut Sight,
    kills: &mut [Kill],
    map: Option<&MapData>,
) {
    observe_for_brain(p, Skill::Legendary, vehicles, kills, map, &mut SeatBuffers::default());
}

/// Filter a single observation and return the players with an unobstructed
/// view in that sample, within a `skill` bot's visual range. The mind can
/// reuse the result with those positions.
pub(crate) fn observe_for_brain(
    p: &mut Perception,
    skill: Skill,
    vehicles: &mut Sight,
    kills: &mut [Kill],
    map: Option<&MapData>,
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
    // A player is seen in the visual range a person has: shorter off the
    // centre of the view, and a few steps for active camouflage unless it
    // fires or is hurt (section 3.1, "Visual range").
    let seen = p
        .others
        .iter()
        .filter(|o| {
            !crate::firefight::is_actor(o.id)
                && p.me.is_some_and(|me| {
                    body_in_view(me, skill, o.position, Cues::of(&*p, o.id), map)
                })
        })
        .fold(0, |mask, o| mask | player_bit(o.id));
    let seen_player = |id| seen & player_bit(id) != 0;
    // What shows through camouflage is a cue on a body in view only.
    p.observations.moving &= seen;
    p.observations.firing &= seen;
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
    // Firefight's actors (crate::firefight) have no player bit: one is
    // kept while in view, never as a teammate.
    p.others.retain(|o| {
        if crate::firefight::is_actor(o.id) {
            return visible(o.position, CHEST)
                && p.me.is_some_and(|me| {
                    within_sight(me, skill, o.position, CHEST, Cues::default())
                });
        }
        (p.teams && p.me.is_some_and(|me| me.team == o.team)) || seen_player(o.id)
    });
    p.observations.ground_grenades.retain(|(at, _)| visible(*at, 0.2));
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
        // The ball's waypoint is on every Oddball HUD: where it lies, or
        // who carries it, is known through walls (bots.md section 3.7).
        let waypoint = o.game == crate::objective::Game::Oddball;
        o.items.retain(|item| {
            waypoint
                || item.carrier.is_some_and(|id| Some(id) == own)
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
    fn every_skill_turns_toward_a_blip_without_firing_or_learning_an_identity() {
        for skill in Skill::ALL {
            let mut mind = crate::mind::Mind::new(skill, 15);
            let mut p = Perception {
                me: Some(Me::default()),
                observations: Observations {
                    radar: vec![[0., 4., 0.]],
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut turned = false;
            for tick in 0..300 {
                p.tick = tick;
                let pad = mind.step(&p, None);
                turned |= pad.right_stick.0 < -0.1;
                assert_eq!((pad.right_trigger, pad.left_trigger), (0., 0.));
                assert!(!matches!(mind.doing, crate::mind::Doing::Fight(_)));
            }
            assert!(turned, "{skill:?}");
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
                game: Game::Ctf,
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
            let mut ball = p.clone();
            ball.objective.as_mut().unwrap().game = Game::Oddball;
            observe(&mut p, &mut Sight::default(), &mut kills, Some(&map));
            assert_eq!(p.others.len(), usize::from(seen));
            assert_eq!(p.objective.unwrap().items.len(), usize::from(seen));
            // The ball's waypoint shows it, and so its carrier's place,
            // through the wall; the carrier as a player stays unseen.
            observe(&mut ball, &mut Sight::default(), &mut [], Some(&map));
            assert_eq!(ball.others.len(), usize::from(seen));
            assert_eq!(ball.objective.unwrap().items.len(), 1);
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
mod sight {
    use super::*;
    use crate::arms::{Arms, Gun, Profile, Trigger};
    use crate::brain::{Contact, Seen};
    use crate::decisions::supplies::Player;

    /// The bot at the origin looking down +X; one enemy (slot 1) at `at`,
    /// with or without active camouflage, on an open map.
    fn world(at: Vec3, camo: bool) -> Perception {
        let mut p = Perception {
            me: Some(Me::default()),
            others: vec![Seen {
                id: 1,
                team: 1,
                position: at,
            }],
            ..Default::default()
        };
        p.observations.supply_players.push(Player {
            id: 1,
            unit: 41,
            position: at,
            hands: [None, None],
            overshield: false,
            camo,
        });
        p.observations.contacts.push(Contact {
            weapon: None,
            id: 1,
            aim: [-1.0, 0.0, 0.0],
            shield: None,
            empty_magazine: false,
            crouching: false,
        });
        p
    }

    fn sees(mut p: Perception, skill: Skill) -> bool {
        let map = MapData::default();
        let mask = observe_for_brain(
            &mut p,
            skill,
            &mut Sight::default(),
            &mut [],
            Some(&map),
            &mut SeatBuffers::default(),
        );
        assert_eq!(mask != 0, !p.others.is_empty());
        mask & player_bit(1) != 0
    }

    #[test]
    fn a_camouflaged_enemy_is_hidden_past_arms_length_at_every_skill() {
        for skill in Skill::ALL {
            let near = camo_range(skill) - 0.3;
            assert!(sees(world([near, 0.0, 0.0], true), skill), "{skill:?}");
            for d in [camo_range(skill) + 0.5, 8.0, 20.0] {
                assert!(!sees(world([d, 0.0, 0.0], true), skill), "{skill:?} at {d}");
                // The same player uncloaked is seen there.
                assert!(sees(world([d, 0.0, 0.0], false), skill), "{skill:?} at {d}");
            }
        }
        // A better eye picks the shimmer out further.
        assert!(camo_range(Skill::Legendary) > camo_range(Skill::Easy));
    }

    #[test]
    fn firing_or_taking_damage_reveals_a_camouflaged_enemy() {
        let skill = Skill::Normal;
        let at = [15.0, 0.0, 0.0];
        let mut firing = world(at, true);
        firing.observations.firing = player_bit(1);
        assert!(sees(firing, skill));
        let mut hurt = world(at, true);
        hurt.observations.contacts[0].shield = Some(0.0);
        assert!(sees(hurt, skill));
        // Another player's shot reveals nobody.
        let mut other = world(at, true);
        other.observations.firing = player_bit(2);
        assert!(!sees(other, skill));
    }

    #[test]
    fn a_moving_camouflaged_enemy_shimmers_a_little_further() {
        let skill = Skill::Heroic;
        let d = camo_range(skill) * 1.5;
        assert!(!sees(world([d, 0.0, 0.0], true), skill));
        let mut moving = world([d, 0.0, 0.0], true);
        moving.observations.moving = player_bit(1);
        assert!(sees(moving.clone(), skill));
        moving.others[0].position = [camo_range(skill) * CAMO_MOVING + 1.0, 0.0, 0.0];
        assert!(!sees(moving, skill));
    }

    #[test]
    fn the_cues_of_an_unseen_player_do_not_reach_the_mind() {
        let mut p = world([30.0, 0.0, 0.0], true);
        p.observations.moving = player_bit(1) | player_bit(3);
        p.observations.firing = player_bit(3);
        let map = MapData::default();
        observe_for_brain(
            &mut p,
            Skill::Legendary,
            &mut Sight::default(),
            &mut [],
            Some(&map),
            &mut SeatBuffers::default(),
        );
        assert_eq!((p.observations.moving, p.observations.firing), (0, 0));
        assert!(p.observations.supply_players.is_empty());
        assert!(p.observations.contacts.is_empty());
    }

    #[test]
    fn an_enemy_past_the_visual_range_is_not_seen_at_any_skill() {
        for skill in Skill::ALL {
            let r = sight_range(skill);
            assert!(sees(world([r - 1.0, 0.0, 0.0], false), skill), "{skill:?}");
            assert!(!sees(world([r + 1.0, 0.0, 0.0], false), skill), "{skill:?}");
        }
        // Better eyes see further.
        let ranges = Skill::ALL.map(sight_range);
        assert!(ranges.windows(2).all(|w| w[0] < w[1]), "{ranges:?}");
    }

    #[test]
    fn off_the_centre_of_view_the_range_shortens_and_past_the_screen_nothing_is_seen() {
        let skill = Skill::Legendary;
        let at = |angle: f32, d: f32| [d * angle.to_radians().cos(), d * angle.to_radians().sin(), 0.0];
        // 30 degrees off: well inside the full range ahead, not at it.
        assert!(sees(world(at(30.0, 20.0), false), skill));
        assert!(!sees(world(at(30.0, sight_range(skill) - 2.0), false), skill));
        // Past the screen's corner: not seen at 5, only felt within
        // NOTICE_NEAR.
        assert!(!sees(world(at(50.0, 5.0), false), skill));
        assert!(!sees(world(at(150.0, 5.0), false), skill));
        assert!(sees(world(at(150.0, NOTICE_NEAR - 0.8), false), skill));
    }

    #[test]
    fn a_crouched_enemy_is_picked_out_less_far() {
        let skill = Skill::Heroic;
        let d = sight_range(skill) * 0.9;
        assert!(sees(world([d, 0.0, 0.0], false), skill));
        let mut low = world([d, 0.0, 0.0], false);
        low.observations.contacts[0].crouching = true;
        assert!(!sees(low.clone(), skill));
        low.others[0].position = [sight_range(skill) * CROUCHED_SIGHT - 1.0, 0.0, 0.0];
        assert!(sees(low, skill));
    }

    #[test]
    fn a_scope_sees_far_only_down_its_own_line() {
        let skill = Skill::Normal;
        let sniper = |p: &mut Perception| {
            p.me.as_mut().unwrap().arms = Some(Arms {
                held: Some(Gun {
                    def: 1,
                    profile: Profile {
                        trigger: Trigger::Single,
                        zoom: 2,
                        pellets: 1,
                        shot_speed: 900.0,
                        ..Profile::default()
                    },
                    ..Gun::default()
                }),
                ..Arms::default()
            });
        };
        let far = sight_range(skill) * 1.5;
        assert!(!sees(world([far, 0.0, 0.0], false), skill), "no scope");
        let mut scoped = world([far, 0.0, 0.0], false);
        sniper(&mut scoped);
        assert!(sees(scoped, skill), "a sniper sees it down the scope");
        // The same sniper does not see it 20 degrees off its line: it
        // has to have looked.
        let a = 20f32.to_radians();
        let mut aside = world([far * a.cos(), far * a.sin(), 0.0], false);
        sniper(&mut aside);
        assert!(!sees(aside, skill));
        // A sniper still needs the line clear of walls.
        let wall = MapData {
            walls: crate::los::Walls::new(vec![[
                [10., -10., -5.],
                [10., 10., -5.],
                [10., 0., 10.],
            ]]),
            ..Default::default()
        };
        let mut blocked = world([far, 0.0, 0.0], false);
        sniper(&mut blocked);
        assert!(!body_in_view(
            blocked.me.unwrap(),
            skill,
            [far, 0.0, 0.0],
            Cues::default(),
            Some(&wall)
        ));
    }

    #[test]
    fn firefight_actors_are_bound_by_the_same_visual_range() {
        let skill = Skill::Easy;
        let actor = crate::firefight::ACTOR_ID_BASE;
        let mut p = Perception {
            me: Some(Me::default()),
            others: vec![
                Seen {
                    id: actor,
                    team: 3,
                    position: [sight_range(skill) - 1.0, 0.0, 0.0],
                },
                Seen {
                    id: actor + 1,
                    team: 3,
                    position: [sight_range(skill) + 1.0, 0.0, 0.0],
                },
            ],
            ..Default::default()
        };
        let map = MapData::default();
        observe_for_brain(
            &mut p,
            skill,
            &mut Sight::default(),
            &mut [],
            Some(&map),
            &mut SeatBuffers::default(),
        );
        assert_eq!(p.others.iter().map(|o| o.id).collect::<Vec<_>>(), [actor]);
    }
}
