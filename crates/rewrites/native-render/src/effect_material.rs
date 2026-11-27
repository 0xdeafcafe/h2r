//! The shape the particle and contrail materials share
//! ([`crate::particle_material`], [`crate::contrail_material`]): what a
//! guest material setup leaves on the device, as data, and [`compare`]
//! against one draw's own, field by field. The bitmap image and the
//! vertex program are named by their tags here; [`texture_header`] and
//! [`program`] resolve them from guest memory as the guest's own binders
//! do, for the game to compare at a draw.

/// The tag instance table pointer (16-byte slots: group, datum, data).
pub const TAG_TABLE: u32 = 0x004e_3b44;
/// The rasterizer data (`[0x00485a80]`): `+0x5c` the engine vertex shader
/// table (8-byte elements, the `vrtx` datum at `+4`), `+0x64` the default
/// bitmap's datum.
pub const RASTERIZER: u32 = 0x0048_5a80;
/// The stamp `0x00012310` compares an image's `+0x70` with: a later stamp
/// is resident and binds its own texture (`+0x50`).
pub const RESIDENT_STAMP: u32 = 0x004e_6488;
/// Words of a `D3DPIXELSHADERDEF`.
pub const WORDS: usize = 60;
/// The hardware constant slot (register + 96) the setups write their
/// 3x4 matrix at (`0x003f7330` with ECX `0xae`).
pub const MATRIX_SLOT: usize = 0xae;

/// What one stage binds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Texture {
    /// `0x000143c0`'s bitmap datum (`u32::MAX`: the rasterizer default)
    /// and image.
    Bitmap { bitmap: u32, image: i32 },
    /// No texture (engine source 0).
    Empty,
}

/// A material: what the setup leaves on the device.
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    /// Render states `(index, value)` in the order the setup sets them
    /// (`raw.render[index]`).
    pub states: Vec<(u8, u32)>,
    /// Texture stage states `(stage, index, value)` in set order
    /// (`raw.stages[stage][index]`).
    pub stages: Vec<(u8, u8, u32)>,
    /// The combiner definition.
    pub combiner: [u32; WORDS],
    /// Bit `n` set: word `n` is driven per draw, not compared.
    pub combiner_driven: u64,
    /// Per stage, `None` where the setup leaves the stage as it was.
    pub textures: [Option<Texture>; 4],
    /// Vertex constants `(hardware slot, value)`, whole rows, in the
    /// order the setup writes them.
    pub constants: Vec<(usize, [f32; 4])>,
    /// The vertex program: the `vrtx` datum and the program's index.
    pub program: (u32, u32),
}

/// A draw's own, as the device snapshot holds it.
pub struct Observed<'a> {
    pub render: &'a [u32],
    pub stages: [&'a [u32]; 4],
    pub combiner: Option<&'a [u32; WORDS]>,
    /// Header addresses of the bound textures.
    pub textures: [Option<u32>; 4],
    /// Constants by hardware slot (register + 96, the device's order).
    pub constants: &'a [[f32; 4]],
    /// The program as the tag holds it (`'x'`, `0x20`, the instruction
    /// count, 16-byte instructions); `None` for fixed function.
    pub program: Option<Vec<u8>>,
}

/// The material's textures and program read from guest memory.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub textures: [Option<Option<u32>>; 4],
    pub program: Vec<u8>,
}

/// Why a material's textures or program do not resolve (a refusal of
/// the sample, `<kind>_material_refused_<name>`).
pub type Unresolved = &'static str;

/// Resolves `m`'s stage textures ([`texture_header`]) and program
/// ([`program`]).
pub fn resolve(
    m: &Material,
    word: &dyn Fn(u32) -> Option<u32>,
    bytes: &dyn Fn(u32, usize) -> Option<Vec<u8>>,
) -> Result<Resolved, Unresolved> {
    let mut textures = [None; 4];
    for (out, t) in textures.iter_mut().zip(&m.textures) {
        *out = match t {
            None => None,
            Some(Texture::Empty) => Some(None),
            Some(Texture::Bitmap { bitmap, image }) => Some(Some(texture_header(word, *bitmap, *image)?)),
        };
    }
    let program = program(word, bytes, m.program.0, m.program.1)?;
    Ok(Resolved { textures, program })
}

