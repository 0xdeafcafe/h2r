//! Which passes a light draws: the per-light flags `render_light_passes`
//! (`0x00031170`) computes after the light's constants, and the
//! implementation each lit material's layer-12 and layer-13 passes take
//! under them (docs/graphics/native-lights-constants.md,
//! "Implementations").
//!
//! The path in the original:
//!
//! - `0x00031170` calls `0x0004a1e0` (`light_constants`), then sets the
//!   flags `[0x004ba014]` and the byte `[0x004ba018]` (`0x000311c8`..
//!   `0x0003128e`, [`pass_flags`]), and may zero colour B first.
//! - Each layer draw builds its entries; structure and model geometry's
//!   build function `0x0004cbf0` stores the implementation in the entry's
//!   byte `+4`, and the draw `0x0004d0b0` hands that byte to the shader draw
//!   begin `0x0001bbf0` (its fourth stack argument), which binds
//!   implementation `n` of the pass through `0x00016b90` (stride `0x132`).
//! - For layers 12 and 13 the byte is `0x00015d70`'s answer: -1 (no entry)
//!   when the material's template has no pass in the layer, otherwise
//!   `0x00030da0` for layer 12 ([`diffuse`]) and `0x00030e00` for layer 13
//!   ([`specular`]), both of the flags, the light definition's first byte
//!   and the shader's integer input 4 (its word `+0x3e`, non-zero or not).
//!
//! Nothing here writes memory but [`PassFlags::write`]; `pass_flags` reads
//! the threshold it compares colour B with from the image at its address
//! ([`constant::SPECULAR`]), so none is copied here.
//!
//! Tests: `engine/tests/light_passes_differential.rs` runs the flag code
//! and both choosers, lifted, on the IR interpreter and compares memory and
//! answers; `engine/tests/light_passes.rs` checks invented lights.

use crate::light_constants::{Error, LightGlobals, Sse};
use guest_layout::memory::{GuestMemory, MemFault};

/// `render_light_passes`: EAX the light datum, ECX the projection volume, stack
/// `(datum, frame, shadow, weight)`, `ret 0x10`.
pub const RENDER_LIGHT_PASSES: u32 = 0x0003_1170;
/// The return address of its `0x0004a1e0` call: the flag code starts here,
/// with ESI the definition, EDI the context and BL the shadow byte.
pub const FLAGS_START: u32 = 0x0003_11c8;
/// The flag code's last instruction (`je` on flag bit 2): its two
/// successors, the shadow setup and the layers.
pub const FLAGS_END: u32 = 0x0003_1293;
pub const FLAGS_EXITS: [u32; 2] = [0x0003_1295, 0x0003_12bc];

/// Structure and model geometry's build function: the entry's byte `+4` is
/// the implementation.
pub const BUILD: u32 = 0x0004_cbf0;
/// The implementation chooser by layer (EDX the shader, EBX the layer,
/// stack `(variant byte, object, record flags)`, `ret 0xc`).
pub const CHOOSE: u32 = 0x0001_5d70;
/// Layer 12's chooser, stdcall `(int input set, 1)`, `ret 8`.
pub const CHOOSE_DIFFUSE: u32 = 0x0003_0da0;
pub const CHOOSE_DIFFUSE_CLEANUP: u32 = 8;
/// Layer 13's chooser, stdcall `(int input set)`, `ret 4`.
pub const CHOOSE_SPECULAR: u32 = 0x0003_0e00;
pub const CHOOSE_SPECULAR_CLEANUP: u32 = 4;

pub const DIFFUSE_LAYER: u32 = 12;
pub const SPECULAR_LAYER: u32 = 13;

pub mod global {
    /// The per-light flags ([`super::flag`]).
    pub const PASS_FLAGS: u32 = 0x004b_a014;
    /// Byte, 1 while a light's passes run.
    pub const PASSES: u32 = 0x004b_a018;
    /// A pointer; with its byte `+5` set colour B is kept.
    pub const OVERRIDE: u32 = 0x0051_0c50;
    /// Scene flags: bit 10 lets a light cast its shadow.
    pub const SCENE_FLAGS: u32 = 0x004c_1a80;
    /// The light definition `0x0004a1e0` stored (`light_constants`).
    pub const DEFINITION: u32 = crate::light_constants::global::DEFINITION;
}

pub mod constant {
    /// Colour B's components are compared with this (`ja`): any above it
    /// sets [`super::flag::SPECULAR`].
    pub const SPECULAR: u32 = 0x0045_dc7c;
}

