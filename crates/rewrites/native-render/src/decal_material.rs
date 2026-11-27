//! The decal material in Rust: what the guest's state wrapper `0x00043a40`
//! sets for each of the seven modes (docs/graphics/native-decals-research.md,
//! "Effective render state"), as data. A render state is a call of
//! `0x000222d0` (`edi` the value, `esi` the index into the device's render
//! words, so `raw.render[index]` of a draw snapshot); the combiner is the
//! 60-word pixel shader definition the wrapper builds at `0x00484f68` and
//! hands to `0x0001ccf0`. [`compare`] checks a draw's own against these,
//! field by field, for the game to prove the port.
//!
//! Not here: the vertex program (engine vertex slot 3, `0x0001cc30`) and the
//! 192 constants, which come from the view and the tags.

use crate::decal::Mode;

/// Render state indices the wrapper sets (`0x000222d0`'s `esi`).
pub mod state {
    pub const ZFUNC: u8 = 0x39;
    pub const ZWRITE: u8 = 0x40;
    pub const BLEND_ENABLE: u8 = 0x3b;
    pub const SRC_BLEND: u8 = 0x3e;
    pub const DEST_BLEND: u8 = 0x3f;
    pub const BLEND_OP: u8 = 0x4a;
    pub const SOLID_OFFSET: u8 = 0x51;
    pub const OFFSET_CONSTANT: u8 = 0x4e;
    pub const OFFSET_SLOPE: u8 = 0x4d;
}

/// The engine vertex shader slot the pass selects (`0x00043fe8`).
pub const VERTEX_SLOT: u32 = 3;
/// The float the wrapper loads as the polygon offset constant for modes 0
/// to 5 (`[0x0045e48c]`).
pub const OFFSET_CONSTANT_ADDRESS: u32 = 0x0045_e48c;
/// Words of the combiner definition.
pub const WORDS: usize = 60;

/// One mode's material.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Material {
    pub mode: Mode,
    /// `(index, value)` in the order the wrapper sets them.
    pub states: Vec<(u8, u32)>,
    /// The combiner definition.
    pub combiner: [u32; WORDS],
}

/// A block built by the wrapper: words addressed as the guest does
/// (`0x00484f68 + 4 * n`).
struct Block([u32; WORDS]);

impl Block {
    fn set(&mut self, address: u32, value: u32) {
        self.0[((address - 0x0048_4f68) / 4) as usize] = value;
    }
}

const TEXTURE_MODES: u32 = 0x0048_5040;
const COMBINER_COUNT: u32 = 0x0048_503c;
const FINAL_CONSTANT0: u32 = 0x0048_5014;
const ALPHA_INPUT0: u32 = 0x0048_4f68;
const ALPHA_INPUT1: u32 = 0x0048_4f6c;
const FINAL_ABCD: u32 = 0x0048_4f88;
const FINAL_EFG: u32 = 0x0048_4f8c;
const CONSTANT0: u32 = 0x0048_4f90;
const ALPHA_OUTPUT0: u32 = 0x0048_4fd0;
const ALPHA_OUTPUT1: u32 = 0x0048_4fd4;
const RGB_INPUT0: u32 = 0x0048_4ff0;
const RGB_INPUT1: u32 = 0x0048_4ff4;
const RGB_OUTPUT0: u32 = 0x0048_501c;
const RGB_OUTPUT1: u32 = 0x0048_5020;

