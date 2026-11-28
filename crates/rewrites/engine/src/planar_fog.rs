//! The planar fog's screen quad (`0x00025d50`,
//! docs/graphics/retire-d3d-fog-quad.md): what its setup `0x00026520`
//! sets and the fan `0x00034420` pushes with the corners `0x00026710`
//! makes, as pure functions of guest memory, for the native renderer to
//! draw the quad in the game's place.
//!
//! (Code, `h2 disasm 0x25d50 30`, `0x26520 140`, `0x26710 90`,
//! `0x34420 120`, `0x131f40 30`.)
//!
//! - [`setup`]: `0x00025d50`'s choice. With the fog byte `0x00485a75` set
//!   it returns at once. Else view mode (`0x0048574c`) 1, 4 or 5, or the
//!   byte `0x00485860`, takes the setup `0x00025e50` (not read); modes 2
//!   and 3 take `0x00026520`; any other makes no fan. Every path but the
//!   first ends in the depth clip control set to 0 (`0x00041d78`).
//! - [`draws`]: the setup's gate. It writes the clip far `[0x00485ae0]`
//!   to the depth word `0x00509400`, then returns 0 (no fan) unless the
//!   density `[0x004857b4]` is above `[0x0045dbd8]` (`comiss; jbe`).
//! - [`colour`]: `0x00131f40(density, &[0x004857a8])`, the final
//!   combiner's constant: density, red, green and blue times
//!   `[0x0045dc38]`, each `fistp`'d under the caller's control word,
//!   packed ARGB, the three colour lanes masked to a byte.
//! - [`combiner`]: the 60-word block at `0x00484f68` the setup zeroes and
//!   fills for `0x0001ccf0`: one combiner stage that writes nothing, the
//!   final combiner `A = c0`, `B = 1`, `G = c0.a`, so the fan's colour is
//!   that constant.
//! - [`STATES`]: the render states the setup sets, by the game's state
//!   word and the D3D render-state word it stores them in.
//! - [`quad`]: the four vertices `0x00034420` pushes, corners `(0, 0)`,
//!   `(0, 1)`, `(1, 1)`, `(1, 0)` of the rectangle it made (`[esp+0x1c]`
//!   in its frame: the window `0x00485648`, or the whole target for
//!   targets `0x12`/`0x14`), each position `0x000350e0` at the depth word
//!   ([`crate::screen_grid`]'s port). The other attributes (1 to 4) feed
//!   texture coordinates the combiner does not read.
//!
//! The arithmetic is the original's: SSE through the `simd` crate under
//! the caller's MXCSR, x87 under its control word.

use crate::light_constants::{Error, Sse, X87};
use crate::memory::{GuestMemory, MemFault};
use crate::screen_grid;

/// The planar fog's quad, called between the sky and layer 16
/// (`0x0002c0d1`).
pub const FOG_QUAD: u32 = 0x0002_5d50;
/// The setup of view modes 2 and 3, handed to the fan in EAX.
pub const SETUP: u32 = 0x0002_6520;
/// The setup of modes 1, 4 and 5 (not read).
pub const OTHER_SETUP: u32 = 0x0002_5e50;
/// The attribute callback the fan is given.
pub const CORNERS: u32 = 0x0002_6710;
/// The fan.
pub const QUAD: u32 = 0x0003_4420;
/// Where the fan's `call edi` (the setup) returns.
pub const SETUP_RETURN: u32 = 0x0003_452e;
/// Where `0x00025d50`'s call of the fan returns.
pub const QUAD_RETURN: u32 = 0x0002_5d9d;
/// The fan's entry ESP above the setup's: `sub esp, 0x54`, two pushes,
/// the setup's argument and its return address.
pub const SETUP_FRAME: u32 = 0x64;

/// Guest globals and image constants the quad reads.
pub mod global {
    /// Set: `0x00025d50` returns at once.
    pub const SKIP: u32 = 0x0048_5a75;
    /// The view's fog mode (`i32`).
    pub const MODE: u32 = 0x0048_574c;
    /// Set: the other setup, whatever the mode.
    pub const OTHER: u32 = 0x0048_5860;
    /// The density (`f32`).
    pub const DENSITY: u32 = 0x0048_57b4;
    /// Red, green and blue (`f32`).
    pub const COLOUR: u32 = 0x0048_57a8;
    /// The clip far the depth word takes.
    pub const CLIP_FAR: u32 = 0x0048_5ae0;
    /// The depth word the corners place the fan at.
    pub const DEPTH: u32 = 0x0050_9400;
    /// 0.0, the density's floor.
    pub const ZERO: u32 = 0x0045_dbd8;
    /// The colour's scale.
    pub const SCALE: u32 = 0x0045_dc38;
    /// The combiner block `0x0001ccf0` applies.
    pub const BLOCK: u32 = 0x0048_4f68;
    /// The view's target (`i16`), the setup's first select (`0x00014bc0`).
    pub const TARGET: u32 = 0x0048_58b8;
}

