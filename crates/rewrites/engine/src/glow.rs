//! The glow `render_screen_effects` draws as its colour pass
//! (`0x000214f0`; docs/graphics/native-post-hud.md, stage 5): what it
//! draws, as pure functions, for the native renderer to draw it in the
//! game's place. The numbers before it are `colour_pass`'s.
//!
//! The pass (Code, `h2 disasm`):
//!
//! 1. **Down-sample.** The view's target bound to stages 0..3, the
//!    combiner [`pass_a`] (four taps), drawn by the quad draw `0x00034a90`
//!    into target 7 with four taps at spread 0.5.
//! 2. **Blur.** `0x000211a0` (EAX 7; target 8 the other buffer): first
//!    `0x00020f30` sets the combiner [`blur_combiner`] (from the fourth
//!    argument and the local `y`), then [`blur`]'s draws, each binding
//!    the last result to stages 0..3 and drawing four taps into the other
//!    buffer with a spread that grows by a factor each step (the last step
//!    weighted by the fractional count). It returns the last result.
//! 3. **Composite.** That result at stage 0, the combiner [`pass_b`],
//!    alpha blending (`0x000222d0`), one tap into the view's target.
//!
//! A combiner is the 60-word `D3DPIXELSHADERDEF` block the game writes
//! at `0x00484f68` (cleared first) and selects. The arithmetic is the
//! original's instruction for instruction (SSE under the caller's MXCSR,
//! x87 under its control word), checked against the lifted original by
//! `engine/tests/glow_differential.rs`. Image constants stay in the image.

use crate::colour_pass::Parameters;
use crate::light_constants::{Error, Sse, X87};
use guest_layout::memory::GuestMemory;

/// The combiner block.
pub const PSDEF: u32 = 0x0048_4f68;
/// Its words.
pub const PSDEF_WORDS: usize = 60;
/// A byte the combiners read (`0x00485a74`): set, they take the second
/// form of two words.
pub const COMBINER_BYTE: u32 = 0x0048_5a74;
/// Image constants.
pub mod constant {
    pub const ONE: u32 = 0x0045_dbc0;
    /// The blur's spread factor term.
    pub const SPREAD: u32 = 0x0045_dc0c;
    /// `0x00020f30`'s scale of `y + 1`.
    pub const Y_SCALE: u32 = 0x0045_dbbc;
    pub const PACK_SCALE: u32 = 0x0045_dc38;
}

/// The targets the pass draws into: the down-sample's, and the blur's
/// other buffer.
pub const TARGET_DOWN: u32 = 7;
pub const TARGET_OTHER: u32 = 8;

/// A combiner block being written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Psdef(pub [u32; PSDEF_WORDS]);

impl Psdef {
    fn new() -> Self {
        Psdef([0; PSDEF_WORDS])
    }
    fn set(&mut self, addr: u32, v: u32) {
        self.0[((addr - PSDEF) / 4) as usize] = v;
    }
    /// The block's bytes, as the game leaves them at [`PSDEF`].
    pub fn bytes(&self) -> Vec<u8> {
        self.0.iter().flat_map(|w| w.to_le_bytes()).collect()
    }
}

/// `fistp dword`.
fn fistp(x87: &mut X87) -> Result<u32, Error> {
    let (code, store) = x87::abi::st_i32(&x87.0, 0);
    X87::ok(code)?;
    x87::abi::store_commit(&mut x87.0, store.status, true);
    Ok(store.value as u32)
}

/// `fld a; fld b; fmulp; fistp`: `a * b` rounded under the control word.
fn product_int(a: u32, b: u32, x87_control: u16) -> Result<u32, Error> {
    let mut x = X87::new(x87_control);
    x.fld(a)?;
    x.fld(b)?;
    x.arith_reg(x87::abi::arith_op::MUL, 1, 0, true)?;
    fistp(&mut x)
}

/// `0x00131ed0`: three lanes scaled (x87), each `fistp`'d and masked to a
/// byte, `v[0] << 16 | v[1] << 8 | v[2]`.
pub fn pack_rgb(v: [u32; 3], scale: u32, x87_control: u16) -> Result<u32, Error> {
    let mut x = X87::new(x87_control);
    for lane in v {
        x.fld(lane)?;
    }
    x.fld(scale)?;
    let mul = x87::abi::arith_op::MUL;
    x.arith_reg(mul, 3, 0, false)?;
    x.arith_reg(mul, 2, 0, false)?;
    x.arith_reg(mul, 1, 0, true)?;
    let l2 = fistp(&mut x)? & 0xff;
    let l1 = fistp(&mut x)? & 0xff;
    let l0 = fistp(&mut x)? & 0xff;
    Ok(l2 | (l1 << 8) | (l0 << 16))
}

