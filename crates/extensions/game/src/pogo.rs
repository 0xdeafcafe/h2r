//! The Pogo Stick's rider (docs/game/pogo-stick.md, "Phases and tricks"):
//! what a player holding the stick is doing, from one read a tick.
//!
//! The stick is carried on the ground and ridden in the air. A [`Rider`]
//! is one player's: stepped once a tick with whether the player holds a
//! stick, the airborne bit, the vertical speed and the bounce the medal
//! chain filed on that tick ([`crate::superbounce_chain`]), it gives the
//! [`Sample`] the pose is drawn from and the [`Cue`] the sounds play on.
//! [`Riders`] keeps one per player slot. Everything here is pure, so the
//! tests are lists of ticks (`pogo_tests.rs`); [`crate::pogo_ride`] reads
//! the game for it.
//!
//! - **Mount.** [`AIR_TICKS`] ticks airborne in a row start the mount,
//!   which takes [`MOUNT_TICKS`].
//! - **Touchdown.** A touchdown ([`crate::superbounce_charge::touchdown`])
//!   compresses the piston. The chain files its verdict when the launch
//!   window closes, so the piston stays in for up to [`VERDICT_TICKS`].
//! - **Bounce.** The verdict springs the piston out with a flare at the
//!   foot and picks the trick of the flight by the chain's count
//!   ([`trick_for`]).
//! - **No bounce.** On the ground the rider dismounts over
//!   [`DISMOUNT_TICKS`]; still in the air it rides on.
//! - **The trick** is timed from the vertical speed: it is centred on the
//!   apex and takes [`TRICK_SHARE`] of the hang time, up to the trick's
//!   longest ([`Trick::longest`]). A flight under [`MIN_HANG`] gets the
//!   tuck alone.
//!
//! It is not in a snapshot: after a state load or a Theater seek a rider
//! starts from the next tick, and a trick in flight is dropped.

use pogo::Style;

use crate::superbounce_assist::GRAVITY;
use crate::superbounce_chain::SLOTS;
use crate::superbounce_charge::{LAUNCH_TICKS, touchdown};

/// Ticks a second.
pub const TICK_RATE: f32 = 30.0;
/// Ticks airborne in a row before the mount starts: a step down a stair
/// is not a flight.
pub const AIR_TICKS: u32 = 3;
/// Ticks the mount takes.
pub const MOUNT_TICKS: u32 = 6;
/// Ticks the dismount takes.
pub const DISMOUNT_TICKS: u32 = 8;
/// Ticks after a touchdown the piston waits for the chain's verdict: the
/// launch window, and one more for the tick the chain files it on.
pub const VERDICT_TICKS: u32 = LAUNCH_TICKS + 1;
/// Ticks the piston takes to spring out.
pub const SPRING_TICKS: u32 = 3;
/// Ticks the flare at the foot fades over.
pub const FLARE_TICKS: u32 = 8;
/// Ticks the knees take to tuck in a flight, and to come back out.
pub const TUCK_TICKS: u32 = 5;
/// A flight shorter than this (seconds, up and back to the launch
/// height) has no trick.
pub const MIN_HANG: f32 = 0.9;
/// The share of the hang time a trick takes.
pub const TRICK_SHARE: f32 = 0.7;
/// Seconds a forced trick takes with no flight to time it from
/// (`pogo.test` on the ground).
pub const TEST_SECONDS: f32 = 1.2;

/// A trick.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Trick {
    NoHander,
    CanCan,
    BarSpin,
    Superman,
    Backflip,
    DoubleBackflip,
    FrontFlip,
}

impl Trick {
    pub const ALL: [Trick; 7] = [
        Trick::NoHander,
        Trick::CanCan,
        Trick::BarSpin,
        Trick::Superman,
        Trick::Backflip,
        Trick::DoubleBackflip,
        Trick::FrontFlip,
    ];

