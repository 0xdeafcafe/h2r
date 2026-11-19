//! The bot's mind, stages 2 and 3 (docs/game/bots.md, sections 3.1 to
//! 3.4): what it sees through the map's walls, what it decides to do, how it
//! moves along the nav mesh, and how a human-like hand aims.
//!
//! [`Mind::step`] is called every frame with what the bot perceives and the
//! map's [`MapData`] (when built: without it the bot sees everyone and walks
//! straight, as stage 1 did). Time is the game's tick (30 a second), so the
//! frame rate does not change how a bot plays.

use input::host::{HostButton, HostPadState};

use crate::arms::{self, Kit, Situation};
use crate::brain::{Perception, Seen, Vec3};
use crate::cache::MapData;
use crate::nav::Follower;
use crate::objective::{self, Goal};
use crate::stuck::{Stuck, around_bodies};
use crate::tactics::{self, Shot, Style, Tactics};

/// How good a bot is (docs/game/bots.md, section 3.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Skill {
    Easy,
    #[default]
    Normal,
    Heroic,
    Legendary,
}

impl Skill {
    pub const ALL: [Skill; 4] = [Skill::Easy, Skill::Normal, Skill::Heroic, Skill::Legendary];

    pub fn parse(s: &str) -> Option<Skill> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "easy" => Skill::Easy,
            "normal" => Skill::Normal,
            "heroic" => Skill::Heroic,
            "legendary" => Skill::Legendary,
            _ => return None,
        })
    }

    /// The aim model's numbers for this skill (section 3.4's table).
    pub fn aim(self) -> Aim {
        match self {
            Skill::Easy => Aim {
                reaction: 14,
                error: 0.10,
                lead: 0.3,
                gain: 1.6,
                fire_cone: 10f32.to_radians(),
                burst: Some((12, 18)),
                grenades_per_minute: 0.5,
            },
            Skill::Normal => Aim {
                reaction: 10,
                error: 0.06,
                lead: 0.6,
                gain: 2.5,
                fire_cone: 7f32.to_radians(),
                burst: Some((24, 8)),
                grenades_per_minute: 1.0,
            },
            Skill::Heroic => Aim {
                reaction: 7,
                error: 0.035,
                lead: 0.85,
                gain: 3.5,
                fire_cone: 5f32.to_radians(),
                burst: None,
                grenades_per_minute: 2.0,
            },
            Skill::Legendary => Aim {
                reaction: 5,
                error: 0.02,
                lead: 1.0,
                gain: 4.5,
                fire_cone: 4f32.to_radians(),
                burst: None,
                grenades_per_minute: 3.0,
            },
        }
    }
}

/// The aim model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aim {
    /// Ticks from first seeing a target to starting to turn to it.
    pub reaction: u32,
    /// The tracking error's spread (radians): a slowly wandering offset.
    pub error: f32,
    /// How much of the needed lead it gives a moving target.
    pub lead: f32,
    /// Right stick per radian of aim error.
    pub gain: f32,
    /// Fires while the aim is this close to the target.
    pub fire_cone: f32,
    /// Trigger on and off ticks, or `None` to hold it.
    pub burst: Option<(u32, u32)>,
    pub grenades_per_minute: f32,
}

/// Ticks a second.
pub const TICKS: f32 = 30.0;
/// The eye's and the target point's heights over a unit's origin.
pub const EYE: f32 = 0.6;
pub const CHEST: f32 = 0.4;
/// Notices an enemy within this angle of its aim, or this close anywhere.
pub const NOTICE_CONE: f32 = 65.0 * std::f32::consts::PI / 180.0;
pub const NOTICE_NEAR: f32 = 3.0;
/// Forgets a last known position after this long.
pub const MEMORY_TICKS: u32 = 300;
/// The speed lead is worked out for (world units a second) when the held
/// weapon's projectile is not known (`arms::Profile::shot_speed`).
pub const SHOT_SPEED: f32 = 90.0;
pub const MELEE_RANGE: f32 = 0.9;
/// With a blade in hand, swings (melee lunges) this close.
pub const BLADE_RANGE: f32 = 1.5;
/// The weapon-blind fire range (`arms::Role::Unknown`); a known weapon
/// shoots to its own (`arms::Role::fire_range`).
pub const FIRE_RANGE: f32 = 25.0;
pub const GRENADE_RANGE: (f32, f32) = crate::grenades::RANGE;
pub const GRENADE_COOLDOWN: u32 = crate::grenades::COOLDOWN;
/// Crouch: clicking the left stick, Halo 2's default layout.
pub const CROUCH: HostButton = HostButton::LeftStick;