/// The down-sample's combiner (`0x00021775`..`0x000218d5`) for `p`.
pub fn pass_a(mem: &dyn GuestMemory, p: &Parameters, x87_control: u16) -> Result<Psdef, Error> {
    let mut d = Psdef::new();
    let byte = mem.u8(COMBINER_BYTE)? != 0;
    for a in [0x0048_4fd0, 0x0048_4fd8, 0x0048_501c, 0x0048_5024] {
        d.set(a, 0x30c00);
    }
    d.set(0x0048_5040, 0x8421);
    d.set(0x0048_503c, 0x11004);
    d.set(0x0048_4f68, 0x1820_1920);
    d.set(0x0048_4f6c, 0x1a20_1b20);
    d.set(0x0048_4fd4, 0x30d00);
    d.set(0x0048_4f70, 0x1c20_1d20);
    d.set(0x0048_4ff0, 0x0820_0920);
    d.set(0x0048_4ff4, 0x0a20_0b20);
    d.set(0x0048_5020, 0x30d00);
    d.set(0x0048_4ff8, 0x0c20_0d20);
    let [a, _] = p.colours(mem, x87_control)?;
    d.set(0x0048_4f9c, a);
    d.set(0x0048_4ffc, 0x0c20_0140);
    d.set(0x0048_5028, 0x20d00);
    d.set(0x0048_4f74, 0x1c11_0000);
    d.set(0x0048_4fdc, 0x10d00);
    // 0x00021882: 0.0 times the pack scale, fistp'd, << 24.
    let w = product_int(0, mem.u32(constant::PACK_SCALE)?, x87_control)?;
    d.set(0x0048_5014, w << 24);
    d.set(0x0048_4f88, ((if byte { 0x1c } else { 0 }) << 24) | 0xf000d);
    d.set(0x0048_4f8c, ((if byte { 0x1d } else { 0 }) | 0xc1100) << 8);
    Ok(d)
}

/// The composite's combiner (`0x0002198f`..`0x00021a16`) for `p`.
pub fn pass_b(mem: &dyn GuestMemory, p: &Parameters, x87_control: u16) -> Result<Psdef, Error> {
    let mut d = Psdef::new();
    let byte = mem.u8(COMBINER_BYTE)? != 0;
    d.set(0x0048_5040, 1);
    d.set(0x0048_503c, 0x11001);
    let [_, b] = p.colours(mem, x87_control)?;
    d.set(0x0048_4f90, b);
    d.set(0x0048_501c, 0x10c00);
    d.set(0x0048_4f88, 0xc);
    d.set(0x0048_4ff0, (if byte { 0x11 } else { 0 }) | 0x0801_1800);
    Ok(d)
}

