//! The second fog's screen fans (`0x00025dc0`,
//! docs/graphics/retire-d3d-fog-fans.md): which fans it draws, as a pure
//! function of guest memory and its two flag bytes.
//!
//! (Code, `h2 disasm 0x25dc0 40`, `0x26880 240`, `0x26e50 410`,
//! `0x276d0 160`, `0x34420 215`.)
//!
//! `0x00025dc0(cl, dl)` draws only when the fog's tag datum
//! `[0x004857b8]` is neither -1 nor 0 and `[0x004857d8]` is above
//! `[0x0045dbd8]` (`comiss; jbe`). Then it stores `cl` and `dl` at
//! `0x005093fd` and `0x005093fe` and draws through the fan `0x00034420`:
//!
//! | Flags | Fans (setup, attribute callback) |
//! |---|---|
//! | `cl` set | [`Fans::Layers`]: `0x00026880`, `0x00026ca0`, into target `0x13`; then `0x00026e50`, `0x00027520` |
//! | `cl` clear, `dl` set | [`Fans::Composite`]: `0x000276d0`, `0x00027960`, target 3 into the view's |
//! | neither | none |
//!
//! Every path ends with the depth clip control `0x00041d78` set to 0 and
//! the words `0x004b8344` and `0x004075e0` zeroed.
//!
//! The composite's fan ([`composite`], `h2 disasm 0x276d0 160`,
//! `0x27960 35`): the setup writes the clip far `[0x00485ae0]` to the
//! depth word `0x00509400`, selects the view's target `[0x004858b8]`
//! (`0x00014bc0`, AL 0), binds target 3 at stage 0 (`0x00014f60`; clamp,
//! point, no mip), sets blend on, `GL_CONSTANT_COLOR` over
//! `GL_ONE_MINUS_SRC_ALPHA`, add, constant `0x00ffffff`, every channel
//! written, alpha test, cull, stencil and depth test off, then the block
//! [`composite_combiner`] and the pass-through vertex program, AL 1. The
//! callback gives attribute 0 the rectangle's lerp at the depth word
//! (`0x000350e0`) and attribute 1 the lerp itself, z 0, w 1.

use crate::light_constants::{Error, Sse};
use crate::memory::{GuestMemory, MemFault};
use crate::screen_grid;

/// `0x00025dc0`.
pub const FOG_FANS: u32 = 0x0002_5dc0;
/// The fan every draw goes through.
pub const FAN: u32 = 0x0003_4420;

/// The fans' setups (handed to the fan in EAX) and attribute callbacks.
pub mod setup {
    /// The fog's four layers into target `0x13`.
    pub const LAYERS: u32 = 0x0002_6880;
    pub const LAYERS_CORNERS: u32 = 0x0002_6ca0;
    /// Target `0x13` over the view, after [`LAYERS`].
    pub const LAYERS_COMPOSITE: u32 = 0x0002_6e50;
    pub const LAYERS_COMPOSITE_CORNERS: u32 = 0x0002_7520;
    /// Target 3 over the view (`dl` alone).
    pub const COMPOSITE: u32 = 0x0002_76d0;
    pub const COMPOSITE_CORNERS: u32 = 0x0002_7960;
}

/// Guest globals and image constants `0x00025dc0` reads.
pub mod global {
    /// The fog's tag datum: -1 or 0, no fans.
    pub const DATUM: u32 = 0x0048_57b8;
    /// The fog's amount (`f32`), drawn only above [`ZERO`].
    pub const AMOUNT: u32 = 0x0048_57d8;
    /// 0.0.
    pub const ZERO: u32 = 0x0045_dbd8;
    /// Where the flags are kept for the setups (`cl`, `dl`).
    pub const FLAG_CL: u32 = 0x0050_93fd;
    pub const FLAG_DL: u32 = 0x0050_93fe;
}

/// The fan's frame as a setup sees it (`0x00034420` calls the setup
/// through EDI at `0x0003452c`): the setup's return, then `SETUP_FRAME`
/// bytes up the fan's return, the rectangle's address (the fan's first
/// argument, which it points at its own rectangle) and the callback.
pub const SETUP_RETURN: u32 = 0x0003_452e;
pub const SETUP_FRAME: u32 = 0x64;
/// `0x00025dc0`'s returns from its two fan calls.
pub const LAYERS_RETURN: u32 = 0x0002_5e04;
pub const SECOND_RETURN: u32 = 0x0002_5e2d;