/// Shields under this: back off in a fight and do not chase
/// (section 3.3)...
pub const LOW_SHIELD: f32 = 0.25;
/// ...until they are back to this.
pub const SHIELD_BACK: f32 = 0.9;
/// No enemy in view for this long is calm: reloads, detours for
/// ammunition.
pub const CALM_TICKS: u32 = 45;
/// No enemy in view for this long is a lull: reloads a half-empty
/// magazine (`arms::Situation::breather`).
pub const BREATHER_TICKS: u32 = 15;
/// A tapped button is held this many ticks...
pub const TAP_TICKS: u32 = 2;
/// ...and not tapped again for this long.
pub const RELOAD_COOLDOWN: u32 = 60;
pub const SWITCH_COOLDOWN: u32 = 40;
/// Holds the action button on a weapon of another kind this close
/// (world units, XY)...
pub const TAKE_RANGE: f32 = 0.5;
/// ...for this long, then gives it up as out of reach.
pub const TAKE_TICKS: u32 = 30;
/// Another enemy this close to the target makes a cluster worth a grenade.
pub const CLUSTER: f32 = 2.5;
/// Lobs one where an enemy went out of view this recently.
pub const COVER_GRENADE_TICKS: u32 = 60;
/// An escort keeps this far from the carrier.
pub const ESCORT_RANGE: f32 = 3.0;
/// A ball carrier runs from enemies within this...
pub const KEEP_AWAY: f32 = 15.0;
/// Halo 2's default layout: X reloads (held, picks up), Y switches
/// weapons, B melees, A jumps.
pub const RELOAD: HostButton = HostButton::West;
pub const SWITCH: HostButton = HostButton::North;
pub const MELEE: HostButton = HostButton::East;
pub const JUMP: HostButton = HostButton::South;
/// Switch grenades: Black, the right bumper (Halo 2's default layout,
/// docs/input/controller-layouts.md).
pub const SWAP_GRENADES: HostButton = HostButton::RightBumper;

/// What the mind is doing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Doing {
    Idle,
    Fight(u32),
    Chase(Vec3),
    Roam,
    /// Going for the weapon at this point (section 3.6).
    Rearm(Vec3),
    /// Shields down: backing away from where the enemy was, facing it.
    Recover(Vec3),
    /// Carrying the flag or bomb there (section 3.7): before everything.
    Carry(Vec3),
    /// Carrying the ball: away from the enemy, meleeing who comes close.
    Keep,
    /// Playing the objective: taking, returning, guarding, hunting the
    /// carrier, escorting, holding the hill.
    Play(Goal),
}

#[derive(Clone, Copy, Debug)]
struct Track {
    at: Vec3,
    tick: u32,
    vel: Vec3,
}

/// One bot's mind.
#[derive(Clone, Debug)]
pub struct Mind {
    pub skill: Skill,
    pub doing: Doing,
    rng: u64,
    tick: Option<u32>,
    /// The target and the tick it was first seen.
    target: Option<(u32, u32)>,
    tracks: std::collections::HashMap<u32, Track>,
    last_seen: Option<(u32, Vec3, u32)>,
    /// The tracking error (yaw, pitch).
    err: (f32, f32),
    /// Weapon skill: the grenade hand, the combos and melee finish, the
    /// left hand when dual wielding (crate::grenades, crate::weapons).
    pub grenade: crate::grenades::Thrower,
    pub combo: crate::weapons::Combo,
    left: crate::weapons::LeftHand,
    follower: Option<Follower>,
    goal: Option<Vec3>,
    /// Getting unstuck (section 3.2).
    pub stuck: Stuck,
    /// Footwork in a fight and roaming memory (section 3.2).
    pub moves: crate::moves::Moves,
    pub roam: crate::roam::Roam,
    /// The last weapons decision (section 3.6).
    pub kit: Kit,
    /// The fight (`tactics`): who it noticed when, the trigger, and the
    /// movement's signals (`fight.band`, `fight.wants_cover`).
    pub fight: Tactics,
    /// The last tick an enemy was in view.
    last_threat: Option<u32>,
    /// A tapped button and the tick it is let go; when the next tap may
    /// come.
    tap: Option<(HostButton, u32)>,
    tap_ready: u32,
    /// The pickup it is holding the action button on, and since when.
    take: Option<(u32, u32)>,
    /// Shields went under [`LOW_SHIELD`] and are not back.
    recovering: bool,
    /// [`Mind::go`] gave up on the goal this tick.
    gave_up: bool,
    /// [`Mind::go`] ran this tick.
    navigated: bool,
    /// The teabag layer (§3.5).
    pub emote: crate::emote::Emoter,
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn len(a: Vec3) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}
fn dist_xy(a: Vec3, b: Vec3) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}
fn up(p: Vec3, h: f32) -> Vec3 {
    [p[0], p[1], p[2] + h]
}
fn angles(v: Vec3) -> (f32, f32) {
    let flat = (v[0] * v[0] + v[1] * v[1]).sqrt();
    (v[1].atan2(v[0]), v[2].atan2(flat))
}
fn wrap(a: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    (a + PI).rem_euclid(TAU) - PI
}
fn angle_between(a: Vec3, b: Vec3) -> f32 {
    let d = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2]) / (len(a) * len(b)).max(1e-9);
    d.clamp(-1.0, 1.0).acos()
}

