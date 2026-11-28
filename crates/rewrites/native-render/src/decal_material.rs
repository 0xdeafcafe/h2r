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

/// The constants a vertex program reads: the registers it names directly
/// and whether it indexes any relatively (`c[a0.x + n]`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reads {
    pub registers: std::collections::BTreeSet<u8>,
    pub relative: bool,
}

/// What the Rust side gives the view's matrix block: the open view's
/// viewport rectangle (left, top, width, height) and the rows of its
/// view-projection in register order, as bits, the relation the carried
/// draws' camera classification proves (`d3d` `scene_sink/camera.rs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct View {
    pub rect: [u32; 4],
    pub rows: [[u32; 4]; 4],
}

impl View {
    pub fn from_camera(cam: &crate::camera::RenderCamera) -> Option<Self> {
        let [top, left, bottom, right] = cam.viewport;
        if top < 0 || left < 0 || !cam.is_usable() {
            return None;
        }
        let m = cam.view_projection();
        m.iter().flatten().all(|v| v.is_finite()).then(|| Self {
            rect: [left as u32, top as u32, (right - left) as u32, (bottom - top) as u32],
            rows: std::array::from_fn(|row| m.map(|column| column[row].to_bits())),
        })
    }
}

/// Where a draw's constants differ from what Rust gives, of the registers
/// its program reads (an unread one cannot change a pixel): `constant_c<n>`
/// for a matrix row that differs, `constant_c<n>_scaled` when it is the
/// expected row times one factor, `constant_c<n>_ungiven` for a read of a
/// register nothing in Rust sets yet, `constant_relative`,
/// `constant_view_unread` and `constant_viewport` for the view itself.
/// `base` is the first of the four matrix rows the program's position
/// uses.
pub fn compare_constants(
    reads: &Reads,
    base: Option<u8>,
    view: Option<&View>,
    viewport: [u32; 4],
    constants: &[[f32; 4]],
) -> Vec<String> {
    let mut out = Vec::new();
    if reads.relative {
        out.push("constant_relative".to_owned());
    }
    let mut matrix = false;
    for &r in &reads.registers {
        let row = base.and_then(|b| r.checked_sub(b)).filter(|&i| i < 4);
        let Some(i) = row else {
            out.push(format!("constant_c{r}_ungiven"));
            continue;
        };
        matrix = true;
        let (Some(view), Some(actual)) = (view, constants.get(usize::from(r))) else {
            if !out.contains(&"constant_view_unread".to_owned()) {
                out.push("constant_view_unread".to_owned());
            }
            continue;
        };
        let want = view.rows[usize::from(i)];
        if actual.map(f32::to_bits) != want {
            let want = want.map(f32::from_bits);
            let ratio = (0..4)
                .find(|&k| want[k] != 0.0 && actual[k].is_finite())
                .map(|k| actual[k] / want[k]);
            let scaled = ratio.is_some_and(|q| {
                q.is_finite() && q != 0.0 && (0..4).all(|k| (actual[k] - want[k] * q).abs() <= 1e-4 * actual[k].abs())
            });
            out.push(format!("constant_c{r}{}", if scaled { "_scaled" } else { "" }));
        }
    }
    if matrix && view.is_some_and(|v| v.rect != viewport) {
        out.push("constant_viewport".to_owned());
    }
    out
}

/// A tag's data: `[[TAG_TABLE] + 16 * index + 8]`.
fn tag_data(word: &dyn Fn(u32) -> Option<u32>, datum: u32) -> Option<u32> {
    word(word(TAG_TABLE)?.checked_add((datum & 0xffff) * 16 + 8)?)
}

/// The image `0x00137550` gives for image `index` of `bitmap`: 0 for a
/// tag that is not there or an index outside the tag's images.
fn image_at(word: &dyn Fn(u32) -> Option<u32>, bitmap: u32, index: i32) -> Option<u32> {
    let tag = tag_data(word, bitmap)?;
    let index = index as i16;
    if tag == 0 || index < 0 {
        return Some(0);
    }
    if i32::from(index) >= word(tag.checked_add(0x44)?)? as i32 {
        return Some(0);
    }
    Some(word(tag.checked_add(0x48)?)?.wrapping_add(index as u32 * 0x74))
}

