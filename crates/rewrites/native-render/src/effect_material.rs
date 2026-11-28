//! The shape the particle and contrail materials share
//! ([`crate::particle_material`], [`crate::contrail_material`]): what a
//! guest material setup leaves on the device, as data. The bitmap image
//! and the vertex program are named by their tags here; [`texture_header`]
//! and [`program`] resolve them from guest memory as the guest's own
//! binders do, for the device's supplied draw.

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
    /// No texture (engine source 0, or the cleared engine texture 4).
    Empty,
    /// An engine texture from the rasterizer data (`0x00019530`,
    /// `crate::screen_look::EngineTexture::Rasterizer`): the bitmap whose
    /// datum is at `[RASTERIZER] + offset`, and its image.
    Rasterizer { offset: u32, image: i32 },
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
    /// Bit `n` set: word `n` is driven per draw.
    pub combiner_driven: u64,
    /// Per stage, `None` where the setup leaves the stage as it was.
    pub textures: [Option<Texture>; 4],
    /// Vertex constants `(hardware slot, value)`, whole rows, in the
    /// order the setup writes them.
    pub constants: Vec<(usize, [f32; 4])>,
    /// Vertex attribute defaults `(register, value)` (`SET_VERTEX_DATA4F_M`,
    /// `0x00018e3b`): what a register the declaration leaves off reads.
    pub defaults: Vec<(u8, [f32; 4])>,
    /// The vertex program: the `vrtx` datum and the program's index.
    pub program: (u32, u32),
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
            Some(Texture::Rasterizer { offset, image }) => {
                let datum = word(word(RASTERIZER).ok_or(UNREAD)?.wrapping_add(*offset)).ok_or(UNREAD)?;
                Some(Some(texture_header(word, datum, *image)?))
            }
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
    let at = image_record(word, bitmap, image)?;
    let stamp = word(at.wrapping_add(0x70)).ok_or(UNREAD)? as i32;
    let header = word(at.wrapping_add(0x50)).ok_or(UNREAD)?;
    if stamp <= word(RESIDENT_STAMP).ok_or(UNREAD)? as i32 || header == 0 {
        return Err("texture_streaming");
    }
    Ok(header)
}

const UNREAD: &str = "texture_unread";

/// The image record `0x000143c0` hands `0x00012310` for `bitmap`'s
/// `image` (`0x00137550`), with the rasterizer default's fall-back.
fn image_record(word: &dyn Fn(u32) -> Option<u32>, bitmap: u32, image: i32) -> Result<u32, Unresolved> {
    let rasterizer = word(RASTERIZER).ok_or(UNREAD)?;
    let default = || word(rasterizer.checked_add(0x64)?);
    let (mut datum, mut image) = (bitmap, image);
    if datum == u32::MAX {
        datum = default().ok_or(UNREAD)?;
        image = 0;
        if datum == u32::MAX {
            return Err("texture_unbound");
        }
    }
    let mut tag = tag_data(word, datum).ok_or(UNREAD)?;
    let count = |tag: u32| word(tag.checked_add(0x44)?).map(|c| c as i32);
    if image < 0 || image >= count(tag).ok_or(UNREAD)? {
        datum = default().ok_or(UNREAD)?;
        image = 0;
        tag = tag_data(word, datum).ok_or(UNREAD)?;
    }
    if tag == 0 || count(tag).ok_or(UNREAD)? <= 0 || image >= count(tag).ok_or(UNREAD)? {
        return Err("texture_unbound");
    }
    Ok(word(tag + 0x48).ok_or(UNREAD)?.wrapping_add(image as u32 * 0x74))
}

/// What `0x000143c0` binds for `bitmap`'s `image`, as [`texture_header`]
/// finds the image, up to the texture cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cached {
    /// `0x00012310`'s fast arm: the header `+0x50`, non-zero, with a stamp
    /// `+0x70` past the cache's or not streamed (`+0x0f` bit 1 clear,
    /// `0x0012cd2c`).
    Header(u32),
    /// Otherwise the image record the guest's `0x00012310` is asked about
    /// (it may start the load, and returns the header it has).
    Streaming { image: u32 },
}

