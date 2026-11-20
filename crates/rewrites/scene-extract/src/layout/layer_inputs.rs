//! The material inputs a layer draw's items read alike
//! ([`frame_scene::LayerInputs`]), read when the draw begins
//! (docs/graphics/framescene.md, slice 4).
//!
//! Research points (Code, `h2 disasm`; the evaluator of record is the
//! port `engine::render_loop::world_registers`):
//!
//! - `0x000132d0` copies the view's `0x298` bytes to the render context
//!   `0x00485600` at each view's begin; the material block `0x00485768`
//!   and the fade block `0x00485858` lie in it. `0x00013450` copies
//!   another `0x120` bytes to `0x0048574c` when the view asks. Nothing in
//!   a layer's item draw writes either (S: no direct store to them was
//!   found; the per-item writes are the lighting pointer `0x00485a58` and
//!   the shader's own `0x00485a64..0x00485a70`, `0x0001bbf0`).
//! - The scalar selector `0x000336f0` (table `0x000338d4`) and the vector
//!   selector `0x00033980` (table `0x00033f8c`) compute each input from
//!   those blocks, the view vectors `0x004e6950`, the fog's words and the
//!   light block; the integer selector `0x00034060` (table `0x000340e0`)
//!   from `0x00485a2c` and the light definition `[0x004858c4]`. The
//!   light's words (`0x00485a30`, `0x004859dc`, `0x004859ec`,
//!   `0x00485a2c`) are written by `0x0004a1e0` under the light passes;
//!   at a layer draw they hold what the last light left, which is what
//!   the guest's draw reads.
//! - The instance property evaluator `0x0001b230` divides the render time
//!   `0x004858a0` (a double, which `0x00012a50` stores) by a game-time
//!   property's period.
//!
//! [`read`] evaluates every input that does not read the item, in the
//! selectors' order of operations, in `f32` (round to nearest; the
//! guest's MXCSR flushes nothing in these paths, S).

use frame_scene::{Defaults, LayerInputs};

use crate::GuestRead;
use crate::layout::object_lighting::DefaultsRead;

/// The render time, seconds (a double).
pub const RENDER_TIME: u32 = 0x0048_58a0;
/// The render context's material block (`0x000336f0`'s `0x00485768`).
pub const MATERIAL: u32 = 0x0048_5768;
/// The render context's fade block.
pub const FADE: u32 = 0x0048_5858;
/// Vector input 0.
pub const VECTOR_0: u32 = 0x0048_5624;
/// Vector inputs 5 and 6: the light block's.
pub const VECTOR_5: u32 = 0x0048_59dc;
pub const VECTOR_6: u32 = 0x0048_59ec;
/// Scalar inputs 16 and 17: the light's weight.
pub const LIGHT_WEIGHT: u32 = 0x0048_5a30;
/// Integer inputs 0 and 1.
pub const LIGHT_INTEGER: u32 = 0x0048_5a2c;
/// The light definition's pointer (integers 2, 3 and 7).
pub const LIGHT_DEFINITION: u32 = 0x0048_58c4;
/// The view vectors: inputs 33 and 34 at `+0`, 36..39 at `+0x40`.
pub const VIEW_VECTORS: u32 = 0x004e_6950;
/// The fog's plane word (`-1`: no fog), near and far, and its vector.
pub const FOG_PLANE: u32 = 0x004b_9f5c;
pub const FOG_NEAR: u32 = 0x004b_9f80;
pub const FOG_FAR: u32 = 0x004b_9f84;
pub const FOG_VECTOR: u32 = 0x004b_9dac;
/// Scalar 40: the view index and the four views' values.
pub const VIEW_SELECT: u32 = 0x004b_9ed8;
pub const VIEW_VALUES: u32 = 0x004e_69c0;
/// Scalars 41 and 42.
pub const SCALARS_41: u32 = 0x004c_19b0;
/// The constant 1.0 the selectors read.
pub const ONE: u32 = 0x0045_dbc0;

/// The scalar inputs [`read`] gives (bit `n`: input `n`): all the
/// selector assigns but 21 (the shader's) and 23 and 37 (the item's
/// lighting).
pub const VIEW_SCALARS: u64 = mask(&[0, 2, 3, 4, 10, 12, 13, 16, 17, 18, 19, 20, 22, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 38, 39, 40, 41, 42]);
/// The vector inputs [`read`] gives: all the selector assigns but the
/// lighting's (3, 4, 14..17, 21..24, 35, 40, 42..45).
pub const VIEW_VECTORS_GIVEN: u64 = mask(&[0, 5, 6, 18, 19, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 36, 37, 38, 39, 41]);
/// The integer inputs [`read`] gives: 0, 1, 2, 3 and 7 (4 and 5 are the
/// shader's, 6 reads the shader cache).
pub const VIEW_INTS: u8 = 1 | 1 << 1 | 1 << 2 | 1 << 3 | 1 << 7;