/// The texture header the decal callback `0x00044040` binds at stage 0
/// for a record (`0x0004409c` to `0x000440ef`): the bitmap datum at
/// `+0x8c` of the record's decal tag (`+0x2c`); the image `selector`
/// (`+0x29`) modulo the bitmap's image count, where that image is 2D
/// (`0x000144f0`, its `+0xa`); else image 1 of the rasterizer's default
/// bitmap (`[[RASTERIZER] + 0x64]`); its header (`+0x50`) only while
/// resident (`0x00012310`).
///
/// `Err` names what the guest body would do that this does not:
/// `texture_streaming` (the loader runs), `texture_unbound` (nothing is
/// bound, the stage keeps the last texture), `texture_image` (an image
/// the guest reads at address 0), `texture_unread`.
pub fn decal_texture(word: &dyn Fn(u32) -> Option<u32>, decal: u32, selector: i8) -> Result<u32, &'static str> {
    const UNREAD: &str = "texture_unread";
    const BITMAP: u32 = 0x6c + 0x20;
    let tag = tag_data(word, decal).ok_or(UNREAD)?;
    let bitmap = word(tag.wrapping_add(BITMAP)).ok_or(UNREAD)?;
    let mut image = 0;
    if bitmap != u32::MAX {
        let images = word(tag_data(word, bitmap).ok_or(UNREAD)?.wrapping_add(0x44)).ok_or(UNREAD)? as i32;
        if images > 0 {
            // `idiv`: the remainder takes the selector's sign, as `%` does.
            let at = image_at(word, bitmap, i32::from(selector) % images).ok_or(UNREAD)?;
            if at == 0 {
                return Err("texture_image");
            }
            // The image's kind, the word at `+0xa`: 0 is 2D.
            if word(at.wrapping_add(8)).ok_or(UNREAD)? >> 16 == 0 {
                image = at;
            }
        }
    }
    if image == 0 {
        let default = word(word(RASTERIZER).ok_or(UNREAD)?.wrapping_add(0x64)).ok_or(UNREAD)?;
        if default == u32::MAX {
            return Err("texture_unbound");
        }
        image = image_at(word, default, 1).ok_or(UNREAD)?;
        if image == 0 {
            return Err("texture_unbound");
        }
    }
    let stamp = word(image.wrapping_add(0x70)).ok_or(UNREAD)? as i32;
    let header = word(image.wrapping_add(0x50)).ok_or(UNREAD)?;
    if stamp <= word(crate::effect_material::RESIDENT_STAMP).ok_or(UNREAD)? as i32 || header == 0 {
        return Err("texture_streaming");
    }
    Ok(header)
}

