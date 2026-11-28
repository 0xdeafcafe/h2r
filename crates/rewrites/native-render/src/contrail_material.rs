//! The contrail material in Rust: what the polygon handler `0x00035b00`'s
//! setup `0x00043720` leaves on the device before `0x0004b220` draws, as
//! data. The setup hands its entry's data (`e`, the handler's entry
//! `+0x1c`) to `0x00052d40(e[+0xc], e[+0x18], 0)` with ECX `e[+0x1c]`:
//! the contrail's material block, its bitmap and its image. The block's
//! `+0x24` is its type (only 1 sets anything), `+0x28` bit 1 the
//! two-stage combiner, `+0x2a` the blend mode, `+0x2e` the map flags.
//!
//! `0x00052d40`, in order: binds the bitmap at stage 0 (`0x000143c0`),
//! writes stage 0's states into `0x00404dd0`, sets cull, colour write,
//! blend, alpha test, stencil, depth and z bias, the blend mode
//! (`0x000142f0`), the stipple, selects engine vertex shader 4's program
//! 0 (`0x0001c590`), writes the identity at `c78..c80` and builds the
//! combiner at `0x00484f68` for `0x00015180`. The stipple pattern is not
//! a state word and is not compared.

use crate::effect_material::{MATRIX_SLOT, Material, Texture, WORDS};

/// The setup's entry, where the game reads [`Inputs`].
pub const SETUP: u32 = 0x0004_3720;
/// The material function the setup calls.
pub const MATERIAL: u32 = 0x0005_2d40;
/// The engine vertex shader slot the material selects (`[+0x5c] + 0x24`).
pub const VERTEX_SLOT: u32 = 4;

/// `0x000142f0`'s tables, by blend mode (12 entries each in the image).
pub const SRC_BLENDS_ADDRESS: u32 = 0x0045_0718;
pub const DEST_BLENDS_ADDRESS: u32 = 0x0045_06e8;
pub const BLEND_OPS_ADDRESS: u32 = 0x0045_06b8;
pub const SRC_BLENDS: [u32; 12] = [0x302, 0x306, 0x306, 1, 1, 1, 1, 1, 0x8001, 0x8002, u32::MAX, u32::MAX];
pub const DEST_BLENDS: [u32; 12] = [0x303, 0, 0x300, 1, 1, 1, 1, 0x303, 0x8002, 0x8001, u32::MAX, u32::MAX];
pub const BLEND_OPS: [u32; 12] =
    [0x8006, 0x8006, 0x8006, 0x8006, 0x800b, 0x8007, 0x8008, 0x8006, 0x8006, 0x8006, u32::MAX, u32::MAX];
/// The blend mode that turns blending off.
pub const OPAQUE_MODE: i16 = 10;

/// Render state indices the material sets.
pub mod state {
    pub const ZFUNC: u8 = 0x39;
    pub const BLEND_ENABLE: u8 = 0x3b;
    pub const ALPHA_TEST: u8 = 0x3c;
    pub const SRC_BLEND: u8 = 0x3e;
    pub const DEST_BLEND: u8 = 0x3f;
    pub const ZWRITE: u8 = 0x40;
    pub const COLOR_WRITE: u8 = 0x43;
    pub const BLEND_OP: u8 = 0x4a;
    pub const ZENABLE: u8 = 0x8f;
    pub const STENCIL_ENABLE: u8 = 0x90;
    pub const CULL: u8 = 0x93;
    pub const ZBIAS: u8 = 0x95;
}

/// What the material reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inputs {
    /// Block `+0x24`.
    pub kind: i16,
    /// Block `+0x28`.
    pub flags: u8,
    /// Block `+0x2a`.
    pub blend: i16,
    /// Block `+0x2e`.
    pub map_flags: u8,
    pub bitmap: u32,
    pub image: i16,
    /// The third argument (0 from the setup): bit 2 skips the blend
    /// mode's combiner stage, bit 5 takes the matrix from the view.
    pub options: u32,
}

/// Reads the setup's inputs from its entry data `e` (`[esp + 4]` at
/// `0x00043720`).
pub fn read(word: &dyn Fn(u32) -> Option<u32>, e: u32) -> Option<Inputs> {
    let block = word(e.checked_add(0xc)?)?;
    let at = |o: u32| word(block.checked_add(o)?);
    Some(Inputs {
        kind: at(0x24)? as u16 as i16,
        flags: at(0x28)? as u8,
        blend: at(0x2a)? as u16 as i16,
        map_flags: at(0x2e)? as u8,
        bitmap: word(e.checked_add(0x18)?)?,
        image: word(e.checked_add(0x1c)?)? as u16 as i16,
        options: 0,
    })
}

/// A branch the port does not model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The block's type is not 1: the material sets nothing, so the draw
    /// takes what the draws before it left.
    Kind,
    /// A blend mode past `0x000142f0`'s tables.
    BlendMode,
    /// Option bit 5: the matrix is the view's (`0x004856c4`...).
    ViewMatrix,
}

impl Refused {
    pub fn name(self) -> &'static str {
        match self {
            Self::Kind => "kind",
            Self::BlendMode => "blend_mode",
            Self::ViewMatrix => "view_matrix",
        }
    }
}

