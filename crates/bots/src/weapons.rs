//! Weapon skill beyond the trigger (docs/game/bots.md, "Built: weapon
//! skill"): how far each gun is good to ([`reach`], from its role and its
//! tag's aim assist reach), the noob combo and the melee finish
//! ([`Combo`]), and the left hand when dual wielding ([`LeftHand`]).
//!
//! Pure and deterministic, time in game ticks (30 a second). The mind
//! feeds [`Combo::step`] every tick of a fight and applies what it says on
//! top of its own trigger and buttons.

use crate::arms::{Gun, Profile, Role, Trigger};
use crate::mind::Skill;

// ---- ranges ----

/// The distances a gun is good at (world units).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reach {
    /// The band the movement keeps the target in and the weapon choice
    /// prefers ([`crate::arms::preferred_range`]).
    pub band: (f32, f32),
    /// Fires at targets from `.0` to `.1` away: beyond that the rounds are
    /// wasted (spread, falloff), nearer a splash weapon hurts its holder.
    pub fire: (f32, f32),
    /// A blade's lunge: it swings this close (0 for a gun).
    pub lunge: f32,
}

/// A blade with no aim assist read lunges this far.
pub const BLADE_LUNGE: f32 = 1.5;
/// A blade's lunge from its aim assist reach is kept within this.
pub const LUNGE_LIMITS: (f32, f32) = (1.0, 3.0);
/// A gun's fire range reaches this far past its band's far end.
pub const FIRE_PAST_BAND: f32 = 1.6;

/// What `p` is good at. The role's band ([`Role::band`]) and fire range
/// ([`Role::fire_range`]), refined by the tag's aim assist reach
/// (`cone_range`, `pull_range`; `weap +0x20c`, `+0x214`) where Bungie set
/// one: aim assist stops where a gun stops being worth firing, so a
/// magnum's band ends sooner than a battle rifle's and an SMG's sooner
/// than a plasma rifle's. Snipers and splash weapons keep their role's
/// (their reach is the scope's); a blade's lunge is its reach.
pub fn reach(p: &Profile) -> Reach {
    let role = p.role();
    let (lo, hi) = role.band();
    let (flo, fhi) = role.fire_range();
    let r = p.cone_range.max(p.pull_range);
    match role {
        Role::Blade => {
            let lunge = if r > 0.0 {
                r.clamp(LUNGE_LIMITS.0, LUNGE_LIMITS.1)
            } else {
                BLADE_LUNGE
            };
            Reach {
                band: (0.0, lunge),
                fire: (0.0, 0.0),
                lunge,
            }
        }
        Role::Scatter | Role::Spray | Role::Precise if r > 0.0 => {
            // Within half and one and a half times the role's: a tag
            // value far off is not trusted to throw the bot about.
            let far = r.clamp((lo + 2.0).max(0.5 * hi), 1.5 * hi).min(fhi);
            Reach {
                band: (lo, far),
                fire: (flo, (far * FIRE_PAST_BAND).clamp(far, fhi)),
                lunge: 0.0,
            }
        }
        _ => Reach {
            band: (lo, hi),
            fire: (flo, fhi),
            lunge: 0.0,
        },
    }
}

/// How far `d` is outside `band`, relative to it: 0 inside.
pub fn misfit(band: (f32, f32), d: f32) -> f32 {
    let (lo, hi) = band;
    if d < lo {
        (lo - d) / lo.max(1.0)
    } else if d > hi {
        (d - hi) / hi.max(1.0)
    } else {
        0.0
    }
}

/// A short gun: its band ends within this (shotgun, SMG, sword, plasma
/// weapons). A long one starts at [`LONG_FROM`] or further (rifles,
/// sniper, rockets). A pairing of one of each covers every range.
pub const SHORT_UP_TO: f32 = 12.0;
pub const LONG_FROM: f32 = 5.0;

/// Which end of the fight a gun is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Short,
    Long,
    /// Not known (weapon-blind) or neither.
    Other,
}

pub fn class(g: &Gun) -> Class {
    if g.role() == Role::Unknown {
        return Class::Other;
    }
    let (lo, hi) = g.reach().band;
    if hi <= SHORT_UP_TO {
        Class::Short
    } else if lo >= LONG_FROM {
        Class::Long
    } else {
        Class::Other
    }
}