/// Words of a combiner block (`D3DPIXELSHADERDEF`).
pub const WORDS: usize = 60;

/// The block's words the setup writes after zeroing it.
pub mod word {
    /// `PSFinalCombinerInputsABCD`: `A = c0`, `B = 1` (zero inverted).
    pub const ABCD: usize = 8;
    pub const ABCD_VALUE: u32 = 0x0120_0000;
    /// `PSFinalCombinerInputsEFG`: `G = c0.a`.
    pub const EFG: usize = 9;
    pub const EFG_VALUE: u32 = 0x1100;
    /// `PSFinalCombinerConstant0`: the colour.
    pub const CONSTANT0: usize = 43;
    /// `PSCombinerCount`: one stage, unique `c0` and `c1`.
    pub const COUNT: usize = 53;
    pub const COUNT_VALUE: u32 = 0x0001_1001;
}

/// The render states the setup sets: the game's state word and the D3D
/// render-state word each is stored in (`0` where the setter keeps only
/// the game's), and the value.
pub const STATES: [(&str, u32, u32, u32); 13] = [
    ("colour_mask", 0x004b_8308, 0x0040_75a4, 0x0101_0101),
    ("blend", 0x004b_82e8, 0x0040_7584, 1),
    ("blend_source", 0x004b_82f4, 0x0040_7590, BLEND_SOURCE),
    ("blend_dest", 0x004b_82f8, 0x0040_7594, BLEND_DEST),
    ("blend_equation", 0x004b_8324, 0x0040_75c0, BLEND_EQUATION),
    ("blend_colour", 0x004b_8328, 0x0040_75c4, BLEND_COLOUR),
    ("alpha_test", 0x004b_82ec, 0x0040_7588, 0),
    ("cull", 0x004b_8448, 0, 0),
    ("stencil", 0x004b_843c, 0, 0),
    ("z_enable", 0x004b_8438, 0, Z_ENABLE),
    ("z_write", 0x004b_82fc, 0x0040_7598, 0),
    ("z_func", 0x004b_82e0, 0x0040_757c, Z_FUNC),
    ("z_bias", 0x004b_8450, 0, 0),
];

/// `GL_CONSTANT_COLOR`, `GL_ONE_MINUS_SRC_ALPHA`, `GL_FUNC_ADD`.
pub const BLEND_SOURCE: u32 = 0x8001;
pub const BLEND_DEST: u32 = 0x0303;
pub const BLEND_EQUATION: u32 = 0x8006;
/// The blend constant, ARGB: white with alpha 0.
pub const BLEND_COLOUR: u32 = 0x00ff_ffff;
/// W-buffered.
pub const Z_ENABLE: u32 = 2;
/// `GL_LEQUAL`.
pub const Z_FUNC: u32 = 0x0203;

/// What `0x00025d50` does this call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setup {
    /// The fog byte is set: nothing.
    Skipped,
    /// No fan: the depth clip control only.
    None,
    /// The fan through `0x00026520`.
    Quad,
    /// The fan through `0x00025e50`.
    Other,
}

/// `0x00025d50`'s branches (`0x00025d50`..`0x00025d9d`).
pub fn setup(mem: &dyn GuestMemory) -> Result<Setup, MemFault> {
    if mem.u8(global::SKIP)? != 0 {
        return Ok(Setup::Skipped);
    }
    let mode = mem.u32(global::MODE)?;
    if matches!(mode, 1 | 4 | 5) || mem.u8(global::OTHER)? != 0 {
        return Ok(Setup::Other);
    }
    Ok(if matches!(mode, 2 | 3) { Setup::Quad } else { Setup::None })
}

