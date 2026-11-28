//! Shader instances (`shad`), shader templates (`stem`) and shader passes
//! (`spas`): the compiled "postprocess" records the retail engine reads at draw
//! time.
//!
//! Self-contained (hand-written codecs, no schema/build.rs involvement), like
//! [`crate::sound`]. Every interpreted field names the engine instruction that
//! reads it; `docs/formats/shader-instances.md` has the full provenance table and the
//! disc results. Fields that no reached engine code reads are kept as raw
//! bytes or undereferenced [`RawBlock`]s, never guessed.
//!
//! The tag tool deduplicates identical child blocks across tags, so a block
//! pointer may land in another tag's payload (or be shared by two blocks of
//! the same tag). Blocks are therefore resolved against the whole ordinary
//! metadata region ([`Metadata`]), and every block has an element budget:
//! a per-block cap ([`MAX_BLOCK_ELEMENTS`]) plus one per-tag [`Budget`] of
//! elements and copied bytes shared by every block of the tag (all
//! postprocess records and their function blobs included), so aliased or
//! hostile counts cannot amplify allocations.
use crate::bytes::{f32_at, i16_at, u16_at, u32_at};
use crate::{Error, Result};
use map::{FourCC, Map, TagId};
use std::cell::Cell;
use std::io::{Read, Seek};
use std::sync::Arc;

pub const SHAD: FourCC = FourCC(*b"shad");
pub const STEM: FourCC = FourCC(*b"stem");
pub const SPAS: FourCC = FourCC(*b"spas");
pub const VRTX: FourCC = FourCC(*b"vrtx");
pub const BITM: FourCC = FourCC(*b"bitm");
/// The particle and the model particle, whose tags a particle draw's
/// shader begin takes in a `shad`'s place.
pub const PRT3: FourCC = FourCC(*b"prt3");
pub const PRTM: FourCC = FourCC(*b"PRTM");

/// Root sizes, fixed by the smallest in-tag child pointer on every disc tag
/// and by the engine's highest root read.
pub const SHAD_ROOT: usize = 0x54;
pub const STEM_ROOT: usize = 0x60;
pub const SPAS_ROOT: usize = 0x24;
pub const PRT3_ROOT: usize = 0xbc;
pub const PRTM_ROOT: usize = 0xdc;
/// `prt3 +0x8c`: the particle's compiled shader, a block of
/// [`SHAD_POSTPROCESS`] elements laid out as `shad +0x20`'s.
pub const PRT3_POSTPROCESS_BLOCK: usize = 0x8c;
/// `PRTM +0x18`: the model particle's `shad` reference (its datum at
/// `+0x1c`).
pub const PRTM_SHADER: usize = 0x18;
/// Element strides (engine address arithmetic, see each record).
pub const SHAD_POSTPROCESS: usize = 0x7c;
pub const STEM_POSTPROCESS: usize = 0x28;
pub const SPAS_POSTPROCESS: usize = 0x58;
pub const IMPLEMENTATION: usize = 0x132;
/// Size of the `D3DPIXELSHADERDEF` embedded at implementation `+6`.
pub const PSDEF_SIZE: usize = 0xf0;

/// Every block decoded here is either reached through an [`IndexRange`]
/// (9-bit start + 7-bit count, so at most 511 + 127 = 638 reachable
/// elements), a `u8` source index (256), or a small engine argument. 1,024
/// therefore bounds every block the engine can address; larger counts are
/// rejected rather than truncated.
pub const MAX_BLOCK_ELEMENTS: u32 = 1024;
/// Total elements one tag may decode across all its blocks.
pub const MAX_TAG_ELEMENTS: usize = 1 << 16;
/// Total bytes one tag may copy out of the metadata region across all its
/// blocks and animation function blobs.
pub const MAX_TAG_BYTES: usize = 1 << 20;
/// Largest animation function blob accepted (`shad` property data ref).
pub const MAX_FUNCTION_BYTES: u32 = 0x1000;

fn ctx(record: &'static str, field: &'static str) -> impl FnOnce(Error) -> Error {
    move |e| Error::Field {
        record,
        field,
        source: Box::new(e),
    }
}

/// The engine's packed child-range word: low 9 bits first element, high 7
/// bits element count (`and 0x1ff` / `shr 9` throughout `0x00016b90`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct IndexRange {
    pub start: u16,
    pub count: u8,
}
impl IndexRange {
    pub fn decode(v: u16) -> Self {
        Self {
            start: v & 0x1ff,
            count: (v >> 9) as u8,
        }
    }
    pub fn encode(self) -> u16 {
        (self.start & 0x1ff) | (u16::from(self.count & 0x7f) << 9)
    }
    pub fn indices(self) -> std::ops::Range<usize> {
        let s = usize::from(self.start);
        s..s + usize::from(self.count)
    }
    pub fn is_empty(self) -> bool {
        self.count == 0
    }
    /// Checks the range lies inside a block of `len` elements.
    pub fn check(self, len: usize, what: &'static str) -> Result<()> {
        if self.indices().end > len {
            return Err(Error::Field {
                record: "IndexRange",
                field: what,
                source: Box::new(Error::Invalid("child range exceeds its block")),
            });
        }
        Ok(())
    }
}

/// An eight-byte `(group, datum)` reference. `None` group / null datum is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TagRef {
    pub group: FourCC,
    pub id: TagId,
}
impl TagRef {
    fn read(b: &[u8], o: usize) -> Self {
        Self {
            group: FourCC::from_u32(u32_at(b, o)),
            id: TagId(u32_at(b, o + 4)),
        }
    }
    pub fn is_null(self) -> bool {
        self.id == TagId::NULL
    }
}

/// A block descriptor that no reached engine code dereferences. Its count
/// and pointer are preserved; its elements are not interpreted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RawBlock {
    pub count: u32,
    pub address: u32,
}
impl RawBlock {
    fn read(b: &[u8], o: usize) -> Self {
        Self {
            count: u32_at(b, o),
            address: u32_at(b, o + 4),
        }
    }
}

