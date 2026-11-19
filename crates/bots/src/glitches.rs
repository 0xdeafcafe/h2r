//! Glitches (docs/game/bots.md, "Glitches"): Halo 2's well-known button
//! combos and movement tricks, done by a bot as a player does them, with
//! timed presses on its pad and nothing else.
//!
//! The Clankers page's **Glitches** row ([`Glitches`]) turns them on: Off
//! never, Some now and then, Lots whenever there is an opening. Skill
//! decides how cleanly: a Legendary bot presses on the tick, an Easy one
//! a tick or three late, often outside the window, and the trick then
//! fails the way a player's does.
//!
//! Two kinds:
//!
//! - **Fight tricks** ([`opening`], [`plan`]): BXR, BYYR, BXB, the double
//!   shot, the quad shot and sword flying. `crate::weapons::Combo` (the
//!   noob combo's scheduler) rolls for them and plays them.
//! - **Show-offs** ([`ShowOff`]): the crouch jump while roaming, and the
//!   super bounce at a measured spot ([`spots`], [`BounceRun`]), only when
//!   calm and with no objective to play.
//!
//! Pure and deterministic; time in game ticks (30 a second). The tricks
//! work only because the recomp runs retail 1.0's code (no title update
//! patched sword flying or super bounces out); each needs checking in a
//! real game (docs/game/bots.md, "Glitches", game-run checklist).

use crate::arms::{Ammo, Gun, Role};
use crate::brain::Vec3;
use crate::mind::Skill;

pub mod spots;
pub use spots::{Bounce, SPOTS, bounces};

/// The Clankers page's Glitches setting (`clankers.glitches`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Glitches {
    /// Never.
    #[default]
    Off,
    /// The occasional show-off.
    Some,
    /// Whenever there is an opening.
    Lots,
}

impl Glitches {
    pub const ALL: [Glitches; 3] = [Glitches::Off, Glitches::Some, Glitches::Lots];

    /// The setting's value (0 Off, 1 Some, 2 Lots); anything over 2 is
    /// Lots.
    pub fn from_index(i: u8) -> Glitches {
        Glitches::ALL[usize::from(i.min(2))]
    }

    pub fn index(self) -> u8 {
        self as u8
    }

    /// The chance a fight trick is tried at an opening (before skill).
    pub fn fight_chance(self) -> f32 {
        match self {
            Glitches::Off => 0.0,
            Glitches::Some => 0.15,
            Glitches::Lots => 0.6,
        }
    }

    /// Ticks between show-offs while calm, drawn from this range; `None`
    /// never.
    pub fn show_off_every(self) -> Option<(u32, u32)> {
        match self {
            Glitches::Off => None,
            Glitches::Some => Some((45 * 30, 90 * 30)),
            Glitches::Lots => Some((8 * 30, 20 * 30)),
        }
    }

    /// Super bounce tries at a spot before giving it up.
    pub fn bounce_tries(self) -> u32 {
        match self {
            Glitches::Off => 0,
            Glitches::Some => 3,
            Glitches::Lots => 8,
        }
    }
}

// ---- the tricks ----

/// One trick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trick {
    /// Melee, reload to cancel the melee's recovery, fire (B, X, R).
    Bxr,
    /// BXR with a part-empty magazine: two switches cancel instead of the
    /// reload (B, Y, Y, R).
    Byyr,
    /// Melee, reload, melee again (B, X, B).
    Bxb,
    /// Two battle rifle bursts at once and a reload (R, R, X).
    DoubleShot,
    /// A double shot, two switches back to the rifle without reloading,
    /// a second double shot (R, R, X, Y, Y, R, R, X).
    QuadShot,
    /// The other gun's red reticle lends the sword its lunge: switch,
    /// reload, fire (Y, X, R), then A for height.
    SwordFly,
    /// Jump, crouch near the top: the feet come up.
    CrouchJump,
}

/// A control a trick presses. The pad's buttons are Halo 2's default
/// layout (`crate::mind`: B melee, X reload, Y switch, A jump, the left
/// stick's click crouch, the right trigger fire).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Melee,
    Reload,
    Switch,
    Fire,
    Jump,
    Crouch,
}

/// A press: `key` down from tick `at` (counted from the trick's start)
/// for `hold` ticks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Beat {
    pub key: Key,
    pub at: u32,
    pub hold: u32,
}

/// What is down on one tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Keys {
    pub melee: bool,
    pub reload: bool,
    pub switch: bool,
    pub fire: bool,
    pub jump: bool,
    pub crouch: bool,
}

/// A trick as presses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Macro {
    pub trick: Trick,
    pub beats: Vec<Beat>,
}