const fn mask(inputs: &[u32]) -> u64 {
    let mut m = 0u64;
    let mut i = 0;
    while i < inputs.len() {
        m |= 1 << inputs[i];
        i += 1;
    }
    m
}

/// The inputs at the draw's begin. An input whose words do not read is
/// not given; the stage refuses an item whose pass reads it.
pub fn read(mem: &dyn GuestRead) -> LayerInputs {
    let mut out = LayerInputs {
        time: mem
            .read_u32(RENDER_TIME)
            .zip(mem.read_u32(RENDER_TIME + 4))
            .map(|(lo, hi)| f64::from_bits(u64::from(lo) | u64::from(hi) << 32)),
        defaults: Defaults::read(mem),
        ..LayerInputs::default()
    };
    for n in 0..64u32 {
        if VIEW_SCALARS >> n & 1 != 0
            && let Some(v) = scalar(mem, n)
        {
            out.set_scalar(n as usize, v.to_bits());
        }
        if VIEW_VECTORS_GIVEN >> n & 1 != 0
            && let Some(v) = vector(mem, n, out.defaults.as_ref())
        {
            out.set_vector(n as usize, v.map(f32::to_bits));
        }
    }
    for n in 0..8u32 {
        if VIEW_INTS >> n & 1 != 0
            && let Some(v) = integer(mem, n)
        {
            out.set_int(n as usize, v);
        }
    }
    out
}

fn f(mem: &dyn GuestRead, at: u32) -> Option<f32> {
    mem.read_f32(at)
}

fn three(mem: &dyn GuestRead, at: u32) -> Option<[f32; 3]> {
    Some([f(mem, at)?, f(mem, at + 4)?, f(mem, at + 8)?])
}