/// Bits of `[0x004ba014]`.
pub mod flag {
    /// The effective volume is spherical (shape code 0 at `0x0003121d`).
    pub const CONTEXT_ZERO: u32 = 1;
    /// A colour B component above the threshold: layer 13 draws (with
    /// [`CONTEXT_ZERO`]).
    pub const SPECULAR: u32 = 2;
    /// The light casts its shadow: layer 10, and the shadow setup.
    pub const SHADOW: u32 = 4;
    /// With [`SHADOW`]: falloff word 4 above zero and definition word
    /// `+0x14` not 2.
    pub const SHADOW_FALLOFF: u32 = 8;
    /// Always cleared here (layer 10 into target 10 when set).
    pub const TARGET_10: u32 = 0x10;
    /// Set with [`SHADOW`].
    pub const SHADOW_B: u32 = 0x20;
}

/// The light definition's first byte, bit 5: layer 12 takes implementation
/// 4 (and layer 11 draws). The same bit picks the special frame in
/// `0x000497a0` (with `0x40000`).
pub const DEFINITION_BIT_5: u8 = 0x20;

/// The shadow word `0x00031170` hands `0x0004a1e0`: the caller's, with its
/// low byte kept only while scene flag bit 10 is set (`mov [esp+1Ch], bl`
/// writes the byte alone). Its low byte is the shadow byte [`pass_flags`]
/// takes.
pub fn shadow_argument(arg: u32, scene_flags: u32) -> u32 {
    if arg & 0xff == 0 {
        arg
    } else {
        (arg & !0xff) | ((scene_flags >> 10) & 1)
    }
}

/// What the flag code stores.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PassFlags {
    /// `[0x004ba014]`.
    pub flags: u32,
    /// Colour B as the passes see it: the block's, or zeros.
    pub colour_b: [u32; 3],
    /// Whether the code zeroed colour B (and so stores it).
    pub colour_b_zeroed: bool,
}

impl PassFlags {
    /// Stores the flags, the byte at `0x004ba018`, and colour B when the
    /// code zeroed it.
    pub fn write(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        m.write(global::PASSES, &[1])?;
        if self.colour_b_zeroed {
            m.write(crate::light_constants::global::COLOUR_B, &[0; 12])?;
        }
        m.write(global::PASS_FLAGS, &self.flags.to_le_bytes())
    }

    /// Layer 13 draws (`0x0003145b`..`0x00031468`).
    pub fn specular_layer(&self) -> bool {
        self.flags & (flag::CONTEXT_ZERO | flag::SPECULAR) == flag::CONTEXT_ZERO | flag::SPECULAR
    }

    /// The shadow setup and layer 10 run.
    pub fn shadowed(&self) -> bool {
        self.flags & flag::SHADOW != 0
    }
}

/// `0x000311c8`..`0x0003128e`: the per-light flags, from the light's
/// globals block (`light_constants::light_globals`, already computed for
/// this light), the shadow byte (the low byte of [`shadow_argument`]) and
/// the caller's MXCSR. Reads the override pointer, the definition's words
/// `+4` and `+0x14` and the threshold from memory.
pub fn pass_flags(m: &dyn GuestMemory, g: &LightGlobals, shadow: u8, mxcsr: u32) -> Result<PassFlags, Error> {
    let inputs = FlagInputs { definition: g.definition, context_word: g.context_word, colour_b: g.colour_b, falloff_4: g.falloff[4] };
    flags_of(m, &inputs, shadow, mxcsr)
}

/// What the flag code reads besides the override and the definition's
/// words: the definition's address (ESI), the context's first word
/// (`[edi]`), colour B (`[0x00485950]`..) and falloff word 4
/// (`[0x00485a40]`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlagInputs {
    pub definition: u32,
    pub context_word: u16,
    pub colour_b: [u32; 3],
    pub falloff_4: u32,
}

impl FlagInputs {
    /// As the flag code reads them after the constants call has run: the
    /// definition at `definition`, the context at `context` (`0x0003121d`),
    /// colour B and the falloff word from the globals block.
    pub fn read(m: &dyn GuestMemory, definition: u32, context: u32) -> Result<Self, MemFault> {
        use crate::light_constants::global::{COLOUR_B, FALLOFF};
        Ok(Self {
            definition,
            context_word: m.u16(context)?,
            colour_b: [m.u32(COLOUR_B)?, m.u32(COLOUR_B + 4)?, m.u32(COLOUR_B + 8)?],
            falloff_4: m.u32(FALLOFF + 16)?,
        })
    }
}