impl Macro {
    /// Ticks from the first press to the last release.
    pub fn len(&self) -> u32 {
        self.beats.iter().map(|b| b.at + b.hold).max().unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.beats.is_empty()
    }

    /// What is down `t` ticks after the start.
    pub fn keys(&self, t: u32) -> Keys {
        let mut k = Keys::default();
        for b in &self.beats {
            if (b.at..b.at + b.hold).contains(&t) {
                match b.key {
                    Key::Melee => k.melee = true,
                    Key::Reload => k.reload = true,
                    Key::Switch => k.switch = true,
                    Key::Fire => k.fire = true,
                    Key::Jump => k.jump = true,
                    Key::Crouch => k.crouch = true,
                }
            }
        }
        k
    }

    /// When `key` is first pressed, if it is.
    pub fn first(&self, key: Key) -> Option<u32> {
        self.beats
            .iter()
            .filter(|b| b.key == key)
            .map(|b| b.at)
            .min()
    }
}

/// A tap's length (ticks).
pub const TAP: u32 = 2;
/// From B to the melee landing (ticks). Not traced: the hit is the melee
/// animation's damage frame. A starting point to tune in a game run.
pub const MELEE_CONNECT: u32 = 5;
/// From the first pull to the battle rifle's third round leaving (ticks).
/// Not traced: the trigger's burst timing. A starting point.
pub const BURST: u32 = 4;
/// Sword flying's switch-to-reload gap at its cleanest (ticks). The
/// community's window is 0.066 s at 30 frames a second: two ticks.
pub const FLY_SLIDE: u32 = 1;
pub const FLY_WINDOW: u32 = 2;
/// From the reload to the pull (ticks): "the third button slower than the
/// first two".
pub const FLY_PULL: u32 = 2;
/// A in the lunge for height, this long after the pull (ticks).
pub const FLY_JUMP: u32 = 8;
/// The crouch jump's crouch, this long after A (ticks): near the top of a
/// standing jump. A starting point.
pub const CROUCH_APEX: u32 = 7;
pub const CROUCH_HOLD: u32 = 10;

/// How late a bot of this skill can be on each press after the first
/// (ticks, inclusive).
pub fn slop(skill: Skill) -> (u32, u32) {
    match skill {
        Skill::Easy => (1, 3),
        Skill::Normal => (0, 2),
        Skill::Heroic => (0, 1),
        Skill::Legendary => (0, 0),
    }
}

/// How much of the setting's chance a bot of this skill takes.
pub fn eagerness(skill: Skill) -> f32 {
    match skill {
        Skill::Easy => 0.4,
        Skill::Normal => 0.7,
        Skill::Heroic => 0.9,
        Skill::Legendary => 1.0,
    }
}

/// The chance a fight trick is tried at an opening.
pub fn chance(setting: Glitches, skill: Skill) -> f32 {
    setting.fight_chance() * eagerness(skill)
}

/// `trick`'s presses for a bot of `skill`; `roll` gives uniform numbers
/// in 0..1 for its lateness.
pub fn plan(trick: Trick, skill: Skill, roll: &mut dyn FnMut() -> f32) -> Macro {
    let (lo, hi) = slop(skill);
    let mut late = || lo + ((roll() * (hi - lo + 1) as f32) as u32).min(hi - lo);
    let tap = |key, at| Beat { key, at, hold: TAP };
    let beats = match trick {
        Trick::Bxr => {
            let x = MELEE_CONNECT + late();
            let r = x + TAP + late();
            // "Hold X while pressing R."
            vec![
                tap(Key::Melee, 0),
                Beat {
                    key: Key::Reload,
                    at: x,
                    hold: r + TAP - x,
                },
                tap(Key::Fire, r),
            ]
        }
        Trick::Byyr => {
            let y1 = MELEE_CONNECT + late();
            let y2 = y1 + TAP + 1 + late();
            let r = y2 + TAP + late();
            vec![
                tap(Key::Melee, 0),
                tap(Key::Switch, y1),
                tap(Key::Switch, y2),
                tap(Key::Fire, r),
            ]
        }
        Trick::Bxb => {
            let x = MELEE_CONNECT + late();
            let b = x + TAP + late();
            vec![tap(Key::Melee, 0), tap(Key::Reload, x), tap(Key::Melee, b)]
        }
        Trick::DoubleShot => double_shot(0, &mut late),
        Trick::QuadShot => {
            let mut v = double_shot(0, &mut late);
            let x = v[2].at;
            let y1 = x + TAP + 1 + late();
            let y2 = y1 + TAP + 1 + late();
            v.push(tap(Key::Switch, y1));
            v.push(tap(Key::Switch, y2));
            v.extend(double_shot(y2 + TAP + 1 + late(), &mut late));
            v
        }
        Trick::SwordFly => {
            let x = FLY_SLIDE + late();
            let r = x + FLY_PULL + late();
            vec![
                Beat {
                    key: Key::Switch,
                    at: 0,
                    hold: 1,
                },
                Beat {
                    key: Key::Reload,
                    at: x,
                    hold: 1,
                },
                tap(Key::Fire, r),
                Beat {
                    key: Key::Jump,
                    at: r + FLY_JUMP,
                    hold: 3,
                },
            ]
        }
        Trick::CrouchJump => vec![
            Beat {
                key: Key::Jump,
                at: 0,
                hold: 3,
            },
            Beat {
                key: Key::Crouch,
                at: CROUCH_APEX + late(),
                hold: CROUCH_HOLD,
            },
        ],
    };
    Macro { trick, beats }
}