fn tag_data(word: &dyn Fn(u32) -> Option<u32>, datum: u32) -> Option<u32> {
    word(word(TAG_TABLE)?.checked_add((datum & 0xffff) * 16 + 8)?)
}

/// The texture header `0x000143c0` binds for `bitmap`'s `image`: the
/// rasterizer default (`+0x64`, image 0) for datum -1 or an image out of
/// range, the image at `+0x48 + 0x74 * image` (`0x00137550`), and its
/// texture `+0x50` when resident (`0x00012310`). A streaming image binds
/// what the loader returns (`texture_streaming`); a bitmap without images
/// binds nothing (`texture_unbound`).
pub fn texture_header(word: &dyn Fn(u32) -> Option<u32>, bitmap: u32, image: i32) -> Result<u32, Unresolved> {
    let unread = "texture_unread";
    let rasterizer = word(RASTERIZER).ok_or(unread)?;
    let default = || word(rasterizer.checked_add(0x64)?);
    let (mut datum, mut image) = (bitmap, image);
    if datum == u32::MAX {
        datum = default().ok_or(unread)?;
        image = 0;
        if datum == u32::MAX {
            return Err("texture_unbound");
        }
    }
    let mut tag = tag_data(word, datum).ok_or(unread)?;
    let count = |tag: u32| word(tag.checked_add(0x44)?).map(|c| c as i32);
    if image < 0 || image >= count(tag).ok_or(unread)? {
        datum = default().ok_or(unread)?;
        image = 0;
        tag = tag_data(word, datum).ok_or(unread)?;
    }
    if tag == 0 || count(tag).ok_or(unread)? <= 0 || image >= count(tag).ok_or(unread)? {
        return Err("texture_unbound");
    }
    let at = word(tag + 0x48).ok_or(unread)?.wrapping_add(image as u32 * 0x74);
    let stamp = word(at.wrapping_add(0x70)).ok_or(unread)? as i32;
    let header = word(at.wrapping_add(0x50)).ok_or(unread)?;
    if stamp <= word(RESIDENT_STAMP).ok_or(unread)? as i32 || header == 0 {
        return Err("texture_streaming");
    }
    Ok(header)
}

/// The datum of engine vertex shader `slot` (`[[0x00485a80] + 0x5c] + 8 *
/// slot + 4`).
pub fn engine_vertex_shader(word: &dyn Fn(u32) -> Option<u32>, slot: u32) -> Option<u32> {
    let table = word(word(RASTERIZER)?.checked_add(0x5c)?)?;
    word(table.checked_add(slot * 8 + 4)?)
}

/// Program `index` of `vrtx` tag `datum` as `0x0001c2e0` selects it: the
/// block at `+8` (`0x1c` bytes an element), its bytes at `+0x10`, their
/// count at `+0xc`. Datum -1 (the built-in table at `0x004670ec`) is
/// `vertex_builtin`.
pub fn program(
    word: &dyn Fn(u32) -> Option<u32>,
    bytes: &dyn Fn(u32, usize) -> Option<Vec<u8>>,
    datum: u32,
    index: u32,
) -> Result<Vec<u8>, Unresolved> {
    let unread = "vertex_program_unread";
    if datum == u32::MAX {
        return Err("vertex_builtin");
    }
    let slot = word(TAG_TABLE).ok_or(unread)?.checked_add((datum & 0xffff) * 16).ok_or(unread)?;
    if word(slot).ok_or(unread)? != u32::from_be_bytes(*b"vrtx") || word(slot + 4).ok_or(unread)? != datum {
        return Err("vertex_not_vrtx");
    }
    let tag = word(slot + 8).ok_or(unread)?;
    if index >= word(tag.checked_add(4).ok_or(unread)?).ok_or(unread)? {
        return Err("vertex_program_index");
    }
    let e = word(tag + 8).ok_or(unread)?.checked_add(index * 0x1c).ok_or(unread)?;
    let (size, at) = (word(e + 0xc).ok_or(unread)?, word(e + 0x10).ok_or(unread)?);
    if size == 0 || size > 0x1_0000 {
        return Err("vertex_program_size");
    }
    bytes(at, size as usize).ok_or(unread)
}