/// The ordinary tag-metadata region of one map, addressable by virtual
/// address. Blocks may point anywhere inside it (deduplicated data).
#[derive(Clone, Debug)]
pub struct Metadata {
    base: u32,
    bytes: Arc<Vec<u8>>,
}
impl Metadata {
    pub fn new(base: u32, bytes: Vec<u8>) -> Self {
        Self::from_shared(base, Arc::new(bytes))
    }
    pub(crate) fn from_shared(base: u32, bytes: Arc<Vec<u8>>) -> Self {
        Self { base, bytes }
    }
    /// Reads the map's whole ordinary metadata region (BSP regions excluded:
    /// shader tags live in ordinary metadata).
    pub fn read<R: Read + Seek>(map: &mut Map<R>) -> Result<Self> {
        let space = map
            .metadata_space()
            .ok_or(Error::Invalid("map has no metadata region"))?;
        let offset = space.translate(space.virtual_base, space.size)?;
        let bytes = map.read_file_range(offset, u64::from(space.size))?;
        Ok(Self::new(space.virtual_base, bytes))
    }
    pub fn base(&self) -> u32 {
        self.base
    }
    pub fn len(&self) -> usize {
        self.bytes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
    /// `len` bytes at virtual `address`, entirely inside the region.
    pub fn bytes(&self, address: u32, len: usize) -> Result<&[u8]> {
        let bad = Error::Bounds {
            address,
            size: len as u64,
        };
        let start = address
            .checked_sub(self.base)
            .ok_or_else(|| bad_clone(&bad))? as usize;
        let end = start.checked_add(len).ok_or_else(|| bad_clone(&bad))?;
        self.bytes.get(start..end).ok_or(bad)
    }
}
fn bad_clone(e: &Error) -> Error {
    match e {
        Error::Bounds { address, size } => Error::Bounds {
            address: *address,
            size: *size,
        },
        _ => Error::Invalid("bounds"),
    }
}

/// Remaining allowance for one tag decode: elements and copied bytes. One
/// budget is shared by every block of a tag, at every depth, so repeated or
/// aliased children are charged each time they are copied.
#[derive(Clone, Debug)]
pub struct Budget {
    elements: Cell<usize>,
    bytes: Cell<usize>,
}
impl Default for Budget {
    fn default() -> Self {
        Self {
            elements: Cell::new(MAX_TAG_ELEMENTS),
            bytes: Cell::new(MAX_TAG_BYTES),
        }
    }
}
impl Budget {
    pub fn remaining_elements(&self) -> usize {
        self.elements.get()
    }
    pub fn remaining_bytes(&self) -> usize {
        self.bytes.get()
    }
    /// Charges `elements` and `bytes`, or fails without charging.
    fn charge(&self, elements: usize, bytes: usize) -> Result<()> {
        let e = self
            .elements
            .get()
            .checked_sub(elements)
            .ok_or(Error::Invalid("tag exceeds its total element budget"))?;
        let b = self
            .bytes
            .get()
            .checked_sub(bytes)
            .ok_or(Error::Invalid("tag exceeds its total byte budget"))?;
        self.elements.set(e);
        self.bytes.set(b);
        Ok(())
    }
}

/// Decodes a child block at `root + offset` with fixed `stride`.
fn block<T>(
    meta: &Metadata,
    budget: &Budget,
    root: &[u8],
    offset: usize,
    stride: usize,
    decode: impl Fn(&[u8]) -> Result<T>,
) -> Result<Vec<T>> {
    let count = u32_at(root, offset);
    let address = u32_at(root, offset + 4);
    if count == 0 {
        return Ok(Vec::new());
    }
    if count > MAX_BLOCK_ELEMENTS {
        return Err(Error::Invalid("block count exceeds the element budget"));
    }
    let n = count as usize;
    budget.charge(n, n * stride)?;
    if !address.is_multiple_of(2) {
        return Err(Error::Invalid("misaligned block pointer"));
    }
    let bytes = meta.bytes(address, n * stride)?;
    bytes.chunks_exact(stride).map(decode).collect()
}

/// Requires a block descriptor to be empty (never populated on the disc and
/// never read by the engine paths decoded here).
fn empty(root: &[u8], offset: usize, field: &'static str) -> Result<()> {
    if u32_at(root, offset) != 0 {
        return Err(Error::UnsupportedBlock(field));
    }
    Ok(())
}

// ---------------------------------------------------------------- shad --

/// `shad` root (0x54 bytes).
#[derive(Clone, Debug, PartialEq)]
pub struct ShaderInstance {
    /// +0x00 `stem` reference. Engine: `0x0001663a` reads the datum at +4.
    pub template: TagRef,
    /// +0x0c, count 1 in every disc tag; not dereferenced by the draw path.
    pub block_0c: RawBlock,
    /// +0x20 compiled postprocess; `0x00016bc7` reads its pointer (+0x24) and
    /// uses element 0. Exactly one element on every disc tag.
    pub postprocess: Vec<ShaderPostprocess>,
    /// +0x2c, up to 98 elements; not dereferenced by the draw path.
    pub block_2c: RawBlock,
    /// +0x34 reference (group bytes `slit` on the disc); not read by the draw path.
    pub reference_34: TagRef,
    /// +0x3e and +0x40 (u16). `0x0001bbf0` copies them to `0x485a64/0x485a68`,
    /// which `0x00034060` returns as engine integer inputs 4 and 5: they
    /// select combiner variants and conditional constants.
    pub int_inputs: [u16; 2],
    /// +0x44: copied to `0x485a6c`; engine scalar input 21 (`0x000337e0`) and a
    /// multiplier on object-driven vector inputs 23 and 35.
    pub scalar_input: f32,
    /// +0x48: copied to `0x485a70`; feeds object-driven vector input 24
    /// (`0x00033c30`).
    pub vector_input: f32,
    /// The whole root, for round trips and the uninterpreted spans.
    pub raw: Vec<u8>,
}

/// One bitmap parameter (shad postprocess +0x04, stride 0xc). `0x00016f60`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BitmapParameter {
    /// +0: `bitm` datum; null selects the rasterizer default (`[0x485a80]+0x64`).
    pub bitmap: TagId,
    /// +4: image (sequence frame) index, converted to float into the slot's
    /// frame field; `0x0001b0a0` wraps it by the bitmap's image count.
    pub image: i32,
    /// +8: added to the draw's LOD term before mip selection (`0x0001b15c`).
    pub lod_offset: f32,
}

/// Shader-side copy of the template's LOD node (shad +0x1c, stride 6). `0x000174d0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InstanceLod {
    /// +0: bit `layer` set when that layer has animation data.
    pub layer_mask: u32,
    /// +4: range into `layers`.
    pub layers: IndexRange,
}

/// Per-implementation animation ranges (shad +0x34, stride 10); each range
/// selects [`AnimationBinding`]s. Engine consumers in `0x00017420/0x000174d0`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AnimationRanges {
    /// +0: texture-slot fields (`0x00017550`).
    pub bitmaps: IndexRange,
    /// +2: render states (`0x00017b60`).
    pub render_states: IndexRange,
    /// +4: texture-stage states (`0x00018560`).
    pub stage_states: IndexRange,
    /// +6: pixel-shader colour constants (`0x000176a0`).
    pub pixel_constants: IndexRange,
    /// +8: vertex constants and vertex defaults (`0x00017960`).
    pub vertex_constants: IndexRange,
}