/// The vertex default `v10` the callback sets from a record's tint
/// (`+0x24`, a `D3DCOLOR`): `D3DDevice_SetVertexData4ub(10, r, g, b, a)`
/// at `0x00044132`, each byte over 255 as the device takes it.
pub fn tint(colour: u32) -> [f32; 4] {
    let [b, g, r, a] = colour.to_le_bytes();
    [r, g, b, a].map(|v| f32::from(v) / 255.0)
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

    #[test]
    fn constants_compare_by_the_registers_the_program_reads() {
        let rows = [[1.0f32, 0.0, 0.0, 0.0], [0.0, 2.0, 0.0, 0.0], [0.0, 0.0, 3.0, 4.0], [0.0, 0.0, 1.0, 0.0]];
        let view = View { rect: [0, 0, 640, 480], rows: rows.map(|r| r.map(f32::to_bits)) };
        let mut c = vec![[0.0f32; 4]; 192];
        c[..4].copy_from_slice(&rows);
        let reads = |regs: &[u8]| Reads { registers: regs.iter().copied().collect(), relative: false };
        let at = [0, 0, 640, 480];
        assert!(compare_constants(&reads(&[0, 1, 2, 3]), Some(0), Some(&view), at, &c).is_empty());
        // An unread register may be anything.
        c[9] = [5.0; 4];
        assert!(compare_constants(&reads(&[0, 3]), Some(0), Some(&view), at, &c).is_empty());
        // A differing row (a lane the row has at 0, so no one factor makes
        // it the expected one), a scaled one, another viewport.
        c[1][0] = 0.5;
        c[2] = rows[2].map(|v| v * 0.5);
        let got = compare_constants(&reads(&[0, 1, 2]), Some(0), Some(&view), [0, 0, 320, 480], &c);
        assert_eq!(got, ["constant_c1", "constant_c2_scaled", "constant_viewport"]);
        // A register nothing gives, a relative read, no view.
        let r = Reads { relative: true, ..reads(&[9, 2]) };
        assert_eq!(
            compare_constants(&r, Some(0), None, at, &c),
            ["constant_relative", "constant_view_unread", "constant_c9_ungiven"]
        );
    }

    /// Tags for the decal texture tests: decal tag 1 names bitmap 2 (three
    /// images, image 1 a cube map), the rasterizer's default is bitmap 3
    /// (two images). Every image is resident.
    fn texture_tags() -> std::collections::HashMap<u32, u32> {
        let mut w = std::collections::HashMap::new();
        w.insert(TAG_TABLE, 0x3000);
        w.insert(RASTERIZER, 0x1000);
        w.insert(0x1000 + 0x64, 0xe300_0003);
        w.insert(crate::effect_material::RESIDENT_STAMP, 10);
        for (index, data) in [(1, 0x4000), (2, 0x5000), (3, 0x6000)] {
            w.insert(0x3000 + 16 * index + 8, data);
        }
        w.insert(0x4000 + 0x8c, 0xe300_0002);
        for (tag, images, at) in [(0x5000, 3, 0x7000), (0x6000, 2, 0x8000)] {
            w.insert(tag + 0x44, images);
            w.insert(tag + 0x48, at);
            for i in 0..images {
                let image = at + 0x74 * i;
                w.insert(image + 8, 0);
                w.insert(image + 0x50, 0xa000 + image);
                w.insert(image + 0x70, 11);
            }
        }
        // Image 1 of bitmap 2 is a cube map.
        w.insert(0x7000 + 0x74 + 8, 1 << 16);
        w
    }

    #[test]
    fn a_decal_binds_its_image_modulo_the_count_else_the_default_image_1() {
        let mut w = texture_tags();
        let header = |w: &std::collections::HashMap<u32, u32>, selector: i8| {
            decal_texture(&|a| w.get(&a).copied(), 0xe300_0001, selector)
        };
        assert_eq!(header(&w, 0), Ok(0xa000 + 0x7000));
        assert_eq!(header(&w, 5), Ok(0xa000 + 0x7000 + 2 * 0x74));
        // Image 1 is not 2D: the default bitmap's image 1.
        assert_eq!(header(&w, 4), Ok(0xa000 + 0x8000 + 0x74));
        // A negative selector leaves a negative remainder: the guest reads
        // the kind of image address 0.
        assert_eq!(header(&w, -2), Err("texture_image"));
        // A decal without a bitmap binds the default's image 1.
        w.insert(0x4000 + 0x8c, u32::MAX);
        assert_eq!(header(&w, 0), Ok(0xa000 + 0x8000 + 0x74));
        // Not resident: the loader would run.
        w.insert(0x8000 + 0x74 + 0x70, 10);
        assert_eq!(header(&w, 0), Err("texture_streaming"));
        // No default bitmap: nothing is bound.
        w.insert(0x1000 + 0x64, u32::MAX);
        assert_eq!(header(&w, 0), Err("texture_unbound"));
    }

    #[test]
    fn the_tint_is_set_as_red_green_blue_alpha_over_255() {
        assert_eq!(tint(0x80ff_0033), [1.0, 0.0, 51.0 / 255.0, 128.0 / 255.0]);
    }
}