/// The composite's globals and image constants.
pub mod composite_global {
    /// The view's target (`i16`).
    pub const VIEW_TARGET: u32 = 0x0048_58b8;
    /// The clip far the setup writes to [`DEPTH`].
    pub const CLIP_FAR: u32 = 0x0048_5ae0;
    /// The depth word the callback reads.
    pub const DEPTH: u32 = 0x0050_9400;
    /// The fog's colour (three `f32`), packed by `0x00131ed0`.
    pub const COLOUR: u32 = 0x0048_57bc;
    /// `0x00131ed0`'s scale (255.0).
    pub const SCALE: u32 = 0x0045_dc38;
}

/// The target the composite samples at stage 0.
pub const COMPOSITE_SOURCE: u8 = 3;

/// Words of a combiner block.
pub const WORDS: usize = 60;

/// The composite's block (`0x000278ce`..`0x00027917`): stage 0's
/// texture times the colour, the colour the final combiner's constant 0.
pub fn composite_combiner(colour: u32) -> [u32; WORDS] {
    let mut w = [0; WORDS];
    w[0] = 0x0820_0000; // alpha inputs, stage 0
    w[8] = 0x1c01_0000; // final combiner ABCD
    w[9] = 0x1c00; // final combiner EFG
    w[26] = 0xc0; // alpha outputs, stage 0
    w[43] = colour; // final combiner constant 0
    w[53] = 0x0001_1001; // combiner count
    w[54] = 1; // texture modes: stage 0 2D
    w
}

/// The composite's fan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Composite {
    /// The view's target the fan draws into (`[0x004858b8]`).
    pub target: i16,
    /// `0x00131ed0` of the fog's colour.
    pub colour: u32,
    pub combiner: [u32; WORDS],
    /// The fan's rectangle: left, right, top, bottom (f32 bits).
    pub rect: [u32; 4],
    /// Each vertex's attribute 0 and attribute 1 (x, y, z, w, f32 bits),
    /// in push order.
    pub positions: [[u32; 4]; 4],
    pub coordinates: [[u32; 4]; 4],
}

/// The fan's corners in the order it pushes them.
pub const CORNER_UV: [[bool; 2]; 4] = [[false, false], [false, true], [true, true], [true, false]];

/// The rectangle's address when the setup whose stack is at `esp` is the
/// fan's of `0x00025dc0` returning to `fan_return` with `callback`.
pub fn fan_call(mem: &dyn GuestMemory, esp: u32, fan_return: u32, callback: u32) -> Option<u32> {
    let fan = esp.wrapping_add(SETUP_FRAME);
    let word = |a: u32| mem.u32(a).ok();
    (word(esp)? == SETUP_RETURN && word(fan)? == fan_return && word(fan.wrapping_add(8))? == callback)
        .then(|| word(fan.wrapping_add(4)))
        .flatten()
}

/// What the composite's setup writes that a later read sees: the clip far
/// to the depth word, which the callback reads.
pub fn composite_writes(mem: &mut dyn GuestMemory) -> Result<(), MemFault> {
    let far = mem.u32(composite_global::CLIP_FAR)?;
    mem.set_u32(composite_global::DEPTH, far)
}

