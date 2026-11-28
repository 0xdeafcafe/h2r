//! The tag-function curve evaluator `13b390` as an idiomatic kernel: the
//! curve every object function value, light and screen effect flows
//! through (docs/engine/object-update.md, "Phase `c0b00`";
//! docs/engine/engine-map.md, `function_evaluate`). The register-level
//! port `engine::function_eval` is the oracle; this kernel is the same
//! arithmetic over typed words, one operation per instruction in the
//! original's order, under the float rules of `sim_core::det`.
//!
//! `13b390 (reference: edi, input: stack 0, range: stack 1) -> xmm0`,
//! `ret 8`: the reference points at a count and the function blob's
//! address; an empty reference is 0. The blob's first byte picks the
//! curve, its flags byte bit 0 a second curve blended in by the range
//! input, and every path but the constant's unblended one ends clamped to
//! 0..1 (NaN passes). Curves:
//!
//! - 0, identity: the input.
//! - 1, constant: the range input when blended, else 0 (unclamped).
//! - 2, transition: `a + (b - a) * shape(sub, x)`.
//! - 3, periodic: `c + (d - c) * wave(sub, x * f + p)`.
//! - 4, linear: `x * m + b`.
//! - 5, linear key: three ramps, each clamped to 0..1, weighted.
//! - 7, spline: a cubic in `x`.
//! - 6, 8 and above 10: 0.
//!
//! Kinds 9 (exponent) and 10 (spline 2) run CRT `pow` sequences on the
//! x87 that this kernel does not carry; it declines them, and the caller
//! runs the register-level port `engine::function_eval`. The shape and wave tables, and every
//! scale constant, are inputs: the adapter reads them from the image, so
//! the same kernel runs on synthetic memory in tests.

use sim_core::det::{self, Fp};

/// The image words and table bases the evaluator reads, gathered by the
/// caller: the constants as f32 bits and the tables' guest addresses
/// (1024 bytes per sub-type).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Image {
    /// 1.0.
    pub one: u32,
    /// The shape lookup's scale (1023) and bias (0.1).
    pub shape_scale: u32,
    pub shape_bias: u32,
    /// The wave lookup's scale.
    pub wave_scale: u32,
    /// A table byte as a fraction (1/255).
    pub byte: u32,
    /// The wave wrap window.
    pub wrap_high: u32,
    pub wrap_low: u32,
    /// The shape and wave tables' base addresses.
    pub shapes: u32,
    pub waves: u32,
}

/// Why the kernel hands a call to the register-level port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decline {
    /// MXCSR not in the pinned state, the x87 not rounding to nearest, an
    /// operand off the fast path, or a conversion out of range.
    Float,
    /// Curve kinds 9 and 10, whose x87 `pow` sequences the kernel does
    /// not carry.
    Kind,
}

impl Decline {
    pub const ALL: [Decline; 2] = [Decline::Float, Decline::Kind];

    pub fn name(self) -> &'static str {
        match self {
            Decline::Float => "float",
            Decline::Kind => "kind",
        }
    }
}

/// One call's answer: the result as f32 bits, or why there is none. A
/// declined call is the register-level port's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Value(u32),
    Declined(Decline),
}

/// The params of the blob at `data` begin at `+0x14`.
const PARAMS: u32 = 0x14;