/// An animated property (shad +0x3c, stride 0x14). Evaluated by `0x0001b230`.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimatedProperty {
    /// +0: 0 = driven by game time; otherwise by object functions.
    pub input: u32,
    /// +4: first object-function input selector (passed to the object callback).
    pub input_a: u32,
    /// +8: second selector, or (as f32) the period in seconds when `input == 0`.
    pub input_b: u32,
    /// +0xc data ref: the mapping-function blob (`0x0013b390` evaluates it).
    /// When `function[1] & 0xf0 == 0`, bytes +4/+8 are the f32 output range.
    pub function: Vec<u8>,
}

/// shad +0x44 (stride 4): a component of one animated property.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PropertyRef {
    pub property: i16,
    pub component: i16,
}

/// shad +0x54 (stride 4): one animation binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnimationBinding {
    /// Byte 0: texture slot or state table index. For a constant this is
    /// the low byte of its destination word (`target | b1 << 8`).
    pub target: u8,
    /// Byte 1: high byte of the constant destination. A pixel word packs
    /// fragment, selector, register and C1 in bits 0..2, 3..5, 6..9 and
    /// 10. A vertex word packs register, bank, mode and texture slot
    /// in bits 0..3, 4, 5..9 and 10..12. Render-state kind 1 is scalar.
    pub b1: u8,
    /// Byte 2: pixel component mode in bits 0..2 and scalar transform in
    /// bits 3..5 (`0x000176e3`). Vertex constants use only the word.
    pub b2: u8,
    /// byte 3: index into `property_ranges`.
    pub properties: u8,
}

/// The shad compiled postprocess element (0x7c bytes).
#[derive(Clone, Debug, PartialEq)]
pub struct ShaderPostprocess {
    /// +0x00 template datum, equal to the root reference on the disc.
    pub template: TagId,
    pub bitmaps: Vec<BitmapParameter>,
    /// +0x0c (stride 4): packed ARGB colours, read by `0x00017170`.
    pub colors: Vec<u32>,
    /// +0x14 (stride 0x10): four floats, read by `0x00017000`.
    pub vectors: Vec<[f32; 4]>,
    pub lods: Vec<InstanceLod>,
    /// +0x24 (u16 ranges into `passes`).
    pub layers: Vec<IndexRange>,
    /// +0x2c (u16 ranges into `implementations`).
    pub passes: Vec<IndexRange>,
    pub implementations: Vec<AnimationRanges>,
    pub properties: Vec<AnimatedProperty>,
    pub property_refs: Vec<PropertyRef>,
    /// +0x4c (u16 ranges into `property_refs`).
    pub property_ranges: Vec<IndexRange>,
    pub animations: Vec<AnimationBinding>,
    /// +0x5c, +0x64, +0x6c: constant counts 5, 2, 6 on the disc, their data
    /// shared between shaders. Engine texture 26 (`0x00019a68`) reads the
    /// i16 at `+0x10` of the first's data, a bitmap parameter index
    /// (`material::PROPERTY_BITMAP_TEXTURE`); the rest is not dereferenced.
    pub opaque_blocks: [RawBlock; 3],
    /// +0x74..+0x7c, uninterpreted.
    pub tail: [u8; 8],
}

impl ShaderInstance {
    pub fn decode(meta: &Metadata, address: u32) -> Result<Self> {
        let budget = Budget::default();
        let root = meta.bytes(address, SHAD_ROOT)?;
        let postprocess = block(meta, &budget, root, 0x20, SHAD_POSTPROCESS, |b| {
            ShaderPostprocess::decode(meta, &budget, b)
        })
        .map_err(ctx("shad", "postprocess"))?;
        Ok(Self {
            template: TagRef::read(root, 0),
            block_0c: RawBlock::read(root, 0x0c),
            postprocess,
            block_2c: RawBlock::read(root, 0x2c),
            reference_34: TagRef::read(root, 0x34),
            int_inputs: [u16_at(root, 0x3e), u16_at(root, 0x40)],
            scalar_input: f32_at(root, 0x44),
            vector_input: f32_at(root, 0x48),
            raw: root.to_vec(),
        })
    }
    /// The element the engine uses (`0x00016bc7` dereferences element 0).
    pub fn compiled(&self) -> Result<&ShaderPostprocess> {
        self.postprocess
            .first()
            .ok_or(Error::Invalid("shad has no postprocess element"))
    }
}

impl ShaderPostprocess {
    /// A particle's (`prt3`) compiled shader: element 0 of its `+0x8c`
    /// block ([`PRT3_POSTPROCESS_BLOCK`]), which the shader begin draws
    /// as a `shad`'s `+0x20`.
    pub fn decode_particle(meta: &Metadata, address: u32) -> Result<Self> {
        let budget = Budget::default();
        let root = meta.bytes(address, PRT3_ROOT)?;
        block(meta, &budget, root, PRT3_POSTPROCESS_BLOCK, SHAD_POSTPROCESS, |b| {
            ShaderPostprocess::decode(meta, &budget, b)
        })
        .map_err(ctx("prt3", "postprocess"))?
        .into_iter()
        .next()
        .ok_or(Error::Invalid("prt3 has no postprocess element"))
    }