// ---- combos ----

/// A plasma pistol's overcharge takes this long held (ticks). Not read:
/// the trigger's charging time is in the `weap` trigger block, not traced
/// (docs/game/bots.md, "Built: weapon skill").
pub const CHARGE_TICKS: u32 = 24;
/// Released by this long past [`CHARGE_TICKS`] even off target: a held
/// overcharge drains the battery.
pub const CHARGE_HOLD: u32 = 36;
/// Gives up drawing the plasma pistol, or putting it away, after this
/// long.
pub const SWAP_WAIT: u32 = 30;
/// After the combo the weapon choice is held this long: the gun drawn for
/// the headshot is not switched away from.
pub const FOLLOW_TICKS: u32 = 60;
/// Starts a combo at this range only (world units): the overcharge homes
/// in on a target in front, not one across the map.
pub const COMBO_RANGE: (f32, f32) = (1.5, 14.0);
/// A melee finishes a target this close (world units): the lunge of a
/// gun's melee, a little over the mind's arm's length.
pub const FINISH_RANGE: f32 = 1.3;
/// Melee presses this far apart at least (ticks).
pub const MELEE_EVERY: u32 = 20;
/// A tapped button is held this many ticks.
pub const TAP: u32 = 2;
/// The battery a combo needs (a plasma pistol's, 0 to 1).
pub const COMBO_BATTERY: f32 = 0.15;

/// What skill changes in combos.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ComboSkill {
    /// The chance it goes for a noob combo on a new target, when it
    /// carries the guns for it.
    pub chance: f32,
    /// Ticks between the overcharge leaving and the switch.
    pub swap_delay: u32,
    /// Melees to finish a target it has been hitting.
    pub finish: bool,
    /// Pulls on target (a once-per-pull gun's shots, or seconds of an
    /// automatic's fire times [`SPRAY_PULLS`]) before a target counts as
    /// shields down.
    pub finish_pulls: u32,
}

/// An automatic counts one "pull" per this many ticks of fire on target.
pub const SPRAY_PULLS: u32 = 8;

impl ComboSkill {
    pub fn of(skill: Skill) -> ComboSkill {
        match skill {
            Skill::Easy => ComboSkill {
                chance: 0.1,
                swap_delay: 18,
                finish: false,
                finish_pulls: u32::MAX,
            },
            Skill::Normal => ComboSkill {
                chance: 0.35,
                swap_delay: 10,
                finish: true,
                finish_pulls: 5,
            },
            Skill::Heroic => ComboSkill {
                chance: 0.7,
                swap_delay: 6,
                finish: true,
                finish_pulls: 4,
            },
            Skill::Legendary => ComboSkill {
                chance: 0.9,
                swap_delay: 3,
                finish: true,
                finish_pulls: 3,
            },
        }
    }
}

/// Where a combo is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    Idle,
    /// Switching to the plasma pistol (from the tick).
    Draw(u32),
    /// Charging (from the tick).
    Charge(u32),
    /// The overcharge is away (at the tick): wait for the switch.
    Release(u32),
    /// Switching to the other gun (from the tick).
    Swap(u32),
    /// Shooting for the head with the other gun, the choice held, until
    /// the tick.
    Follow(u32),
}

/// What the mind gives [`Combo::step`].
#[derive(Clone, Copy, Debug)]
pub struct ComboIn {
    pub tick: u32,
    /// The target (its id), if it fights one.
    pub target: Option<u32>,
    pub dist: f32,
    /// The aim is on the target (inside the fire cone).
    pub on_target: bool,
    /// Its reaction to the target has passed.
    pub reacted: bool,
    pub held: Option<Gun>,
    pub other: Option<Gun>,
    /// Whether the trigger is down this tick (the mind's own decision).
    pub firing: bool,
}

/// What [`Combo::step`] asks of the pad.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ComboOut {
    /// Overrides the right trigger: `Some(true)` held (charging),
    /// `Some(false)` up.
    pub trigger: Option<bool>,
    /// Press switch weapons.
    pub switch: bool,
    /// Press melee.
    pub melee: bool,
    /// A combo is under way: the weapons decision's switches wait.
    pub busy: bool,
}