/// `13b390` at `input` and range input `range` on the reference at
/// `reference`: the f32 result bits. `mxcsr` and the x87 `control` word
/// are the caller's, and the kernel declines unless MXCSR is in the
/// pinned state and the x87 rounds to nearest (the only bit of its state
/// the stores read). `word` and `byte` read guest memory; a read that
/// fails is the caller's error.
pub fn evaluate<E>(
    image: &Image,
    mxcsr: u32,
    control: u16,
    reference: u32,
    input: u32,
    range: u32,
    mut word: impl FnMut(u32) -> Result<u32, E>,
    mut byte: impl FnMut(u32) -> Result<u8, E>,
) -> Result<Answer, E> {
    if !det::fast_env(mxcsr) || control & 0xc00 != 0 || !det::host_fast_env() {
        return Ok(Answer::Declined(Decline::Float));
    }
    let data = word(reference.wrapping_add(4))?;
    if data == 0 || word(reference)? as i32 <= 0 {
        return Ok(Answer::Value(0));
    }
    let kind = byte(data)?;
    let blended = byte(data.wrapping_add(1))? & 1 != 0;
    let p = data.wrapping_add(PARAMS);
    let mut fp = Fp::new();
    fp.check(input);
    fp.check(range);
    fp.check(image.one);
    let one = image.one;
    // A step returns `None` when the exact path leaves it (an operand off
    // the fast path, a conversion out of range); the call is declined.
    let v = match kind {
        0 => input,
        1 => {
            if !blended {
                return Ok(Answer::Value(0));
            }
            range
        }
        2 => {
            let sub = byte(data.wrapping_add(2))?;
            let s = match shape(&mut fp, image, sub, input, &mut byte)? {
                Some(s) => s,
                None => return Ok(Answer::Declined(Decline::Float)),
            };
            let v = lerp(&mut fp, word(p.wrapping_add(0))?, word(p.wrapping_add(4))?, s);
            if !blended {
                v
            } else {
                let sub = byte(data.wrapping_add(3))?;
                let s = match shape(&mut fp, image, sub, input, &mut byte)? {
                    Some(s) => s,
                    None => return Ok(Answer::Declined(Decline::Float)),
                };
                let w = lerp(&mut fp, word(p.wrapping_add(8))?, word(p.wrapping_add(0xc))?, s);
                mix(&mut fp, one, range, v, w)
            }
        }
        3 => {
            let i = fp.mul(word(p.wrapping_add(0))?, input);
            let i = fp.add(i, word(p.wrapping_add(4))?);
            let sub = byte(data.wrapping_add(2))?;
            let s = match wave(&mut fp, image, sub, i, &mut byte)? {
                Some(s) => s,
                None => return Ok(Answer::Declined(Decline::Float)),
            };
            let v = lerp(&mut fp, word(p.wrapping_add(8))?, word(p.wrapping_add(0xc))?, s);
            if !blended {
                v
            } else {
                let i = fp.mul(word(p.wrapping_add(0x10))?, input);
                let i = fp.add(i, word(p.wrapping_add(0x14))?);
                let sub = byte(data.wrapping_add(3))?;
                let s = match wave(&mut fp, image, sub, i, &mut byte)? {
                    Some(s) => s,
                    None => return Ok(Answer::Declined(Decline::Float)),
                };
                let w = lerp(&mut fp, word(p.wrapping_add(0x18))?, word(p.wrapping_add(0x1c))?, s);
                mix(&mut fp, one, range, v, w)
            }
        }
        4 => {
            let v = fp.mul(word(p.wrapping_add(0x10))?, input);
            let v = fp.add(v, word(p.wrapping_add(0x14))?);
            if !blended {
                v
            } else {
                let w = fp.mul(word(p.wrapping_add(0x28))?, input);
                let w = fp.add(w, word(p.wrapping_add(0x2c))?);
                mix(&mut fp, one, range, v, w)
            }
        }
        5 => {
            let v = match keys(&mut fp, image, p, input, &mut word)? {
                Some(v) => v,
                None => return Ok(Answer::Declined(Decline::Float)),
            };
            if !blended {
                v
            } else {
                let w = match keys(&mut fp, image, p.wrapping_add(0x50), input, &mut word)? {
                    Some(w) => w,
                    None => return Ok(Answer::Declined(Decline::Float)),
                };
                mix(&mut fp, one, range, v, w)
            }
        }
        7 => {
            let v = cubic(&mut fp, data.wrapping_add(0x34), input, &mut word)?;
            if !blended {
                v
            } else {
                let w = cubic(&mut fp, data.wrapping_add(0x64), input, &mut word)?;
                mix(&mut fp, one, range, v, w)
            }
        }
        9 | 10 => return Ok(Answer::Declined(Decline::Kind)),
        _ => return Ok(Answer::Value(0)),
    };
    if !fp.exact() {
        return Ok(Answer::Declined(Decline::Float));
    }
    Ok(Answer::Value(clamp01(&mut fp, v, one)))
}

/// `second * r + (1 - r) * first`.
fn mix(fp: &mut Fp, one: u32, range: u32, first: u32, second: u32) -> u32 {
    let rest = fp.sub(one, range);
    let rest = fp.mul(rest, first);
    let scaled = fp.mul(second, range);
    fp.add(scaled, rest)
}