    /// The trick's id on the agent API and in the pose keys.
    pub fn id(self) -> &'static str {
        match self {
            Trick::NoHander => "no_hander",
            Trick::CanCan => "can_can",
            Trick::BarSpin => "bar_spin",
            Trick::Superman => "superman",
            Trick::Backflip => "backflip",
            Trick::DoubleBackflip => "double_backflip",
            Trick::FrontFlip => "front_flip",
        }
    }

    pub fn from_id(id: &str) -> Option<Trick> {
        Trick::ALL.into_iter().find(|t| t.id() == id)
    }

    /// The trick's name.
    pub fn name(self) -> &'static str {
        match self {
            Trick::NoHander => "No-Hander",
            Trick::CanCan => "Can-Can",
            Trick::BarSpin => "Bar Spin",
            Trick::Superman => "Superman",
            Trick::Backflip => "Backflip",
            Trick::DoubleBackflip => "Double Backflip",
            Trick::FrontFlip => "Front Flip",
        }
    }

    /// The longest the trick takes, seconds: a high bounce hangs for
    /// several seconds, and a flip that slow reads as a float.
    pub fn longest(self) -> f32 {
        match self {
            Trick::NoHander | Trick::CanCan => 1.0,
            Trick::BarSpin => 0.9,
            Trick::Superman => 1.3,
            Trick::Backflip | Trick::FrontFlip => 1.1,
            Trick::DoubleBackflip => 1.5,
        }
    }

    /// Whole turns the rider and the stick make about the pitch axis over
    /// the trick, backward positive.
    pub fn flips(self) -> f32 {
        match self {
            Trick::Backflip => 1.0,
            Trick::DoubleBackflip => 2.0,
            Trick::FrontFlip => -1.0,
            _ => 0.0,
        }
    }
}

/// The trick of the flight after a bounce at chain count `chain`: one a
/// count up to six, the sixth's for every count over it, and the Front
/// Flip for a Monster Bounce whatever the count. `None` for no bounce.
pub fn trick_for(chain: u32, monster: bool) -> Option<Trick> {
    if chain == 0 {
        return None;
    }
    if monster {
        return Some(Trick::FrontFlip);
    }
    Some(match chain {
        1 => Trick::NoHander,
        2 => Trick::CanCan,
        3 => Trick::BarSpin,
        4 => Trick::Superman,
        5 => Trick::Backflip,
        _ => Trick::DoubleBackflip,
    })
}

/// Seconds a body launched upward at `vz` (units/s) is above its launch
/// height.
pub fn hang_time(vz: f32) -> f32 {
    2.0 * vz.max(0.0) / GRAVITY
}

/// Seconds the trick of a flight launched at `vz` takes; `None` when the
/// flight is too short for one.
pub fn trick_seconds(trick: Trick, vz: f32) -> Option<f32> {
    let hang = hang_time(vz);
    (hang >= MIN_HANG).then(|| (hang * TRICK_SHARE).min(trick.longest()))
}

/// How far through a trick of `seconds` a body moving at `vz` is, 0 to 1:
/// half at the apex (`vz` 0), from the time left to it (`vz` / gravity).
pub fn trick_progress(vz: f32, seconds: f32) -> f32 {
    if seconds <= 0.0 || !vz.is_finite() {
        return 1.0;
    }
    (0.5 - vz / GRAVITY / seconds).clamp(0.0, 1.0)
}

/// What a rider is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Phase {
    /// On the ground, the stick in the hands.
    Carried,
    /// Getting on, in the air.
    Mount,
    /// On the stick, in the air.
    Ride,
    /// Touched down, the piston in, the verdict not in yet.
    Compress,
    /// Bounced: the piston springing out.
    Spring,
    /// Getting off, on the ground.
    Dismount,
}

impl Phase {
    pub fn id(self) -> &'static str {
        match self {
            Phase::Carried => "carried",
            Phase::Mount => "mount",
            Phase::Ride => "ride",
            Phase::Compress => "compress",
            Phase::Spring => "spring",
            Phase::Dismount => "dismount",
        }
    }
}

/// A sound's moment, for the local player.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cue {
    /// The piston goes in: a touchdown.
    Compress,
    /// The piston springs out: a bounce.
    Spring,
    /// The piston eases out: no bounce.
    Settle,
}

/// The bounce the chain filed on a tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounce {
    pub chain: u32,
    pub monster: bool,
}

/// One tick's read of a player who holds a stick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RiderTick {
    /// The stick held.
    pub style: Style,
    /// Biped flags bit 0.
    pub airborne: bool,
    /// Vertical speed, units/s, up positive.
    pub vz: f32,
    /// The bounce filed this tick.
    pub bounce: Option<Bounce>,
}