/// `0x00043a40`'s material for `mode`; `offset` is the bits of
/// `[0x0045e48c]`.
pub fn material(mode: Mode, offset: u32) -> Material {
    use state::*;
    let mut b = Block([0; WORDS]);
    let mut states = Vec::new();
    match mode {
        Mode::Opaque => {
            states.extend([(ZFUNC, 0x207), (ZWRITE, 1)]);
            b.set(TEXTURE_MODES, 0);
            b.set(COMBINER_COUNT, 0x11001);
            b.set(FINAL_CONSTANT0, 0x00ff_0000);
            b.set(FINAL_ABCD, 1);
            b.set(FINAL_EFG, 0);
            states.extend([(BLEND_ENABLE, 0), (SOLID_OFFSET, 0)]);
            return Material { mode, states, combiner: b.0 };
        }
        Mode::Alpha0 | Mode::Alpha1 => {
            one_stage(&mut b);
            b.set(FINAL_ABCD, 0x0f15_0000);
            b.set(FINAL_EFG, 0x0c1c_1c00);
            states.extend([(BLEND_ENABLE, 1), (SRC_BLEND, 1), (DEST_BLEND, 0x303), (BLEND_OP, 0x8006)]);
        }
        Mode::Multiply | Mode::Darken => {
            let multiply = mode == Mode::Multiply;
            let rgb = (if multiply { 0x80 } else { 0 } + 0x20) << 8 | 0x081c_003c;
            b.set(RGB_INPUT1, rgb);
            b.set(TEXTURE_MODES, 1);
            b.set(COMBINER_COUNT, 0x11002);
            b.set(ALPHA_INPUT0, 0x1814_0000);
            b.set(ALPHA_OUTPUT0, 0xc0);
            b.set(RGB_OUTPUT1, 0xc00);
            b.set(FINAL_ABCD, 0xc);
            states.extend([
                (BLEND_ENABLE, 1),
                (SRC_BLEND, 0x306),
                (DEST_BLEND, if multiply { 0x300 } else { 0 }),
                (BLEND_OP, 0x8006),
            ]);
        }
        Mode::Maximum => {
            one_stage(&mut b);
            b.set(FINAL_ABCD, 0x0c1c_0000);
            states.extend([(BLEND_ENABLE, 1), (SRC_BLEND, 1), (DEST_BLEND, 1), (BLEND_OP, 0x8008)]);
        }
        Mode::Alpha5 => {
            b.set(TEXTURE_MODES, 1);
            b.set(COMBINER_COUNT, 0x11002);
            b.set(CONSTANT0, 0xff);
            b.set(RGB_INPUT0, 0x0804_0501);
            b.set(RGB_OUTPUT0, 0xcd);
            b.set(ALPHA_INPUT0, 0x3814_0000);
            b.set(ALPHA_OUTPUT0, 0xc0);
            b.set(RGB_INPUT1, 0x182b_200b);
            b.set(RGB_OUTPUT1, 0xd00);
            b.set(ALPHA_INPUT1, 0x1c0d_0000);
            b.set(ALPHA_OUTPUT1, 0xc0);
            b.set(FINAL_ABCD, 0x0f14_0000);
            b.set(FINAL_EFG, 0x0c0d_1c00);
            states.extend([(BLEND_ENABLE, 1), (SRC_BLEND, 1), (DEST_BLEND, 0x303), (BLEND_OP, 0x8006)]);
        }
    }
    states.extend([(SOLID_OFFSET, 1), (OFFSET_CONSTANT, offset), (OFFSET_SLOPE, 0)]);
    Material { mode, states, combiner: b.0 }
}

/// The one combiner stage of modes 0, 1 and 4: texture times diffuse.
fn one_stage(b: &mut Block) {
    b.set(TEXTURE_MODES, 1);
    b.set(COMBINER_COUNT, 0x11001);
    b.set(RGB_INPUT0, 0x0804_0000);
    b.set(RGB_OUTPUT0, 0xc0);
    b.set(ALPHA_INPUT0, 0x1814_0000);
    b.set(ALPHA_OUTPUT0, 0xc0);
}

/// The rasterizer data (`[0x00485a80]`), whose `+0x5c` is the table of
/// engine vertex shaders (8-byte elements, the `vrtx` datum at `+4`).
pub const RASTERIZER: u32 = 0x0048_5a80;
/// The tag instance table pointer (16-byte slots: group, datum, data).
pub const TAG_TABLE: u32 = 0x004e_3b44;

/// The slot's datum and every program of its `vrtx` tag, each as the
/// bytes the guest hands the device (`'x'`, `0x20`, the instruction
/// count, 16-byte instructions): what `0x0001cc30(3)` selects among by
/// the declaration's class. `None` when a table does not read or the
/// tag is not the `vrtx` the datum names.
pub fn vertex_programs(
    word: &dyn Fn(u32) -> Option<u32>,
    bytes: &dyn Fn(u32, usize) -> Option<Vec<u8>>,
) -> Option<(u32, Vec<Vec<u8>>)> {
    let table = word(word(RASTERIZER)?.checked_add(0x5c)?)?;
    let datum = word(table.checked_add(VERTEX_SLOT * 8 + 4)?)?;
    let slot = word(TAG_TABLE)?.checked_add((datum & 0xffff) * 16)?;
    if word(slot)? != u32::from_be_bytes(*b"vrtx") || word(slot + 4)? != datum {
        return None;
    }
    let tag = word(slot + 8)?;
    let (count, block) = (word(tag + 4)?, word(tag + 8)?);
    let mut out = Vec::new();
    for i in 0..count.min(64) {
        let e = block.checked_add(i * 0x1c)?;
        let (size, ptr) = (word(e + 0xc)?, word(e + 0x10)?);
        if size != 0 {
            out.push(bytes(ptr, size as usize)?);
        }
    }
    Some((datum, out))
}

/// Whether `code` (a draw's program with its four header bytes) is one of
/// `programs`.
pub fn program_matches(programs: &[Vec<u8>], code: &[u8]) -> bool {
    programs.iter().any(|p| p == code)
}