/// `0x00020f30` up to its selection (`0x0002115f`): the blur's combiner
/// for `strength` (XMM3, the colour pass's fifth argument) and `y` (XMM0,
/// its local `y`).
pub fn blur_combiner(mem: &dyn GuestMemory, strength: u32, y: u32, mxcsr: u32, x87_control: u16) -> Result<Psdef, Error> {
    let mut sse = Sse { mxcsr };
    let one = mem.u32(constant::ONE)?;
    let zero = 0;
    let mut d = Psdef::new();
    d.set(0x0048_5040, 0x8421);
    let strength = sse.clamp(strength, zero, one)?;
    let edi = 0xc00;
    let ecx = 0x30c00;
    // 0x00020f69: y below 0, or not above 0 once at most 1, takes the
    // plain form.
    let y = if sse.above(zero, y)? {
        None
    } else if sse.above(y, one)? {
        Some(one)
    } else if sse.above(y, zero)? {
        Some(y)
    } else {
        None
    };
    match y {
        Some(y) => {
            let v = sse.add(y, one)?;
            let v = sse.mul(v, mem.u32(constant::Y_SCALE)?)?;
            let v = sse.clamp(v, zero, one)?;
            let w = product_int(v, mem.u32(constant::PACK_SCALE)?, x87_control)?;
            d.set(0x0048_4fb8, w << 24);
            d.set(0x0048_4f70, 0x1c12_1d12);
            d.set(0x0048_4fd8, edi);
        }
        None => {
            d.set(0x0048_4f70, 0x1c20_1d20);
            d.set(0x0048_4fd8, ecx);
        }
    }
    d.set(0x0048_4f68, 0x1820_1920);
    d.set(0x0048_4fd0, ecx);
    d.set(0x0048_4f6c, 0x1a20_1b20);
    d.set(0x0048_4fd4, 0x30d00);
    d.set(0x0048_4f8c, 0x1c00);
    let stages = if sse.above(strength, zero)? {
        let c = pack_rgb([strength; 3], mem.u32(constant::PACK_SCALE)?, x87_control)?;
        for a in [0x0048_4f94, 0x0048_4fa0, 0x0048_4fac] {
            d.set(a, c);
        }
        for (a, v) in [
            (0x0048_4ff0, 0x0820_0940),
            (0x0048_501c, 0x20c00),
            (0x0048_4ff4, 0xcc01_2020),
            (0x0048_5020, ecx),
            (0x0048_4ff8, 0x0c08_2c09),
            (0x0048_5024, edi),
            (0x0048_4ffc, 0x0a20_0b40),
            (0x0048_5028, 0x20d00),
            (0x0048_5000, 0xcd01_2020),
            (0x0048_502c, 0x30d00),
            (0x0048_5004, 0x0d0a_2d0b),
            (0x0048_5030, 0x0d00),
            (0x0048_5008, 0x0c20_0d40),
            (0x0048_5034, 0x20400),
            (0x0048_500c, 0xc401_2020),
            (0x0048_5038, 0x30400),
            (0x0048_4f88, 0x040c_0d00),
        ] {
            d.set(a, v);
        }
        8
    } else {
        for (a, v) in [
            (0x0048_4ff0, 0x0820_0920),
            (0x0048_501c, ecx),
            (0x0048_4ff4, 0x0a20_0b20),
            (0x0048_5020, 0x30d00),
            (0x0048_4ff8, 0x0c20_0d20),
            (0x0048_5024, ecx),
            (0x0048_4f88, 0xc),
        ] {
            d.set(a, v);
        }
        3
    };
    d.set(0x0048_503c, stages | 0x11000);
    Ok(d)
}

/// One of the blur's draws: `source` bound to stages 0..3, four taps at
/// `spread` into `dest`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlurDraw {
    pub source: u32,
    pub dest: u32,
    /// The quad draw's seventh argument (f32 bits): `fraction * scale`.
    pub spread: u32,
}

/// What `0x000211a0` draws after its combiner, and the target it returns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blur {
    pub draws: Vec<BlurDraw>,
    /// The target the last draw wrote (or `source` with none), and the
    /// copy into `last` when that is another target (`0x000351a0`).
    pub result: u32,
    pub copy: Option<(u32, u32)>,
}

/// The arguments of `0x000211a0` the draws depend on (stack words 0..5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlurArgs {
    /// EAX: the target blurred.
    pub source: u32,
    /// The step count (f32 bits; the last step takes its fraction).
    pub steps: u32,
    /// The first spread scale (f32 bits).
    pub scale: u32,
    /// The growth term: each step's scale is the last times
    /// `1 - growth + growth * SPREAD`.
    pub growth: u32,
    /// The buffer drawn into first.
    pub dest: u32,
    /// The other buffer (-1: the source).
    pub other: u32,
    /// Where the result is copied unless -1 or the result itself.
    pub last: u32,
}

impl BlurArgs {
    /// The colour pass's call (`0x00021901`..`0x00021929`): its first
    /// argument as the step count, scale 1.0, its fourth as the growth.
    pub fn of(p: &Parameters) -> Self {
        BlurArgs {
            source: TARGET_DOWN,
            steps: p.args[0],
            scale: 0x3f80_0000,
            growth: p.args[3],
            dest: TARGET_OTHER,
            other: u32::MAX,
            last: u32::MAX,
        }
    }
}

/// The most steps [`blur`] takes: the colour pass asks for a handful,
/// and at infinity (or past 2^24, where `i + 1` is `i`) the original
/// never leaves its loop.
pub const MAX_BLUR_STEPS: u32 = 64;