/// Combiner words, by the guest's address (`0x00484f68 + 4 * n`).
fn at(address: u32) -> usize {
    ((address - 0x0048_4f68) / 4) as usize
}
const ALPHA_INPUT: u32 = 0x0048_4f68;
const ALPHA_OUTPUT: u32 = 0x0048_4fd0;
const RGB_INPUT: u32 = 0x0048_4ff0;
const RGB_OUTPUT: u32 = 0x0048_501c;
const FINAL_ABCD: u32 = 0x0048_4f88;
const FINAL_EFG: u32 = 0x0048_4f8c;
const COMBINER_COUNT: u32 = 0x0048_503c;
const TEXTURE_MODES: u32 = 0x0048_5040;

/// `0x00052d40`'s material for `i`; `vertex_shader` is engine vertex
/// shader [`VERTEX_SLOT`]'s datum
/// ([`crate::effect_material::engine_vertex_shader`]).
pub fn material(i: &Inputs, vertex_shader: u32) -> Result<Material, Refused> {
    use state::*;
    if i.kind != 1 {
        return Err(Refused::Kind);
    }
    if !(0..12).contains(&i.blend) {
        return Err(Refused::BlendMode);
    }
    if i.options & 0x20 != 0 {
        return Err(Refused::ViewMatrix);
    }
    let f = u32::from(i.map_flags);
    let filter = if f & 1 != 0 { 1 } else { 2 };
    // 0x00052d89..0x00052e09, in the order written.
    let stages = [
        (0, (f & 2) | 1),
        (1, ((f & 4) | 2) >> 1),
        (3, filter),
        (4, filter),
        (8, 0),
        (6, 0),
        (7, 0),
        (10, 0),
        (11, 0),
        (5, filter),
    ]
    .map(|(index, value)| (0, index, value))
    .to_vec();
    let mut states = vec![
        (CULL, 0),
        (COLOR_WRITE, 0x0001_0101),
        (BLEND_ENABLE, 1),
        (ALPHA_TEST, 0),
        (STENCIL_ENABLE, 0),
        (ZENABLE, 2),
        (ZWRITE, 0),
        (ZFUNC, 0x203),
        (ZBIAS, 0),
    ];
    // 0x000142f0.
    let mode = i.blend as usize;
    states.push((BLEND_ENABLE, u32::from(i.blend != OPAQUE_MODE)));
    if i.blend != OPAQUE_MODE {
        states.extend([(SRC_BLEND, SRC_BLENDS[mode]), (DEST_BLEND, DEST_BLENDS[mode]), (BLEND_OP, BLEND_OPS[mode])]);
    }
    // 0x0005304f..0x00053177.
    let mut c = [0u32; WORDS];
    c[at(ALPHA_INPUT)] = 0x1820_2015;
    c[at(ALPHA_OUTPUT)] = 0xc4;
    c[at(FINAL_ABCD)] = 0xc;
    c[at(FINAL_EFG)] = 0x1c00;
    let mut stage = if (i.flags >> 1) & 1 != 0 {
        c[at(RGB_INPUT)] = 0x0808_0000;
        c[at(RGB_OUTPUT)] = 0xc0;
        c[at(RGB_INPUT) + 1] = 0x0c0c_0000;
        c[at(RGB_OUTPUT) + 1] = 0xc0;
        c[at(RGB_INPUT) + 2] = 0x250c_0508;
        c[at(RGB_OUTPUT) + 2] = 0xc00;
        3
    } else {
        c[at(RGB_INPUT)] = 0x0805_0000;
        c[at(RGB_OUTPUT)] = 0xc0;
        1
    };
    c[at(TEXTURE_MODES)] = 1;
    if i.options & 4 == 0 {
        // The jump table at 0x00053190; modes 8 to 11 add an empty stage.
        let (alpha, rgb) = match i.blend {
            0 => (Some(0x1c14_0000), None),
            1 | 5 => (None, Some(0x0c14_2034)),
            2 => (None, Some(0x0c14_a034)),
            3 | 4 | 6 => (None, Some(0x0c14_0000)),
            7 => (Some(0x1c14_0000), Some(0x0c14_0000)),
            _ => (None, None),
        };
        if let Some(word) = alpha {
            c[at(ALPHA_INPUT) + stage] = word;
            c[at(ALPHA_OUTPUT) + stage] = 0xc00;
        }
        if let Some(word) = rgb {
            c[at(RGB_INPUT) + stage] = word;
            c[at(RGB_OUTPUT) + stage] = 0xc00;
        }
        stage += 1;
    }
    c[at(COMBINER_COUNT)] = stage as u32;
    let identity = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];
    Ok(Material {
        states,
        stages,
        combiner: c,
        combiner_driven: 0,
        textures: [Some(Texture::Bitmap { bitmap: i.bitmap, image: i32::from(i.image) }), None, None, None],
        constants: identity.iter().enumerate().map(|(n, row)| (MATRIX_SLOT + n, *row)).collect(),
        defaults: Vec::new(),
        program: (vertex_shader, 0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn material_of(i: &Inputs) -> Result<Material, Refused> {
        material(i, 0xe000_0005)
    }

    fn inputs(blend: i16, flags: u8, map_flags: u8) -> Inputs {
        Inputs { kind: 1, flags, blend, map_flags, bitmap: 0xe000_0002, image: 0, options: 0 }
    }

    #[test]
    fn the_inputs_are_read_from_the_entry_data_and_its_block() {
        let mut w = crate::effect_material::tests::Words::default();
        w.set(0x100 + 0xc, 0x200);
        w.set(0x100 + 0x18, 0xe000_0002);
        w.set(0x100 + 0x1c, 0xffff_0003);
        w.set(0x224, 0x0042_0001);
        w.set(0x228, 0x0007_0002);
        w.set(0x22a, 0x0000_0007);
        w.set(0x22e, 0x05);
        let i = read(&|a| w.word(a), 0x100).unwrap();
        assert_eq!(i, Inputs { kind: 1, flags: 2, blend: 7, map_flags: 5, bitmap: 0xe000_0002, image: 3, options: 0 });
    }

    #[test]
    fn the_map_flags_pick_stage_zeros_addressing_and_filters() {
        let s = |f| material_of(&inputs(0, 0, f)).unwrap().stages;
        assert_eq!(s(0)[..4], [(0, 0, 1), (0, 1, 1), (0, 3, 2), (0, 4, 2)]);
        assert_eq!(s(7)[..4], [(0, 0, 3), (0, 1, 3), (0, 3, 1), (0, 4, 1)]);
        assert_eq!(s(7)[9], (0, 5, 1));
        assert_eq!(s(0)[4..9], [(0, 8, 0), (0, 6, 0), (0, 7, 0), (0, 10, 0), (0, 11, 0)]);
    }

    #[test]
    fn the_blend_mode_comes_from_the_images_tables_and_mode_ten_turns_blending_off() {
        let m = material_of(&inputs(2, 0, 0)).unwrap();
        assert_eq!(m.states[..3], [(0x93, 0), (0x43, 0x10101), (0x3b, 1)]);
        assert_eq!(m.states[9..], [(0x3b, 1), (0x3e, 0x306), (0x3f, 0x300), (0x4a, 0x8006)]);
        let opaque = material_of(&inputs(10, 0, 0)).unwrap();
        assert_eq!(opaque.states.last(), Some(&(0x3b, 0)));
        assert_eq!(opaque.states.len(), 10);
        assert_eq!(material_of(&inputs(12, 0, 0)), Err(Refused::BlendMode));
        assert_eq!(material_of(&Inputs { kind: 0, ..inputs(0, 0, 0) }), Err(Refused::Kind));
        assert_eq!(material_of(&Inputs { options: 0x20, ..inputs(0, 0, 0) }), Err(Refused::ViewMatrix));
    }

    #[test]
    fn the_combiner_has_one_or_three_stages_then_the_blend_modes() {
        let c = |blend, flags| material_of(&inputs(blend, flags, 0)).unwrap().combiner;
        // Words by definition index: 0 alpha input, 26 alpha output, 8/9
        // final, 34 rgb input, 45 rgb output, 53 count, 54 texture modes.
        let one = c(3, 0);
        assert_eq!((one[0], one[26], one[8], one[9], one[54]), (0x1820_2015, 0xc4, 0xc, 0x1c00, 1));
        assert_eq!((one[34], one[45], one[35], one[46], one[53]), (0x0805_0000, 0xc0, 0x0c14_0000, 0xc00, 2));
        let three = c(0, 2);
        assert_eq!((three[34], three[35], three[36], three[47]), (0x0808_0000, 0x0c0c_0000, 0x250c_0508, 0xc00));
        assert_eq!((three[3], three[29], three[37], three[53]), (0x1c14_0000, 0xc00, 0, 4));
        let seven = c(7, 0);
        assert_eq!((seven[1], seven[27], seven[35], seven[46]), (0x1c14_0000, 0xc00, 0x0c14_0000, 0xc00));
        assert_eq!((c(1, 0)[35], c(2, 0)[35]), (0x0c14_2034, 0x0c14_a034));
        // Modes past the jump table still count a stage, with no words.
        let nine = c(9, 0);
        assert_eq!((nine[35], nine[1], nine[53]), (0, 0, 2));
        let skipped = material_of(&Inputs { options: 4, ..inputs(0, 0, 0) }).unwrap().combiner;
        assert_eq!((skipped[1], skipped[53]), (0, 1));
    }

    #[test]
    fn the_texture_matrix_and_program_are_stage_zeros_the_identity_and_slot_fours() {
        let m = material_of(&Inputs { image: 3, ..inputs(0, 0, 0) }).unwrap();
        assert_eq!(m.textures[0], Some(Texture::Bitmap { bitmap: 0xe000_0002, image: 3 }));
        assert_eq!(m.constants, [(174, [1.0, 0.0, 0.0, 0.0]), (175, [0.0, 1.0, 0.0, 0.0]), (176, [0.0, 0.0, 1.0, 0.0])]);
        assert_eq!(m.program, (0xe000_0005, 0));
    }
}