/// A walk: the way to walk and the way to face (the same, but while a
/// sidestep gets it unstuck).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Walk {
    walk: [f32; 2],
    face: [f32; 2],
}

impl Walk {
    fn straight(d: [f32; 2]) -> Self {
        Walk { walk: d, face: d }
    }
}

impl Mind {
    pub fn new(skill: Skill, seed: u64) -> Self {
        Mind {
            skill,
            doing: Doing::Idle,
            rng: seed | 1,
            tick: None,
            target: None,
            tracks: Default::default(),
            last_seen: None,
            err: (0.0, 0.0),
            grenade: crate::grenades::Thrower::new(seed ^ 0x6e_ade),
            combo: crate::weapons::Combo::new(seed ^ 0xc0_4b0),
            left: crate::weapons::LeftHand::default(),
            follower: None,
            goal: None,
            stuck: Stuck::new(),
            moves: crate::moves::Moves::new(seed),
            roam: crate::roam::Roam::new(seed),
            kit: Kit::Fine,
            fight: Tactics::default(),
            last_threat: None,
            tap: None,
            tap_ready: 0,
            take: None,
            recovering: false,
            gave_up: false,
            navigated: false,
            emote: crate::emote::Emoter::new(crate::emote::DEFAULT_TAUNT, seed ^ 0x7ea_ba6),
        }
    }