/// The composite's fan whose rectangle is at `rect_at`, under the call's
/// MXCSR and x87 control word. The setup has just written the clip far to
/// the depth word, so the callback's depth is the clip far.
pub fn composite(mem: &dyn GuestMemory, mxcsr: u32, x87_control: u16, rect_at: u32) -> Result<Composite, Error> {
    use composite_global as g;
    let target = mem.u16(g::VIEW_TARGET)? as i16;
    let lane = |i: u32| mem.u32(g::COLOUR + 4 * i);
    let colour = crate::glow::pack_rgb([lane(0)?, lane(1)?, lane(2)?], mem.u32(g::SCALE)?, x87_control)?;
    let depth = mem.u32(g::CLIP_FAR)?;
    let mut rect = [0; 4];
    for (i, r) in rect.iter_mut().enumerate() {
        *r = mem.u32(rect_at.wrapping_add(4 * i as u32))?;
    }
    let one = mem.u32(screen_grid::constant::ONE)?;
    let mut sse = Sse { mxcsr };
    let (mut positions, mut coordinates) = ([[0; 4]; 4], [[0; 4]; 4]);
    for (n, [u, v]) in CORNER_UV.into_iter().enumerate() {
        let uv = [if u { one } else { 0 }, if v { one } else { 0 }];
        positions[n] = screen_grid::position(mem, &mut sse, &rect, uv, depth)?;
        let [left, right, top, bottom] = rect;
        let x = sse.sub(right, left)?;
        let x = sse.mul(x, uv[0])?;
        let x = sse.add(x, left)?;
        let y = sse.sub(bottom, top)?;
        let y = sse.mul(y, uv[1])?;
        let y = sse.add(y, top)?;
        coordinates[n] = [x, y, 0, one];
    }
    Ok(Composite { target, colour, combiner: composite_combiner(colour), rect, positions, coordinates })
}

/// What `0x00025dc0` draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fans {
    /// No fan: the gate stopped it, or neither flag is set.
    None,
    /// `cl`: the layers into target `0x13`, then their composite.
    Layers,
    /// `dl` alone: target 3's composite.
    Composite,
}

impl Fans {
    /// The cause an unported draw panics with.
    pub const fn cause(self) -> &'static str {
        match self {
            Fans::None => "none",
            Fans::Layers => "layers (0x00026880 into target 0x13, then 0x00026e50)",
            Fans::Composite => "composite (0x000276d0, target 3)",
        }
    }
}

/// The gate: the datum neither -1 nor 0 and the amount above 0, under
/// the call's MXCSR.
pub fn gate(mem: &dyn GuestMemory, mxcsr: u32) -> Result<bool, Error> {
    let datum = mem.u32(global::DATUM)?;
    if datum == u32::MAX || datum == 0 {
        return Ok(false);
    }
    Sse { mxcsr }.above(mem.u32(global::AMOUNT)?, mem.u32(global::ZERO)?)
}