/// The combo state, kept by the mind.
#[derive(Clone, Debug, Default)]
pub struct Combo {
    pub phase: Phase,
    target: Option<u32>,
    /// It rolled to combo this target.
    go: bool,
    /// Pulls (or bursts) on the target so far: shots that likely landed.
    pub hits: u32,
    firing_since: Option<u32>,
    was_firing: bool,
    melee_ready: u32,
    melee_until: u32,
    rng: u64,
}

fn charge(g: Option<Gun>) -> bool {
    g.is_some_and(|g| {
        g.profile.trigger == Trigger::Charge
            && match g.ammo {
                crate::arms::Ammo::Battery(b) => b >= COMBO_BATTERY,
                a => a.ready(),
            }
    })
}
/// A gun to follow an overcharge with: one aimed for the head (magnum,
/// battle rifle, carbine) or a fast automatic (SMG).
fn follow_up(g: Option<Gun>) -> bool {
    g.is_some_and(|g| {
        g.ready()
            && match g.role() {
                Role::Precise => true,
                Role::Spray => g.profile.trigger == Trigger::Automatic,
                _ => false,
            }
    })
}

impl Combo {
    pub fn new(seed: u64) -> Combo {
        Combo {
            rng: seed | 1,
            ..Combo::default()
        }
    }

    fn uniform(&mut self) -> f32 {
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        let r = self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d);
        (r >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Forgets the fight (the bot died).
    pub fn reset(&mut self) {
        let rng = self.rng;
        *self = Combo::default();
        self.rng = rng;
    }

    /// A combo is under way.
    pub fn busy(&self) -> bool {
        self.phase != Phase::Idle
    }

    /// This tick's combo: drawing, charging and releasing the plasma
    /// pistol, switching to the follow-up gun, and the melee finish.
    pub fn step(&mut self, c: &ComboIn, skill: Skill) -> ComboOut {
        let k = ComboSkill::of(skill);
        let mut out = ComboOut::default();
        // A new target: a fresh count and a fresh roll. A combo under way
        // goes on (the overcharge is spent on whoever).
        if c.target != self.target {
            self.target = c.target;
            self.hits = 0;
            self.firing_since = None;
            self.go = c.target.is_some() && self.uniform() < k.chance;
            if matches!(self.phase, Phase::Draw(_) | Phase::Charge(_)) {
                self.phase = Phase::Idle;
            }
        }
        if c.target.is_none() {
            if let Phase::Follow(_) = self.phase {
                self.phase = Phase::Idle;
            }
            self.was_firing = false;
            return out;
        }
        // Shots on the target: a once-per-pull gun's pulls, an automatic's
        // time on target.
        let auto = c
            .held
            .is_some_and(|g| g.profile.trigger == Trigger::Automatic);
        if c.firing && c.on_target {
            if auto {
                let since = *self.firing_since.get_or_insert(c.tick);
                if c.tick.wrapping_sub(since) >= SPRAY_PULLS {
                    self.hits = self.hits.saturating_add(1);
                    self.firing_since = Some(c.tick);
                }
            } else if !self.was_firing {
                self.hits = self.hits.saturating_add(1);
            }
        } else {
            self.firing_since = None;
        }
        self.was_firing = c.firing;

        let t = c.tick;
        let in_range = (COMBO_RANGE.0..COMBO_RANGE.1).contains(&c.dist);
        match self.phase {
            Phase::Idle => {
                if self.go && c.reacted && in_range {
                    if charge(c.held) && follow_up(c.other) {
                        self.phase = Phase::Charge(t);
                        out.trigger = Some(true);
                    } else if c.held.is_some_and(|g| g.role() == Role::Precise)
                        && charge(c.other)
                        && self.hits == 0
                    {
                        // A fresh target and the pistol in the other
                        // hand: open with it.
                        self.phase = Phase::Draw(t);
                        out.switch = true;
                        out.trigger = Some(false);
                    }
                }
            }
            Phase::Draw(since) => {
                if charge(c.held) {
                    self.phase = Phase::Charge(t);
                    out.trigger = Some(true);
                } else if t.wrapping_sub(since) > SWAP_WAIT {
                    self.phase = Phase::Idle;
                    self.go = false;
                } else {
                    out.switch = t.wrapping_sub(since) < TAP;
                    out.trigger = Some(false);
                }
            }
            Phase::Charge(since) => {
                let held = t.wrapping_sub(since);
                if !charge(c.held) {
                    self.phase = Phase::Idle;
                    self.go = false;
                } else if held >= CHARGE_TICKS && (c.on_target || held >= CHARGE_HOLD) {
                    // Let go: the overcharge flies.
                    self.phase = Phase::Release(t);
                    out.trigger = Some(false);
                } else {
                    out.trigger = Some(true);
                }
            }
            Phase::Release(at) => {
                out.trigger = Some(false);
                if t.wrapping_sub(at) >= k.swap_delay {
                    self.phase = Phase::Swap(t);
                    out.switch = true;
                }
            }
            Phase::Swap(since) => {
                let switched = c.held.is_some_and(|g| g.profile.trigger != Trigger::Charge);
                if switched || t.wrapping_sub(since) > SWAP_WAIT {
                    self.phase = Phase::Follow(t + FOLLOW_TICKS);
                    self.go = false;
                    // The overcharge popped the shields: finish them.
                    self.hits = self.hits.max(k.finish_pulls);
                } else {
                    out.switch = t.wrapping_sub(since) < TAP;
                    out.trigger = Some(false);
                }
            }
            Phase::Follow(until) => {
                if t >= until {
                    self.phase = Phase::Idle;
                }
            }
        }
        out.busy = self.busy();
        // The melee finish: close, and it has been hitting the target.
        let gun = c
            .held
            .is_some_and(|g| matches!(g.role(), Role::Precise | Role::Spray | Role::Scatter));
        if k.finish
            && gun
            && c.reacted
            && c.dist < FINISH_RANGE
            && self.hits >= k.finish_pulls
            && t >= self.melee_ready
        {
            self.melee_until = t + TAP;
            self.melee_ready = t + MELEE_EVERY;
        }
        out.melee = t < self.melee_until;
        out
    }
}

// ---- the left hand ----

/// The left trigger when dual wielding: Halo 2 fires the left-hand gun
/// with it (and throws no grenades). A charge gun (the plasma pistol) is
/// held to charge and let go on target; a once-per-pull gun is pulled
/// and let go at its rate; an automatic is held.
#[derive(Clone, Debug, Default)]
pub struct LeftHand {
    next: u32,
    until: u32,
    charge: Option<u32>,
}

/// After an overcharge from the left hand, the next charge waits this
/// long (ticks).
pub const LEFT_RECHARGE: u32 = 15;

impl LeftHand {
    pub fn reset(&mut self) {
        *self = LeftHand::default();
    }