fn double_shot(at: u32, late: &mut dyn FnMut() -> u32) -> Vec<Beat> {
    let r2 = at + BURST + late();
    let x = r2 + TAP + late();
    vec![
        Beat {
            key: Key::Fire,
            at,
            hold: TAP,
        },
        // "Holding the second R while pressing X."
        Beat {
            key: Key::Fire,
            at: r2,
            hold: x + TAP - r2,
        },
        Beat {
            key: Key::Reload,
            at: x,
            hold: TAP,
        },
    ]
}

/// Whether sword flying's switch and reload landed in the window: the
/// trick goes, else it is an ordinary switch.
pub fn clean_fly(m: &Macro) -> bool {
    match (m.first(Key::Switch), m.first(Key::Reload)) {
        (Some(y), Some(x)) => x > y && x - y <= FLY_WINDOW,
        _ => false,
    }
}

/// A macro under way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub mac: Macro,
    pub start: u32,
}

impl Run {
    pub fn new(mac: Macro, start: u32) -> Run {
        Run { mac, start }
    }

    /// What is down at `tick`; `None` once it is over.
    pub fn keys(&self, tick: u32) -> Option<Keys> {
        let t = tick.wrapping_sub(self.start);
        (t < self.mac.len()).then(|| self.mac.keys(t))
    }
}

// ---- when ----

/// What the mind knows of the fight, for whether a trick is allowed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Gate {
    pub setting: Glitches,
    /// Carrying the flag, the bomb or the ball: never.
    pub carrying: bool,
    /// It would obviously lose: shields down, withdrawing, or another
    /// enemy close by.
    pub losing: bool,
    /// The mind would melee the target now (in reach, facing it).
    pub in_melee: bool,
    /// Another enemy is near the target: a sword fly would land in a
    /// group.
    pub crowded: bool,
}

/// The target as the fight sees it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Moment {
    pub dist: f32,
    pub on_target: bool,
    pub reacted: bool,
    pub held: Option<Gun>,
    pub other: Option<Gun>,
}

/// A sword fly starts this much past the sword's own lunge (world
/// units): nearer, the plain lunge does it.
pub const FLY_MARGIN: f32 = 1.0;
/// ...and no further than this, whatever the gun's reticle reaches.
pub const FLY_MAX: f32 = 20.0;

/// X cancels a melee without reloading: the magazine is full, or there is
/// nothing to load (Halopedia, "BXR").
pub fn cancels(a: Ammo) -> bool {
    match a {
        Ammo::Rounds {
            loaded,
            capacity,
            reserve,
            ..
        } => loaded >= capacity || reserve <= 0,
        _ => false,
    }
}

/// A battle rifle, as far as the tag shows: once per pull, accurate, one
/// zoom level, a magazine of 30 or more (36). The double shot is the
/// battle rifle's.
pub fn burst_rifle(g: &Gun) -> bool {
    g.role() == Role::Precise
        && g.profile.zoom >= 1
        && matches!(g.ammo, Ammo::Rounds { capacity, .. } if capacity >= 30)
}

fn loaded(a: Ammo) -> (i16, i16) {
    match a {
        Ammo::Rounds {
            loaded, reserve, ..
        } => (loaded, reserve),
        _ => (0, 0),
    }
}

