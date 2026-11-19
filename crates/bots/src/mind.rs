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
use crate::decisions::{Decisions, Fight, Posture};
use crate::incoming::{Evasion, Grenade};
use crate::nav::Follower;
use crate::objective::{self, Goal};
use crate::sound::ShotSound;
use crate::stuck::{Stuck, around_bodies};
use crate::tactics::{self, Shot, Style, Tactics};

pub(crate) mod feel;
mod look;
pub(crate) mod pad;
pub(crate) use look::{TURN_RATE, turn_rate};

use feel::{Rng, reached};
use look::{Hand, angle_error};
use pad::{Buttons, Feet, Fidget, Tap};

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
                reaction: 6,
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
    /// Nominal reaction in ticks. The actual delay is sampled on acquisition.
    pub reaction: u32,
    /// The tracking error's spread (radians): a slowly wandering offset.
    pub error: f32,
    /// How much of the needed lead it gives a moving target.
    pub lead: f32,
    /// Fraction of full turn rate requested per radian of aim error.
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
/// Minimum ticks between reload / switch presses.
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
    /// Investigating a teammate's call or regrouping at their position.
    Support(Vec3),
    /// Going to a sighted powerup or a supply expected back soon.
    Supply(Vec3),
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
    pub decisions: Decisions,
    pub doing: Doing,
    rng: Rng,
    tick: Option<u32>,
    last_pad: HostPadState,
    /// The target and the tick when its reaction finishes.
    target: Option<(u32, u32)>,
    tracks: std::collections::HashMap<u32, Track>,
    last_seen: Option<(u32, Vec3, u32)>,
    hand: Hand,
    feet: Feet,
    buttons: Buttons,
    evasion: Evasion,
    fidget: Fidget,
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
    /// The Clankers page's Glitches (crate::glitches): the fight tricks go
    /// through `combo`, the show-offs through `show_off` and `stunt`.
    pub glitches: crate::glitches::Glitches,
    /// This map's super bounce spots (crate::glitches::spots).
    pub bounces: Vec<crate::glitches::Bounce>,
    show_off: crate::glitches::ShowOff,
    stunt: Option<Stunt>,
}

/// A show-off under way (crate::glitches).
#[derive(Clone, Debug)]
enum Stunt {
    /// A crouch jump while roaming, from the tick.
    Hop(crate::glitches::Run),
    /// A super bounce: walking to the spot, then the run at it.
    Bounce(crate::glitches::Bounce, Option<crate::glitches::BounceRun>),
}