/// Where a draw differs from the material: `combiner_w<n>` for a
/// definition word, `render_<index>` for a render word.
pub fn compare(m: &Material, combiner: &[u32; WORDS], render: &[u32]) -> Vec<String> {
    let mut out = Vec::new();
    for (n, (a, b)) in m.combiner.iter().zip(combiner).enumerate() {
        if a != b {
            out.push(format!("combiner_w{n}"));
        }
    }
    for &(index, value) in &m.states {
        if render.get(usize::from(index)) != Some(&value) {
            out.push(format!("render_{index:#x}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_mode_sets_the_wrappers_states_in_order() {
        let get = |m| material(Mode::from_guest(m).unwrap(), 0x3f80_0000).states;
        assert_eq!(get(6), [(0x39, 0x207), (0x40, 1), (0x3b, 0), (0x51, 0)]);
        let tail = [(0x51, 1), (0x4e, 0x3f80_0000), (0x4d, 0)];
        assert_eq!(get(0)[..4], [(0x3b, 1), (0x3e, 1), (0x3f, 0x303), (0x4a, 0x8006)]);
        assert_eq!(get(0)[4..], tail);
        assert_eq!(get(1), get(0));
        assert_eq!(get(5), get(0));
        assert_eq!(get(2)[..4], [(0x3b, 1), (0x3e, 0x306), (0x3f, 0x300), (0x4a, 0x8006)]);
        assert_eq!(get(3)[2], (0x3f, 0));
        assert_eq!(get(4)[..4], [(0x3b, 1), (0x3e, 1), (0x3f, 1), (0x4a, 0x8008)]);
        assert_eq!(get(4)[4..], tail);
    }

    #[test]
    fn each_mode_builds_the_wrappers_combiner() {
        let c = |m| material(Mode::from_guest(m).unwrap(), 0).combiner;
        // Words by definition index: 0 alpha input, 8 final abcd, 34 rgb
        // input, 53 count, 54 texture modes.
        assert_eq!((c(6)[53], c(6)[54], c(6)[43], c(6)[8]), (0x11001, 0, 0x00ff_0000, 1));
        assert_eq!((c(0)[0], c(0)[8], c(0)[9], c(0)[34], c(0)[45], c(0)[26]), (0x1814_0000, 0x0f15_0000, 0x0c1c_1c00, 0x0804_0000, 0xc0, 0xc0));
        assert_eq!(c(0), c(1));
        assert_eq!((c(2)[35], c(3)[35]), (0x081c_a03c, 0x081c_203c));
        assert_eq!((c(2)[53], c(2)[46], c(2)[8]), (0x11002, 0xc00, 0xc));
        assert_eq!((c(4)[8], c(4)[53]), (0x0c1c_0000, 0x11001));
        assert_eq!((c(5)[10], c(5)[34], c(5)[35], c(5)[45], c(5)[27], c(5)[1], c(5)[0]), (0xff, 0x0804_0501, 0x182b_200b, 0xcd, 0xc0, 0x1c0d_0000, 0x3814_0000));
    }

    #[test]
    fn the_slot_three_programs_come_from_the_vrtx_tag() {
        use std::collections::HashMap;
        let mut w: HashMap<u32, u32> = HashMap::new();
        let prog = vec![0x78, 0x20, 1, 0, 9, 9, 9, 9, 8, 8, 8, 8, 7, 7, 7, 7, 6, 6, 6, 6];
        w.insert(RASTERIZER, 0x1000);
        w.insert(0x1000 + 0x5c, 0x2000);
        w.insert(0x2000 + 3 * 8 + 4, 0xe300_0002);
        w.insert(TAG_TABLE, 0x3000);
        let slot = 0x3000 + 2 * 16;
        w.insert(slot, u32::from_be_bytes(*b"vrtx"));
        w.insert(slot + 4, 0xe300_0002);
        w.insert(slot + 8, 0x4000);
        w.insert(0x4000 + 4, 2);
        w.insert(0x4000 + 8, 0x5000);
        w.insert(0x5000 + 0xc, prog.len() as u32);
        w.insert(0x5000 + 0x10, 0x6000);
        // The second class is empty.
        w.insert(0x5000 + 0x1c + 0xc, 0);
        w.insert(0x5000 + 0x1c + 0x10, 0);
        let word = |a: u32| w.get(&a).copied();
        let bytes = |a: u32, n: usize| (a == 0x6000 && n == prog.len()).then(|| prog.clone());
        let (datum, programs) = vertex_programs(&word, &bytes).unwrap();
        assert_eq!((datum, programs.len()), (0xe300_0002, 1));
        assert!(program_matches(&programs, &prog));
        assert!(!program_matches(&programs, &prog[..8]));
        w.insert(slot, 0);
        assert!(vertex_programs(&|a| w.get(&a).copied(), &bytes).is_none());
    }

    #[test]
    fn a_draw_that_differs_is_named_by_field() {
        let m = material(Mode::Darken, 7);
        let mut render = vec![0u32; 0x60];
        for &(i, v) in &m.states {
            render[usize::from(i)] = v;
        }
        assert!(compare(&m, &m.combiner, &render).is_empty());
        let mut combiner = m.combiner;
        combiner[53] = 0;
        render[0x4a] = 0x8007;
        assert_eq!(compare(&m, &combiner, &render), ["combiner_w53", "render_0x4a"]);
        assert_eq!(compare(&m, &m.combiner, &render[..0x10]).len(), m.states.len());
    }
}