/// The fight trick that fits now, if any is allowed.
pub fn opening(g: &Gate, m: &Moment) -> Option<Trick> {
    if g.setting == Glitches::Off || g.carrying || g.losing || !m.reacted {
        return None;
    }
    let held = m.held?;
    // Sword flying: the gun's reticle red on a target past the sword's
    // own lunge, the sword the other weapon.
    if let Some(o) = m.other
        && o.role() == Role::Blade
        && held.role() != Role::Blade
        && held.ready()
        && m.on_target
        && !g.crowded
    {
        let red = held.profile.cone_range.max(held.profile.pull_range);
        if m.dist > o.reach().lunge + FLY_MARGIN && m.dist <= red.min(FLY_MAX) {
            return Some(Trick::SwordFly);
        }
    }
    let gun = matches!(
        held.role(),
        Role::Precise | Role::Spray | Role::Scatter | Role::Sniper | Role::Blast
    );
    if g.in_melee && gun {
        let other_ready = m.other.is_some_and(|o| o.ready());
        return if held.ready() && cancels(held.ammo) {
            Some(Trick::Bxr)
        } else if held.ready() && other_ready {
            Some(Trick::Byyr)
        } else if cancels(held.ammo) {
            Some(Trick::Bxb)
        } else {
            None
        };
    }
    if burst_rifle(&held) && m.on_target && !g.in_melee {
        let (lo, hi) = held.reach().fire;
        let (rounds, reserve) = loaded(held.ammo);
        if (lo..hi).contains(&m.dist) && reserve > 0 {
            if rounds >= 12 && m.other.is_some() {
                return Some(Trick::QuadShot);
            }
            if rounds >= 6 {
                return Some(Trick::DoubleShot);
            }
        }
    }
    None
}

// ---- show-offs ----

/// The show-offs' timer: one now and then while calm, never with the
/// setting off.
#[derive(Clone, Debug, Default)]
pub struct ShowOff {
    next: Option<u32>,
    rng: u64,
}

impl ShowOff {
    pub fn new(seed: u64) -> ShowOff {
        ShowOff {
            next: None,
            rng: seed | 1,
        }
    }

    pub fn uniform(&mut self) -> f32 {
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        let r = self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d);
        (r >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Forgets the wait (the bot died, or the setting changed).
    pub fn reset(&mut self) {
        self.next = None;
    }

    /// Whether a show-off is due at `tick`. `free`: calm (nobody in view
    /// for a while), no objective to play, nothing carried. Not free puts
    /// the clock back: a show-off comes a while after things calm down.
    pub fn due(&mut self, tick: u32, setting: Glitches, skill: Skill, free: bool) -> bool {
        let Some((lo, hi)) = setting.show_off_every() else {
            self.next = None;
            return false;
        };
        let wait = |s: &mut Self| {
            let span = (hi - lo) as f32 / eagerness(skill).max(0.1);
            tick.wrapping_add(lo + (s.uniform() * span) as u32)
        };
        if !free {
            if self
                .next
                .is_none_or(|n| crate::mind::feel::reached(tick, n))
            {
                self.next = Some(wait(self));
            }
            return false;
        }
        let next = match self.next {
            Some(n) => n,
            None => {
                let n = wait(self);
                self.next = Some(n);
                n
            }
        };
        if crate::mind::feel::reached(tick, next) {
            self.next = Some(wait(self));
            return true;
        }
        false
    }
}

// ---- the super bounce ----

/// Within this of the spot (XY) it stands and jumps.
pub const SETTLE: f32 = 0.05;
/// Gives up settling after this long and jumps from where it is.
pub const SETTLE_TICKS: u32 = 90;
/// Stands still this long before jumping.
pub const STILL_TICKS: u32 = 4;
/// A rise this much over its lowest point after starting to fall is the
/// launch (world units). A plain landing never rises again.
pub const LAUNCH_RISE: f32 = 0.4;
/// Starting to fall: this far under the jump's top.
pub const FALLING: f32 = 0.2;
/// Back on the floor (or caught on the crease): within this of the
/// floor, unmoving for [`REST_TICKS`].
pub const REST: f32 = 0.2;
pub const REST_TICKS: u32 = 3;
/// A jump that has not come down after this long has gone somewhere
/// else: over.
pub const AIR_TICKS: u32 = 120;
/// The stick toward the spot's drift while in the air.
pub const DRIFT: f32 = 0.15;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// Walking the last few centimetres onto the spot; still since.
    Settle {
        since: u32,
        still: Option<u32>,
    },
    /// A held, from the tick.
    Jump(u32),
    /// In the air since the tick: its top and its lowest since falling.
    Air {
        since: u32,
        top: f32,
        low: Option<f32>,
        rest: Option<(f32, u32)>,
    },
    Done(bool),
}

/// What a bounce asks of the pad this tick.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BounceOut {
    /// The left stick as a world direction (its length the deflection);
    /// `None` hands off.
    pub walk: Option<[f32; 2]>,
    pub jump: bool,
    /// Over: `Some(true)` launched, `Some(false)` gave up.
    pub done: Option<bool>,
}