/// The final value of each index set in `writes`, in first-set order.
pub fn last_values<K: PartialEq + Copy, V>(writes: impl Iterator<Item = (K, V)>) -> Vec<(K, V)> {
    let mut out: Vec<(K, V)> = Vec::new();
    for (k, v) in writes {
        match out.iter_mut().find(|(o, _)| *o == k) {
            Some(slot) => slot.1 = v,
            None => out.push((k, v)),
        }
    }
    out
}

/// Where a draw differs from the material: `render_<index>`,
/// `stage<n>_<index>`, `combiner_w<n>` (`combiner_missing` for a fixed
/// function draw), `texture<n>`, `constant<register>`, `vertex_program`
/// (`vertex_program_missing`). A state set twice is compared at its last
/// value.
pub fn compare(m: &Material, r: &Resolved, d: &Observed<'_>) -> Vec<String> {
    let mut out = Vec::new();
    for (index, value) in last_values(m.states.iter().copied()) {
        if d.render.get(usize::from(index)) != Some(&value) {
            out.push(format!("render_{index:#x}"));
        }
    }
    for ((stage, index), value) in last_values(m.stages.iter().map(|&(s, i, v)| ((s, i), v))) {
        if d.stages[usize::from(stage) & 3].get(usize::from(index)) != Some(&value) {
            out.push(format!("stage{stage}_{index:#x}"));
        }
    }
    match d.combiner {
        None => out.push("combiner_missing".to_owned()),
        Some(words) => {
            for (n, (a, b)) in m.combiner.iter().zip(words.iter()).enumerate() {
                if m.combiner_driven & (1 << n) == 0 && a != b {
                    out.push(format!("combiner_w{n}"));
                }
            }
        }
    }
    for (n, (want, have)) in r.textures.iter().zip(d.textures).enumerate() {
        if want.is_some_and(|want| want != have) {
            out.push(format!("texture{n}"));
        }
    }
    for (slot, value) in last_values(m.constants.iter().copied()) {
        if d.constants.get(slot) != Some(&value) {
            out.push(format!("constant{slot}"));
        }
    }
    match &d.program {
        None => out.push("vertex_program_missing".to_owned()),
        Some(code) if *code != r.program => out.push("vertex_program".to_owned()),
        Some(_) => {}
    }
    out
}

/// `D3D__RenderState` (guest numbering) and `D3D__TextureState` (32
/// words a stage): what a draw takes for every slot a material does not
/// set.
pub const RENDER_STATE: u32 = 0x0040_7498;
pub const RENDER_WORDS: usize = 166;
pub const TEXTURE_STATE: u32 = 0x0040_4dd0;
pub const STAGE_WORDS: usize = 32;
/// Render slots 0..56 are the combiner definition's first words, as
/// `SetPixelShader` (`0x003fa3d0`) copies them (`rep movsd` at
/// `0x003fa4d5`).
pub const COMBINER_SLOTS: usize = 57;

/// The guest's state arrays at a setup's call: what the setup starts from.
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub render: Vec<u32>,
    pub stages: [Vec<u32>; 4],
}

impl State {
    pub fn read(word: &dyn Fn(u32) -> Option<u32>) -> Option<Self> {
        let words = |base: u32, n: usize| (0..n as u32).map(|i| word(base + 4 * i)).collect::<Option<Vec<_>>>();
        let stage = |s: u32| words(TEXTURE_STATE + s * 4 * STAGE_WORDS as u32, STAGE_WORDS);
        Some(Self { render: words(RENDER_STATE, RENDER_WORDS)?, stages: [stage(0)?, stage(1)?, stage(2)?, stage(3)?] })
    }