    /// Whether the left trigger is down this tick, wanting to fire
    /// (`want`: on target and in range) with `gun` in the left hand.
    pub fn trigger(&mut self, tick: u32, gun: &Gun, want: bool) -> bool {
        if !gun.ammo.ready() {
            self.charge = None;
            return false;
        }
        match gun.profile.trigger {
            Trigger::Charge => match self.charge {
                Some(since) => {
                    let held = tick.wrapping_sub(since);
                    if held >= CHARGE_TICKS && (want || held >= CHARGE_HOLD) {
                        self.charge = None;
                        self.next = tick + LEFT_RECHARGE;
                        false
                    } else {
                        true
                    }
                }
                None if want && tick >= self.next => {
                    self.charge = Some(tick);
                    true
                }
                None => false,
            },
            Trigger::Single => {
                if tick < self.until {
                    return true;
                }
                if want && tick >= self.next {
                    let every = if gun.profile.rate > 0.0 {
                        (30.0 / gun.profile.rate).round() as u32
                    } else {
                        crate::tactics::PULL_DEFAULT
                    };
                    self.until = tick + crate::tactics::PULL_TICKS;
                    self.next = tick + every.max(crate::tactics::PULL_MIN);
                    return true;
                }
                false
            }
            Trigger::Automatic | Trigger::Unknown => want,
            Trigger::Absent => false,
        }
    }
}

#[cfg(test)]
mod tests;