/// [`Cached`] for `bitmap`'s `image`; `Err` as [`texture_header`]'s
/// (`texture_unbound`, `texture_unread`).
pub fn texture_cached(word: &dyn Fn(u32) -> Option<u32>, bitmap: u32, image: i32) -> Result<Cached, Unresolved> {
    let at = image_record(word, bitmap, image)?;
    let stamp = word(at.wrapping_add(0x70)).ok_or(UNREAD)? as i32;
    let header = word(at.wrapping_add(0x50)).ok_or(UNREAD)?;
    let streamed = (word(at.wrapping_add(0xc)).ok_or(UNREAD)? >> 24) & 2 != 0;
    let fresh = stamp > word(RESIDENT_STAMP).ok_or(UNREAD)? as i32;
    if header != 0 && (fresh || !streamed) {
        return Ok(Cached::Header(header));
    }
    Ok(Cached::Streaming { image: at })
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

/// The input semantics of program `index` of `vrtx` tag `datum`, by
/// register, as `0x0001c330` reads them when it builds the declaration:
/// the element's `+4` count of u16 at `+8`, register `i` reading entry
/// `i`. The element is [`program`]'s, so its refusals are the same.
pub fn program_semantics(
    word: &dyn Fn(u32) -> Option<u32>,
    bytes: &dyn Fn(u32, usize) -> Option<Vec<u8>>,
    datum: u32,
    index: u32,
) -> Result<Vec<u16>, Unresolved> {
    let unread = "vertex_program_unread";
    program(word, bytes, datum, index)?;
    let tag = word(word(TAG_TABLE).ok_or(unread)?.wrapping_add((datum & 0xffff) * 16 + 8)).ok_or(unread)?;
    let e = word(tag + 8).ok_or(unread)?.wrapping_add(index * 0x1c);
    let count = word(e + 4).ok_or(unread)?;
    if count > 16 {
        return Err("vertex_program_inputs");
    }
    let list = word(e + 8).ok_or(unread)?;
    let b = bytes(list, 2 * count as usize).ok_or(unread)?;
    Ok(b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect())
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

    fn material(textures: [Option<Texture>; 4]) -> Material {
        Material {
            states: Vec::new(),
            stages: Vec::new(),
            combiner: [0; WORDS],
            combiner_driven: 0,
            textures,
            constants: Vec::new(),
            defaults: Vec::new(),
            program: (0xe000_0005, 1),
        }
    }

    #[test]
    fn a_rasterizer_engine_texture_binds_the_bitmap_its_data_names() {
        let w = guest();
        let m = material([Some(Texture::Rasterizer { offset: 0x64, image: 0 }), Some(Texture::Empty), None, None]);
        let r = resolve(&m, &|a| w.word(a), &|_, n| Some(vec![0; n])).unwrap();
        assert_eq!(r.textures, [Some(Some(0xc000)), Some(None), None, None]);
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
    fn the_cache_takes_a_fresh_or_unstreamed_header_and_asks_for_the_rest() {
        let mut w = guest();
        for image in [0x3000, 0x3074, 0x4000] {
            w.set(image + 0xc, 0);
        }
        let at = |w: &Words, b, i| texture_cached(&|a| w.word(a), b, i);
        assert_eq!(at(&w, 0xe000_0002, 0), Ok(Cached::Header(0xb000)));
        // A stale stamp: not streamed is taken, streamed (`+0x0f` bit 1) asks.
        assert_eq!(at(&w, 0xe000_0002, 1), Ok(Cached::Header(0xb100)));
        w.set(0x3074 + 0xc, 0x0200_0000);
        assert_eq!(at(&w, 0xe000_0002, 1), Ok(Cached::Streaming { image: 0x3074 }));
        w.set(0x3000 + 0x50, 0);
        assert_eq!(at(&w, 0xe000_0002, 0), Ok(Cached::Streaming { image: 0x3000 }));
        // The default's fall-back, as the binder's.
        assert_eq!(at(&w, 0xe000_0002, 7), Ok(Cached::Header(0xc000)));
        assert_eq!(at(&w, u32::MAX, 0), Ok(Cached::Header(0xc000)));
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
    fn a_programs_inputs_are_its_elements_semantic_list_by_register() {
        let mut w = guest();
        w.set(0x6000 + 0x1c + 4, 2);
        w.set(0x6000 + 0x1c + 8, 0x7100);
        let word = |a| w.word(a);
        let bytes = |a: u32, n: usize| match a {
            0x7000 => Some(vec![0x78; n]),
            0x7100 => Some([3u8, 0, 0, 0].into_iter().take(n).collect()),
            _ => None,
        };
        assert_eq!(program_semantics(&word, &bytes, 0xe000_0005, 1), Ok(vec![3, 0]));
        assert_eq!(program_semantics(&word, &bytes, 0xe000_0005, 2), Err("vertex_program_index"));
        w.set(0x6000 + 0x1c + 4, 17);
        let word = |a| w.word(a);
        assert_eq!(program_semantics(&word, &bytes, 0xe000_0005, 1), Err("vertex_program_inputs"));
    }
}