/// The fans `0x00025dc0` draws for the flag bytes `cl` and `dl`.
pub fn fans(mem: &dyn GuestMemory, mxcsr: u32, cl: u8, dl: u8) -> Result<Fans, Error> {
    if !gate(mem, mxcsr)? {
        return Ok(Fans::None);
    }
    Ok(match (cl, dl) {
        (0, 0) => Fans::None,
        (0, _) => Fans::Composite,
        _ => Fans::Layers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::FlatMemory;

    const MXCSR: u32 = 0x1f80;

    fn mem() -> FlatMemory {
        let mut m = FlatMemory::new(0x0040_0000, 0x0012_0000);
        m.set_u32(global::DATUM, 0xe1a3_0012).unwrap();
        m.set_f32(global::AMOUNT, 0.5).unwrap();
        m.set_f32(global::ZERO, 0.0).unwrap();
        m
    }

    #[test]
    fn cl_draws_the_layers_and_dl_alone_the_composite() {
        let m = mem();
        assert_eq!(fans(&m, MXCSR, 1, 0).unwrap(), Fans::Layers);
        assert_eq!(fans(&m, MXCSR, 1, 1).unwrap(), Fans::Layers);
        assert_eq!(fans(&m, MXCSR, 0, 1).unwrap(), Fans::Composite);
        assert_eq!(fans(&m, MXCSR, 0, 0).unwrap(), Fans::None);
    }

    #[test]
    fn no_datum_or_no_amount_draws_nothing() {
        for datum in [0, u32::MAX] {
            let mut m = mem();
            m.set_u32(global::DATUM, datum).unwrap();
            assert_eq!(fans(&m, MXCSR, 1, 1).unwrap(), Fans::None, "{datum:#x}");
        }
        for amount in [0.0f32, -1.0, f32::NAN] {
            let mut m = mem();
            m.set_f32(global::AMOUNT, amount).unwrap();
            assert_eq!(fans(&m, MXCSR, 1, 1).unwrap(), Fans::None, "{amount}");
        }
    }

    fn composite_mem() -> (FlatMemory, u32, u32) {
        let (esp, rect) = (0x0050_0000, 0x0050_1000);
        let mut m = mem();
        let f = |m: &mut FlatMemory, a: u32, v: f32| m.set_f32(a, v).unwrap();
        f(&mut m, screen_grid::constant::ONE, 1.0);
        f(&mut m, screen_grid::constant::DEPTH, 16_777_215.0);
        f(&mut m, screen_grid::constant::NEAR, 0.0625);
        f(&mut m, screen_grid::constant::FAR, 1024.0);
        f(&mut m, composite_global::SCALE, 255.0);
        f(&mut m, composite_global::CLIP_FAR, 1024.0);
        for (i, c) in [1.0f32, 0.5, 0.25].into_iter().enumerate() {
            f(&mut m, composite_global::COLOUR + 4 * i as u32, c);
        }
        for (i, v) in [0.0f32, 640.0, 0.0, 480.0].into_iter().enumerate() {
            f(&mut m, rect + 4 * i as u32, v);
        }
        m.set_u32(esp, SETUP_RETURN).unwrap();
        m.set_u32(esp + SETUP_FRAME, SECOND_RETURN).unwrap();
        m.set_u32(esp + SETUP_FRAME + 4, rect).unwrap();
        m.set_u32(esp + SETUP_FRAME + 8, setup::COMPOSITE_CORNERS).unwrap();
        (m, esp, rect)
    }

    #[test]
    fn the_composites_fan_is_found_by_its_frame() {
        let (mut m, esp, rect) = composite_mem();
        assert_eq!(fan_call(&m, esp, SECOND_RETURN, setup::COMPOSITE_CORNERS), Some(rect));
        assert_eq!(fan_call(&m, esp, SECOND_RETURN, setup::LAYERS_COMPOSITE_CORNERS), None);
        assert_eq!(fan_call(&m, esp, LAYERS_RETURN, setup::COMPOSITE_CORNERS), None);
        m.set_u32(esp, 0x0003_4530).unwrap();
        assert_eq!(fan_call(&m, esp, SECOND_RETURN, setup::COMPOSITE_CORNERS), None);
    }

    #[test]
    fn the_composite_is_the_rectangle_with_the_colours_block() {
        let (mut m, _, rect) = composite_mem();
        m.set_u16(composite_global::VIEW_TARGET, 0).unwrap();
        let c = composite(&m, MXCSR, 0x027f, rect).unwrap();
        assert_eq!(c.target, 0);
        // 255, 127.5 (to even: 128), 63.75 (64), red high.
        assert_eq!(c.colour, 0x00ff_8040);
        assert_eq!(c.combiner[43], c.colour);
        assert_eq!(c.combiner[53], 0x0001_1001);
        assert_eq!(c.combiner[54], 1);
        let f = |v: [u32; 4]| v.map(f32::from_bits);
        assert_eq!(f(c.coordinates[0]), [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(f(c.coordinates[1]), [0.0, 480.0, 0.0, 1.0]);
        assert_eq!(f(c.coordinates[2]), [640.0, 480.0, 0.0, 1.0]);
        assert_eq!(f(c.coordinates[3]), [640.0, 0.0, 0.0, 1.0]);
        for n in 0..4 {
            assert_eq!(c.positions[n][..2], c.coordinates[n][..2], "{n}");
        }
        assert!(c.positions.iter().all(|p| p[2] == c.positions[0][2] && p[3] == c.positions[0][3]));
    }

    #[test]
    fn the_composites_writes_are_the_depth_word() {
        let (mut m, _, _) = composite_mem();
        composite_writes(&mut m).unwrap();
        assert_eq!(m.u32(composite_global::DEPTH).unwrap(), 1024.0f32.to_bits());
    }

    #[test]
    fn a_denormal_amount_draws() {
        // The Pentium III has no DAZ (`simd`), so the smallest denormal is
        // above 0.
        let mut m = mem();
        m.set_u32(global::AMOUNT, 1).unwrap();
        assert_eq!(fans(&m, MXCSR, 1, 0).unwrap(), Fans::Layers);
        assert_eq!(fans(&m, MXCSR | 0x0040, 1, 0).unwrap(), Fans::Layers);
    }
}