/// `(b - a) * t + a`.
fn lerp(fp: &mut Fp, a: u32, b: u32, t: u32) -> u32 {
    let d = fp.sub(b, a);
    let d = fp.mul(d, t);
    fp.add(d, a)
}

/// `comiss lo, v; ja` then `comiss v, hi; ja`: `lo` if `lo > v`, `hi` if
/// `v > hi`, else `v` (NaN passes, as the unordered compares do not take
/// either branch). The operands are recorded, so a NaN or infinite `v`
/// declines the call before its answer is used.
fn clamp01(fp: &mut Fp, v: u32, hi: u32) -> u32 {
    fp.check(v);
    fp.check(hi);
    if det::compare(0, v) == std::cmp::Ordering::Greater {
        0
    } else if det::compare(v, hi) == std::cmp::Ordering::Greater {
        hi
    } else {
        v
    }
}

/// `comiss a, b; ja`: ordered greater, on recorded operands.
fn above(fp: &mut Fp, a: u32, b: u32) -> bool {
    fp.check(a);
    fp.check(b);
    det::compare(a, b) == std::cmp::Ordering::Greater
}

/// `fmod(v, 1.0)` on the f32 `v`, stored as f32: a finite non-zero `v`
/// its exact remainder, zero +0, a NaN quieted. An infinity takes the
/// CRT's indefinite path, which reads the image's extended constants:
/// declined.
fn fmod_one(v: u32) -> Option<u32> {
    let f = f32::from_bits(v);
    if f.is_nan() {
        Some(v | 0x0040_0000)
    } else if f.is_infinite() {
        None
    } else if f == 0.0 {
        Some(0)
    } else {
        Some((f % 1.0).to_bits())
    }
}

/// `fld` the f32 and `fistp m32` under round to nearest: the store of an
/// in-range value. Out of range or NaN the x87 leaves its integer
/// indefinite: declined.
fn fistp(v: u32) -> Option<i32> {
    let r = f32::from_bits(v).round_ties_even();
    (-2147483648.0..2147483648.0).contains(&r).then_some(r as i32)
}

/// `17ca10 (sub, x)`: `x` clamped to 0..1; sub-type 0 that, else the
/// shape table's bytes at `x * 1023` interpolated, clamped. `None`: off
/// the exact path.
fn shape<E>(
    fp: &mut Fp,
    image: &Image,
    sub: u8,
    x: u32,
    byte: &mut impl FnMut(u32) -> Result<u8, E>,
) -> Result<Option<u32>, E> {
    let one = image.one;
    let x = clamp01(fp, x, one);
    if sub == 0 {
        return Ok(Some(x));
    }
    let scaled = fp.mul(x, image.shape_scale);
    let Some(frac) = fmod_one(scaled) else {
        return Ok(None);
    };
    let at = fp.sub(scaled, image.shape_bias);
    let Some(i) = fistp(at) else {
        return Ok(None);
    };
    let table = image.shapes.wrapping_add(u32::from(sub) << 10);
    let k = image.byte;
    let v = match i as u16 {
        0x3ff => {
            let b = det::from_int(i32::from(byte(table.wrapping_add(0x3ff))?));
            fp.mul(b, k)
        }
        short => {
            let entry = table.wrapping_add(short as i16 as u32);
            let a = det::from_int(i32::from(byte(entry)?));
            let b = det::from_int(i32::from(byte(entry.wrapping_add(1))?));
            let a = fp.mul(a, k);
            let rest = fp.sub(one, frac);
            let a = fp.mul(a, rest);
            let b = fp.mul(b, k);
            let b = fp.mul(b, frac);
            fp.add(a, b)
        }
    };
    Ok(Some(clamp01(fp, v, one)))
}