    fn decode(meta: &Metadata, budget: &Budget, b: &[u8]) -> Result<Self> {
        let r = |v: &[u8]| Ok(IndexRange::decode(u16_at(v, 0)));
        Ok(Self {
            template: TagId(u32_at(b, 0)),
            bitmaps: block(meta, budget, b, 0x04, 0xc, |v| {
                Ok(BitmapParameter {
                    bitmap: TagId(u32_at(v, 0)),
                    image: u32_at(v, 4) as i32,
                    lod_offset: f32_at(v, 8),
                })
            })
            .map_err(ctx("ShaderPostprocess", "bitmaps"))?,
            colors: block(meta, budget, b, 0x0c, 4, |v| Ok(u32_at(v, 0)))
                .map_err(ctx("ShaderPostprocess", "colors"))?,
            vectors: block(meta, budget, b, 0x14, 0x10, |v| {
                Ok([f32_at(v, 0), f32_at(v, 4), f32_at(v, 8), f32_at(v, 12)])
            })
            .map_err(ctx("ShaderPostprocess", "vectors"))?,
            lods: block(meta, budget, b, 0x1c, 6, |v| {
                Ok(InstanceLod {
                    layer_mask: u32_at(v, 0),
                    layers: IndexRange::decode(u16_at(v, 4)),
                })
            })
            .map_err(ctx("ShaderPostprocess", "lods"))?,
            layers: block(meta, budget, b, 0x24, 2, r)
                .map_err(ctx("ShaderPostprocess", "layers"))?,
            passes: block(meta, budget, b, 0x2c, 2, r)
                .map_err(ctx("ShaderPostprocess", "passes"))?,
            implementations: block(meta, budget, b, 0x34, 10, |v| {
                let g = |o| IndexRange::decode(u16_at(v, o));
                Ok(AnimationRanges {
                    bitmaps: g(0),
                    render_states: g(2),
                    stage_states: g(4),
                    pixel_constants: g(6),
                    vertex_constants: g(8),
                })
            })
            .map_err(ctx("ShaderPostprocess", "implementations"))?,
            properties: block(meta, budget, b, 0x3c, 0x14, |v| {
                let size = u32_at(v, 0xc);
                let function = if size == 0 {
                    Vec::new()
                } else if size > MAX_FUNCTION_BYTES {
                    return Err(Error::Invalid("animation function exceeds its budget"));
                } else {
                    budget.charge(0, size as usize)?;
                    meta.bytes(u32_at(v, 0x10), size as usize)?.to_vec()
                };
                Ok(AnimatedProperty {
                    input: u32_at(v, 0),
                    input_a: u32_at(v, 4),
                    input_b: u32_at(v, 8),
                    function,
                })
            })
            .map_err(ctx("ShaderPostprocess", "properties"))?,
            property_refs: block(meta, budget, b, 0x44, 4, |v| {
                Ok(PropertyRef {
                    property: i16_at(v, 0),
                    component: i16_at(v, 2),
                })
            })
            .map_err(ctx("ShaderPostprocess", "property_refs"))?,
            property_ranges: block(meta, budget, b, 0x4c, 2, r)
                .map_err(ctx("ShaderPostprocess", "property_ranges"))?,
            animations: block(meta, budget, b, 0x54, 4, |v| {
                Ok(AnimationBinding {
                    target: v[0],
                    b1: v[1],
                    b2: v[2],
                    properties: v[3],
                })
            })
            .map_err(ctx("ShaderPostprocess", "animations"))?,
            opaque_blocks: [
                RawBlock::read(b, 0x5c),
                RawBlock::read(b, 0x64),
                RawBlock::read(b, 0x6c),
            ],
            tail: b[0x74..0x7c].try_into().expect("8 bytes"),
        })
    }

    /// Structural checks: every range fits its block, every index fits.
    pub fn validate(&self) -> Result<()> {
        for l in &self.lods {
            l.layers.check(self.layers.len(), "shad lod layers")?;
        }
        for r in &self.layers {
            r.check(self.passes.len(), "shad layer passes")?;
        }
        for r in &self.passes {
            r.check(self.implementations.len(), "shad pass implementations")?;
        }
        for i in &self.implementations {
            for r in [
                i.bitmaps,
                i.render_states,
                i.stage_states,
                i.pixel_constants,
                i.vertex_constants,
            ] {
                r.check(self.animations.len(), "shad animation bindings")?;
            }
        }
        for a in &self.animations {
            if usize::from(a.properties) >= self.property_ranges.len() {
                return Err(Error::Invalid(
                    "animation binding names a missing property range",
                ));
            }
        }
        for r in &self.property_ranges {
            r.check(self.property_refs.len(), "shad property refs")?;
        }
        for p in &self.property_refs {
            if p.property < 0 || p.property as usize >= self.properties.len() {
                return Err(Error::Invalid("property ref names a missing property"));
            }
        }
        Ok(())
    }

    /// The animation leaf for one draw, following `0x000174d0`'s bounds checks
    /// (out-of-range indices or a clear layer bit mean "no animation").
    pub fn animation_leaf(
        &self,
        lod: usize,
        layer: usize,
        pass: usize,
        imp: usize,
    ) -> Option<&AnimationRanges> {
        let l = self.lods.get(lod)?;
        if layer >= usize::from(l.layers.count) || layer >= 32 || l.layer_mask & (1 << layer) == 0 || layer >= usize::from(l.layers.count) {
            return None;
        }
        let pr = *self.layers.get(usize::from(l.layers.start) + layer)?;
        if pass >= usize::from(pr.count) {
            return None;
        }
        let ir = *self.passes.get(usize::from(pr.start) + pass)?;
        if imp >= usize::from(ir.count) {
            return None;
        }
        self.implementations.get(usize::from(ir.start) + imp)
    }
}

// ---------------------------------------------------------------- stem --

/// `stem` root (0x60 bytes).
#[derive(Clone, Debug, PartialEq)]
pub struct ShaderTemplate {
    /// +0x40 and +0x4c: `shad` references (null on most disc tags); not read
    /// by the draw path.
    pub references: [TagRef; 2],
    /// +0x58 compiled postprocess; `0x00016bf4` reads the pointer (+0x5c),
    /// element 0. Exactly one element on every disc tag.
    pub postprocess: Vec<TemplatePostprocess>,
    pub raw: Vec<u8>,
}

/// stem postprocess +0x00 (stride 10). Indexed by the draw's LOD argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TemplateLod {
    /// +0: range into `layers` (one entry per layer index).
    pub layers: IndexRange,
    /// +2: bit `layer` set when the layer is drawn (`0x0001666d`); also passed
    /// to engine integer input 6 (`0x000340a4`).
    pub layer_mask: u32,
    /// +6, uninterpreted.
    pub unknown_6: u32,
}

/// stem postprocess +0x10 (stride 10): one pass of a layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TemplatePass {
    /// +0: `spas` reference; `0x00016c2c` reads the datum at +4.
    pub pass: TagRef,
    /// +8: range into `implementations`, one entry per spas implementation.
    pub implementations: IndexRange,
}

/// stem postprocess +0x18 (stride 6): which bindings feed one implementation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImplementationBindings {
    /// +0: bitmap bindings (`0x00016f60`).
    pub bitmaps: IndexRange,
    /// +2: pixel-shader colour constants (`0x00017170`).
    pub pixel_constants: IndexRange,
    /// +4: vertex constants / vertex defaults (`0x00017000`).
    pub vertex_constants: IndexRange,
}

/// stem postprocess +0x20 (stride 4): binds one instance parameter to an
/// input. The low word's meaning depends on which range selects it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParameterBinding {
    /// bytes 0-1. Bitmap: byte 0 = texture slot. Colour: bits 0-2 combiner
    /// fragment, 3-5 required selector value, 6-9 register (8 = final
    /// combiner), bit 10 = `c1`. Vector: bits 0-3 register, bit 4 = bank
    /// (0 vertex default `vN`, 1 vertex constant), bits 5-9 component mode.
    pub word: u16,
    /// byte 2. Colour: bits 0-2 channel mask selector (`0x467068`).
    pub b2: u8,
    /// byte 3: index into the instance's bitmaps / colors / vectors.
    pub source: u8,
}

