//! The bound controller's device bits (`185be0`, `0x185c6c`..`0x185ff4`
//! and `0x18644f`..`0x186593`; docs/engine/sim-rewrite.md, "Seats and
//! rules (phase 7)"): what a seat with a controller reads from its port
//! before the look curve or, input blocked, before the visibility walk.
//!
//! [`read`] is the kernel, over a [`Device`] the arm ([`arm`]) copies out
//! of guest memory:
//!
//! - the port's look rates, scaled, and its 20-word state block;
//! - in a vehicle seat, the rates bent by the vehicle's speed against the
//!   seat's range: `t = clamp((|v| - lo) / (hi - lo), 0, 1)`, raised to
//!   the seat's exponent, and each rate pair `(a, b)` that has a positive
//!   end becomes `(a + (b - a) * t) * scale`;
//! - unblocked, the stick pair normalised so its larger axis is one
//!   (`s = hypot` of the pair over that axis) and each axis scaled by `s`
//!   and clamped to `low..=one`;
//! - blocked, the wanted buttons this frame did not press are dropped
//!   ([`select`]) and the rest's state is recopied.
//!
//! SSE arithmetic goes through [`Fp`] (exact or inexact: the arm declines
//! an inexact read). The x87 pieces (the speed, `pow`, the magnitudes and
//! the hypot) go through [`Fpu`], which the arm runs on the x87 model so
//! their stack, tag and status are retail's.

use sim_core::det::{self, Fp};
use std::cmp::Ordering;

pub mod arm;

/// The image's constants the read uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Constants {
    /// `0x45dbc0`.
    pub one: u32,
    /// `0x45dbd8`.
    pub zero: u32,
    /// `0x45dc54`: the rate scale.
    pub scale: u32,
    /// `0x45dc68`: the stick dead zone.
    pub dead: u32,
    /// `0x45dbcc`: the clamp's floor.
    pub low: u32,
}

/// A vehicle seat's look scaling: the parent's velocity and the seat
/// definition's fields (`+0x44`..`+0x5c`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Seat {
    /// The parent object's velocity (`+0x88`).
    pub velocity: [u32; 3],
    /// The speed range's ends (`+0x54`, `+0x58`).
    pub lo: u32,
    pub hi: u32,
    /// `+0x5c`: zero leaves the ratio as it is.
    pub exponent: u32,
    /// The yaw rate's ends (`+0x44`, `+0x48`).
    pub yaw: [u32; 2],
    /// The pitch rate's ends (`+0x4c`, `+0x50`).
    pub pitch: [u32; 2],
}

/// One bound seat's inputs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Device {
    /// The port's yaw and pitch rates (`0x51ea18 + port * 0x1c`).
    pub rates: [u32; 2],
    /// The port's state block (`0x51ea88 + port * 0x50`): 16 button
    /// bytes, 16 hold words, then eight words (the view pair at 14 and
    /// 15, the stick pair at 16 and 17).
    pub block: [u32; 20],
    /// The unit's vehicle seat, when it sits in one.
    pub seat: Option<Seat>,
    /// Input is blocked: the console's flag or the tick's pause pair.
    pub blocked: bool,
    /// The seat's wanted buttons, both halves (`+0xc`, `+0xe`).
    pub wanted: [u16; 2],
    pub k: Constants,
}

impl Device {
    /// Button `i`'s state byte in the block.
    pub fn pressed(&self, i: usize) -> u8 {
        self.block[i / 4].to_le_bytes()[i % 4]
    }

    /// Button `i`'s hold word in the block.
    pub fn held(&self, i: usize) -> u16 {
        let w = self.block[4 + i / 2];
        if i % 2 == 0 {
            w as u16
        } else {
            (w >> 16) as u16
        }
    }
}

