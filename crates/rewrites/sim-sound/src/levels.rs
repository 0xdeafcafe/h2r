//! `0x1251e0`, a sound's level for its scale, from inputs the bridge reads
//! (docs/engine/sound-instances.md, "Levels"). The class and scale bounds
//! are mixed in linear gain on the x87 (`0x2195f0`), taken back to dB
//! (`0x2197f0`), and the class's level (`0x127010`, with its ducking) and
//! the source's level offset are added in SSE. It draws nothing.

use crate::environment::{GainCurve, log_gain};
use crate::instances::FPU_CONTROL;
use x87::abi::{self as fp, X87Abi, arith_op, src_kind};

/// The image constants the five functions read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Constants {
    /// `0x44a0b4`: the dB floor (-64).
    pub floor: u32,
    /// `0x45dbd8`: the dB ceiling (0).
    pub ceiling: u32,
    /// `0x45dc1c`, `0x44a0bc`: dB to linear (0.05, ln 10).
    pub curve: GainCurve,
    /// `0x45dc08`: linear to dB (20).
    pub decibel_scale: u32,
    /// `0x45dbc0`: one.
    pub one: u32,
    /// `0x45dc70`: the shortest duck time that counts (0.001).
    pub shortest: u32,
}

impl Constants {
    pub const RETAIL: Constants = Constants {
        floor: 0xc280_0000,
        ceiling: 0,
        curve: GainCurve { decibel_scale: 0x3d4c_cccd, exponent_scale: 0x4013_5d8e },
        decibel_scale: 0x41a0_0000,
        one: 0x3f80_0000,
        shortest: 0x3a83_126f,
    };
}

/// `0x2195f0` and `0x2197f0` return this bit pattern under the floor,
/// whatever `0x44a0b4` holds.
const FLOOR_BITS: u32 = 0xc280_0000;

/// One row of a class's duck table (class `+0x28`, 16 bytes a row).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Duck {
    /// `+0`: the level the duck adds, in dB.
    pub level: u32,
    /// `+4`: the time the duck takes to come in.
    pub rise: u32,
    /// `+8`: how long it holds.
    pub hold: u32,
    /// `+0xC`: the time it takes to go out.
    pub fall: u32,
}

/// The sound globals' duck state (`[0x4e6380] + 0x1f8`) and the class's
/// rows for its two indices (zero for `-1`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ducking {
    /// `+0x1f8`: the duck coming in, `-1` for none.
    pub current: i32,
    /// `+0x1fc`.
    pub current_time: u32,
    /// `+0x200`: the duck going out, `-1` for none.
    pub previous: i32,
    /// `+0x204`.
    pub previous_time: u32,
    pub current_row: Duck,
    pub previous_row: Duck,
}

/// What `0x1251e0` reads, as `f32` bit patterns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Inputs {
    /// Class `+0x20`, `+0x24`.
    pub class_bounds: [u32; 2],
    /// The gestalt scale row's `+0`, `+4` (tag `+0xB` picks the row).
    pub scale_bounds: [u32; 2],
    /// The class's runtime level (`[0x502118] + 16 * class + 4`).
    pub class_level: u32,
    /// `None` when the class's runtime flag bit 0 is set: no ducking.
    pub ducking: Option<Ducking>,
    /// Source word 2 (request `+0x14`).
    pub offset: u32,
    /// Source word 1 (request `+0x10`): where in the bounds the sound plays.
    pub scale: u32,
}

fn sse(op: fn(&mut u32, u128, u32) -> Result<u128, simd::Xm>, m: &mut u32, a: u32, b: u32) -> u32 {
    op(m, u128::from(a), b).expect("masked sound SSE") as u32
}

fn add(m: &mut u32, a: u32, b: u32) -> u32 {
    sse(simd::addss, m, a, b)
}
fn sub(m: &mut u32, a: u32, b: u32) -> u32 {
    sse(simd::subss, m, a, b)
}
fn mul(m: &mut u32, a: u32, b: u32) -> u32 {
    sse(simd::mulss, m, a, b)
}
fn div(m: &mut u32, a: u32, b: u32) -> u32 {
    sse(simd::divss, m, a, b)
}