/// The stem compiled postprocess element (0x28 bytes).
#[derive(Clone, Debug, PartialEq)]
pub struct TemplatePostprocess {
    pub lods: Vec<TemplateLod>,
    /// +0x08 (u16 ranges into `passes`), one per layer.
    pub layers: Vec<IndexRange>,
    pub passes: Vec<TemplatePass>,
    pub implementations: Vec<ImplementationBindings>,
    pub bindings: Vec<ParameterBinding>,
}

impl ShaderTemplate {
    pub fn decode(meta: &Metadata, address: u32) -> Result<Self> {
        let budget = Budget::default();
        let root = meta.bytes(address, STEM_ROOT)?;
        let postprocess = block(meta, &budget, root, 0x58, STEM_POSTPROCESS, |b| {
            TemplatePostprocess::decode(meta, &budget, b)
        })
        .map_err(ctx("stem", "postprocess"))?;
        Ok(Self {
            references: [TagRef::read(root, 0x40), TagRef::read(root, 0x4c)],
            postprocess,
            raw: root.to_vec(),
        })
    }
    pub fn compiled(&self) -> Result<&TemplatePostprocess> {
        self.postprocess
            .first()
            .ok_or(Error::Invalid("stem has no postprocess element"))
    }
}

impl TemplatePostprocess {
    fn decode(meta: &Metadata, budget: &Budget, b: &[u8]) -> Result<Self> {
        Ok(Self {
            lods: block(meta, budget, b, 0x00, 10, |v| {
                Ok(TemplateLod {
                    layers: IndexRange::decode(u16_at(v, 0)),
                    layer_mask: u32_at(v, 2),
                    unknown_6: u32_at(v, 6),
                })
            })
            .map_err(ctx("TemplatePostprocess", "lods"))?,
            layers: block(meta, budget, b, 0x08, 2, |v| {
                Ok(IndexRange::decode(u16_at(v, 0)))
            })
            .map_err(ctx("TemplatePostprocess", "layers"))?,
            passes: block(meta, budget, b, 0x10, 10, |v| {
                Ok(TemplatePass {
                    pass: TagRef::read(v, 0),
                    implementations: IndexRange::decode(u16_at(v, 8)),
                })
            })
            .map_err(ctx("TemplatePostprocess", "passes"))?,
            implementations: block(meta, budget, b, 0x18, 6, |v| {
                let g = |o| IndexRange::decode(u16_at(v, o));
                Ok(ImplementationBindings {
                    bitmaps: g(0),
                    pixel_constants: g(2),
                    vertex_constants: g(4),
                })
            })
            .map_err(ctx("TemplatePostprocess", "implementations"))?,
            bindings: block(meta, budget, b, 0x20, 4, |v| {
                Ok(ParameterBinding {
                    word: u16_at(v, 0),
                    b2: v[2],
                    source: v[3],
                })
            })
            .map_err(ctx("TemplatePostprocess", "bindings"))?,
        })
    }

    pub fn validate(&self) -> Result<()> {
        for l in &self.lods {
            l.layers.check(self.layers.len(), "stem lod layers")?;
        }
        for r in &self.layers {
            r.check(self.passes.len(), "stem layer passes")?;
        }
        for p in &self.passes {
            p.implementations
                .check(self.implementations.len(), "stem pass implementations")?;
        }
        for i in &self.implementations {
            for r in [i.bitmaps, i.pixel_constants, i.vertex_constants] {
                r.check(self.bindings.len(), "stem bindings")?;
            }
        }
        Ok(())
    }

    /// The pass list of one (lod, layer), as `0x00016610` walks it.
    pub fn layer_passes(&self, lod: usize, layer: usize) -> Option<&[TemplatePass]> {
        let l = self.lods.get(lod)?;
        if layer >= 32 || l.layer_mask & (1 << layer) == 0 {
            return None;
        }
        let r = *self.layers.get(usize::from(l.layers.start) + layer)?;
        self.passes.get(r.indices())
    }
}

// ---------------------------------------------------------------- spas --

/// `spas` root (0x24 bytes).
#[derive(Clone, Debug, PartialEq)]
pub struct ShaderPass {
    /// +0x1c compiled postprocess; `0x00016c3c` reads the pointer (+0x20),
    /// element 0. Exactly one element on every disc tag.
    pub postprocess: Vec<PassPostprocess>,
    pub raw: Vec<u8>,
}

/// spas +0x08 (stride 4): binds a texture stage. `0x00018a90`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureBinding {
    /// byte 0: texture slot filled from the instance (`0xff` = engine source).
    pub slot: u8,
    /// byte 1 (when `slot == 0xff`): 0 = no texture, 11 = active lightmap,
    /// other values are engine textures (`0x00019530`).
    pub source: u8,
    /// byte 2: D3D texture stage.
    pub stage: u8,
    /// byte 3: bit 1 = record the selected image's width/height (`0x00018afd`).
    pub flags: u8,
}

/// spas +0x10 (stride 5): one render state. `0x00018900`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderStateValue {
    /// Xbox `D3DRENDERSTATETYPE` (see [`render_state_name`]).
    pub state: u8,
    pub value: u32,
}

/// spas +0x18 (stride 0x18): one texture stage's sampler state. `0x000189a0`
/// stores these into the deferred texture-state array at `0x404dd0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StageStates {
    pub address_u: u8,
    pub address_v: u8,
    pub address_w: u8,
    pub mag_filter: u8,
    pub min_filter: u8,
    pub mip_filter: u8,
    /// f32 bits, as D3D stores it.
    pub mip_lod_bias: u32,
    pub max_mip_level: u8,
    pub max_anisotropy: u8,
    pub color_key_op: u8,
    pub color_sign: u32,
    pub alpha_kill: u8,
    pub border_color: u32,
    pub color_key_color: u32,
    /// Byte 0 low nibble and byte 7, never read.
    pub unread: [u8; 2],
}

/// spas +0x20 (stride 4): a combiner fragment slot of a variant pixel shader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CombinerFragment {
    /// +0: engine integer input selecting the variant (-1 = fixed).
    pub selector: i16,
    /// +2 (low 9 bits): first variant index.
    pub base: u16,
}

/// spas +0x28 (stride 6): one variant of a fragment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CombinerVariant {
    pub unknown_0: u16,
    /// +2: bit 0 = the last stage is the final combiner.
    pub flags: u8,
    pub unknown_3: u8,
    /// +4: range into `stages`.
    pub stages: IndexRange,
}

/// spas +0x30 (stride 0x20): one general combiner stage (or final combiner).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CombinerStage {
    pub rgb_inputs: u32,
    pub rgb_outputs: u32,
    pub alpha_inputs: u32,
    pub alpha_outputs: u32,
    pub constant0: u32,
    pub constant1: u32,
    pub unread: [u32; 2],
}