/// `0x000211a0` after its combiner (`0x000211d5`..`0x0002135a`).
/// [`Error::Unbounded`] past [`MAX_BLUR_STEPS`] steps.
pub fn blur(mem: &dyn GuestMemory, a: &BlurArgs, mxcsr: u32) -> Result<Blur, Error> {
    let mut sse = Sse { mxcsr };
    let one = mem.u32(constant::ONE)?;
    let zero = 0;
    let mut current = a.source;
    let (mut dest, mut other) = (a.dest, if a.other == u32::MAX { a.source } else { a.other });
    let mut draws = Vec::new();
    if sse.above(a.steps, zero)? {
        let g = sse.sub(one, a.growth)?;
        let t = sse.mul(a.growth, mem.u32(constant::SPREAD)?)?;
        let factor = sse.add(g, t)?;
        let (mut i, mut scale) = (zero, a.scale);
        let mut taken = 0;
        loop {
            let left = sse.sub(a.steps, i)?;
            let fraction = if sse.above(zero, left)? {
                None
            } else if sse.above(left, one)? {
                Some(one)
            } else if sse.above(left, zero)? {
                Some(left)
            } else {
                None
            };
            if let Some(f) = fraction {
                let spread = sse.mul(f, scale)?;
                draws.push(BlurDraw {
                    source: current,
                    dest,
                    spread,
                });
                current = dest;
                (dest, other) = (other, current);
            }
            scale = sse.mul(factor, scale)?;
            i = sse.add(i, one)?;
            if !sse.above(a.steps, i)? {
                break;
            }
            taken += 1;
            if taken >= MAX_BLUR_STEPS {
                return Err(Error::Unbounded);
            }
        }
    }
    if a.last != u32::MAX && a.last != current {
        return Ok(Blur {
            draws,
            result: a.last,
            copy: Some((current, a.last)),
        });
    }
    Ok(Blur {
        draws,
        result: current,
        copy: None,
    })
}

/// `0x00021360`'s image constants: the scale's factor, and the second
/// and third offsets' (the step factor is [`constant::SPREAD`]).
pub mod wide_constant {
    pub const SCALE: u32 = 0x0045_e4e0;
    pub const SECOND: u32 = 0x0045_e1f4;
    pub const THIRD: u32 = 0x0045_e4dc;
}

/// The arguments of `0x00021360` (`ret 0x20`), a screen effect's
/// convolution with flag bit 2 (`engine::screen_effect`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WideArgs {
    /// EAX: the target convolved.
    pub source: u32,
    /// ECX: the draws (none at 0 or below).
    pub taps: i32,
    /// XMM0: the first draw's scale (f32 bits), before
    /// [`wide_constant::SCALE`].
    pub scale: u32,
    /// Stack words 0 and 1, passed through to each draw (`0x00034d90`'s
    /// sixth and seventh).
    pub words: [u32; 2],
    /// Stack words 2 and 3: the buffer drawn into first, the other
    /// (-1: the source).
    pub dest: u32,
    pub other: u32,
    /// Stack word 4: where the result is copied unless -1 or the result.
    pub last: u32,
}

/// One draw of `0x00021360`: `source` bound to stages 0..3 (`0x00014f60`),
/// `0x00034d90` into `dest` (EAX) with EDI pointing at `offsets` (f32
/// bits: `s`, `s * SECOND`, `s * THIRD`, `0 - s`) and the stack words
/// `0xa`, `0x01010101`, 0, 0, 1.0, the two [`WideArgs::words`],
/// `0x00021360`'s stack word 6 (0 from `0x0002d140`), 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WideDraw {
    pub source: u32,
    pub dest: u32,
    pub offsets: [u32; 4],
}

/// What `0x00021360` draws after its combiner, and the target it returns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wide {
    pub draws: Vec<WideDraw>,
    /// As [`Blur::result`].
    pub result: u32,
    pub copy: Option<(u32, u32)>,
}

/// The most draws [`wide`] takes (the original's count is an integer, so
/// it ends, but past this it is not a screen effect worth drawing).
pub const MAX_WIDE_TAPS: i32 = 64;