/// `comiss a, b` then `ja`: ordered and greater.
fn above(m: &mut u32, a: u32, b: u32) -> bool {
    simd::comiss(m, a, b).expect("masked sound SSE") & 0x41 == 0
}

/// `comiss a, b` then `jb`: less, or unordered.
fn below(m: &mut u32, a: u32, b: u32) -> bool {
    simd::comiss(m, a, b).expect("masked sound SSE") & 1 != 0
}

/// `v * t` as the class level writes it: `(v - 0) * t + 0`.
fn faded(m: &mut u32, v: u32, t: u32) -> u32 {
    let d = sub(m, v, 0);
    let d = mul(m, d, t);
    add(m, d, 0)
}

/// `min(x, one)` as `comiss one, x; jbe`: unordered takes one.
fn up_to_one(m: &mut u32, k: &Constants, x: u32) -> u32 {
    if above(m, k.one, x) { x } else { k.one }
}

/// `0x2195f0`'s clamp: the floor's bits under the floor, 0 over the ceiling.
pub fn bounded(m: &mut u32, k: &Constants, decibels: u32) -> u32 {
    if above(m, k.floor, decibels) {
        FLOOR_BITS
    } else if above(m, decibels, k.ceiling) {
        0
    } else {
        decibels
    }
}

/// `0x2195f0`: push the linear gain of `decibels`, bounded.
pub fn push_linear(fpu: &mut X87Abi, m: &mut u32, k: &Constants, decibels: u32) {
    let decibels = bounded(m, k, decibels);
    k.curve.linear(decibels, fpu);
}

/// `0x2197f0`: a linear gain to dB, bounded to `[floor, 0]`. The log is
/// taken of the gain clamped to `[0, 1]`, but only when the gain itself is
/// over 0; the floor test reads the unrounded log.
pub fn to_decibels(fpu: &mut X87Abi, m: &mut u32, k: &Constants, gain: u32) -> u32 {
    let clamped = if above(m, 0, gain) {
        0
    } else if above(m, gain, k.one) {
        k.one
    } else {
        gain
    };
    let decibels = if above(m, gain, 0) {
        let (out, under) = log_gain(fpu, clamped, k.decibel_scale, k.floor);
        if under {
            k.floor
        } else if above(m, out, 0) {
            0
        } else {
            out
        }
    } else {
        k.floor
    };
    if above(m, k.floor, decibels) {
        FLOOR_BITS
    } else if above(m, decibels, 0) {
        0
    } else {
        decibels
    }
}

/// `0x127010`: the class's level plus its ducks. With one duck held across
/// both slots it fades in over `rise`, holds, then fades out over `fall`;
/// otherwise the outgoing duck fades out and the incoming one fades in.
pub fn class_level(m: &mut u32, k: &Constants, level: u32, ducking: Option<&Ducking>) -> u32 {
    let Some(d) = ducking else { return level };
    if d.previous != -1 && d.current == d.previous {
        let e = d.current_row;
        let elapsed = sub(m, d.current_time, d.previous_time);
        if !above(m, elapsed, e.hold) {
            return add(m, e.level, level);
        }
        if below(m, e.rise, k.shortest) {
            return level;
        }
        if !below(m, e.fall, k.shortest) {
            let end = add(m, e.fall, e.hold);
            if above(m, end, elapsed) {
                let per_rise = div(m, k.one, e.rise);
                let past = sub(m, elapsed, e.hold);
                let past = mul(m, past, per_rise);
                let past = mul(m, past, e.fall);
                let x = sub(m, d.current_time, elapsed);
                let x = add(m, x, past);
                let t = div(m, x, e.rise);
                // The test reads `x * (1 / rise)`, the value `x / rise`.
                let test = mul(m, x, per_rise);
                let t = if above(m, k.one, test) { t } else { k.one };
                let v = faded(m, e.level, t);
                return add(m, level, v);
            }
        }
        let x = sub(m, d.current_time, elapsed);
        let x = div(m, x, e.rise);
        let t = up_to_one(m, k, x);
        let v = faded(m, e.level, t);
        return add(m, v, level);
    }
    let mut level = level;
    if d.previous != -1 {
        let e = d.previous_row;
        if !below(m, e.fall, k.shortest) {
            let lead = div(m, e.hold, e.fall);
            let x = sub(m, d.previous_time, lead);
            let t = if above(m, 0, x) {
                0
            } else if above(m, x, k.one) {
                k.one
            } else {
                x
            };
            let v = sub(m, 0, e.level);
            let v = mul(m, v, t);
            let v = add(m, v, e.level);
            level = add(m, v, level);
        }
    }
    if d.current == -1 {
        return level;
    }
    let e = d.current_row;
    if below(m, e.rise, k.shortest) {
        return level;
    }
    let x = div(m, d.current_time, e.rise);
    let t = up_to_one(m, k, x);
    let v = faded(m, e.level, t);
    add(m, v, level)
}