/// [`pass_flags`] of the flag code's own reads ([`FlagInputs`]).
pub fn flags_of(m: &dyn GuestMemory, i: &FlagInputs, shadow: u8, mxcsr: u32) -> Result<PassFlags, Error> {
    let mut sse = Sse { mxcsr };
    let zero = 0u32;

    let over = m.u32(global::OVERRIDE)?;
    let kept = over != 0 && m.u8(over.wrapping_add(5))? != 0;
    let colour_b_zeroed = !kept && m.u16(i.definition.wrapping_add(4))? == 0;
    let colour_b = if colour_b_zeroed { [zero; 3] } else { i.colour_b };

    let mut flags = (i.context_word == 0) as u32;
    let threshold = m.u32(constant::SPECULAR)?;
    let mut above = false;
    for c in colour_b {
        if sse.above(c, threshold)? {
            above = true;
            break;
        }
    }
    if above {
        flags |= flag::SPECULAR;
    } else {
        flags &= !flag::SPECULAR;
    }

    if shadow != 0 && flags & flag::CONTEXT_ZERO == 0 {
        flags |= flag::SHADOW | flag::SHADOW_B;
        let falloff = i.falloff_4;
        if sse.above(falloff, zero)? && m.u16(i.definition.wrapping_add(0x14))? != 2 {
            flags |= flag::SHADOW_FALLOFF;
        } else {
            flags &= !flag::SHADOW_FALLOFF;
        }
    } else {
        flags &= !(flag::SHADOW | flag::SHADOW_FALLOFF | flag::SHADOW_B);
    }
    flags &= !flag::TARGET_10;

    Ok(PassFlags {
        flags,
        colour_b,
        colour_b_zeroed,
    })
}

/// `0x00030da0` as `0x00015d70` calls it (second argument 1): layer 12's
/// implementation, from the flags, the light definition's first byte and
/// whether the shader's integer input 4 is non-zero.
pub fn diffuse(flags: u32, definition_byte: u8, int_input: bool) -> u32 {
    if flags & flag::CONTEXT_ZERO != 0 {
        1
    } else if definition_byte & DEFINITION_BIT_5 != 0 {
        4
    } else if flags & flag::SPECULAR != 0 && int_input {
        0
    } else if flags & flag::SHADOW != 0 {
        2
    } else {
        3
    }
}

/// `0x00030e00`: layer 13's implementation, `None` for no entry (the
/// original's -1).
pub fn specular(flags: u32, int_input: bool) -> Option<u32> {
    let both = flag::CONTEXT_ZERO | flag::SPECULAR;
    (flags & both == both && int_input).then_some(0)
}

/// Layer 12's or 13's implementation for a material whose template has
/// passes in that layer (`0x00015d70`'s gate, the template's pass count,
/// is the caller's); `None` for no entry or another layer.
pub fn implementation(layer: u32, flags: u32, definition_byte: u8, int_input: u16) -> Option<u32> {
    match layer {
        DIFFUSE_LAYER => Some(diffuse(flags, definition_byte, int_input != 0)),
        SPECULAR_LAYER => specular(flags, int_input != 0),
        _ => None,
    }
}

/// The light definition's dword `+0x7c`: its gel bitmap's datum.
pub const DEFINITION_GEL: u32 = 0x7c;
/// The definition's dword `+0xe0`: the datum of a shader drawn into render
/// target 13 as the gel (-1: none).
pub const DEFINITION_GEL_SHADER: u32 = 0xe0;
/// Definition flag: engine texture 10 is texture 18 instead.
pub const DEFINITION_TEXTURE_18: u32 = 0x0008_0000;

/// The bitmap engine texture 10 binds during a light's passes
/// (`0x00019530`'s case for id 10, `0x00019553`..`0x000196c7`): with the
/// context's first word non-zero (`[0x004858c8]`, which `0x0004a1e0`
/// copies; flag bit 0 clear), the definition's flag `0x80000` clear and no
/// gel shader (`+0xe0` -1; `[0x00485a77]`, set only while that shader
/// draws, is clear), image 0 of the bitmap at `+0x7c` through
/// `0x000144f0`. `None` for the other cases (texture 18, render target 13,
/// the context-zero case at `0x000196d3`) and for no bitmap (-1: the
/// rasterizer's default). S: `0x000144f0` binds the default too when the
/// image's word `+0xa` is not 0; the host decodes the image as 2D.
pub fn gel(flags: u32, definition_flags: u32, gel_shader: u32, gel: u32) -> Option<u32> {
    let other = flags & flag::CONTEXT_ZERO != 0 || definition_flags & DEFINITION_TEXTURE_18 != 0 || gel_shader != u32::MAX;
    (!other && gel != u32::MAX).then_some(gel)
}