    /// The state a draw from `m` takes, as the device's supplied draw
    /// builds it: the combiner's words in slots 0..56, then the material's
    /// render and stage words, over `self`.
    pub fn with(&self, m: &Material) -> Self {
        let mut out = self.clone();
        out.render[..COMBINER_SLOTS].copy_from_slice(&m.combiner[..COMBINER_SLOTS]);
        for &(slot, value) in &m.states {
            if let Some(w) = out.render.get_mut(usize::from(slot)) {
                *w = value;
            }
        }
        for &(stage, slot, value) in &m.stages {
            if let Some(w) = out.stages[usize::from(stage) & 3].get_mut(usize::from(slot)) {
                *w = value;
            }
        }
        out
    }
}

/// Where a draw's whole render and stage state differs from what a draw
/// from `m` over `before` (the arrays at its setup's call) takes:
/// `state_render_<n>`, `state_stage<s>_<n>`. A combiner word the draw
/// drives is not compared. Any setup write the material does not carry
/// (a restore of the implementation before, a state its tag does not
/// list) is named here.
pub fn compare_state(m: &Material, before: &State, render: &[u32], stages: [&[u32]; 4]) -> Vec<String> {
    let want = before.with(m);
    let mut out = Vec::new();
    for (n, (a, b)) in want.render.iter().zip(render).enumerate() {
        if a != b && !(n < COMBINER_SLOTS && m.combiner_driven & (1 << n) != 0) {
            out.push(format!("state_render_{n}"));
        }
    }
    for (s, (want, have)) in want.stages.iter().zip(stages).enumerate() {
        for (n, (a, b)) in want.iter().zip(have).enumerate() {
            if a != b {
                out.push(format!("state_stage{s}_{n}"));
            }
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Guest words for the resolvers.
    #[derive(Default)]
    pub(crate) struct Words(pub HashMap<u32, u32>);
    impl Words {
        pub(crate) fn set(&mut self, at: u32, v: u32) {
            self.0.insert(at, v);
        }
        pub(crate) fn word(&self, at: u32) -> Option<u32> {
            self.0.get(&at).copied()
        }
    }

    /// A tag table at 0x1000 with bitmap 2 (two images at 0x3000, the
    /// second streaming), default bitmap 3 (one image at 0x4000) and
    /// `vrtx` 5 (two programs).
    pub(crate) fn guest() -> Words {
        let mut w = Words::default();
        w.set(TAG_TABLE, 0x1000);
        w.set(RASTERIZER, 0x8000);
        w.set(0x8000 + 0x5c, 0x9000);
        w.set(0x9000 + 4 * 8 + 4, 0xe000_0005);
        w.set(0x8000 + 0x64, 0xe000_0003);
        w.set(RESIDENT_STAMP, 10);
        for (datum, data) in [(2u32, 0x2000u32), (3, 0x2100)] {
            w.set(0x1000 + datum * 16, u32::from_be_bytes(*b"bitm"));
            w.set(0x1000 + datum * 16 + 8, data);
        }
        w.set(0x2000 + 0x44, 2);
        w.set(0x2000 + 0x48, 0x3000);
        w.set(0x3000 + 0x70, 11);
        w.set(0x3000 + 0x50, 0xb000);
        w.set(0x3074 + 0x70, 10);
        w.set(0x3074 + 0x50, 0xb100);
        w.set(0x2100 + 0x44, 1);
        w.set(0x2100 + 0x48, 0x4000);
        w.set(0x4000 + 0x70, 12);
        w.set(0x4000 + 0x50, 0xc000);
        w.set(0x1000 + 5 * 16, u32::from_be_bytes(*b"vrtx"));
        w.set(0x1000 + 5 * 16 + 4, 0xe000_0005);
        w.set(0x1000 + 5 * 16 + 8, 0x5000);
        w.set(0x5000 + 4, 2);
        w.set(0x5000 + 8, 0x6000);
        w.set(0x6000 + 0x1c + 0xc, 8);
        w.set(0x6000 + 0x1c + 0x10, 0x7000);
        w
    }

    #[test]
    fn a_resident_image_binds_its_texture_and_others_fall_back_as_the_binder_does() {
        let w = guest();
        let word = |a| w.word(a);
        assert_eq!(texture_header(&word, 0xe000_0002, 0), Ok(0xb000));
        assert_eq!(texture_header(&word, 0xe000_0002, 1), Err("texture_streaming"));
        // Out of range and datum -1 both bind the default bitmap's image 0.
        assert_eq!(texture_header(&word, 0xe000_0002, 2), Ok(0xc000));
        assert_eq!(texture_header(&word, u32::MAX, 5), Ok(0xc000));
    }

    #[test]
    fn a_program_is_the_vrtx_element_at_its_index() {
        let w = guest();
        let word = |a| w.word(a);
        let bytes = |a: u32, n: usize| (a == 0x7000).then(|| vec![0x78; n]);
        assert_eq!(program(&word, &bytes, 0xe000_0005, 1), Ok(vec![0x78; 8]));
        assert_eq!(program(&word, &bytes, 0xe000_0005, 2), Err("vertex_program_index"));
        assert_eq!(program(&word, &bytes, u32::MAX, 0), Err("vertex_builtin"));
        assert_eq!(program(&word, &bytes, 0xe000_0002, 0), Err("vertex_not_vrtx"));
        assert_eq!(engine_vertex_shader(&word, 4), Some(0xe000_0005));
    }

    #[test]
    fn a_draw_that_differs_is_named_by_field_and_a_state_set_twice_by_its_last_value() {
        let m = Material {
            states: vec![(0x3b, 1), (0x39, 0x203), (0x3b, 0)],
            stages: vec![(0, 0, 3)],
            combiner: [0; WORDS],
            combiner_driven: 1 << 10,
            textures: [Some(Texture::Bitmap { bitmap: 2, image: 0 }), None, None, None],
            constants: vec![(174, [1.0, 0.0, 0.0, 0.0])],
            program: (5, 0),
        };
        let r = Resolved { textures: [Some(Some(0xb000)), None, None, None], program: vec![1, 2] };
        let mut render = vec![0u32; 0xa6];
        render[0x39] = 0x203;
        let stage0 = [3u32; 32];
        let other = [0u32; 32];
        let mut constants = vec![[0.0f32; 4]; 192];
        constants[174] = [1.0, 0.0, 0.0, 0.0];
        let mut combiner = [0u32; WORDS];
        combiner[10] = 7;
        let d = Observed {
            render: &render,
            stages: [&stage0, &other, &other, &other],
            combiner: Some(&combiner),
            textures: [Some(0xb000), Some(1), None, None],
            constants: &constants,
            program: Some(vec![1, 2]),
        };
        assert!(compare(&m, &r, &d).is_empty());
        render[0x3b] = 1;
        combiner[11] = 1;
        let d = Observed {
            render: &render,
            stages: [&stage0, &other, &other, &other],
            combiner: Some(&combiner),
            textures: [Some(0xb100), None, None, None],
            constants: &constants,
            program: None,
        };
        assert_eq!(compare(&m, &r, &d), ["render_0x3b", "combiner_w11", "texture0", "vertex_program_missing"]);
    }

    #[test]
    fn a_setup_write_the_material_does_not_carry_is_named_by_its_slot() {
        let mut combiner = [0u32; WORDS];
        combiner[3] = 9;
        combiner[10] = 4;
        let m = Material {
            states: vec![(0x3b, 1)],
            stages: vec![(1, 2, 5)],
            combiner,
            combiner_driven: 1 << 10,
            textures: [None; 4],
            constants: Vec::new(),
            program: (5, 0),
        };
        let before = State { render: vec![7; RENDER_WORDS], stages: std::array::from_fn(|_| vec![0; STAGE_WORDS]) };
        let after = before.with(&m);
        assert_eq!((after.render[3], after.render[0x3b], after.render[60], after.stages[1][2]), (9, 1, 7, 5));
        let stages = |s: &State| -> [Vec<u32>; 4] { s.stages.clone() };
        let st = stages(&after);
        let view = [&st[0][..], &st[1][..], &st[2][..], &st[3][..]];
        assert!(compare_state(&m, &before, &after.render, view).is_empty());
        let mut render = after.render.clone();
        render[10] = 0; // driven: not compared
        render[0x40] = 0; // a restore the material does not carry
        assert_eq!(compare_state(&m, &before, &render, view), ["state_render_64"]);
    }
}