/// `0x1251e0`: the dB of the bounds' linear mix at `scale`, plus the
/// class's level and the offset. Runs under [`FPU_CONTROL`] with all SSE
/// exceptions masked, as the sound frame does.
pub fn level(k: &Constants, i: &Inputs) -> u32 {
    let mut m = 0x1f80;
    let mut fpu = X87Abi::default();
    fp::ldcw(&mut fpu, FPU_CONTROL);
    // `0x218d70`, `0x218de0`: scale row plus class, each end.
    let lo = add(&mut m, i.scale_bounds[0], i.class_bounds[0]);
    let hi = add(&mut m, i.scale_bounds[1], i.class_bounds[1]);
    push_linear(&mut fpu, &mut m, k, lo);
    let (_, low) = fp::st_f32(&fpu, 0);
    fp::store_commit(&mut fpu, low.status, true);
    push_linear(&mut fpu, &mut m, k, hi);
    fp::arith(&mut fpu, arith_op::SUB, 0, src_kind::F32, low.value.into());
    fp::arith(&mut fpu, arith_op::MUL, 0, src_kind::F32, i.scale.into());
    fp::arith(&mut fpu, arith_op::ADD, 0, src_kind::F32, low.value.into());
    let (_, mixed) = fp::st_f32(&fpu, 0);
    fp::store_commit(&mut fpu, mixed.status, true);
    let decibels = to_decibels(&mut fpu, &mut m, k, mixed.value);
    let class = class_level(&mut m, k, i.class_level, i.ducking.as_ref());
    let class = add(&mut m, class, i.offset);
    add(&mut m, decibels, class)
}

#[cfg(test)]
mod tests {
    use super::*;

    const K: Constants = Constants::RETAIL;

    fn b(x: f32) -> u32 {
        x.to_bits()
    }
    fn f(x: u32) -> f32 {
        f32::from_bits(x)
    }

    fn inputs(lo: f32, hi: f32, scale: f32) -> Inputs {
        Inputs { class_bounds: [b(lo), b(hi)], scale_bounds: [0, 0], scale: b(scale), ..Inputs::default() }
    }

    #[test]
    fn scale_zero_and_one_play_the_bounds() {
        assert!((f(level(&K, &inputs(-12.0, -3.0, 0.0))) + 12.0).abs() < 1e-4);
        assert!((f(level(&K, &inputs(-12.0, -3.0, 1.0))) + 3.0).abs() < 1e-4);
    }

    #[test]
    fn the_mix_is_linear_in_gain_not_in_decibels() {
        let mid = f(level(&K, &inputs(-20.0, 0.0, 0.5)));
        let expected = 20.0 * ((0.1f64 + 1.0) / 2.0).log10();
        assert!((f64::from(mid) - expected).abs() < 1e-4, "{mid} vs {expected}");
    }