/// The x87 sequences the read runs, each as retail's instructions leave
/// the stack.
pub trait Fpu {
    /// `call 24590` on `velocity` (its length, left on the stack), `fsub
    /// lo; fdiv span; fst m32`, then `fld zero; fcomip st(1); fstp st(0)`:
    /// the ratio as stored, and whether it is below zero. The ratio stays
    /// on the stack, as retail leaves it.
    fn speed(&mut self, velocity: [u32; 3], lo: u32, span: u32, zero: u32) -> Option<(u32, bool)>;
    /// `fld t; fld e; call pow; fstp m32`.
    fn pow(&mut self, t: u32, e: u32) -> Option<u32>;
    /// `fld v; fabs; fstp m32`.
    fn abs(&mut self, v: u32) -> Option<u32>;
    /// `fld dead; fld a; fcomip st(1); fstp st(0)`: whether `a` is above
    /// `dead` (`ja`).
    fn above(&mut self, a: u32, dead: u32) -> Option<bool>;
    /// `fld q; fmul q; fld p; fmul p; faddp; fsqrt; fstp m32`.
    fn hypot(&mut self, q: u32, p: u32) -> Option<u32>;
}

/// The last x87 instruction the vehicle step ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Last {
    /// None ran.
    None,
    /// The ratio's compare (`0x185dce`).
    Compare,
    /// The exponent's store (`0x185e07`).
    Pow,
}

/// The vehicle step's slots (`+0x14` the range's start, `+0x18` its
/// span, `+0x10` the scaled ratio, also left in `xmm1`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vehicle {
    pub lo: u32,
    pub span: u32,
    pub t: u32,
    pub last: Last,
}

/// The unblocked stick pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Steer {
    /// `+0x10` and `+0x14` as the curve call finds them: the clamped
    /// horizontal and vertical axes.
    pub axes: [u32; 2],
    /// `+0x18`, when the hypot ran (both magnitudes past the dead zone).
    pub hypot: Option<u32>,
    /// The unclamped products, left in `xmm1` (horizontal) and `xmm0`.
    pub scaled: [u32; 2],
}

/// The blocked path's buttons ([`select`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    /// Both halves of the seat's wanted mask, after the drop.
    pub wanted: [u16; 2],
    /// The mask the drop walked (both halves anded), when not zero.
    pub walked: Option<u16>,
    /// The frame's button bytes (`+0x20`) and hold words (`+0x4c`): a
    /// wanted button's are zero, the others' the block's.
    pub states: [u8; 16],
    pub holds: [u16; 16],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Path {
    Steer(Steer),
    Blocked(Selection),
}

/// What the device read leaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// `+0x30` and `+0x34`: the yaw and pitch rates.
    pub rates: [u32; 2],
    pub vehicle: Option<Vehicle>,
    pub path: Path,
}

fn above(fp: &mut Fp, a: u32, b: u32) -> bool {
    fp.check_compared(a);
    fp.check_compared(b);
    det::compare(a, b) == Ordering::Greater
}

/// The device read, or `None` when an operand left the exact range or
/// the x87 model refused a step.
pub fn read(d: &Device, fpu: &mut impl Fpu) -> Option<Outcome> {
    read_with(&mut Fp::new(), d, fpu)
}

/// [`read`] under the guest's `mxcsr` ([`Fp::tracked`]; it must pass
/// [`det::guest_env`]): the outcome and the MXCSR after it, PE raised
/// when an operation was inexact.
pub fn read_tracked(d: &Device, mxcsr: u32, fpu: &mut impl Fpu) -> Option<(Outcome, u32)> {
    let mut fp = Fp::tracked(mxcsr);
    let outcome = read_with(&mut fp, d, fpu)?;
    Some((outcome, fp.mxcsr().unwrap_or(mxcsr)))
}