/// The setup's gate: whether it goes on to its state and the fan
/// (`comiss [0x004857b4], [0x0045dbd8]; jbe`).
pub fn draws(mem: &dyn GuestMemory, mxcsr: u32) -> Result<bool, Error> {
    Sse { mxcsr }.above(mem.u32(global::DENSITY)?, mem.u32(global::ZERO)?)
}

/// What the setup writes before its gate, and all it writes when the gate
/// stops it: the clip far to the depth word.
pub fn gate_writes(mem: &mut dyn GuestMemory) -> Result<(), MemFault> {
    let far = mem.u32(global::CLIP_FAR)?;
    mem.set_u32(global::DEPTH, far)
}

/// `0x00131f40` of the density and the colour: the final combiner's
/// constant.
pub fn colour(mem: &dyn GuestMemory, x87_control: u16) -> Result<u32, Error> {
    let mut x87 = X87::new(x87_control);
    x87.fld(mem.u32(global::DENSITY)?)?;
    for i in 0..3 {
        x87.fld(mem.u32(global::COLOUR + 4 * i)?)?;
    }
    x87.fld(mem.u32(global::SCALE)?)?;
    let mul = x87::abi::arith_op::MUL;
    x87.arith_reg(mul, 4, 0, false)?;
    x87.arith_reg(mul, 3, 0, false)?;
    x87.arith_reg(mul, 2, 0, false)?;
    x87.arith_reg(mul, 1, 0, true)?;
    // Blue first, then green and red, each masked; the density's bits
    // above the byte shift out.
    let b = crate::colour_pass::fistp(&mut x87)? & 0xff;
    let g = crate::colour_pass::fistp(&mut x87)? & 0xff;
    let r = crate::colour_pass::fistp(&mut x87)? & 0xff;
    let a = crate::colour_pass::fistp(&mut x87)?;
    Ok(b | g << 8 | r << 16 | a << 24)
}

/// The block the setup hands `0x0001ccf0`.
pub fn combiner(colour: u32) -> [u32; WORDS] {
    let mut w = [0; WORDS];
    w[word::COUNT] = word::COUNT_VALUE;
    w[word::CONSTANT0] = colour;
    w[word::ABCD] = word::ABCD_VALUE;
    w[word::EFG] = word::EFG_VALUE;
    w
}

/// The fan's corners in the order it pushes them.
pub const CORNER_UV: [[bool; 2]; 4] = [[false, false], [false, true], [true, true], [true, false]];

/// One quad as the setup and the fan make it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FogQuad {
    /// The final combiner's constant ([`colour`]).
    pub colour: u32,
    /// The block ([`combiner`]).
    pub combiner: [u32; WORDS],
    /// The depth word (f32 bits).
    pub depth: u32,
    /// The fan's rectangle: left, right, top, bottom (f32 bits).
    pub rect: [u32; 4],
    /// Each vertex's position, x, y, z, w (f32 bits), in push order.
    pub positions: [[u32; 4]; 4],
}