/// What the pose is drawn from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub style: Style,
    pub phase: Phase,
    /// 0 carried, 1 on the stick.
    pub mounted: f32,
    /// 0 the piston out, 1 all the way in.
    pub compress: f32,
    /// 1 on the tick of a bounce, fading to 0: the flare at the foot.
    pub flare: f32,
    /// 0 legs out, 1 knees tucked: the flight's own pose, which a trick
    /// is drawn over.
    pub tuck: f32,
    /// The trick and how far through it, 0 to 1.
    pub trick: Option<(Trick, f32)>,
}

impl Sample {
    /// Whether the pose differs from the game's own: anything but the
    /// stick carried with the piston out.
    pub fn posed(&self) -> bool {
        self.mounted > 0.0 || self.compress > 0.0
    }
}

/// A step's result.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Step {
    /// `None` when the player holds no stick.
    pub sample: Option<Sample>,
    pub cue: Option<Cue>,
}

/// The trick of a flight.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Flight {
    trick: Trick,
    /// Seconds the trick takes.
    seconds: f32,
}

/// A trick forced by `pogo.test`: shown from its start whatever the
/// rider is doing, timed by ticks.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Forced {
    trick: Trick,
    ticks: u32,
    of: u32,
}

/// One player's rider.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rider {
    phase: Phase,
    /// Ticks in the phase.
    ticks: u32,
    /// Ticks airborne in a row.
    air: u32,
    mounted: f32,
    compress: f32,
    flare: f32,
    tuck: f32,
    /// The tick before: airborne and vertical speed.
    prev: Option<(bool, f32)>,
    flight: Option<Flight>,
    forced: Option<Forced>,
}

impl Default for Rider {
    fn default() -> Self {
        Rider::new()
    }
}

fn toward(value: f32, goal: f32, step: f32) -> f32 {
    if value < goal {
        (value + step).min(goal)
    } else {
        (value - step).max(goal)
    }
}