/// spas +0x38 (stride 4): an engine-driven binding. Field meaning depends on
/// the implementation range that selects it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExternalBinding {
    pub bytes: [u8; 4],
}
impl ExternalBinding {
    pub fn word(self) -> u16 {
        u16::from_le_bytes([self.bytes[0], self.bytes[1]])
    }
}

/// One pass implementation (0x132 bytes).
#[derive(Clone, Debug, PartialEq)]
pub struct Implementation {
    /// +0x000 texture bindings.
    pub textures: IndexRange,
    /// +0x002 render states applied on entry.
    pub render_states: IndexRange,
    /// +0x004 texture-stage states; position in the range = stage.
    pub stage_states: IndexRange,
    /// +0x006 `D3DPIXELSHADERDEF`, verbatim.
    pub pixel_shader: [u8; PSDEF_SIZE],
    /// +0x0f6 combiner fragments; nonzero count makes the pixel shader a
    /// variant built by `0x0001ba00`.
    pub fragments: IndexRange,
    /// +0x0f8, uninterpreted.
    pub unknown_f8: [u8; 4],
    /// +0x0fc `vrtx` reference; `0x00016718` reads the datum at +0x100.
    pub vertex_shader: TagRef,
    /// +0x10c/+0x110: engine scalar inputs this implementation needs.
    /// `0x00016e04` copies them to state `+0x44/+0x48`; `0x0001ae70` walks
    /// that mask calling the scalar evaluator `0x000336f0` (`0x0001aee6`).
    pub scalar_inputs: u64,
    /// +0x104/+0x108: engine vector inputs, copied to state `+0x4c/+0x50`
    /// (`0x00016e16`) and walked calling `0x00033980` (`0x0001af7d`).
    pub vector_inputs: u64,
    /// +0x114: engine integer inputs (state `+0x54`).
    pub int_inputs: u32,
    /// +0x118: flags. Bit 20 ([`Self::copies_target`]) is the only one a
    /// draw is known to read.
    pub unknown_118: u32,
    /// +0x11c render states applied when the next implementation replaces this one.
    pub restore_states: IndexRange,
    /// +0x11e engine-driven render states (`0x0001a170`).
    pub animated_render_states: IndexRange,
    /// +0x120 engine-driven texture-stage states (`0x0001ab50`).
    pub animated_stage_states: IndexRange,
    /// +0x122 engine-driven pixel constants (`0x00019ca0`).
    pub external_pixel_constants: IndexRange,
    /// +0x124 engine-driven vertex constants (`0x00019be0`).
    pub external_vertex_constants: IndexRange,
    /// +0x126..+0x132, uninterpreted.
    pub unknown_126: [u8; 12],
}

/// The spas compiled postprocess element (0x58 bytes).
#[derive(Clone, Debug, PartialEq)]
pub struct PassPostprocess {
    pub implementations: Vec<Implementation>,
    pub textures: Vec<TextureBinding>,
    pub render_states: Vec<RenderStateValue>,
    pub stage_states: Vec<StageStates>,
    pub fragments: Vec<CombinerFragment>,
    pub variants: Vec<CombinerVariant>,
    pub stages: Vec<CombinerStage>,
    pub externals: Vec<ExternalBinding>,
}

impl ShaderPass {
    pub fn decode(meta: &Metadata, address: u32) -> Result<Self> {
        let budget = Budget::default();
        let root = meta.bytes(address, SPAS_ROOT)?;
        let postprocess = block(meta, &budget, root, 0x1c, SPAS_POSTPROCESS, |b| {
            PassPostprocess::decode(meta, &budget, b)
        })
        .map_err(ctx("spas", "postprocess"))?;
        Ok(Self {
            postprocess,
            raw: root.to_vec(),
        })
    }
    pub fn compiled(&self) -> Result<&PassPostprocess> {
        self.postprocess
            .first()
            .ok_or(Error::Invalid("spas has no postprocess element"))
    }
}

impl Implementation {
    pub fn decode(v: &[u8]) -> Result<Self> {
        if v.len() != IMPLEMENTATION {
            return Err(Error::RecordSize {
                actual: v.len(),
                expected: IMPLEMENTATION,
            });
        }
        let g = |o| IndexRange::decode(u16_at(v, o));
        Ok(Self {
            textures: g(0),
            render_states: g(2),
            stage_states: g(4),
            pixel_shader: v[6..6 + PSDEF_SIZE].try_into().expect("psdef"),
            fragments: g(0xf6),
            unknown_f8: v[0xf8..0xfc].try_into().expect("4"),
            vertex_shader: TagRef::read(v, 0xfc),
            scalar_inputs: u64::from(u32_at(v, 0x10c)) | u64::from(u32_at(v, 0x110)) << 32,
            vector_inputs: u64::from(u32_at(v, 0x104)) | u64::from(u32_at(v, 0x108)) << 32,
            int_inputs: u32_at(v, 0x114),
            unknown_118: u32_at(v, 0x118),
            restore_states: g(0x11c),
            animated_render_states: g(0x11e),
            animated_stage_states: g(0x120),
            external_pixel_constants: g(0x122),
            external_vertex_constants: g(0x124),
            unknown_126: v[0x126..0x132].try_into().expect("12"),
        })
    }
    /// Flag bit 20 (`+0x118`): the geometry draw `0x0004d0b0` of a shader
    /// with flag bit 0 calls `0x0003bea0` for an entry whose program has
    /// it (`0x0004d561`..`0x0004d572`), which the first time a frame
    /// clears the shader cache and, when the active structure has a
    /// parameter block, copies and binds the structure's target
    /// (docs/graphics/native-objects.md, "Fade, the shader flag and the
    /// item's argument").
    pub fn copies_target(&self) -> bool {
        self.unknown_118 >> 20 & 1 != 0
    }
    /// PSDEF dword `i` (SDK field order).
    pub fn psdef_word(&self, i: usize) -> u32 {
        u32_at(&self.pixel_shader, i * 4)
    }
}