fn read_with(fp: &mut Fp, d: &Device, fpu: &mut impl Fpu) -> Option<Outcome> {
    let k = d.k;
    let mut rates = [fp.mul(d.rates[0], k.scale), fp.mul(d.rates[1], k.scale)];
    let vehicle = match &d.seat {
        None => None,
        Some(s) => Some(vehicle(fp, fpu, s, k, &mut rates)?),
    };
    let path = if d.blocked {
        Path::Blocked(select(d))
    } else {
        Path::Steer(steer(fp, fpu, d)?)
    };
    fp.exact().then_some(Outcome {
        rates,
        vehicle,
        path,
    })
}

fn vehicle(
    fp: &mut Fp,
    fpu: &mut impl Fpu,
    s: &Seat,
    k: Constants,
    rates: &mut [u32; 2],
) -> Option<Vehicle> {
    let span = fp.sub(s.hi, s.lo);
    let mut last = Last::None;
    let mut t = 0;
    if above(fp, span, 0) {
        let (ratio, below) = fpu.speed(s.velocity, s.lo, span, k.zero)?;
        last = Last::Compare;
        if !below {
            t = if above(fp, ratio, k.one) {
                k.one
            } else {
                ratio
            };
        }
    }
    // `ucomiss` against zero, then `lahf; test ah, 0x44; jnp`: only an
    // equal exponent skips `pow`.
    fp.check_compared(s.exponent);
    if s.exponent & 0x7fff_ffff != 0 {
        t = fpu.pow(t, s.exponent)?;
        last = Last::Pow;
    }
    for (i, [a, b]) in [s.yaw, s.pitch].into_iter().enumerate() {
        if above(fp, a, 0) || above(fp, b, 0) {
            let delta = fp.sub(b, a);
            let bent = fp.mul(delta, t);
            let rate = fp.add(bent, a);
            rates[i] = fp.mul(rate, k.scale);
        }
    }
    Some(Vehicle {
        lo: s.lo,
        span,
        t,
        last,
    })
}

fn steer(fp: &mut Fp, fpu: &mut impl Fpu, d: &Device) -> Option<Steer> {
    let k = d.k;
    let (vertical, horizontal) = (d.block[16], d.block[17]);
    let a = fpu.abs(horizontal)?;
    let b = fpu.abs(vertical)?;
    let mut hypot = None;
    let s = if fpu.above(a, k.dead)? && above(fp, b, k.dead) {
        let (p, q) = if above(fp, a, b) {
            (k.one, fp.div(b, a))
        } else {
            (fp.div(a, b), k.one)
        };
        let s = fpu.hypot(q, p)?;
        hypot = Some(s);
        s
    } else {
        k.one
    };
    let clamp = |fp: &mut Fp, v: u32| {
        if above(fp, k.low, v) {
            k.low
        } else if above(fp, v, k.one) {
            k.one
        } else {
            v
        }
    };
    let y = fp.mul(s, vertical);
    let ny = clamp(fp, y);
    let x = fp.mul(s, horizontal);
    let nx = clamp(fp, x);
    Some(Steer {
        axes: [nx, ny],
        hypot,
        scaled: [x, y],
    })
}

/// The blocked path's button walk (`0x186465`..`0x186593`): each wanted
/// button (both halves set) this frame did not press leaves both halves;
/// every button no longer wanted gets the block's state and hold.
pub fn select(d: &Device) -> Selection {
    let mask = d.wanted[0] & d.wanted[1];
    let mut wanted = d.wanted;
    for i in 0..16 {
        let bit = 1u16 << i;
        if mask & bit != 0 && d.pressed(i) == 0 {
            wanted[0] &= !bit;
            wanted[1] &= !bit;
        }
    }
    let mut states = [0; 16];
    let mut holds = [0; 16];
    for i in 0..16 {
        if wanted[0] & (1 << i) == 0 {
            states[i] = d.pressed(i);
            holds[i] = d.held(i);
        }
    }
    Selection {
        wanted,
        walked: (mask != 0).then_some(mask),
        states,
        holds,
    }
}

#[cfg(test)]
mod tests;