/// `17c900 (sub, x)`: sub-type 0 is 1; else the wave table's bytes at
/// `x * WAVE_SCALE` (wrapping at 1024) interpolated; sub-types 6 and 7
/// wrap the value through 1.
fn wave<E>(
    fp: &mut Fp,
    image: &Image,
    sub: u8,
    x: u32,
    byte: &mut impl FnMut(u32) -> Result<u8, E>,
) -> Result<Option<u32>, E> {
    let one = image.one;
    if sub == 0 {
        return Ok(Some(one));
    }
    let scaled = fp.mul(x, image.wave_scale);
    let Some(frac) = fmod_one(scaled) else {
        return Ok(None);
    };
    let whole = fp.sub(scaled, frac);
    let Some(i) = fistp(whole) else {
        return Ok(None);
    };
    let i = (i as u32) & 0x3ff;
    let table = image.waves.wrapping_add(u32::from(sub) << 10);
    let a = det::from_int(i32::from(byte(table.wrapping_add(i))?));
    let b = det::from_int(i32::from(byte(table.wrapping_add((i + 1) & 0x3ff))?));
    let k = image.byte;
    let a = fp.mul(a, k);
    let mut b = fp.mul(b, k);
    let wraps = (1u32 << (sub & 31)) as u8 & 0xc0 != 0;
    if wraps && above(fp, a, image.wrap_high) && above(fp, image.wrap_low, b) {
        b = fp.add(b, one);
    }
    let rest = fp.sub(one, frac);
    let v = fp.mul(rest, a);
    let b = fp.mul(b, frac);
    let v = fp.add(v, b);
    Ok(Some(if wraps && above(fp, v, one) {
        fp.sub(v, one)
    } else {
        v
    }))
}

/// Linear key's three clamped ramps at `p` (`+0x24..` the starts,
/// `+0x34..` the slopes, `+0x40` the constant, `+0x44..` the weights).
fn keys<E>(
    fp: &mut Fp,
    image: &Image,
    p: u32,
    x: u32,
    word: &mut impl FnMut(u32) -> Result<u32, E>,
) -> Result<Option<u32>, E> {
    let one = image.one;
    let ramp = |fp: &mut Fp, i: u32, word: &mut dyn FnMut(u32) -> Result<u32, E>| {
        let v = fp.sub(x, word(p.wrapping_add(0x24 + 4 * i))?);
        let v = fp.mul(v, word(p.wrapping_add(0x34 + 4 * i))?);
        Ok::<u32, E>(clamp01(fp, v, one))
    };
    let w0 = ramp(fp, 0, word)?;
    let w1 = ramp(fp, 1, word)?;
    let w2 = ramp(fp, 2, word)?;
    let v = fp.mul(word(p.wrapping_add(0x4c))?, w2);
    let b = fp.mul(word(p.wrapping_add(0x48))?, w1);
    let v = fp.add(v, b);
    let a = fp.mul(word(p.wrapping_add(0x44))?, w0);
    let v = fp.add(v, a);
    Ok(Some(fp.add(v, word(p.wrapping_add(0x40))?)))
}

/// The cubic with coefficients at `k` (t^3, t^2, t, 1) at `t`.
fn cubic<E>(
    fp: &mut Fp,
    k: u32,
    t: u32,
    word: &mut impl FnMut(u32) -> Result<u32, E>,
) -> Result<u32, E> {
    let t2 = fp.mul(t, t);
    let t3 = fp.mul(t2, t);
    let a = fp.mul(word(k.wrapping_add(4))?, t2);
    let b = fp.mul(word(k.wrapping_add(8))?, t);
    let v = fp.add(a, b);
    let d = fp.mul(word(k.wrapping_add(0))?, t3);
    let v = fp.add(v, d);
    Ok(fp.add(v, word(k.wrapping_add(0xc))?))
}

#[cfg(test)]
mod tests {
    use super::*;

    const IMAGE: Image = Image {
        one: 1.0f32.to_bits(),
        shape_scale: 1023.0f32.to_bits(),
        shape_bias: 0.1f32.to_bits(),
        wave_scale: (1024.0f32 / 28.0).to_bits(),
        byte: (1.0f32 / 255.0).to_bits(),
        wrap_high: 0.75f32.to_bits(),
        wrap_low: 0.25f32.to_bits(),
        shapes: 0x1000,
        waves: 0x2000,
    };

    /// The reference and the blob's first bytes.
    const REFERENCE: u32 = 0x40;
    const DATA: u32 = 0x3000;

    /// Word-addressed synthetic memory: the two tables' sub-type 1 ramps
    /// and whatever the test adds. A byte read takes its lane of the word
    /// that holds it, as the blob header's packed bytes need.
    #[derive(Clone, Default)]
    struct Ram(Vec<(u32, u32)>);

    impl Ram {
        fn word(&self, at: u32) -> Result<u32, u32> {
            self.0.iter().find(|&&(a, _)| a == at).map(|&(_, v)| v).ok_or(at)
        }