impl PassPostprocess {
    fn decode(meta: &Metadata, budget: &Budget, b: &[u8]) -> Result<Self> {
        for (o, f) in [(0x40, "block_40"), (0x48, "block_48"), (0x50, "block_50")] {
            empty(b, o, f)?;
        }
        Ok(Self {
            implementations: block(
                meta,
                budget,
                b,
                0x00,
                IMPLEMENTATION,
                Implementation::decode,
            )
            .map_err(ctx("PassPostprocess", "implementations"))?,
            textures: block(meta, budget, b, 0x08, 4, |v| {
                Ok(TextureBinding {
                    slot: v[0],
                    source: v[1],
                    stage: v[2],
                    flags: v[3],
                })
            })
            .map_err(ctx("PassPostprocess", "textures"))?,
            render_states: block(meta, budget, b, 0x10, 5, |v| {
                Ok(RenderStateValue {
                    state: v[0],
                    value: u32_at(v, 1),
                })
            })
            .map_err(ctx("PassPostprocess", "render_states"))?,
            stage_states: block(meta, budget, b, 0x18, 0x18, |v| {
                Ok(StageStates {
                    min_filter: v[0] >> 4,
                    mag_filter: v[1] & 0xf,
                    mip_filter: v[1] >> 4,
                    alpha_kill: v[2] & 0xf,
                    color_key_op: v[2] >> 4,
                    max_anisotropy: v[3] & 0xf,
                    max_mip_level: v[3] >> 4,
                    address_u: v[4],
                    address_v: v[5],
                    address_w: v[6],
                    border_color: u32_at(v, 8),
                    color_key_color: u32_at(v, 0xc),
                    color_sign: u32_at(v, 0x10),
                    mip_lod_bias: u32_at(v, 0x14),
                    unread: [v[0] & 0xf, v[7]],
                })
            })
            .map_err(ctx("PassPostprocess", "stage_states"))?,
            fragments: block(meta, budget, b, 0x20, 4, |v| {
                Ok(CombinerFragment {
                    selector: i16_at(v, 0),
                    base: u16_at(v, 2) & 0x1ff,
                })
            })
            .map_err(ctx("PassPostprocess", "fragments"))?,
            variants: block(meta, budget, b, 0x28, 6, |v| {
                Ok(CombinerVariant {
                    unknown_0: u16_at(v, 0),
                    flags: v[2],
                    unknown_3: v[3],
                    stages: IndexRange::decode(u16_at(v, 4)),
                })
            })
            .map_err(ctx("PassPostprocess", "variants"))?,
            stages: block(meta, budget, b, 0x30, 0x20, |v| {
                Ok(CombinerStage {
                    rgb_inputs: u32_at(v, 0),
                    rgb_outputs: u32_at(v, 4),
                    alpha_inputs: u32_at(v, 8),
                    alpha_outputs: u32_at(v, 0xc),
                    constant0: u32_at(v, 0x10),
                    constant1: u32_at(v, 0x14),
                    unread: [u32_at(v, 0x18), u32_at(v, 0x1c)],
                })
            })
            .map_err(ctx("PassPostprocess", "stages"))?,
            externals: block(meta, budget, b, 0x38, 4, |v| {
                Ok(ExternalBinding {
                    bytes: v.try_into().expect("4"),
                })
            })
            .map_err(ctx("PassPostprocess", "externals"))?,
        })
    }

    pub fn validate(&self) -> Result<()> {
        for i in &self.implementations {
            i.textures.check(self.textures.len(), "spas textures")?;
            i.render_states
                .check(self.render_states.len(), "spas render states")?;
            i.restore_states
                .check(self.render_states.len(), "spas restore states")?;
            i.stage_states
                .check(self.stage_states.len(), "spas stage states")?;
            if i.stage_states.count > 4 {
                return Err(Error::Invalid("more than four texture stages"));
            }
            i.fragments.check(self.fragments.len(), "spas fragments")?;
            if i.fragments.count > 8 {
                return Err(Error::Invalid("more than eight combiner fragments"));
            }
            for r in [
                i.animated_render_states,
                i.animated_stage_states,
                i.external_pixel_constants,
                i.external_vertex_constants,
            ] {
                r.check(self.externals.len(), "spas externals")?;
            }
        }
        for t in &self.textures {
            if t.stage >= 4 {
                return Err(Error::Invalid("texture binding stage above 3"));
            }
            if t.slot != 0xff && t.slot >= 64 {
                return Err(Error::Invalid("texture binding slot above 63"));
            }
        }
        for v in &self.variants {
            v.stages.check(self.stages.len(), "spas combiner stages")?;
        }
        Ok(())
    }
}