    fn next(&mut self) -> u64 {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn uniform(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
    fn normal(&mut self) -> f32 {
        let (u, v) = (self.uniform().max(1e-7), self.uniform());
        (-2.0 * u.ln()).sqrt() * (std::f32::consts::TAU * v).cos()
    }

    /// Who the bot can see this tick: enemies in its notice cone (or very
    /// near) with nothing solid between its eye and their chest.
    pub fn visible(p: &Perception, map: Option<&MapData>) -> Vec<Seen> {
        let Some(me) = p.me else { return Vec::new() };
        let eye = up(me.position, EYE);
        p.others
            .iter()
            .filter(|o| !p.teams || o.team != me.team)
            .filter(|o| {
                let to = sub(up(o.position, CHEST), eye);
                len(to) < NOTICE_NEAR || angle_between(to, me.aim) < NOTICE_CONE
            })
            .filter(|o| map.is_none_or(|m| m.walls.clear(eye, up(o.position, CHEST))))
            .copied()
            .collect()
    }

    /// This frame's pad.
    pub fn step(&mut self, p: &Perception, map: Option<&MapData>) -> HostPadState {
        let mut pad = HostPadState::default();
        let Some(me) = p.me else {
            self.follower = None;
            self.target = None;
            self.doing = Doing::Idle;
            self.stuck.reset();
            self.take = None;
            self.recovering = false;
            self.fight.reset();
            self.grenade.reset();
            self.combo.reset();
            self.left.reset();
            return pad;
        };
        let new_tick = self.tick != Some(p.tick);
        let dt = self.tick.map_or(1, |t| p.tick.wrapping_sub(t)).clamp(1, 30);
        self.tick = Some(p.tick);
        self.gave_up = false;
        self.navigated = false;
        let aim = self.skill.aim();

        // Tracks: velocity from the last position.
        if new_tick {
            for o in &p.others {
                let vel = match self.tracks.get(&o.id) {
                    Some(t) if p.tick > t.tick => {
                        scale(sub(o.position, t.at), TICKS / (p.tick - t.tick) as f32)
                    }
                    Some(t) => t.vel,
                    None => [0.0; 3],
                };
                self.tracks.insert(
                    o.id,
                    Track {
                        at: o.position,
                        tick: p.tick,
                        vel,
                    },
                );
            }
            // The tracking error wanders (Ornstein-Uhlenbeck, per tick).
            let theta = 0.15f32;
            for _ in 0..dt.min(5) {
                let k = aim.error * (2.0 * theta).sqrt();
                let (ny, np) = (self.normal(), self.normal());
                self.err.0 += -theta * self.err.0 + k * ny;
                self.err.1 += -theta * self.err.1 + k * np * 0.5;
            }
        }

        let seen = Self::visible(p, map);
        let eye = up(me.position, EYE);
        // The target (tactics): the nearest and most in front, sticking
        // with the one it has; reacting anew only to one not seen lately.
        let style = Style::of(self.skill);
        if new_tick {
            self.fight.see(&seen, p.tick, aim.reaction);
        }
        let chosen = tactics::choose(&seen, eye, me.aim, self.target.map(|(id, _)| id))
            .map(|o| (o, self.fight.since(o.id, p.tick)));
        self.target = chosen.map(|(o, t0)| (o.id, t0));
        if let Some((o, _)) = chosen {
            self.last_seen = Some((o.id, o.position, p.tick));
            self.roam.saw(o.position, p.tick);
            self.last_threat = Some(p.tick);
        }
        if self
            .last_seen
            .is_some_and(|(_, _, t)| p.tick.wrapping_sub(t) > MEMORY_TICKS)
        {
            self.last_seen = None;
        }
        let calm = self
            .last_threat
            .is_none_or(|t| p.tick.wrapping_sub(t) > CALM_TICKS);

        // Weapons (section 3.6): only with arms read.
        let enemy_dist = chosen.map(|(o, _)| len(sub(up(o.position, CHEST), eye)));
        self.kit = match me.arms {
            Some(a) => {
                let (st, tick) = (&self.stuck, p.tick);
                arms::plan(
                    &Situation {
                        arms: &a,
                        at: me.position,
                        enemy: enemy_dist,
                        calm,
                        breather: self
                            .last_threat
                            .is_none_or(|t| p.tick.wrapping_sub(t) > BREATHER_TICKS),
                        by_range: style.by_range,
                        pickups: &p.pickups,
                    },
                    |pk| st.blocked(pk.position, tick),
                )
            }
            None => Kit::Fine,
        };
        // The objective (section 3.7). Carrying, nothing else counts: no
        // reload or switch (a switch drops the flag, the ball or the bomb),
        // no detour for a gun.
        let goal = p
            .objective
            .as_ref()
            .and_then(|o| objective::plan(o, &me, &p.others, p.teams));
        let carrying = matches!(goal, Some(Goal::Carry(_) | Goal::Keep));
        if carrying {
            self.kit = Kit::Fine;
        }
        // A combo under way chose the gun (crate::weapons): no switch away.
        if self.combo.busy() && self.kit == Kit::Switch {
            self.kit = Kit::Fine;
        }
        // Shields: down under LOW_SHIELD, up again at SHIELD_BACK.
        if me.shield < LOW_SHIELD {
            self.recovering = true;
        } else if me.shield >= SHIELD_BACK {
            self.recovering = false;
        }
        self.fight
            .signal(me.arms.as_ref(), self.kit, self.recovering, enemy_dist);

        self.doing = match (chosen, self.last_seen, self.kit, goal) {
            (_, _, _, Some(Goal::Carry(at))) => Doing::Carry(at),
            (_, _, _, Some(Goal::Keep)) => Doing::Keep,
            (_, _, Kit::Seek(pk), _) => Doing::Rearm(pk.position),
            (Some((o, _)), _, _, _) => Doing::Fight(o.id),
            (None, Some((_, at, _)), _, _) if self.recovering => Doing::Recover(at),
            (None, _, _, Some(g)) => Doing::Play(g),
            (None, Some((_, at, _)), _, None) => Doing::Chase(at),
            (None, None, _, None) => Doing::Roam,
        };
        if !matches!(self.doing, Doing::Rearm(_)) {
            self.take = None;
        }

        // Where to look and where to go.
        let bodies: Vec<Vec3> = p
            .others
            .iter()
            .filter(|o| chosen.is_none_or(|(t, _)| t.id != o.id))
            .map(|o| o.position)
            .collect();
        let mut look: Option<Vec3> = None;
        let mut walk: Option<Walk> = None;
        // Whom a grenade is for this tick (crate::grenades), and whether it
        // is closing in with a short gun.
        let mut mark: Option<crate::grenades::Mark> = None;
        let mut pushing = false;
        match self.doing {
            Doing::Fight(_) => {
                let (o, t0) = chosen.unwrap();
                // The weapon in hand (tactics): the trigger, the lead
                // speed, the range it shoots at, and head, chest or feet.
                let held = me.arms.and_then(|a| a.held);
                let shot = Shot::of(held, me.arms.is_some(), &style, aim.burst, SHOT_SPEED);
                let target = up(o.position, shot.height);
                let dist = len(sub(target, eye));
                let vel = self.tracks.get(&o.id).map_or([0.0; 3], |t| t.vel);
                let lead = scale(vel, aim.lead * dist / shot.speed);
                let reacted = p.tick.wrapping_sub(t0) >= aim.reaction;
                let blade = shot.role == arms::Role::Blade;
                let melee = self.kit == Kit::Melee || blade;
                if reacted {
                    look = Some(add(target, lead));
                    // Fire while near the target: never dry, only in the
                    // weapon's range.
                    let off = angle_between(sub(target, eye), me.aim);
                    let cone = aim.fire_cone + (0.4 / dist.max(0.5)).min(0.3);
                    let want = off < cone && shot.in_range(dist) && shot.ready && !melee;
                    let fired = self.fight.trigger(p.tick, t0, shot.pull, want);
                    // Weapon skill (crate::weapons): the noob combo may hold
                    // or let go the trigger and switch; the melee finish.
                    let combo = self.combo.step(
                        &crate::weapons::ComboIn {
                            tick: p.tick,
                            target: Some(o.id),
                            dist,
                            on_target: off < cone,
                            reacted,
                            held,
                            other: me.arms.and_then(|a| a.other),
                            firing: fired,
                        },
                        self.skill,
                    );
                    if combo.trigger.unwrap_or(fired) {
                        pad.right_trigger = 1.0;
                    }
                    if combo.switch {
                        pad.buttons.set(SWITCH, true);
                    }
                    if combo.melee {
                        pad.buttons.set(MELEE, true);
                    }
                    // Dual wielding: the left trigger fires the left gun.
                    if let Some(l) = me.arms.and_then(|a| a.left) {
                        let (lo, hi) = l.reach().fire;
                        let want = off < cone && (lo..hi).contains(&dist) && !melee;
                        if self.left.trigger(p.tick, &l, want) {
                            pad.left_trigger = 1.0;
                        }
                    }
                    let reach = if blade {
                        shot.lunge
                    } else if melee {
                        MELEE_RANGE * 1.3
                    } else {
                        MELEE_RANGE
                    };
                    if dist < reach && p.tick % 20 < 2 {
                        pad.buttons.set(MELEE, true);
                    }
                    // A grenade: the thrower rolls for it, more often into a
                    // group or before pushing in with a short gun.
                    mark = Some(crate::grenades::Mark {
                        at: o.position,
                        vel,
                        seen: true,
                    });
                    pushing = matches!(
                        shot.role,
                        arms::Role::Scatter | arms::Role::Spray | arms::Role::Blade
                    ) && dist > self.fight.band.1;
                }
                // Until it reacts it stands, caught out. Then its footwork
                // (crate::moves): strafing, keeping its range, reacting to
                // hits, backing to cover; with no ammunition it runs straight
                // in to melee.
                if !reacted {
                    self.follower = None;
                } else {
                    let to = sub(o.position, me.position);
                    let flat = (to[0] * to[0] + to[1] * to[1]).sqrt().max(1e-6);
                    let fight = crate::moves::FightIn {
                        tick: p.tick,
                        skill: self.skill,
                        at: me.position,
                        foe: o.position,
                        shield: me.shield,
                        rush: melee,
                        retreat: self.recovering,
                        // The combat side's signals (tactics): cover while
                        // reloading or switching in range, and its range band.
                        reloading: self.fight.wants_cover,
                        range: self.fight.band,
                    };
                    let mv = self.moves.fight(&fight, map, &bodies);
                    if mv.jump {
                        pad.buttons.set(JUMP, true);
                    }
                    if mv.crouch {
                        pad.buttons.set(CROUCH, true);
                    }
                    walk = Some(Walk {
                        walk: mv.walk,
                        face: [to[0] / flat, to[1] / flat],
                    });
                    self.follower = None;
                }
            }
            Doing::Chase(at) => {
                walk = self.go(me.position, at, p.tick, map, &mut pad, &bodies);
                if walk.is_none() || self.gave_up {
                    // Got there and nobody is here, or cannot get there.
                    self.last_seen = None;
                }
                // A grenade where they went out of view, now and then.
                let gone = self
                    .last_seen
                    .map_or(u32::MAX, |(_, _, t)| p.tick.wrapping_sub(t));
                if gone < COVER_GRENADE_TICKS {
                    // Banked or rolled round the corner, at Heroic and up.
                    mark = Some(crate::grenades::Mark {
                        at,
                        vel: [0.0; 3],
                        seen: false,
                    });
                }
            }
            Doing::Recover(from) => {
                // Back away from it, facing it, while the shields come back.
                look = Some(up(from, CHEST));
                let away = sub(me.position, from);
                let flat = (away[0] * away[0] + away[1] * away[1]).sqrt().max(1e-6);
                let d = [away[0] / flat, away[1] / flat];
                let on_mesh = |d: [f32; 2]| {
                    map.is_none_or(|m| {
                        m.nav
                            .locate(add(me.position, [d[0] * 0.6, d[1] * 0.6, 0.0]), 1.0)
                            .is_some()
                    })
                };
                let side = [-d[1], d[0]];
                let pick = [d, side, [-side[0], -side[1]]]
                    .into_iter()
                    .find(|&d| on_mesh(d));
                if let Some(d) = pick {
                    let d = around_bodies(me.position, d, &bodies);
                    walk = Some(Walk { walk: d, face: d });
                }
                self.follower = None;
            }
            Doing::Rearm(at) => {
                let pk = match self.kit {
                    Kit::Seek(pk) => Some(pk),
                    _ => None,
                };
                let near = dist_xy(me.position, at) < TAKE_RANGE;
                let other_kind =
                    pk.is_some_and(|pk| me.arms.is_some_and(|a| !a.carries(pk.gun.def)));
                if near && other_kind {
                    // On it: hold the action button to take it. Walking over
                    // one of the same kind takes its ammunition by itself.
                    let id = pk.map_or(0, |pk| pk.id);
                    let since = match self.take {
                        Some((t, since)) if t == id => since,
                        _ => p.tick,
                    };
                    self.take = Some((id, since));
                    if p.tick.wrapping_sub(since) < TAKE_TICKS {
                        pad.buttons.set(RELOAD, true);
                    } else {
                        // It will not come: out of reach (on a ledge, in
                        // a wall). Some other one.
                        self.stuck.blacklist(at, p.tick);
                        self.take = None;
                    }
                } else {
                    walk = self.go(me.position, at, p.tick, map, &mut pad, &bodies);
                    if self.gave_up || (walk.is_none() && !near && dist_xy(me.position, at) > 1.0) {
                        // No way there.
                        self.stuck.blacklist(at, p.tick);
                    }
                }
            }
            Doing::Roam => {
                if self.goal.is_none() || self.follower.as_ref().is_none_or(Follower::done) {
                    self.goal = self.roam_goal(me.position, p.tick, map, &p.pickups);
                    self.follower = None;
                }
                if let Some(g) = self.goal {
                    walk = self.go(me.position, g, p.tick, map, &mut pad, &bodies);
                    if walk.is_none() || self.gave_up {
                        self.goal = None;
                    }
                }
            }
            Doing::Carry(home) => {
                // Home along the nav mesh. It cannot shoot with the flag;
                // whoever is in its way at arm's length gets the flag.
                walk = self.go(me.position, home, p.tick, map, &mut pad, &bodies);
                if let Some((o, t0)) = chosen
                    && len(sub(up(o.position, CHEST), eye)) < MELEE_RANGE * 1.3
                    && p.tick.wrapping_sub(t0) >= aim.reaction
                {
                    look = Some(up(o.position, CHEST));
                    if p.tick % 20 < 2 {
                        pad.buttons.set(MELEE, true);
                    }
                }
            }
            Doing::Keep => {
                // Away from the nearest enemy it knows of; one close up is
                // meleed with the ball.
                let near = p
                    .others
                    .iter()
                    .filter(|o| !p.teams || o.team != me.team)
                    .map(|o| (o.position, dist_xy(o.position, me.position)))
                    .filter(|(_, d)| *d < KEEP_AWAY)
                    .min_by(|a, b| a.1.total_cmp(&b.1));
                self.follower = None;
                if let Some((at, d)) = near {
                    if d < MELEE_RANGE * 1.3 {
                        look = Some(up(at, CHEST));
                        if p.tick % 20 < 2 {
                            pad.buttons.set(MELEE, true);
                        }
                        let to = sub(at, me.position);
                        let f = (to[0] * to[0] + to[1] * to[1]).sqrt().max(1e-6);
                        walk = Some(Walk::straight([to[0] / f, to[1] / f]));
                    } else {
                        walk = self.away(me.position, at, map, &bodies);
                    }
                }
            }
            Doing::Play(g) => {
                let (to, near_enough) = match g {
                    Goal::Go(at) | Goal::Hunt(_, at) => (at, 0.0),
                    Goal::Escort(at) => (at, ESCORT_RANGE),
                    Goal::Hold(at, r) => (at, r),
                    Goal::Carry(at) => (at, 0.0),
                    Goal::Keep => (me.position, 0.0),
                };
                if dist_xy(me.position, to) > near_enough {
                    walk = self.go(me.position, to, p.tick, map, &mut pad, &bodies);
                    if self.gave_up {
                        self.follower = None;
                    }
                } else {
                    // There: stand, facing where the enemy was last seen.
                    self.follower = None;
                    if let Some((_, at, _)) = self.last_seen {
                        look = Some(up(at, CHEST));
                    }
                }
            }
            Doing::Idle => {}
        }
        // Pre-aim (tactics): chasing, it looks where the enemy went.
        if look.is_none()
            && style.pre_aim
            && let Doing::Chase(at) = self.doing
        {
            look = tactics::pre_aim(me.position, at, walk.map(|w| w.walk));
        }
        if !self.navigated {
            self.stuck.reset();
        }
        if !matches!(self.doing, Doing::Fight(_)) {
            self.moves.calm();
            self.combo.reset();
        }
        // The grenade hand (crate::grenades): the throw flown against the
        // map's walls (a flat floor without them); never dual wielding or
        // carrying.
        let floor = crate::grenades::Floor(mark.map_or(me.position[2], |m| m.at[2]));
        let world: &dyn crate::grenades::Solid = match map {
            Some(m) if !m.walls.tris.is_empty() => &m.walls,
            _ => &floor,
        };
        let mates: Vec<Vec3> = p
            .others
            .iter()
            .filter(|o| p.teams && o.team == me.team)
            .map(|o| o.position)
            .collect();
        let hand = self.grenade.step(
            &crate::grenades::Moment {
                tick: p.tick,
                dt: if new_tick { dt } else { 0 },
                eye,
                feet: me.position,
                aim: me.aim,
                mark,
                enemies: &seen,
                mates: &mates,
                grenades: me.arms.map(|a| a.grenades),
                selected: me
                    .arms
                    .and_then(|a| crate::grenades::Kind::from_index(a.selected)),
                busy: carrying || me.arms.is_some_and(|a| a.left.is_some()),
                pushing,
            },
            world,
            self.skill,
        );
        if hand.look.is_some() {
            look = hand.look;
        }
        if hand.throw {
            pad.left_trigger = 1.0;
        }
        if hand.swap {
            pad.buttons.set(SWAP_GRENADES, true);
        }

        // Reload and switch: taps, spaced.
        let tap = match self.kit {
            Kit::Reload => Some((RELOAD, RELOAD_COOLDOWN)),
            Kit::Switch => Some((SWITCH, SWITCH_COOLDOWN)),
            _ => None,
        };
        if let Some((b, cooldown)) = tap
            && p.tick >= self.tap_ready
        {
            self.tap = Some((b, p.tick + TAP_TICKS));
            self.tap_ready = p.tick + cooldown;
        }
        if let Some((b, until)) = self.tap {
            if p.tick < until {
                pad.buttons.set(b, true);
            } else {
                self.tap = None;
            }
        }

        // Turn: toward what it looks at, else the way it means to go.
        let face = walk.map(|w| w.face);
        let want = match (look, face) {
            (Some(l), _) => {
                let (y, pi) = angles(sub(l, eye));
                Some((y + self.err.0, pi + self.err.1))
            }
            (None, Some(d)) if d[0].abs() + d[1].abs() > 1e-3 => Some((d[1].atan2(d[0]), 0.0)),
            _ => None,
        };
        let (yaw, pitch) = angles(me.aim);
        if let Some((wy, wp)) = want {
            pad.right_stick = (
                (-wrap(wy - yaw) * aim.gain).clamp(-1.0, 1.0),
                ((wp - pitch) * aim.gain).clamp(-1.0, 1.0),
            );
        }
        if let Some(Walk { walk: d, .. }) = walk {
            let (c, s) = (yaw.cos(), yaw.sin());
            let (fwd, right) = (d[0] * c + d[1] * s, d[0] * s - d[1] * c);
            let m = (fwd * fwd + right * right).sqrt();
            let k = if m > 1.0 { 1.0 / m } else { 1.0 };
            pad.left_stick = (right * k, fwd * k);
        }
        // An emote over a dumb death: stand, crouch in rhythm, nothing
        // else. Never while an objective is live for it: carrying,
        // guarding, hunting a carrier, in or going to the hill.
        if let Some(crouch) = self
            .emote
            .step(me.position, !seen.is_empty(), goal.is_some(), p.tick)
        {
            pad = HostPadState::default();
            pad.buttons.set(CROUCH, crouch);
        }
        pad
    }

    /// Walks toward `goal` along the nav mesh (straight without one),
    /// getting unstuck on the way ([`Stuck`]). `None` once there, or with
    /// no way there. Sets `gave_up` when the stuck machine gives up on it.
    fn go(
        &mut self,
        at: Vec3,
        goal: Vec3,
        tick: u32,
        map: Option<&MapData>,
        pad: &mut HostPadState,
        bodies: &[Vec3],
    ) -> Option<Walk> {
        self.navigated = true;
        let (dir, waypoint) = match map {
            None => {
                let d = [goal[0] - at[0], goal[1] - at[1]];
                let l = (d[0] * d[0] + d[1] * d[1]).sqrt();
                if l <= crate::nav::REACHED {
                    return None;
                }
                ([d[0] / l, d[1] / l], Some(goal))
            }
            Some(m) => {
                let stale = self
                    .follower
                    .as_ref()
                    .and_then(Follower::goal)
                    .is_none_or(|g| len(sub(g, goal)) > 1.0);
                if stale {
                    // Around the edges it got stuck on, if it can be.
                    let st = &self.stuck;
                    let path = m
                        .nav
                        .path_avoiding(at, goal, &|e| st.blocks_segment(e.a, e.b, tick))
                        .or_else(|| m.nav.path(at, goal));
                    // Kept with the goal asked for: a goal off the mesh ends
                    // the path at the nearest point on it, and that is not
                    // a new goal.
                    self.follower = path.map(|p| Follower::new(p, u64::from(tick)).toward(goal));
                    self.follower.as_ref()?;
                }
                let f = self.follower.as_mut()?;
                let s = f.steer(at, u64::from(tick));
                if s.replan {
                    let done = f.done();
                    self.follower = None;
                    return (!done).then_some(Walk::straight([0.0, 0.0]));
                }
                if s.jump {
                    pad.buttons.set(JUMP, true);
                }
                (s.dir, f.waypoint())
            }
        };
        let u = self.stuck.step(tick, at, Some(dir), waypoint, Some(goal));
        if u.repath {
            self.follower = None;
            self.gave_up = true;
            return Some(Walk::straight([0.0, 0.0]));
        }
        if u.jump {
            pad.buttons.set(JUMP, true);
        }
        if u.crouch {
            pad.buttons.set(CROUCH, true);
        }
        Some(Walk {
            walk: around_bodies(at, u.dir.unwrap_or(dir), bodies),
            face: dir,
        })
    }

    /// A step away from `from` on the nav mesh (straight back, else to one
    /// side or the other), around bodies.
    fn away(&self, at: Vec3, from: Vec3, map: Option<&MapData>, bodies: &[Vec3]) -> Option<Walk> {
        let a = sub(at, from);
        let flat = (a[0] * a[0] + a[1] * a[1]).sqrt().max(1e-6);
        let d = [a[0] / flat, a[1] / flat];
        let on_mesh = |d: [f32; 2]| {
            map.is_none_or(|m| {
                m.nav
                    .locate(add(at, [d[0] * 0.6, d[1] * 0.6, 0.0]), 1.0)
                    .is_some()
            })
        };
        let side = [-d[1], d[0]];
        [d, side, [-side[0], -side[1]]]
            .into_iter()
            .find(|&d| on_mesh(d))
            .map(|d| Walk::straight(around_bodies(at, d, bodies)))
    }

    /// Somewhere to roam to (crate::roam): where enemies were seen
    /// lately, a weapon worth taking, or somewhere new, not where it has
    /// just been; reachable, and none it got stuck going to.
    fn roam_goal(
        &mut self,
        at: Vec3,
        tick: u32,
        map: Option<&MapData>,
        pickups: &[crate::arms::Pickup],
    ) -> Option<Vec3> {
        let m = map?;
        if let Some(g) = self.goal {
            self.roam.visit(g, tick);
        }
        self.roam.visit(at, tick);
        let here = m.nav.poly_near(at);
        let st = &self.stuck;
        let ok = |c: Vec3| {
            !st.blocked(c, tick)
                && here.is_some_and(|h| {
                    m.nav
                        .poly_near(c)
                        .is_some_and(|i| i == h || m.nav.corridor(h, i).is_some())
                })
        };
        self.roam.pick(at, tick, &m.nav, pickups, self.skill, &ok)
    }
}

#[cfg(test)]
mod fight_tests;
#[cfg(test)]
mod tests;