/// The quad of a fan whose rectangle is at `rect_at`, under the call's
/// MXCSR and x87 control word, with the setup's gate passed.
///
/// The callback reads the depth word and writes the clip far back when it
/// is not above 0 (`0x00026713`); the setup has just written that, so the
/// depth is the clip far.
pub fn quad(mem: &dyn GuestMemory, mxcsr: u32, x87_control: u16, rect_at: u32) -> Result<FogQuad, Error> {
    let colour = colour(mem, x87_control)?;
    let depth = mem.u32(global::CLIP_FAR)?;
    let mut rect = [0; 4];
    for (i, r) in rect.iter_mut().enumerate() {
        *r = mem.u32(rect_at.wrapping_add(4 * i as u32))?;
    }
    let one = mem.u32(screen_grid::constant::ONE)?;
    let mut sse = Sse { mxcsr };
    let mut positions = [[0; 4]; 4];
    for (p, [u, v]) in positions.iter_mut().zip(CORNER_UV) {
        let uv = [if u { one } else { 0 }, if v { one } else { 0 }];
        *p = screen_grid::position(mem, &mut sse, &rect, uv, depth)?;
    }
    Ok(FogQuad { colour, combiner: combiner(colour), depth, rect, positions })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::FlatMemory;

    fn mem() -> FlatMemory {
        let mut m = FlatMemory::new(0x0040_0000, 0x0012_0000);
        let f = |m: &mut FlatMemory, a: u32, v: f32| m.set_f32(a, v).unwrap();
        f(&mut m, screen_grid::constant::ONE, 1.0);
        f(&mut m, screen_grid::constant::DEPTH, 16_777_215.0);
        f(&mut m, screen_grid::constant::NEAR, 0.0625);
        f(&mut m, screen_grid::constant::FAR, 1024.0);
        f(&mut m, global::SCALE, 255.0);
        f(&mut m, global::CLIP_FAR, 1024.0);
        f(&mut m, global::DENSITY, 0.5);
        f(&mut m, global::COLOUR, 1.0);
        f(&mut m, global::COLOUR + 4, 0.5);
        f(&mut m, global::COLOUR + 8, 0.25);
        m
    }

    /// The x87 control word the game runs under: every exception masked,
    /// 53-bit precision, round to nearest.
    const CONTROL: u16 = 0x027f;
    const MXCSR: u32 = 0x1f80;

    #[test]
    fn the_branches_follow_the_fog_byte_the_mode_and_the_other_byte() {
        let mut m = mem();
        let at = |m: &mut FlatMemory, mode: u32| {
            m.set_u32(global::MODE, mode).unwrap();
            setup(m).unwrap()
        };
        assert_eq!(at(&mut m, 2), Setup::Quad);
        assert_eq!(at(&mut m, 3), Setup::Quad);
        assert_eq!(at(&mut m, 0), Setup::None);
        assert_eq!(at(&mut m, 6), Setup::None);
        for mode in [1, 4, 5] {
            assert_eq!(at(&mut m, mode), Setup::Other);
        }
        m.set_u8(global::OTHER, 1).unwrap();
        assert_eq!(at(&mut m, 2), Setup::Other);
        m.set_u8(global::SKIP, 1).unwrap();
        assert_eq!(at(&mut m, 2), Setup::Skipped);
    }

    #[test]
    fn the_gate_passes_only_a_density_above_zero() {
        let mut m = mem();
        assert!(draws(&m, MXCSR).unwrap());
        for v in [0.0f32, -1.0, f32::NAN] {
            m.set_f32(global::DENSITY, v).unwrap();
            assert!(!draws(&m, MXCSR).unwrap(), "{v}");
        }
    }

    #[test]
    fn the_colour_is_argb_rounded_under_the_control_word() {
        let mut m = mem();
        // 0.5 * 255 = 127.5 rounds to even, 128; 0.25 * 255 = 63.75, 64.
        assert_eq!(colour(&m, CONTROL).unwrap(), 0x80ff_8040);
        // Truncating: 127 and 63.
        assert_eq!(colour(&m, CONTROL | 0x0c00).unwrap(), 0x7fff_7f3f);
        // A lane past a byte keeps its low byte; the density's spill
        // shifts out.
        m.set_f32(global::COLOUR, 2.0).unwrap();
        m.set_f32(global::DENSITY, 1.5).unwrap();
        assert_eq!(colour(&m, CONTROL).unwrap(), (382u32 << 24) | 0x00fe_8040);
    }

    #[test]
    fn the_block_is_the_constant_through_the_final_combiner() {
        let b = combiner(0x1234_5678);
        let set: Vec<(usize, u32)> = b.iter().copied().enumerate().filter(|(_, w)| *w != 0).collect();
        assert_eq!(set, [(8, 0x0120_0000), (9, 0x1100), (43, 0x1234_5678), (53, 0x0001_1001)]);
    }

    #[test]
    fn the_corners_are_the_rectangle_at_the_clip_far() {
        let mut m = mem();
        let rect_at = 0x0050_0000;
        for (i, v) in [0.0f32, 640.0, 0.0, 480.0].into_iter().enumerate() {
            m.set_f32(rect_at + 4 * i as u32, v).unwrap();
        }
        let q = quad(&m, MXCSR, CONTROL, rect_at).unwrap();
        assert_eq!(q.depth, 1024.0f32.to_bits());
        let xy: Vec<[f32; 2]> = q.positions.iter().map(|p| [f32::from_bits(p[0]), f32::from_bits(p[1])]).collect();
        assert_eq!(xy, [[0.0, 0.0], [0.0, 480.0], [640.0, 480.0], [640.0, 0.0]]);
        // At the far plane z and w are the depth's top.
        assert!(q.positions.iter().all(|p| f32::from_bits(p[3]) == 16_777_215.0));
        assert!(q.positions.iter().all(|p| p[2] == q.positions[0][2]));
        assert_eq!(q.combiner[word::CONSTANT0], q.colour);
    }
}