/// A super bounce at one spot: stand on it, a plain standing jump, hands
/// off on the way down, and again from a nudged place until it launches
/// or the tries run out. Not every landing bounces
/// (docs/game/superbounce.md: 5 of 20), so it is tried again.
#[derive(Clone, Debug)]
pub struct BounceRun {
    pub spot: Bounce,
    /// Where it stands for this try.
    pub target: [f32; 2],
    pub tries: u32,
    max_tries: u32,
    phase: Phase,
    rng: u64,
}

impl BounceRun {
    pub fn new(spot: Bounce, tries: u32, tick: u32, seed: u64) -> BounceRun {
        BounceRun {
            spot,
            target: [spot.stand[0], spot.stand[1]],
            tries: 0,
            max_tries: tries.max(1),
            phase: Phase::Settle {
                since: tick,
                still: None,
            },
            rng: seed | 1,
        }
    }

    fn signed(&mut self) -> f32 {
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        let r = self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d);
        2.0 * ((r >> 40) as f32 / (1u64 << 24) as f32) - 1.0
    }

    /// In the air, from the jump to the landing or the launch.
    pub fn airborne(&self) -> bool {
        matches!(self.phase, Phase::Jump(_) | Phase::Air { .. })
    }

    /// Where it would like to be (the spot's floor) for the walk there.
    pub fn stand(&self) -> Vec3 {
        [self.target[0], self.target[1], self.spot.stand[2]]
    }

    /// This tick, standing at `at`.
    pub fn step(&mut self, at: Vec3, tick: u32) -> BounceOut {
        let mut out = BounceOut::default();
        let floor = self.spot.stand[2];
        match self.phase {
            Phase::Settle { since, still } => {
                let d = [self.target[0] - at[0], self.target[1] - at[1]];
                let far = d[0].hypot(d[1]);
                let late = tick.wrapping_sub(since) > SETTLE_TICKS;
                if far > SETTLE && !late {
                    let pace = (far * 4.0).clamp(0.12, 0.6) / far;
                    out.walk = Some([d[0] * pace, d[1] * pace]);
                    self.phase = Phase::Settle { since, still: None };
                } else {
                    let still = still.unwrap_or(tick);
                    if tick.wrapping_sub(still) >= STILL_TICKS || late {
                        self.phase = Phase::Jump(tick);
                        out.jump = true;
                    } else {
                        self.phase = Phase::Settle {
                            since,
                            still: Some(still),
                        };
                    }
                }
            }
            Phase::Jump(from) => {
                if tick.wrapping_sub(from) < 3 {
                    out.jump = true;
                } else {
                    self.phase = Phase::Air {
                        since: from,
                        top: at[2],
                        low: None,
                        rest: None,
                    };
                }
            }
            Phase::Air {
                since,
                top,
                low,
                rest,
            } => {
                let top = top.max(at[2]);
                let low = if at[2] < top - FALLING {
                    Some(low.map_or(at[2], |l: f32| l.min(at[2])))
                } else {
                    low
                };
                out.walk = Some([self.spot.drift[0] * DRIFT, self.spot.drift[1] * DRIFT]);
                if low.is_some_and(|l| at[2] > l + LAUNCH_RISE) {
                    // Sank into the seam and was thrown up.
                    self.phase = Phase::Done(true);
                    out.walk = None;
                    out.done = Some(true);
                    return out;
                }
                let resting = (at[2] - floor).abs() < REST;
                let rest = match rest {
                    Some((z, t)) if resting && (at[2] - z).abs() < 0.01 => Some((z, t)),
                    _ if resting && low.is_some() => Some((at[2], tick)),
                    _ => None,
                };
                let landed = rest.is_some_and(|(_, t)| tick.wrapping_sub(t) >= REST_TICKS);
                if landed || tick.wrapping_sub(since) > AIR_TICKS {
                    out.walk = None;
                    self.tries += 1;
                    if self.tries >= self.max_tries || !landed {
                        self.phase = Phase::Done(false);
                        out.done = Some(false);
                    } else {
                        // Again, from a little to one side.
                        let s = self.spot.spread;
                        self.target = [
                            self.spot.stand[0] + s * self.signed(),
                            self.spot.stand[1] + s * self.signed(),
                        ];
                        self.phase = Phase::Settle {
                            since: tick,
                            still: None,
                        };
                    }
                } else {
                    self.phase = Phase::Air {
                        since,
                        top,
                        low,
                        rest,
                    };
                }
            }
            Phase::Done(ok) => out.done = Some(ok),
        }
        out
    }
}

#[cfg(test)]
mod tests;