/// A super bounce spot this far away (XY) or nearer is worth walking to.
pub const BOUNCE_REACH: f32 = 60.0;

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
            decisions: Decisions::new(seed),
            doing: Doing::Idle,
            rng: Rng::new(seed),
            tick: None,
            last_pad: HostPadState::default(),
            target: None,
            tracks: Default::default(),
            last_seen: None,
            hand: Hand::new(seed),
            feet: Feet::new(seed),
            buttons: Buttons::new(seed),
            evasion: Evasion::default(),
            fidget: Fidget::new(seed ^ 0xf31a),
            grenade: crate::grenades::Thrower::new(seed ^ 0x6e_ade),
            combo: crate::weapons::Combo::new(seed ^ 0xc0_4b0),
            left: crate::weapons::LeftHand::default(),
            follower: None,
            goal: None,
            stuck: Stuck::new(),
            moves: crate::moves::Moves::new(seed),
            roam: crate::roam::Roam::new(seed),
            kit: Kit::Fine,
            fight: Tactics::new(seed),
            last_threat: None,
            take: None,
            recovering: false,
            gave_up: false,
            navigated: false,
            emote: crate::emote::Emoter::new(crate::emote::DEFAULT_TAUNT, seed ^ 0x7ea_ba6),
            glitches: crate::glitches::Glitches::Off,
            bounces: Vec::new(),
            show_off: crate::glitches::ShowOff::new(seed ^ 0x5b0_0ce),
            stunt: None,
        }
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

    fn release(&mut self, tick: u32) {
        self.follower = None;
        self.target = None;
        self.doing = Doing::Idle;
        self.stuck.reset();
        self.take = None;
        self.recovering = false;
        self.fight.reset();
        self.hand.release();
        self.feet.stop();
        self.buttons.release();
        self.moves.release(tick);
        self.tracks.clear();
        self.last_seen = None;
        self.last_threat = None;
        self.decisions.died(tick);
        self.goal = None;
        self.evasion = Evasion::default();
        self.fidget.reset();
        self.grenade.reset();
        self.combo.reset();
        self.left.reset();
        self.show_off.reset();
        self.stunt = None;
        self.tick = None;
    }

    /// This frame's pad.
    pub fn step(&mut self, p: &Perception, map: Option<&MapData>) -> HostPadState {
        self.step_with_cues(p, map, &[], &[])
    }

    pub(crate) fn step_with_cues(
        &mut self,
        p: &Perception,
        map: Option<&MapData>,
        sounds: &[ShotSound],
        grenades: &[Grenade],
    ) -> HostPadState {
        let mut pad = HostPadState::default();
        let Some(me) = p.me else {
            self.release(p.tick);
            return pad;
        };
        if self.tick == Some(p.tick) {
            return self.last_pad;
        }
        if self.tick.is_some_and(|t| !reached(p.tick, t)) {
            // A new match can reset the clock without an intervening dead frame.
            self.release(p.tick);
        }
        let dt = self.tick.map_or(1, |t| p.tick.wrapping_sub(t)).clamp(1, 30);
        self.tick = Some(p.tick);
        self.gave_up = false;
        self.navigated = false;
        let aim = self.skill.aim();

        let seen = Self::visible(p, map);
        // Lead comes from consecutive sight samples only. Losing LOS
        // drops the velocity immediately, even within reaction grace.
        self.tracks.retain(|id, _| seen.iter().any(|o| o.id == *id));
        for o in &seen {
            let vel = self
                .tracks
                .get(&o.id)
                .filter(|t| (1..=3).contains(&p.tick.wrapping_sub(t.tick)))
                .map_or([0.0; 3], |t| {
                    scale(
                        sub(o.position, t.at),
                        TICKS / p.tick.wrapping_sub(t.tick) as f32,
                    )
                });
            self.tracks.insert(
                o.id,
                Track {
                    at: o.position,
                    tick: p.tick,
                    vel,
                },
            );
        }

        let eye = up(me.position, EYE);
        let facing = angles(me.aim);
        let style = Style::of(self.skill);
        // The target (crate::decisions): the team's split, trades and
        // threats weigh tactics' nearest-and-in-front choice.
        let (goal, intent) = self.decide(p, me, &seen, map);
        let chosen = intent
            .chosen
            .map(|o| (o, self.fight.focus(o.id, p.tick, self.skill)));
        self.target = chosen.map(|(o, ready)| (o.id, ready));
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
                        pickups: if goal.is_some()
                            && [a.held, a.other].into_iter().flatten().any(|g| g.ready())
                        {
                            &[]
                        } else {
                            &p.pickups
                        },
                    },
                    |pk| {
                        st.blocked(pk.position, tick)
                            || p.objective
                                .as_ref()
                                .and_then(|o| o.hill.as_ref())
                                .is_some_and(|h| !h.contains(pk.position))
                    },
                )
            }
            None => Kit::Fine,
        };
        // The objective (section 3.7). Carrying, nothing else counts: no
        // reload or switch (a switch drops the flag, the ball or the bomb),
        // no detour for a gun.
        let carrying = matches!(goal, Some(Goal::Carry(_) | Goal::Keep));
        if goal != Some(Goal::Keep) {
            self.decisions.map.leave_shelter();
        }
        // A reload or switch press under way is let go too: carrying is
        // not `weapon_allowed` in the buttons (mind::pad).
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

        self.choose_activity(p, me, goal, &intent, calm);
        if !matches!(self.doing, Doing::Rearm(_)) {
            self.take = None;
        }
        // Glitches only fighting or roaming on foot, with the setting on;
        // a show-off only roaming.
        if !glitch_doing(self.doing) || self.glitches == crate::glitches::Glitches::Off {
            self.drop_glitches();
        }
        if !matches!(self.doing, Doing::Roam) {
            self.stunt = None;
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
        // The fight's aim, tracked after the grenade hand has had its say
        // (it may want the throw's line instead): the target, where it
        // stands and where the shot leads to, from the eye.
        let mut track: Option<(u32, Vec3, Vec3)> = None;
        let mut aim_error = None;
        let mut melee_press = None;
        if !matches!(self.doing, Doing::Fight(_)) {
            self.hand.lose_target();
            self.fight.stop_trigger(p.tick);
        }
        match self.doing {
            Doing::Fight(_) => {
                let (o, ready) = chosen.unwrap();
                // The weapon in hand (tactics): the trigger, the lead
                // speed, the range it shoots at, and head, chest or feet.
                let held = me.arms.and_then(|a| a.held);
                let shot = Shot::of(held, me.arms.is_some(), &style, aim.burst, SHOT_SPEED);
                let clear = |at| map.is_none_or(|m| m.walls.clear(eye, at));
                let preferred = up(o.position, shot.height);
                let target = if clear(preferred) {
                    preferred
                } else {
                    up(o.position, CHEST)
                };
                let dist = len(sub(target, eye));
                let vel = self.tracks.get(&o.id).map_or([0.0; 3], |t| t.vel);
                let projected = add(target, scale(vel, aim.lead * dist / shot.speed));
                let led = if clear(projected) { projected } else { target };
                let reacted = reached(p.tick, ready);
                let blade = shot.role == arms::Role::Blade;
                let melee = self.kit == Kit::Melee || blade;
                if reacted {
                    let off = angle_between(sub(led, eye), me.aim);
                    let cone = aim.fire_cone + (0.4 / dist.max(0.5)).min(0.3);
                    // Fire while near the target: never dry, only in the
                    // weapon's range.
                    let fired = if shot.in_range(dist) && shot.ready && !melee {
                        self.fight.trigger(p.tick, shot.pull, off < cone)
                    } else {
                        self.fight.stop_trigger(p.tick);
                        false
                    };
                    let reach = if blade {
                        shot.lunge
                    } else if melee {
                        MELEE_RANGE * 1.3
                    } else {
                        MELEE_RANGE
                    };
                    let in_melee = dist < reach && angle_between(sub(target, eye), me.aim) < 0.85;
                    // Glitches (crate::glitches) only where the fight is not
                    // obviously lost: not low, not withdrawing, nobody else
                    // close; a sword fly never into a group.
                    let oid = o.id;
                    let others = || seen.iter().filter(move |e| e.id != oid);
                    let glitch = crate::glitches::Gate {
                        setting: self.glitches,
                        carrying,
                        losing: self.recovering
                            || self.decisions.posture == Posture::Withdraw
                            || others().any(|e| dist_xy(e.position, me.position) < 8.0),
                        in_melee,
                        crowded: others().any(|e| dist_xy(e.position, o.position) < CLUSTER),
                    };
                    // Weapon skill (crate::weapons): the noob combo may hold
                    // or let go the trigger and switch; the melee finish;
                    // a glitch's presses.
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
                            glitch,
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
                    if combo.reload {
                        pad.buttons.set(RELOAD, true);
                    }
                    if combo.jump {
                        pad.buttons.set(JUMP, true);
                    }
                    // Dual wielding: the left trigger fires the left gun.
                    if let Some(l) = me.arms.and_then(|a| a.left) {
                        let (lo, hi) = l.reach().fire;
                        let want = off < cone && (lo..hi).contains(&dist) && !melee;
                        if self.left.trigger(p.tick, &l, want) {
                            pad.left_trigger = 1.0;
                        }
                    }
                    if in_melee && combo.trick.is_none() {
                        melee_press = Some(Tap::Melee(o.id));
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
                    track = Some((o.id, sub(target, eye), sub(led, eye)));
                }
                // Until it reacts it stands, caught out. Then its footwork
                // (crate::moves): strafing, keeping its range, reacting to
                // hits, backing to cover; with no ammunition it runs straight
                // in to melee.
                if !reacted {
                    self.hand.lose_target();
                    self.fight.stop_trigger(p.tick);
                }
                if let Some((at, radius)) = intent.anchor {
                    if len(sub(me.position, at)) > radius {
                        walk = self.go(me.position, at, p.tick, map, &mut pad, &bodies);
                    }
                } else if !reacted {
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
                        rush: melee && self.decisions.posture != Posture::Withdraw,
                        retreat: self.recovering || self.decisions.posture == Posture::Withdraw,
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
            Doing::Support(at) | Doing::Supply(at) => {
                walk = self.go(me.position, at, p.tick, map, &mut pad, &bodies);
                if walk.is_none() && len(sub(me.position, at)) > 1.0 {
                    self.stuck.blacklist(at, p.tick);
                }
            }
            Doing::Roam => {
                let free = calm && goal.is_none() && !carrying && seen.is_empty();
                if self.stunt.is_none()
                    && self.show_off.due(p.tick, self.glitches, self.skill, free)
                {
                    self.stunt = self.pick_stunt(me.position, p.tick);
                }
                if !free && matches!(self.stunt, Some(Stunt::Bounce(..))) {
                    self.stunt = None;
                }
                if let Some(Stunt::Bounce(spot, run)) = self.stunt.clone() {
                    let (w, keep) =
                        self.bounce(spot, run, me.position, p.tick, map, &mut pad, &bodies);
                    walk = w;
                    self.stunt = keep;
                } else {
                    walk = self.roam_walk(me.position, p, map, &mut pad, &bodies);
                    // A crouch jump on the way.
                    if let Some(Stunt::Hop(run)) = &self.stunt {
                        match run.keys(p.tick) {
                            Some(k) => {
                                if k.jump {
                                    pad.buttons.set(JUMP, true);
                                }
                                if k.crouch {
                                    pad.buttons.set(CROUCH, true);
                                }
                            }
                            None => self.stunt = None,
                        }
                    }
                }
            }
            Doing::Carry(home) => {
                // Home along the nav mesh. It cannot shoot with the flag;
                // whoever is in its way at arm's length gets the flag.
                walk = self.go(me.position, home, p.tick, map, &mut pad, &bodies);
                if let Some((o, ready)) = chosen
                    && len(sub(up(o.position, CHEST), eye)) < MELEE_RANGE * 1.3
                    && reached(p.tick, ready)
                {
                    look = Some(up(o.position, CHEST));
                    if angle_between(sub(up(o.position, CHEST), eye), me.aim) < 0.85 {
                        melee_press = Some(Tap::Melee(o.id));
                    }
                }
            }
            Doing::Keep => {
                let (at, to, foe) =
                    self.keep_ball(me.position, &seen, p.tick, map, &bodies, &mut pad);
                (look, walk) = (at, to);
                // A melee with the ball: the same reaction and the same aim
                // in front as any other.
                if let Some(o) = foe
                    && chosen.is_some_and(|(t, ready)| t.id == o.id && reached(p.tick, ready))
                    && angle_between(sub(up(o.position, CHEST), eye), me.aim) < 0.85
                {
                    melee_press = Some(Tap::Melee(o.id));
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
                if len(sub(me.position, to)) > near_enough {
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
        let nade = self.grenade.step(
            &crate::grenades::Moment {
                tick: p.tick,
                // A repeated frame returned the last pad above: always new.
                dt,
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
                busy: carrying
                    || me.arms.is_some_and(|a| a.left.is_some())
                    || self.combo.tricking()
                    || self.stunt.is_some(),
                pushing,
            },
            world,
            self.skill,
        );
        // Aiming a throw: the hand turns to the throw's line instead of the
        // target (tracked still, so the flick and lag are the same hand's).
        match (track, nade.look) {
            (Some((id, raw, led)), at) => {
                let goal = at.map_or(led, |l| sub(l, eye));
                aim_error = Some(self.hand.track(self.skill, id, p.tick, facing, raw, goal));
            }
            (None, Some(at)) => look = Some(at),
            (None, None) => {}
        }
        if nade.throw {
            pad.left_trigger = 1.0;
        }
        if nade.swap {
            pad.buttons.set(SWAP_GRENADES, true);
        }

        // A glitch under way (crate::glitches) presses for itself.
        let tap = melee_press
            .or(match self.kit {
                Kit::Reload => Some(Tap::Reload),
                Kit::Switch => Some(Tap::Switch),
                _ => None,
            })
            .filter(|_| !self.combo.tricking());
        if let Some(b) = self.buttons.step(
            tap,
            p.tick,
            self.skill,
            !carrying && !pad.buttons.contains(RELOAD) && !nade.throw,
        ) {
            pad.buttons.set(b, true);
        }
        // A throw, reload, switch or melee lets go of the right trigger;
        // the left gun's trigger (dual wielding) does not, nor a glitch's
        // (it holds R and X together).
        if nade.throw
            || (!self.combo.tricking()
                && [RELOAD, SWITCH, MELEE]
                    .into_iter()
                    .any(|b| pad.buttons.contains(b)))
        {
            self.fight.stop_trigger(p.tick);
            pad.right_trigger = 0.0;
        }

        let dodge = self.evasion.dodge(p.tick, self.skill, &me, map, grenades);
        if let Some((dir, jump)) = dodge {
            walk = Some(Walk {
                walk: dir,
                face: walk.map_or(dir, |w| w.face),
            });
            pad.buttons.set(JUMP, jump);
            pad.buttons.set(CROUCH, false);
            self.stuck.reset();
        }

        if aim_error.is_none() {
            let travel = walk.filter(|w| w.face[0].hypot(w.face[1]) > 1e-3);
            let look_angles = if let Some(at) = look {
                Some(angles(sub(at, eye)))
            } else if !matches!(self.doing, Doing::Fight(_)) {
                self.hand
                    .sound(eye, p.tick, map, sounds)
                    .map(|yaw| {
                        let base = travel.map_or(facing.0, |w| w.face[1].atan2(w.face[0]));
                        let limit = if travel.is_some() { 0.9 } else { 1.5 };
                        (base + wrap(yaw - base).clamp(-limit, limit), 0.0)
                    })
                    .or_else(|| {
                        travel.map(|w| {
                            let corner = if self.navigated {
                                self.follower.as_ref().and_then(|f| f.corner(me.position))
                            } else {
                                None
                            };
                            self.hand.travel(eye, w.face, p.tick, map, corner)
                        })
                    })
            } else {
                None
            };
            aim_error = look_angles.map(|to| angle_error(facing, to));
        }
        pad.right_stick = self.hand.steer(self.skill, aim_error, dt);
        if aim_error.is_none() && walk.is_none() && seen.is_empty() && pad.buttons.bits() == 0 {
            pad.right_stick = self.fidget.pad(u64::from(p.tick)).right_stick;
        } else {
            self.fidget.reset();
        }

        let jumping = pad.buttons.contains(JUMP);
        let jump = self.buttons.jump(jumping, p.tick);
        pad.buttons.set(JUMP, jump);
        if !jump
            && dodge.is_none()
            && self.stuck.level == crate::stuck::Level::Moving
            && walk.is_some_and(|w| w.walk[0].hypot(w.walk[1]) > 1e-3)
            && objective_approach(self.doing, me.position)
        {
            pad.buttons.set(CROUCH, true);
        }
        // A crouch jump's crouch is in the air: no slowing for it.
        let hop = matches!(self.stunt, Some(Stunt::Hop(_)));
        let slow = if pad.buttons.contains(CROUCH) && !jump && !hop {
            0.55
        } else {
            1.0
        };
        let [x, y] = self.feet.step(
            walk.map(|w| w.walk),
            dt,
            slow,
            jump || dodge.is_some(),
            |d| map.is_none_or(|m| crate::moves::bend_clear(m, me.position, d)),
        );
        let (s, c) = facing.0.sin_cos();
        pad.left_stick = (x * s - y * c, x * c + y * s);
        // An emote over a dumb death: stand, crouch in rhythm, nothing
        // else. Never while an objective is live for it: carrying,
        // guarding, hunting a carrier, in or going to the hill.
        if let Some(crouch) = self
            .emote
            .step(me.position, !seen.is_empty(), goal.is_some(), p.tick)
        {
            pad = HostPadState::default();
            pad.buttons.set(CROUCH, crouch);
            self.hand.release();
            self.feet.stop();
            self.buttons.release();
            self.fidget.reset();
        }
        self.last_pad = pad;
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
        let (dir, waypoint, pace) = match map {
            None => {
                let d = [goal[0] - at[0], goal[1] - at[1]];
                let l = (d[0] * d[0] + d[1] * d[1]).sqrt();
                if l <= crate::nav::REACHED {
                    return None;
                }
                (
                    [d[0] / l, d[1] / l],
                    Some(goal),
                    crate::nav::approach_pace(l),
                )
            }
            Some(m) => {
                let stale = self.carrier_threat_changed(tick)
                    || self
                        .follower
                        .as_ref()
                        .and_then(Follower::goal)
                        .is_none_or(|g| len(sub(g, goal)) > 1.0);
                if stale {
                    // Around the edges it got stuck on, if it can be.
                    let path = self.decisions.map.path(
                        &m.nav,
                        at,
                        goal,
                        tick,
                        &self.stuck,
                        self.decisions.personality.caution,
                    );
                    // Kept with the goal asked for: a goal off the mesh ends
                    // the path at the nearest point on it, and that is not
                    // a new goal.
                    self.follower = path.map(|p| Follower::new(p, u64::from(tick)).toward(goal));
                    self.follower.as_ref()?;
                }
                let f = self.follower.as_mut()?;
                let s = f.steer_with_clearance(at, u64::from(tick), |d| {
                    crate::moves::bend_clear(m, at, d)
                });
                if s.replan {
                    let done = f.done();
                    self.follower = None;
                    return (!done).then_some(Walk::straight([0.0, 0.0]));
                }
                if s.jump {
                    pad.buttons.set(JUMP, true);
                }
                (s.dir, f.waypoint(), f.pace(at))
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
        let walk = around_bodies(at, u.dir.unwrap_or(dir), bodies);
        Some(Walk {
            walk: if u.dir.is_some() || pad.buttons.contains(JUMP) {
                walk
            } else {
                walk.map(|c| c * pace)
            },
            face: dir,
        })
    }

    /// Stops any glitch under way (crate::glitches): the fight trick in
    /// the combo and the show-off. A path that makes the whole pad itself
    /// and returns early (a vehicle's seat) must call this first.
    pub(crate) fn drop_glitches(&mut self) {
        self.combo.drop_trick();
        self.stunt = None;
    }

    /// Roaming (crate::roam): a new goal when there is none or it is
    /// reached, and the walk to it.
    fn roam_walk(
        &mut self,
        at: Vec3,
        p: &Perception,
        map: Option<&MapData>,
        pad: &mut HostPadState,
        bodies: &[Vec3],
    ) -> Option<Walk> {
        if self.goal.is_none() || self.follower.as_ref().is_none_or(Follower::done) {
            self.goal = self.roam_goal(at, p.tick, map, &p.pickups);
            self.follower = None;
        }
        let g = self.goal?;
        let walk = self.go(at, g, p.tick, map, pad, bodies);
        if walk.is_none() || self.gave_up {
            self.goal = None;
        }
        walk
    }

    /// A show-off (crate::glitches): a super bounce at a spot of this
    /// map's within [`BOUNCE_REACH`], else a crouch jump.
    fn pick_stunt(&mut self, at: Vec3, tick: u32) -> Option<Stunt> {
        let spot = self
            .bounces
            .iter()
            .filter(|b| dist_xy(b.stand, at) < BOUNCE_REACH && !self.stuck.blocked(b.stand, tick))
            .min_by(|a, b| dist_xy(a.stand, at).total_cmp(&dist_xy(b.stand, at)))
            .copied();
        if let Some(b) = spot {
            return Some(Stunt::Bounce(b, None));
        }
        let mut roll = || self.show_off.uniform();
        let mac = crate::glitches::plan(crate::glitches::Trick::CrouchJump, self.skill, &mut roll);
        Some(Stunt::Hop(crate::glitches::Run::new(mac, tick)))
    }

    /// A super bounce: the walk to behind the spot along the nav mesh,
    /// then the run at it ([`crate::glitches::BounceRun`]). The walk this
    /// tick, and the stunt to keep (`None` when it is over or there is no
    /// way there).
    #[allow(clippy::too_many_arguments)]
    fn bounce(
        &mut self,
        spot: crate::glitches::Bounce,
        run: Option<crate::glitches::BounceRun>,
        at: Vec3,
        tick: u32,
        map: Option<&MapData>,
        pad: &mut HostPadState,
        bodies: &[Vec3],
    ) -> (Option<Walk>, Option<Stunt>) {
        let mut run = match run {
            Some(r) => r,
            None => {
                let from = spot.from();
                let walk = self.go(at, from, tick, map, pad, bodies);
                if self.gave_up {
                    self.stuck.blacklist(spot.stand, tick);
                    return (walk, None);
                }
                if walk.is_some() && dist_xy(at, from) > 0.3 {
                    return (walk, Some(Stunt::Bounce(spot, None)));
                }
                if dist_xy(at, from) > 1.0 {
                    // No way there.
                    self.stuck.blacklist(spot.stand, tick);
                    return (None, None);
                }
                self.follower = None;
                let seed = u64::from(tick) ^ 0xb0_0b5;
                crate::glitches::BounceRun::new(spot, self.glitches.bounce_tries(), tick, seed)
            }
        };
        let o = run.step(at, tick);
        if o.jump {
            pad.buttons.set(JUMP, true);
        }
        // Facing the way it comes in, the stick as the run asks.
        let face = if spot.drift[0].hypot(spot.drift[1]) > 1e-3 {
            spot.drift
        } else {
            let d = [run.target[0] - at[0], run.target[1] - at[1]];
            let l = d[0].hypot(d[1]).max(1e-6);
            [d[0] / l, d[1] / l]
        };
        let walk = Walk {
            walk: o.walk.unwrap_or([0.0, 0.0]),
            face,
        };
        if o.done.is_some() {
            return (Some(walk), None);
        }
        (Some(walk), Some(Stunt::Bounce(spot, Some(run))))
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
        let mut places = self.roam.candidates(at, tick, &m.nav, pickups, self.skill);
        self.decisions.map.safe_first(&mut places, tick);
        places.into_iter().map(|(_, at)| at).find(|&at| ok(at))
    }
}

/// Whether a glitch (crate::glitches) may run while doing `doing`: only
/// fighting or roaming on foot. An allowlist, so anything else (a vehicle
/// to mount, drive or ride in when those land, an objective, a pickup)
/// never has one.
pub(crate) fn glitch_doing(doing: Doing) -> bool {
    matches!(doing, Doing::Fight(_) | Doing::Roam)
}

fn objective_approach(doing: Doing, at: Vec3) -> bool {
    let Doing::Play(Goal::Go(to) | Goal::Hold(to, _) | Goal::Escort(to)) = doing else {
        return false;
    };
    (to[2] - at[2]).abs() < 1.2 && dist_xy(to, at) < 3.0
}

mod strategy;

#[cfg(test)]
mod fight_tests;
#[cfg(test)]
mod tests;