/// Xbox `D3DRENDERSTATETYPE` names for build 5849. Indices 0-91 are checked
/// against the engine's NV2A method table at `0x43e6f0` (`0x00018900`
/// pushes `table[state]`); 136-165 against the complex-state setter table
/// at `0x4038d8`, whose entries match the XDK `D3DDevice_SetRenderState_*`
/// signatures. 92-135 (deferred states) follow the XDK ordering only.
pub fn render_state_name(state: u8) -> Option<&'static str> {
    const PS: [&str; 92] = [
        "PSALPHAINPUTS0",
        "PSALPHAINPUTS1",
        "PSALPHAINPUTS2",
        "PSALPHAINPUTS3",
        "PSALPHAINPUTS4",
        "PSALPHAINPUTS5",
        "PSALPHAINPUTS6",
        "PSALPHAINPUTS7",
        "PSFINALCOMBINERINPUTSABCD",
        "PSFINALCOMBINERINPUTSEFG",
        "PSCONSTANT0_0",
        "PSCONSTANT0_1",
        "PSCONSTANT0_2",
        "PSCONSTANT0_3",
        "PSCONSTANT0_4",
        "PSCONSTANT0_5",
        "PSCONSTANT0_6",
        "PSCONSTANT0_7",
        "PSCONSTANT1_0",
        "PSCONSTANT1_1",
        "PSCONSTANT1_2",
        "PSCONSTANT1_3",
        "PSCONSTANT1_4",
        "PSCONSTANT1_5",
        "PSCONSTANT1_6",
        "PSCONSTANT1_7",
        "PSALPHAOUTPUTS0",
        "PSALPHAOUTPUTS1",
        "PSALPHAOUTPUTS2",
        "PSALPHAOUTPUTS3",
        "PSALPHAOUTPUTS4",
        "PSALPHAOUTPUTS5",
        "PSALPHAOUTPUTS6",
        "PSALPHAOUTPUTS7",
        "PSRGBINPUTS0",
        "PSRGBINPUTS1",
        "PSRGBINPUTS2",
        "PSRGBINPUTS3",
        "PSRGBINPUTS4",
        "PSRGBINPUTS5",
        "PSRGBINPUTS6",
        "PSRGBINPUTS7",
        "PSCOMPAREMODE",
        "PSFINALCOMBINERCONSTANT0",
        "PSFINALCOMBINERCONSTANT1",
        "PSRGBOUTPUTS0",
        "PSRGBOUTPUTS1",
        "PSRGBOUTPUTS2",
        "PSRGBOUTPUTS3",
        "PSRGBOUTPUTS4",
        "PSRGBOUTPUTS5",
        "PSRGBOUTPUTS6",
        "PSRGBOUTPUTS7",
        "PSCOMBINERCOUNT",
        "PSTEXTUREMODES_UNUSED",
        "PSDOTMAPPING",
        "PSINPUTTEXTURE",
        "ZFUNC",
        "ALPHAFUNC",
        "ALPHABLENDENABLE",
        "ALPHATESTENABLE",
        "ALPHAREF",
        "SRCBLEND",
        "DESTBLEND",
        "ZWRITEENABLE",
        "DITHERENABLE",
        "SHADEMODE",
        "COLORWRITEENABLE",
        "STENCILZFAIL",
        "STENCILPASS",
        "STENCILFUNC",
        "STENCILREF",
        "STENCILMASK",
        "STENCILWRITEMASK",
        "BLENDOP",
        "BLENDCOLOR",
        "SWATHWIDTH",
        "POLYGONOFFSETZSLOPESCALE",
        "POLYGONOFFSETZOFFSET",
        "POINTOFFSETENABLE",
        "WIREFRAMEOFFSETENABLE",
        "SOLIDOFFSETENABLE",
        "DEPTHCLIPCONTROL",
        "STIPPLEENABLE",
        "SIMPLE_UNUSED8",
        "SIMPLE_UNUSED7",
        "SIMPLE_UNUSED6",
        "SIMPLE_UNUSED5",
        "SIMPLE_UNUSED4",
        "SIMPLE_UNUSED3",
        "SIMPLE_UNUSED2",
        "SIMPLE_UNUSED1",
    ];
    const DEFERRED: [&str; 44] = [
        "FOGENABLE",
        "FOGTABLEMODE",
        "FOGSTART",
        "FOGEND",
        "FOGDENSITY",
        "RANGEFOGENABLE",
        "WRAP0",
        "WRAP1",
        "WRAP2",
        "WRAP3",
        "LIGHTING",
        "SPECULARENABLE",
        "LOCALVIEWER",
        "COLORVERTEX",
        "BACKSPECULARMATERIALSOURCE",
        "BACKDIFFUSEMATERIALSOURCE",
        "BACKAMBIENTMATERIALSOURCE",
        "BACKEMISSIVEMATERIALSOURCE",
        "SPECULARMATERIALSOURCE",
        "DIFFUSEMATERIALSOURCE",
        "AMBIENTMATERIALSOURCE",
        "EMISSIVEMATERIALSOURCE",
        "BACKAMBIENT",
        "AMBIENT",
        "POINTSIZE",
        "POINTSIZE_MIN",
        "POINTSPRITEENABLE",
        "POINTSCALEENABLE",
        "POINTSCALE_A",
        "POINTSCALE_B",
        "POINTSCALE_C",
        "POINTSIZE_MAX",
        "PATCHEDGESTYLE",
        "PATCHSEGMENTS",
        "SWAPFILTER",
        "PRESENTATIONINTERVAL",
        "DEFERRED_UNUSED8",
        "DEFERRED_UNUSED7",
        "DEFERRED_UNUSED6",
        "DEFERRED_UNUSED5",
        "DEFERRED_UNUSED4",
        "DEFERRED_UNUSED3",
        "DEFERRED_UNUSED2",
        "DEFERRED_UNUSED1",
    ];
    const COMPLEX: [&str; 30] = [
        "PSTEXTUREMODES",
        "VERTEXBLEND",
        "FOGCOLOR",
        "FILLMODE",
        "BACKFILLMODE",
        "TWOSIDEDLIGHTING",
        "NORMALIZENORMALS",
        "ZENABLE",
        "STENCILENABLE",
        "STENCILFAIL",
        "FRONTFACE",
        "CULLMODE",
        "TEXTUREFACTOR",
        "ZBIAS",
        "LOGICOP",
        "EDGEANTIALIAS",
        "MULTISAMPLEANTIALIAS",
        "MULTISAMPLEMASK",
        "MULTISAMPLEMODE",
        "MULTISAMPLERENDERTARGETMODE",
        "SHADOWFUNC",
        "LINEWIDTH",
        "SAMPLEALPHA",
        "DXT1NOISEENABLE",
        "YUVENABLE",
        "OCCLUSIONCULLENABLE",
        "STENCILCULLENABLE",
        "ROPZCMPALWAYSREAD",
        "ROPZREAD",
        "DONOTCULLUNCOMPRESSED",
    ];
    let s = usize::from(state);
    match s {
        0..=91 => Some(PS[s]),
        92..=135 => Some(DEFERRED[s - 92]),
        136..=165 => Some(COMPLEX[s - 136]),
        _ => None,
    }
}

/// Xbox `D3DTEXTURESTAGESTATETYPE` names (build 5849) for the ids the
/// animation tables at `0x46703c` use. 0-11 go to the deferred stage array
/// directly (`cmp 0xc` at `0x0001abdf`), 12-21 through a dirty mask, 28
/// through `0x003f5d20`; bump-env and colour ids match the XDK order.
pub fn stage_state_name(state: u16) -> Option<&'static str> {
    const N: [&str; 31] = [
        "ADDRESSU",
        "ADDRESSV",
        "ADDRESSW",
        "MAGFILTER",
        "MINFILTER",
        "MIPFILTER",
        "MIPMAPLODBIAS",
        "MAXMIPLEVEL",
        "MAXANISOTROPY",
        "COLORKEYOP",
        "COLORSIGN",
        "ALPHAKILL",
        "COLOROP",
        "COLORARG0",
        "COLORARG1",
        "COLORARG2",
        "ALPHAOP",
        "ALPHAARG0",
        "ALPHAARG1",
        "ALPHAARG2",
        "RESULTARG",
        "TEXTURETRANSFORMFLAGS",
        "BUMPENVMAT00",
        "BUMPENVMAT01",
        "BUMPENVMAT11",
        "BUMPENVMAT10",
        "BUMPENVLSCALE",
        "BUMPENVLOFFSET",
        "TEXCOORDINDEX",
        "BORDERCOLOR",
        "COLORKEYCOLOR",
    ];
    N.get(usize::from(state)).copied()
}

/// Animatable render states (`0x467020`, index = binding target):
/// `(state, component, kind)` with kind 0 = colour, 1 = float, 2 = integer.
pub const ANIMATED_RENDER_STATES: [(u8, u8, u8); 7] = [
    (0x4b, 0, 0),
    (0x4b, 1, 0),
    (0x3d, 2, 2),
    (0x4d, 3, 1),
    (0x4e, 4, 1),
    (0x9d, 5, 1),
    (0x8a, 6, 0),
];
/// Animatable texture-stage states (`0x46703c`), same layout.
pub const ANIMATED_STAGE_STATES: [(u8, u8, u8); 10] = [
    (0x06, 0, 1),
    (0x1e, 1, 0),
    (0x1d, 2, 0),
    (0x1d, 3, 0),
    (0x16, 4, 1),
    (0x17, 5, 1),
    (0x19, 6, 1),
    (0x18, 7, 1),
    (0x1a, 8, 1),
    (0x1b, 9, 1),
];

#[cfg(test)]
#[path = "shader_tests.rs"]
mod tests;