    #[test]
    fn bounds_above_zero_play_at_zero_and_under_the_floor_at_the_floor() {
        assert_eq!(level(&K, &inputs(6.0, 9.0, 0.5)), 0);
        assert!((f(level(&K, &inputs(-90.0, -70.0, 0.5))) + 64.0).abs() < 1e-3);
    }

    #[test]
    fn class_level_and_offset_add_after_the_mix() {
        let mut i = inputs(0.0, 0.0, 0.0);
        i.class_level = b(-2.0);
        i.offset = b(-1.5);
        assert_eq!(f(level(&K, &i)), -3.5);
    }

    #[test]
    fn a_class_without_ducking_keeps_its_level() {
        assert_eq!(class_level(&mut 0x1f80, &K, b(-4.0), None), b(-4.0));
    }

    fn duck() -> Duck {
        Duck { level: b(-10.0), rise: b(2.0), hold: b(1.0), fall: b(4.0) }
    }

    fn same(current_time: f32, previous_time: f32) -> Ducking {
        Ducking {
            current: 3,
            current_time: b(current_time),
            previous: 3,
            previous_time: b(previous_time),
            current_row: duck(),
            previous_row: duck(),
        }
    }

    #[test]
    fn a_held_duck_adds_its_full_level() {
        let d = same(1.5, 1.0);
        assert_eq!(f(class_level(&mut 0x1f80, &K, 0, Some(&d))), -10.0);
    }

    #[test]
    fn a_held_duck_past_its_hold_fades_by_its_time() {
        // Elapsed 3 is past the hold (1) and inside hold + fall (5):
        // x = (4 - 3) + (3 - 1) / 2 * 4 = 5, t = min(5 / 2, 1).
        let d = same(4.0, 1.0);
        assert_eq!(f(class_level(&mut 0x1f80, &K, 0, Some(&d))), -10.0);
        // elapsed 6 is past hold + fall: t = (6.5 - 6) / 2.
        let d = same(6.5, 0.5);
        assert_eq!(f(class_level(&mut 0x1f80, &K, 0, Some(&d))), -2.5);
    }

    #[test]
    fn a_duck_too_short_to_rise_adds_nothing() {
        let mut d = same(4.0, 1.0);
        d.current_row.rise = 0;
        assert_eq!(f(class_level(&mut 0x1f80, &K, b(-1.0), Some(&d))), -1.0);
    }

    #[test]
    fn an_outgoing_duck_fades_out_and_an_incoming_one_fades_in() {
        let d = Ducking {
            current: 1,
            current_time: b(1.0),
            previous: 2,
            previous_time: b(0.5),
            current_row: duck(),
            previous_row: Duck { level: b(-8.0), rise: b(1.0), hold: b(0.0), fall: b(1.0) },
        };
        // out: -8 * (1 - 0.5) = -4; in: -10 * (1 / 2) = -5.
        assert_eq!(f(class_level(&mut 0x1f80, &K, 0, Some(&d))), -9.0);
    }

    #[test]
    fn no_duck_in_either_slot_keeps_the_level() {
        let d = Ducking { current: -1, previous: -1, ..Ducking::default() };
        assert_eq!(f(class_level(&mut 0x1f80, &K, b(-6.0), Some(&d))), -6.0);
    }

    #[test]
    fn to_decibels_floors_silence_and_zero_caps_gain_over_one() {
        let mut fpu = X87Abi::default();
        assert_eq!(to_decibels(&mut fpu, &mut 0x1f80, &K, 0), FLOOR_BITS);
        assert_eq!(to_decibels(&mut fpu, &mut 0x1f80, &K, b(-1.0)), FLOOR_BITS);
        assert_eq!(to_decibels(&mut fpu, &mut 0x1f80, &K, b(3.0)), 0);
        assert_eq!(f(to_decibels(&mut fpu, &mut 0x1f80, &K, b(0.1))), -20.0);
    }
}