impl Rider {
    pub const fn new() -> Rider {
        Rider {
            phase: Phase::Carried,
            ticks: 0,
            air: 0,
            mounted: 0.0,
            compress: 0.0,
            flare: 0.0,
            tuck: 0.0,
            prev: None,
            flight: None,
            forced: None,
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Shows `trick` from its start on the next step, whatever the rider
    /// is doing (`pogo.test`): over the trick's own time when a flight
    /// gives none.
    pub fn force(&mut self, trick: Trick) {
        self.forced = Some(Forced {
            trick,
            ticks: 0,
            of: (TEST_SECONDS.min(trick.longest()) * TICK_RATE) as u32,
        });
    }

    fn enter(&mut self, phase: Phase) {
        self.phase = phase;
        self.ticks = 0;
    }

    /// One tick. `t` is `None` when the player holds no stick (another
    /// weapon, dead, not in the game): the rider starts over.
    pub fn step(&mut self, t: Option<&RiderTick>) -> Step {
        let Some(t) = t else {
            *self = Rider::new();
            return Step {
                sample: None,
                cue: None,
            };
        };
        let vz = if t.vz.is_finite() { t.vz } else { 0.0 };
        let landed = self
            .prev
            .is_some_and(|(was_airborne, was_vz)| touchdown(was_airborne, was_vz, t.airborne, vz));
        self.prev = Some((t.airborne, vz));
        self.air = if t.airborne { self.air + 1 } else { 0 };
        self.ticks += 1;
        let mut cue = None;

        if let Some(b) = t.bounce.filter(|b| b.chain > 0) {
            // The verdict: whatever the phase, a bounce is a bounce.
            self.enter(Phase::Spring);
            self.flare = 1.0;
            self.flight = trick_for(b.chain, b.monster).and_then(|trick| {
                trick_seconds(trick, vz).map(|seconds| Flight { trick, seconds })
            });
            cue = Some(Cue::Spring);
        } else {
            match self.phase {
                Phase::Carried => {
                    if self.air >= AIR_TICKS {
                        self.enter(Phase::Mount);
                    }
                }
                Phase::Mount | Phase::Ride => {
                    if landed {
                        self.enter(Phase::Compress);
                        self.flight = None;
                        cue = Some(Cue::Compress);
                    } else if !t.airborne {
                        // Set down with no fall to speak of.
                        self.enter(Phase::Dismount);
                        self.flight = None;
                    } else if self.phase == Phase::Mount && self.ticks >= MOUNT_TICKS {
                        self.enter(Phase::Ride);
                    }
                }
                Phase::Compress => {
                    if self.ticks >= VERDICT_TICKS {
                        if t.airborne {
                            self.enter(Phase::Ride);
                        } else {
                            self.enter(Phase::Dismount);
                        }
                        cue = Some(Cue::Settle);
                    }
                }
                Phase::Spring => {
                    if self.ticks >= SPRING_TICKS {
                        self.enter(Phase::Ride);
                    }
                }
                Phase::Dismount => {
                    if self.air >= AIR_TICKS {
                        self.enter(Phase::Mount);
                    } else if self.mounted <= 0.0 {
                        self.enter(Phase::Carried);
                    }
                }
            }
        }

        // The blends, each toward what its phase asks.
        let (mounted, compress, tuck) = match self.phase {
            Phase::Carried | Phase::Dismount => (0.0, 0.0, 0.0),
            Phase::Mount | Phase::Ride => (1.0, 0.0, 1.0),
            Phase::Compress => (1.0, 1.0, 0.0),
            Phase::Spring => (1.0, 0.0, 0.0),
        };
        let off = matches!(self.phase, Phase::Carried | Phase::Dismount);
        self.mounted = toward(
            self.mounted,
            mounted,
            1.0 / if off { DISMOUNT_TICKS } else { MOUNT_TICKS } as f32,
        );
        self.compress = toward(
            self.compress,
            compress,
            match self.phase {
                // In hard, over two ticks.
                Phase::Compress => 0.5,
                Phase::Spring => 1.0 / SPRING_TICKS as f32,
                _ => 1.0 / DISMOUNT_TICKS as f32,
            },
        );
        self.tuck = toward(self.tuck, tuck, 1.0 / TUCK_TICKS as f32);
        if self.phase != Phase::Spring || self.ticks > 0 {
            self.flare = toward(self.flare, 0.0, 1.0 / FLARE_TICKS as f32);
        }

        let mut trick = match (self.phase, self.flight) {
            (Phase::Spring | Phase::Ride, Some(f)) => {
                let p = trick_progress(vz, f.seconds);
                (p > 0.0 && p < 1.0).then_some((f.trick, p))
            }
            _ => None,
        };
        let mut mounted = self.mounted;
        if let Some(f) = self.forced.as_mut() {
            f.ticks += 1;
            if f.ticks >= f.of {
                self.forced = None;
            } else {
                trick = Some((f.trick, f.ticks as f32 / f.of as f32));
                mounted = 1.0;
            }
        }
        Step {
            sample: Some(Sample {
                style: t.style,
                phase: self.phase,
                mounted,
                compress: self.compress,
                flare: self.flare,
                tuck: self.tuck,
                trick,
            }),
            cue,
        }
    }
}

/// Every player's rider, by slot.
#[derive(Clone, Debug, PartialEq)]
pub struct Riders {
    slots: [Rider; SLOTS],
}

impl Default for Riders {
    fn default() -> Self {
        Riders::new()
    }
}

impl Riders {
    pub const fn new() -> Riders {
        Riders {
            slots: [Rider::new(); SLOTS],
        }
    }

    /// One tick of the rider in `slot`; a slot the game has none of gives
    /// no sample.
    pub fn step(&mut self, slot: u8, t: Option<&RiderTick>) -> Step {
        match self.slots.get_mut(usize::from(slot)) {
            Some(r) => r.step(t),
            None => Step {
                sample: None,
                cue: None,
            },
        }
    }

    /// Starts over every slot not in `seen`: players who left.
    pub fn keep(&mut self, seen: &[u8]) {
        for (i, r) in self.slots.iter_mut().enumerate() {
            if !seen.contains(&(i as u8)) {
                *r = Rider::new();
            }
        }
    }

    pub fn rider(&self, slot: u8) -> Option<&Rider> {
        self.slots.get(usize::from(slot))
    }

    /// [`Rider::force`] on the rider in `slot`.
    pub fn force(&mut self, slot: u8, trick: Trick) -> bool {
        match self.slots.get_mut(usize::from(slot)) {
            Some(r) => {
                r.force(trick);
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
#[path = "pogo_tests.rs"]
mod tests;