/// `0x00021360` after its combiner (`0x0002139e`..`0x000214e2`; with its
/// sixth argument's low byte set it first calls `0x00020f30`, XMM0 0 and
/// XMM3 its eighth, as [`blur`]'s caller does). Per draw `s` is the last
/// one's times [`constant::SPREAD`], starting at `scale *
/// wide_constant::SCALE`; the buffers ping-pong as the blur's.
/// [`Error::Unbounded`] past [`MAX_WIDE_TAPS`].
pub fn wide(mem: &dyn GuestMemory, a: &WideArgs, mxcsr: u32) -> Result<Wide, Error> {
    let mut sse = Sse { mxcsr };
    if a.taps > MAX_WIDE_TAPS {
        return Err(Error::Unbounded);
    }
    let mut s = sse.mul(a.scale, mem.u32(wide_constant::SCALE)?)?;
    let mut current = a.source;
    let (mut dest, mut other) = (a.dest, if a.other == u32::MAX { a.source } else { a.other });
    let mut draws = Vec::new();
    if a.taps > 0 {
        let second = mem.u32(wide_constant::SECOND)?;
        let third = mem.u32(wide_constant::THIRD)?;
        let factor = mem.u32(constant::SPREAD)?;
        for _ in 0..a.taps {
            let offsets = [s, sse.mul(s, second)?, sse.mul(s, third)?, sse.sub(0, s)?];
            draws.push(WideDraw {
                source: current,
                dest,
                offsets,
            });
            current = dest;
            (dest, other) = (other, current);
            s = sse.mul(s, factor)?;
        }
    }
    if a.last != u32::MAX && a.last != current {
        return Ok(Wide {
            draws,
            result: a.last,
            copy: Some((current, a.last)),
        });
    }
    Ok(Wide {
        draws,
        result: current,
        copy: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    /// The two constants the blur reads (1.0, and a spread term of 2.0
    /// chosen here), on otherwise empty memory.
    fn constants() -> FlatMemory {
        let mut m = FlatMemory::new(0x0045_d000, 0x1000);
        m.set_u32(constant::ONE, 1.0f32.to_bits()).unwrap();
        m.set_u32(constant::SPREAD, 2.0f32.to_bits()).unwrap();
        m
    }

    fn steps(s: f32) -> BlurArgs {
        BlurArgs {
            source: TARGET_DOWN,
            steps: s.to_bits(),
            scale: 1.0f32.to_bits(),
            growth: 0.25f32.to_bits(),
            dest: TARGET_OTHER,
            other: u32::MAX,
            last: u32::MAX,
        }
    }

    #[test]
    fn the_blur_stops_at_its_budget() {
        let m = constants();
        let draws = |s: f32| blur(&m, &steps(s), 0x1f80).map(|b| b.draws.len());
        assert_eq!(draws(3.5), Ok(4), "the last step its fraction");
        assert_eq!(draws(MAX_BLUR_STEPS as f32), Ok(MAX_BLUR_STEPS as usize), "the budget itself");
        assert_eq!(draws(MAX_BLUR_STEPS as f32 + 0.5), Err(Error::Unbounded));
        assert_eq!(draws(f32::INFINITY), Err(Error::Unbounded), "the original never returns");
        assert_eq!(draws(f32::NAN), Ok(0));
        assert_eq!(draws(0.0), Ok(0));
    }

    fn taps(n: i32, other: u32, last: u32) -> WideArgs {
        WideArgs {
            source: 1,
            taps: n,
            scale: 2.0f32.to_bits(),
            words: [5, 6],
            dest: 0x19,
            other,
            last,
        }
    }

    #[test]
    fn the_wide_convolution_ping_pongs_with_a_growing_scale() {
        // Constants chosen here: 2 the spread factor, 0.5 the scale's
        // factor, 2 and 3 the second and third offsets'.
        let mut m = FlatMemory::new(0x0045_d000, 0x2000);
        m.set_u32(constant::SPREAD, 2.0f32.to_bits()).unwrap();
        m.set_u32(wide_constant::SCALE, 0.5f32.to_bits()).unwrap();
        m.set_u32(wide_constant::SECOND, 2.0f32.to_bits()).unwrap();
        m.set_u32(wide_constant::THIRD, 3.0f32.to_bits()).unwrap();
        let f = |v: f32| v.to_bits();
        let w = wide(&m, &taps(3, 0x1a, u32::MAX), 0x1f80).unwrap();
        assert_eq!(
            w.draws,
            vec![
                WideDraw { source: 1, dest: 0x19, offsets: [f(1.0), f(2.0), f(3.0), f(-1.0)] },
                WideDraw { source: 0x19, dest: 0x1a, offsets: [f(2.0), f(4.0), f(6.0), f(-2.0)] },
                WideDraw { source: 0x1a, dest: 0x19, offsets: [f(4.0), f(8.0), f(12.0), f(-4.0)] },
            ],
            "each draw's scale the last's times the spread factor"
        );
        assert_eq!((w.result, w.copy), (0x19, None));
        let w = wide(&m, &taps(2, u32::MAX, 0x1b), 0x1f80).unwrap();
        assert_eq!(w.draws[1].dest, 1, "without another buffer, back into the source");
        assert_eq!((w.result, w.copy), (0x1b, Some((1, 0x1b))));
        let w = wide(&m, &taps(0, 0x1a, u32::MAX), 0x1f80).unwrap();
        assert_eq!((w.draws.len(), w.result), (0, 1), "no taps: the source");
        assert_eq!(wide(&m, &taps(-3, 0x1a, 1), 0x1f80).unwrap().copy, None, "the source is the last");
        assert_eq!(wide(&m, &taps(MAX_WIDE_TAPS + 1, 0x1a, u32::MAX), 0x1f80), Err(Error::Unbounded));
    }
}