        fn byte(&self, at: u32) -> Result<u8, u32> {
            self.word(at & !3).map(|v| (v >> (8 * (at & 3))) as u8)
        }

        fn set(&mut self, at: u32, v: u32) {
            self.0.retain(|&(a, _)| a != at);
            self.0.push((at, v));
        }

        /// A blob of `kind` with `flags`, its params `words`.
        fn blob(&mut self, kind: u8, flags: u8, sub: [u8; 2], words: &[f32]) {
            self.set(REFERENCE, 1);
            self.set(REFERENCE + 4, DATA);
            self.set(DATA, u32::from(kind) | u32::from(flags) << 8 | u32::from(sub[0]) << 16 | u32::from(sub[1]) << 24);
            for (i, &w) in words.iter().enumerate() {
                self.set(DATA + PARAMS + 4 * i as u32, w.to_bits());
            }
        }

        fn tables(&mut self) {
            for i in 0..1024u32 {
                self.set(IMAGE.shapes + 1024 + i, u32::from((i / 4) as u8));
                self.set(IMAGE.waves + 1024 + i, u32::from((i / 4) as u8));
            }
        }

        fn eval(&self, x: f32, r: f32) -> Answer {
            evaluate(
                &IMAGE,
                0x1fa0,
                0x027f,
                REFERENCE,
                x.to_bits(),
                r.to_bits(),
                |a| self.word(a),
                |a| self.byte(a),
            )
            .unwrap()
        }
    }

    #[test]
    fn an_empty_reference_is_zero_and_kinds_six_eight_and_above_ten_are_zero() {
        let mut ram = Ram::default();
        ram.tables();
        ram.set(REFERENCE, 0);
        ram.set(REFERENCE + 4, DATA);
        ram.set(DATA, 6);
        assert_eq!(ram.eval(0.5, 0.25), Answer::Value(0), "empty reference");
        ram.set(REFERENCE, 1);
        for kind in [6u8, 8, 11] {
            ram.set(DATA, u32::from(kind));
            assert_eq!(ram.eval(0.5, 0.25), Answer::Value(0), "kind {kind}");
        }
    }

    #[test]
    fn the_identity_curve_clamps_and_the_constant_blends() {
        let mut ram = Ram::default();
        ram.tables();
        ram.blob(0, 0, [0, 0], &[]);
        assert_eq!(ram.eval(0.5, 0.9), Answer::Value(0.5f32.to_bits()));
        assert_eq!(ram.eval(1.5, 0.9), Answer::Value(1.0f32.to_bits()));
        assert_eq!(ram.eval(-0.5, 0.9), Answer::Value(0.0f32.to_bits()));
        // The constant's unblended zero returns early; blended, the range
        // input is the value, and the closing clamp still applies.
        ram.blob(1, 0, [0, 0], &[]);
        assert_eq!(ram.eval(0.5, 0.9), Answer::Value(0));
        ram.blob(1, 1, [0, 0], &[]);
        assert_eq!(ram.eval(0.5, 0.9), Answer::Value(0.9f32.to_bits()));
        assert_eq!(ram.eval(0.5, -2.0), Answer::Value(0.0f32.to_bits()));
    }

    #[test]
    fn kinds_nine_and_ten_decline_named() {
        let mut ram = Ram::default();
        ram.tables();
        ram.blob(9, 0, [0, 0], &[]);
        assert_eq!(ram.eval(0.5, 0.5), Answer::Declined(Decline::Kind));
        ram.blob(10, 0, [0, 0], &[]);
        assert_eq!(ram.eval(0.5, 0.5), Answer::Declined(Decline::Kind));
        assert_eq!(Decline::ALL.map(Decline::name), ["float", "kind"]);
    }

    #[test]
    fn an_unpinned_environment_declines() {
        let mut ram = Ram::default();
        ram.tables();
        ram.blob(0, 0, [0, 0], &[]);
        for (mxcsr, control) in [(0x1f80u32, 0x027fu16), (0x1fa0, 0x0b7f)] {
            assert_eq!(
                evaluate(&IMAGE, mxcsr, control, REFERENCE, 0.5f32.to_bits(), 0.5f32.to_bits(), |a| {
                    ram.word(a)
                }, |a| ram.byte(a))
                .unwrap(),
                Answer::Declined(Decline::Float),
                "mxcsr {mxcsr:#x} control {control:#x}"
            );
        }
    }
}