/// `comiss`'s clamp: below `lo` is `lo`, above `hi` is `hi`, NaN stays.
fn clamp(v: f32, lo: f32, hi: f32) -> f32 {
    if lo > v {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// `1 / (far - near)` when the fog is on and its range is not empty.
#[allow(clippy::neg_cmp_op_on_partial_ord)] // NaN is no range, as `comiss`
fn fog_reciprocal(mem: &dyn GuestRead) -> Option<Option<f32>> {
    if mem.read_u32(FOG_PLANE)? == u32::MAX {
        return Some(None);
    }
    let (near, far) = (f(mem, FOG_NEAR)?, f(mem, FOG_FAR)?);
    if !(far > near) {
        return Some(None);
    }
    Some(Some(f(mem, ONE)? / (far - near)))
}

/// Scalar input `input` as `0x000336f0` computes it; `None` for one it
/// does not assign, one that reads the item, or words that do not read.
pub fn scalar(mem: &dyn GuestRead, input: u32) -> Option<f32> {
    let m = |o: u32| f(mem, MATERIAL + o);
    let fade = |o: u32| f(mem, FADE + o);
    let one = || f(mem, ONE);
    Some(match input {
        0 | 2 | 13 | 38 => 0.0,
        3 | 33 => m(0x90)?,
        4 | 25 => fade(12)?,
        10 | 24 => m(12)?,
        12 | 22 => one()?,
        16 => {
            let weight = f(mem, LIGHT_WEIGHT)?;
            let one = one()?;
            if one > weight { weight } else { one }
        }
        17 => f(mem, LIGHT_WEIGHT)?,
        18 | 27 => fade(12)? * m(12)?,
        19 | 36 => fade(0)? * m(0x90)?,
        20 | 35 => (one()? - fade(0)?) * m(0x90)?,
        26 => (one()? - fade(12)?) * m(0x90)?,
        28 => m(0x24)?,
        29 => fade(0x10)?,
        30 => (one()? - fade(0x10)?) * m(0x24)?,
        31 => fade(0x10)? * m(12)?,
        32 => m(0x3c)?,
        34 => fade(0)?,
        39 => match fog_reciprocal(mem)? {
            Some(inv) => 0.0 - inv * f(mem, FOG_NEAR)?,
            None => 0.0,
        },
        40 => {
            let view = mem.read_u32(VIEW_SELECT)? as i32;
            if (0..4).contains(&view) { f(mem, VIEW_VALUES + view as u32 * 4)? } else { one()? }
        }
        41 | 42 => f(mem, SCALARS_41 + (input - 41) * 4)?,
        _ => return None,
    })
}

/// Vector input `input` as `0x00033980` computes it; `None` for one it
/// does not assign, one that reads the item, or words that do not read.
/// Input 41 without fog is the default vector (`defaults`).
pub fn vector(mem: &dyn GuestRead, input: u32, defaults: Option<&Defaults>) -> Option<[f32; 3]> {
    let scaled = |base: u32| -> Option<[f32; 3]> {
        let factor = f(mem, base + 12)?;
        Some(three(mem, base)?.map(|x| x * factor))
    };
    Some(match input {
        0 => three(mem, VECTOR_0)?,
        5 => three(mem, VECTOR_5)?,
        6 => three(mem, VECTOR_6)?,
        18 | 25 => three(mem, MATERIAL)?,
        19 | 31 => three(mem, MATERIAL + 0x84)?,
        27 => three(mem, MATERIAL + 0x18)?,
        29 => three(mem, MATERIAL + 0x30)?,
        26 => scaled(MATERIAL)?,
        28 => scaled(MATERIAL + 0x18)?,
        30 => scaled(MATERIAL + 0x30)?,
        32 => scaled(MATERIAL + 0x84)?,
        33 | 34 => three(mem, VIEW_VECTORS + (input - 33) * 12)?,
        36..=39 => three(mem, VIEW_VECTORS + 0x40 + (input - 36) * 12)?,
        41 => match fog_reciprocal(mem)? {
            Some(inv) => three(mem, FOG_VECTOR)?.map(|lane| 0.0 - lane * inv),
            None => defaults?.vector.map(f32::from_bits),
        },
        _ => return None,
    })
}

/// A sign-extended half word.
fn read_i16(mem: &dyn GuestRead, at: u32) -> Option<i16> {
    let w = mem.read_u32(at & !3)?;
    Some(if at & 2 == 0 { w as u16 as i16 } else { (w >> 16) as u16 as i16 })
}

/// Integer input `input` as `0x00034060` computes it; `None` for 4 and 5
/// (the shader's), 6 (the shader cache's), one it does not assign, or
/// words that do not read.
pub fn integer(mem: &dyn GuestRead, input: u32) -> Option<u32> {
    let definition = || mem.read_u32(LIGHT_DEFINITION);
    Some(match input {
        0 | 1 => mem.read_u32(LIGHT_INTEGER)?,
        2 | 3 => {
            let at = definition()?.checked_add(if input == 2 { 0x8a } else { 0x80 })?;
            read_i16(mem, at)? as i32 as u32
        }
        7 => u32::from(mem.read_u32(definition()?)? & 2 == 0),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::object_lighting::{DEFAULT_COLOUR, DEFAULT_VECTOR};
    use crate::testing::Mem;

    /// A render context with every word the selectors read: the material
    /// block's words name their offset (`1000 + offset`), the fade
    /// block's are fractions, the fog on over 10..30.
    fn context() -> Mem {
        let mut m = Mem::default();
        m.f(ONE, 1.0);
        for o in (0..0xa0).step_by(4) {
            m.f(MATERIAL + o, 1000.0 + o as f32);
        }
        for (o, v) in [(0, 0.25), (12, 0.5), (0x10, 0.75)] {
            m.f(FADE + o, v);
        }
        for k in 0..3 {
            m.f(VECTOR_0 + 4 * k, 1.0 + k as f32);
            m.f(VECTOR_5 + 4 * k, 5.0 + k as f32);
            m.f(VECTOR_6 + 4 * k, 6.0 + k as f32);
            m.f(FOG_VECTOR + 4 * k, 2.0 * (k as f32 + 1.0));
        }
        for o in (0..0x70).step_by(4) {
            m.f(VIEW_VECTORS + o, 3000.0 + o as f32);
        }
        m.f(LIGHT_WEIGHT, 2.0);
        m.w(LIGHT_INTEGER, 3);
        m.w(FOG_PLANE, 0);
        m.f(FOG_NEAR, 10.0);
        m.f(FOG_FAR, 30.0);
        m.w(VIEW_SELECT, 1);
        m.f(VIEW_VALUES + 4, 0.125);
        m.f(SCALARS_41, 41.0);
        m.f(SCALARS_41 + 4, 42.0);
        let time = 12.5f64.to_bits();
        m.w(RENDER_TIME, time as u32);
        m.w(RENDER_TIME + 4, (time >> 32) as u32);
        m.w(DEFAULT_VECTOR, 0x0070_0000);
        m.w(DEFAULT_COLOUR, 0x0070_0100);
        for k in 0..3 {
            m.f(0x0070_0004 + 4 * k, -1.0);
            m.f(0x0070_0100 + 4 * k, 0.5);
        }
        m.w(LIGHT_DEFINITION, 0x0071_0000);
        m.w(0x0071_0000, 2);
        m.w(0x0071_0088, 0xfffe_0000); // +0x8a: -2
        m.w(0x0071_0080, 7);
        m
    }

    #[test]
    fn every_view_input_reads_as_its_selector_computes_it() {
        let m = context();
        let i = read(&m);
        assert_eq!(i.time, Some(12.5));
        assert_eq!(i.scalars_given, VIEW_SCALARS, "every view scalar reads");
        assert_eq!(i.vectors_given, VIEW_VECTORS_GIVEN, "every view vector reads");
        assert_eq!(i.ints_given, VIEW_INTS);
        assert_eq!(i.scalar(3), Some(1000.0 + 0x90 as f32));
        assert_eq!(i.scalar(10), Some(1012.0));
        assert_eq!(i.scalar(12), Some(1.0));
        assert_eq!(i.scalar(16), Some(1.0), "the weight is held to 1");
        assert_eq!(i.scalar(17), Some(2.0));
        assert_eq!(i.scalar(18), Some(0.5 * 1012.0));
        assert_eq!(i.scalar(20), Some(0.75 * (1000.0 + 0x90 as f32)));
        assert_eq!(i.scalar(30), Some(0.25 * (1000.0 + 0x24 as f32)));
        assert_eq!(i.scalar(39), Some(0.0 - (1.0 / 20.0) * 10.0));
        assert_eq!(i.scalar(40), Some(0.125), "view 1's value");
        assert_eq!(i.scalar(42), Some(42.0));
        assert_eq!(i.scalar(21), None, "the shader's own");
        assert_eq!(i.scalar(23), None, "the item's lighting");
        assert_eq!(i.vector(0), Some([1.0, 2.0, 3.0]));
        assert_eq!(i.vector(26), Some([1000.0, 1004.0, 1008.0].map(|x| x * 1012.0)));
        assert_eq!(i.vector(34), Some([3012.0, 3016.0, 3020.0]));
        assert_eq!(i.vector(39), Some([3000.0 + 0x64 as f32, 3000.0 + 0x68 as f32, 3000.0 + 0x6c as f32]));
        assert_eq!(i.vector(41), Some([2.0, 4.0, 6.0].map(|x: f32| 0.0 - x * (1.0 / 20.0))));
        assert_eq!(i.vector(14), None, "the item's lighting");
        assert_eq!((i.int(0), i.int(2), i.int(3), i.int(7)), (Some(3), Some(-2), Some(7), Some(0)));
        assert_eq!(i.int(4), None, "the shader's own");
        assert_eq!(i.defaults.map(|d| d.colour), Some([0.5f32.to_bits(); 3]));
    }

    #[test]
    fn without_fog_or_a_view_the_selectors_fall_back() {
        let mut m = context();
        m.w(FOG_PLANE, u32::MAX);
        m.w(VIEW_SELECT, 4);
        let i = read(&m);
        assert_eq!(i.scalar(39), Some(0.0));
        assert_eq!(i.vector(41), Some([-1.0; 3]), "the default vector");
        assert_eq!(i.scalar(40), Some(1.0));
        m.w(FOG_PLANE, 0);
        m.f(FOG_FAR, 10.0);
        assert_eq!(read(&m).scalar(39), Some(0.0), "an empty fog range is no fog");
    }

    #[test]
    fn an_input_whose_words_do_not_read_is_not_given() {
        let mut m = context();
        m.0.remove(&(FADE + 12));
        m.0.remove(&RENDER_TIME);
        let i = read(&m);
        assert_eq!(i.time, None);
        for n in [4, 18, 25, 26, 27] {
            assert_eq!(i.scalar(n), None, "scalar {n} reads the fade's +12");
        }
        assert_eq!(i.scalar(3), Some(1000.0 + 0x90 as f32), "the others still read");
    }
}
