//! Resolved materials: a `shad` turned into the ordered passes the engine
//! draws, each with its texture stages, constants and render state, in the
//! order and with the semantics of the draw-time setup `0x00016b90`.
//!
//! Engine-driven values (game time, object functions, lighting) are taken
//! from an [`EngineInputs`] snapshot; the defaults are zero except the three
//! inputs the instance root supplies. Animated instance parameters are listed
//! with their targets but not evaluated (the mapping-function evaluator
//! `0x0013b390` is not reimplemented here). See `docs/formats/shader-instances.md`.
use crate::bytes::u32_at;
use crate::shader::*;
use crate::{Error, Result};
use map::{FourCC, TagId};
use std::collections::HashMap;

/// Engine inputs for one draw (`0x0001ae70` fills them from the masks in
/// each implementation). Integer inputs select combiner variants.
#[derive(Clone, Debug, PartialEq)]
pub struct EngineInputs {
    pub ints: [i32; 32],
    pub scalars: [f32; 64],
    pub vectors: [[f32; 3]; 64],
}
impl Default for EngineInputs {
    fn default() -> Self {
        Self {
            ints: [0; 32],
            scalars: [0.0; 64],
            vectors: [[0.0; 3]; 64],
        }
    }
}
impl EngineInputs {
    /// Zero inputs plus the ones the instance root provides: integers 4 and
    /// 5 (`0x0003408f`/`0x00034097`) and scalar 21 (`0x000337e0`).
    pub fn for_shader(shader: &ShaderInstance) -> Self {
        let mut e = Self::default();
        e.ints[4] = i32::from(shader.int_inputs[0]);
        e.ints[5] = i32::from(shader.int_inputs[1]);
        e.scalars[21] = shader.scalar_input;
        e
    }
}

/// Where a register value came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// A static instance parameter.
    Instance,
    /// Computed from [`EngineInputs`]; recompute per draw.
    Engine,
}

/// What a texture stage samples.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TextureSource {
    /// An instance bitmap. `image` is the selected image (frame), wrapped
    /// by the bitmap's image count as `0x0001b0a0` does.
    Bitmap {
        bitmap: TagId,
        image: u32,
        lod_offset: f32,
    },
    /// The parameter's bitmap is null: the rasterizer default texture
    /// (`[0x485a80] + 0x64`).
    Default,
    /// The slot is not written by this implementation's bindings (the engine
    /// keeps whatever the previous draw left).
    UnsetSlot(u8),
    /// The active BSP lightmap (engine source 11).
    Lightmap,
    /// Another engine texture (`0x00019530` source id).
    Engine(u8),
    /// Explicitly no texture (engine source 0).
    None,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StageTexture {
    pub source: TextureSource,
    /// Width/height the binding writes back to its *slot* (`0x00018b90` /
    /// `0x00018ba2`): only when it asks (`flags & 2`), the slot's raw frame
    /// is below the image count (`0x00018b31`, before any wrapping) and the
    /// image has flag `0x10`. Later bindings of the slot see it, this stage
    /// does not.
    pub size: Option<(u16, u16)>,
    /// Width/height the stage carries, which engine dimension modes read
    /// (`0x0001b435`). `0x00018afb` copies the whole slot to the stage
    /// *before* the refresh above, so this is the slot's earlier size: zero
    /// from the resolver's fresh slot state unless an earlier binding of the
    /// same implementation refreshed it. `None` when unknown (engine sources,
    /// whose size `0x00019530` writes, and slots refreshed from the
    /// rasterizer default texture).
    pub stage_size: Option<(u16, u16)>,
}

/// One float4 register with the components this material writes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Register {
    pub index: u8,
    pub value: [f32; 4],
    /// Bit 0 = x ... bit 3 = w.
    pub written: u8,
    pub origin: Origin,
}

/// Where an engine vector input landed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VectorTarget {
    /// Vertex register `reg` of `bank` (1: the constant `c[18 + reg]`, 0:
    /// the default `v[reg]`, as [`Register::index`]); the input's x, y and
    /// z in the `components` still its own (bit 0 = x).
    Vertex { bank: u8, reg: u8, components: u8 },
    /// Combiner constant `slot` (0..7 the stages', 8 the final
    /// combiner's) of C1 or C0; the input packed with
    /// [`pack_engine_colour`] (`half` its option) in the byte lanes of
    /// `mask` still its own. A scalar goes through [`engine_scalar`] with
    /// `transform` first (`0x00019ca0`; vectors ignore it).
    Pixel { c1: bool, slot: u8, mask: u32, half: bool, transform: u8 },
}

/// One engine vector input's binding ([`ResolvedImplementation::vector_bindings`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VectorBinding {
    /// The engine vector input.
    pub source: u8,
    pub target: VectorTarget,
}

/// An animated target, not evaluated.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimatedTarget {
    pub kind: AnimationKind,
    pub binding: AnimationBinding,
    /// Active pixel destination (C1, slot) after fragment selection. Slot
    /// 8 is the final combiner. Inactive and non-pixel bindings have none.
    pub pixel_register: Option<(bool, u8)>,
    /// The property components feeding it.
    pub properties: Vec<PropertyRef>,
    /// A [`AnimationKind::Bitmap`] target's texture stages: each stage this
    /// implementation's texture bindings copy from the target's slot
    /// (`binding.target`; `0x00018afb` copies the whole slot, its frame
    /// included, after the animation wrote it), the last binding of a stage
    /// winning. Empty for the other kinds.
    pub stages: Vec<FrameStage>,
}

/// One texture stage an animated bitmap slot feeds ([`AnimatedTarget::stages`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameStage {
    pub stage: u8,
    /// The image count of the slot's bitmap, which `0x0001b0a0` wraps the
    /// frame by; 0 when the slot holds no instance bitmap (the rasterizer
    /// default texture, or no parameter).
    pub images: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimationKind {
    Bitmap,
    RenderState,
    StageState,
    PixelConstant,
    VertexConstant,
}

impl AnimationKind {
    /// Direct constants dereference the range start even when its count is
    /// zero. Texture transform modes use the bitmap slot instead.
    pub fn property_refs(self, sp: &ShaderPostprocess, binding: AnimationBinding) -> &[PropertyRef] {
        let Some(range) = sp.property_ranges.get(usize::from(binding.properties)) else { return &[] };
        let word = u16::from_le_bytes([binding.target, binding.b1]);
        match self {
            Self::VertexConstant if (word >> 5) & 31 >= 5 => &[],
            Self::VertexConstant | Self::PixelConstant => {
                sp.property_refs.get(usize::from(range.start)).map_or(&[], std::slice::from_ref)
            }
            _ => sp.property_refs.get(range.indices()).unwrap_or(&[]),
        }
    }
}

/// One resolved implementation of a pass: everything a D3D/WGSL layer needs.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedImplementation {
    pub index: usize,
    pub vertex_shader: Option<TagId>,
    /// Texture per stage 0-3.
    pub textures: [Option<StageTexture>; 4],
    /// Sampler state per stage (range position = stage).
    pub stage_states: Vec<StageStates>,
    /// Render states applied on entry, in order.
    pub render_states: Vec<RenderStateValue>,
    /// Render states applied when leaving this implementation.
    pub restore_states: Vec<RenderStateValue>,
    /// The final `D3DPIXELSHADERDEF` (variant combiners built, constants patched).
    pub pixel_shader: [u8; PSDEF_SIZE],
    /// True when the combiners came from fragments (`0x0001ba00`).
    pub variant: bool,
    /// Constant dwords written: bit n of `c0`/`c1` = stage n, bit 8 = final.
    pub c0_written: u16,
    pub c1_written: u16,
    pub pixel_constants_from_engine: bool,
    /// Reads an engine input that can change from draw to draw: anything
    /// but the instance's own integers 4 and 5 and scalar 21 (which
    /// `0x0001bbf0` copies from the `shad` before each draw) and the stage
    /// sizes. Without it the resolved implementation (variant, constants)
    /// is the same for every draw of the shader ([`per_draw_inputs`]).
    pub per_draw_inputs: bool,
    /// Every engine input the implementation reads ([`input_reads`]).
    pub input_reads: InputReads,
    /// Where its engine vector inputs landed (mode-4 engine constants), so
    /// another value of those inputs can be put in without resolving
    /// again.
    pub vector_bindings: Vec<VectorBinding>,
    /// Where its engine scalar inputs landed (modes 0 to 3), the same way:
    /// `source` a scalar input; a vertex target's `components` the one
    /// component it writes, a pixel target's `mask` its lanes (the
    /// constant's transform not kept: enough to tell what reads it).
    pub scalar_bindings: Vec<VectorBinding>,
    /// Hardware vertex constants (`c[18 + i]`, `0x00018eab`).
    pub vertex_constants: Vec<Register>,
    /// Vertex attribute defaults `v[i]` (`SET_VERTEX_DATA4F_M`, `0x00018e3b`).
    pub vertex_defaults: Vec<Register>,
    pub animated: Vec<AnimatedTarget>,
    /// Engine-driven render-state / stage-state bindings, raw.
    pub engine_render_states: Vec<ExternalBinding>,
    pub engine_stage_states: Vec<ExternalBinding>,
    /// The engine vertex constants that read a stage's texture size (modes
    /// 5..20), in the order `0x00019be0` applies them: their resolved
    /// values hold the size the resolver knew (0 by 0 for an engine
    /// texture's, which `0x00019530` sets when it binds it), so a host
    /// binding an engine texture puts its size in again
    /// ([`EngineVertexConstant::eval_size`]).
    pub dimension_bindings: Vec<EngineVertexConstant>,
    /// The implementation's flag bit 20 ([`Implementation::copies_target`]):
    /// a flagged shader's draw with it calls `0x0003bea0`, which, when the
    /// frame's structure target copy is not made yet, clears the shader
    /// cache and makes the copy.
    pub copies_target: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedPass {
    /// Index within the layer.
    pub index: usize,
    pub pass: TagId,
    pub implementations: Vec<ResolvedImplementation>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedLayer {
    pub lod: usize,
    pub layer: usize,
    pub passes: Vec<ResolvedPass>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    pub shader: TagId,
    pub template: TagId,
    pub inputs: EngineInputs,
    pub layers: Vec<ResolvedLayer>,
}

impl Material {
    /// Whether the material is built from tag `tag`: its shader, its
    /// template, a pass or an implementation's vertex shader. Matched by
    /// tag index (the datum's low half), as a guest datum and the map's
    /// index entry name one tag whatever their salts
    /// (docs/editor/realtime-edits.md, "The native refresh").
    pub fn uses(&self, tag: TagId) -> bool {
        let same = |t: TagId| t.0 & 0xffff == tag.0 & 0xffff;
        same(self.shader)
            || same(self.template)
            || self.layers.iter().flat_map(|l| &l.passes).any(|p| {
                same(p.pass) || p.implementations.iter().any(|i| i.vertex_shader.is_some_and(same))
            })
    }
}

/// Parameter storage shared by consecutive full material binds. The two
/// banks are vertex attribute defaults and hardware constants c[18..34].
/// Texture parameters retain their raw frame until copied to a stage.
#[derive(Clone, Debug, PartialEq)]
pub struct ParameterState {
    pub slots: HashMap<u8, (TextureSource, i32)>,
    pub slot_sizes: HashMap<u8, Option<(u16, u16)>>,
    pub stages: [Option<StageTexture>; 4],
    pub banks: [[[f32; 4]; 16]; 2],
    pub pixel_shader: [u8; PSDEF_SIZE],
    /// Last bound spas implementation, its exit list and fixed-function
    /// state from implementation switches. Per-draw overrides are separate.
    pub fixed: ImplementationState,
}

/// Static entry and exit lists in the shader cache. A change of shad
/// alone can rebind parameters while retaining the same spas implementation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImplementationState {
    pub current: Option<(TagId, usize)>,
    pub restore: Vec<RenderStateValue>,
    pub render: HashMap<u8, u32>,
    pub samplers: [Option<StageStates>; 4],
}

impl ImplementationState {
    fn switch(&mut self, pass: TagId, imp: &ResolvedImplementation) -> Vec<RenderStateValue> {
        let next = Some((pass, imp.index));
        if self.current == next {
            return Vec::new();
        }
        // 0x16de0 applies the outgoing implementation's restore list
        // before 0x16e2e enters the new one. Preserve duplicate setters
        // in order: different render states can share a hardware method.
        let mut writes = std::mem::take(&mut self.restore);
        writes.extend_from_slice(&imp.render_states);
        for write in &writes {
            self.render.insert(write.state, write.value);
        }
        self.restore.clone_from(&imp.restore_states);
        for (stage, &sampler) in self.samplers.iter_mut().zip(&imp.stage_states) {
            *stage = Some(sampler);
        }
        self.current = next;
        writes
    }
}

impl Default for ParameterState {
    fn default() -> Self {
        Self { slots: HashMap::new(), slot_sizes: HashMap::new(), stages: [None; 4],
            banks: [[[0.; 4]; 16]; 2], pixel_shader: [0; PSDEF_SIZE], fixed: ImplementationState::default() }
    }
}

/// One implementation selected by an already prepared scene entry.
#[derive(Clone, Copy, Debug)]
pub struct PassSelection {
    pub lod: usize,
    pub layer: usize,
    pub pass: usize,
    pub implementation: usize,
}

/// The shader a shader begin draws for a tag (`0x0001bbf0`): a `shad`'s
/// own postprocess (`+0x20`) under its root's inputs
/// ([`EngineInputs::for_shader`]); a model particle's (`PRTM`) `shad`
/// (`+0x1c`) under zero inputs; a particle's (`prt3`) embedded
/// postprocess (`+0x8c`, its template the element's `+0`) under zero
/// inputs.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawnShader {
    /// The tag whose postprocess is drawn: the `shad`, or the `prt3`.
    pub shader: TagId,
    pub template: TagId,
    pub postprocess: ShaderPostprocess,
    pub inputs: EngineInputs,
}

/// A selected tag binding and its position within the layer.
pub struct SelectedImplementation {
    /// Ordered fixed render-state setters when resolving over live state.
    pub render_writes: Vec<RenderStateValue>,
    pub pass: TagId,
    pub layer_passes: usize,
    pub implementation: ResolvedImplementation,
}

/// Most implementations one material may resolve to (summed over every
/// (lod, layer) pass list), checked from the template before any expansion.
pub const MAX_MATERIAL_IMPLEMENTATIONS: usize = 1 << 12;
/// Most passes one material may resolve to, checked with the above.
pub const MAX_MATERIAL_PASSES: usize = 1 << 12;
/// Most binding, state, combiner and animation records one material may
/// read or copy while resolving its implementations.
pub const MAX_MATERIAL_WORK: usize = 1 << 18;

/// Remaining resolution work for one material.
struct Work(usize);
impl Work {
    fn charge(&mut self, n: usize) -> Result<()> {
        self.0 = self.0.checked_sub(n).ok_or(Error::Invalid(
            "material exceeds its resolution work budget",
        ))?;
        Ok(())
    }
}

/// Engine texture 26 (`0x00019a68`): a bitmap parameter of the drawn
/// shader, which its postprocess's `+0x5c` block names. The draw context's
/// shader (`ctx + 8`) gives the postprocess (`shad +0x24`); the i16 at
/// `+0x10` of that block's data is the index into its bitmap parameters
/// (`+4`). An index of -1 takes the globals' rasterizer shader
/// (`[0x00485a80] + 0xd0`) and its postprocess instead, without checking its
/// index again.
/// The parameter's bitmap and image (the low half of `+4`) bind through
/// `0x000143c0`, which takes the rasterizer default texture (`+0x64`, image
/// 0) for a null bitmap or an image outside the bitmap, and writes the
/// image's width and height as the stage's size. No LOD offset is applied.
pub const PROPERTY_BITMAP_TEXTURE: u8 = 26;
/// Byte offset of the bitmap index in the postprocess `+0x5c` block's data.
const PROPERTY_BITMAP_INDEX: u32 = 0x10;
/// `matg +0x108`: the rasterizer data block (`[0x00485a80]` is its first
/// element); `+0xd0` the datum of its shader reference.
const RASTERIZER_DATA: u32 = 0x108;
const RASTERIZER_SHADER: u32 = 0xd0;
const MATG: FourCC = FourCC(*b"matg");

/// Tag lookup plus decoded caches over one map's metadata.
pub struct ShaderLibrary {
    meta: Metadata,
    tags: HashMap<TagId, (FourCC, u32)>,
    templates: HashMap<TagId, ShaderTemplate>,
    passes: HashMap<TagId, ShaderPass>,
    /// The rasterizer shader's parameter for engine texture 26, once read
    /// (`None` inside when the globals do not name one that reads).
    rasterizer_property: std::sync::OnceLock<Option<BitmapParameter>>,
}

/// Bitmap image facts the binding path reads (`bitm +0x44/+0x48`, image
/// stride 0x74, `+4/+6` size, `+0xe` flags).
#[derive(Clone, Copy, Debug)]
struct BitmapImage {
    width: u16,
    height: u16,
    flags: u8,
}

impl ShaderLibrary {
    pub fn new(meta: Metadata, tags: impl IntoIterator<Item = (TagId, FourCC, u32)>) -> Self {
        Self {
            meta,
            tags: tags.into_iter().map(|(i, g, a)| (i, (g, a))).collect(),
            templates: HashMap::new(),
            passes: HashMap::new(),
            rasterizer_property: std::sync::OnceLock::new(),
        }
    }
    /// Loads the metadata region and tag table of a map.
    pub fn read<R: std::io::Read + std::io::Seek>(map: &mut map::Map<R>) -> Result<Self> {
        let meta = Metadata::read(map)?;
        Ok(Self::with_metadata(map, meta))
    }
    pub fn with_metadata<R: std::io::Read + std::io::Seek>(map: &map::Map<R>, meta: Metadata) -> Self {
        let tags = map
            .tags()
            .iter()
            .filter_map(|t| t.location.map(|l| (t.id, t.group, l.address)));
        Self::new(meta, tags)
    }
    pub fn metadata(&self) -> &Metadata {
        &self.meta
    }
    fn address(&self, id: TagId, group: FourCC) -> Result<u32> {
        match self.tags.get(&id) {
            Some(&(g, a)) if g == group => Ok(a),
            Some(_) => Err(Error::Invalid("tag reference has the wrong group")),
            None => Err(Error::Invalid("tag reference does not resolve")),
        }
    }
    pub fn group_of(&self, id: TagId) -> Option<FourCC> {
        self.tags.get(&id).map(|t| t.0)
    }
    pub fn shader(&self, id: TagId) -> Result<ShaderInstance> {
        ShaderInstance::decode(&self.meta, self.address(id, SHAD)?)
    }
    pub fn template(&mut self, id: TagId) -> Result<&ShaderTemplate> {
        if !self.templates.contains_key(&id) {
            let t = ShaderTemplate::decode(&self.meta, self.address(id, STEM)?)?;
            self.templates.insert(id, t);
        }
        Ok(&self.templates[&id])
    }
    pub fn pass(&mut self, id: TagId) -> Result<&ShaderPass> {
        if !self.passes.contains_key(&id) {
            let p = ShaderPass::decode(&self.meta, self.address(id, SPAS)?)?;
            self.passes.insert(id, p);
        }
        Ok(&self.passes[&id])
    }
    /// The image count of a bitmap and, when `index` is in range, that
    /// image. Reads one image; the whole array must lie in the region.
    fn bitmap_image(&self, id: TagId, index: Option<usize>) -> Result<(u32, Option<BitmapImage>)> {
        let a = self.address(id, BITM)?;
        let root = self.meta.bytes(a, 0x4c)?;
        let n = u32::from_le_bytes(root[0x44..0x48].try_into().expect("4"));
        let p = u32::from_le_bytes(root[0x48..0x4c].try_into().expect("4"));
        if n > MAX_BLOCK_ELEMENTS {
            return Err(Error::Invalid(
                "bitmap image count exceeds the element budget",
            ));
        }
        let b = if n == 0 {
            &[][..]
        } else {
            self.meta.bytes(p, n as usize * 0x74)?
        };
        let image = index
            .and_then(|k| b.as_chunks::<0x74>().0.get(k))
            .map(|i| BitmapImage {
                width: u16::from_le_bytes([i[4], i[5]]),
                height: u16::from_le_bytes([i[6], i[7]]),
                flags: i[0xe],
            });
        Ok((n, image))
    }

    /// The bitmap parameter index a postprocess's `+0x5c` block names
    /// ([`PROPERTY_BITMAP_TEXTURE`]); `None` when its data does not read.
    fn property_index(&self, sp: &ShaderPostprocess) -> Option<i16> {
        let at = sp.opaque_blocks[0].address.checked_add(PROPERTY_BITMAP_INDEX)?;
        let b = self.meta.bytes(at, 2).ok()?;
        Some(i16::from_le_bytes([b[0], b[1]]))
    }

    /// The globals' rasterizer shader's parameter for engine texture 26:
    /// its own index, taken as is (`0x00019a90`..`0x00019aae`).
    fn rasterizer_property(&self) -> Option<BitmapParameter> {
        *self.rasterizer_property.get_or_init(|| {
            let globals = self.tags.values().find(|(g, _)| *g == MATG)?.1;
            let block = self.meta.bytes(globals.checked_add(RASTERIZER_DATA)?, 8).ok()?;
            let (count, first) = (u32_at(block, 0), u32_at(block, 4));
            if count == 0 {
                return None;
            }
            let datum = u32_at(self.meta.bytes(first.checked_add(RASTERIZER_SHADER)?, 4).ok()?, 0);
            let shader = self.shader(TagId(datum)).ok()?;
            let sp = shader.compiled().ok()?;
            let index = usize::try_from(self.property_index(sp)?).ok()?;
            sp.bitmaps.get(index).copied()
        })
    }

    /// What engine texture 26 binds for a shader whose postprocess is `sp`
    /// ([`PROPERTY_BITMAP_TEXTURE`]): the parameter's image, or the
    /// rasterizer default texture when `0x000143c0` falls back to it.
    /// `None` when an index or tag does not read; the stage then stays
    /// [`TextureSource::Engine`] for a consumer to refuse.
    fn property_bitmap(&self, sp: &ShaderPostprocess) -> Option<StageTexture> {
        let parameter = match self.property_index(sp)? {
            -1 => self.rasterizer_property()?,
            i => *sp.bitmaps.get(usize::try_from(i).ok()?)?,
        };
        let fallback = StageTexture { source: TextureSource::Default, size: None, stage_size: None };
        if parameter.bitmap == TagId::NULL {
            return Some(fallback);
        }
        // `mov cx, word [eax + 4]`; `0x000143f4` sign-extends it.
        let image = parameter.image as u16 as i16;
        let Ok(image) = usize::try_from(image) else { return Some(fallback) };
        // An image past the bitmap's count reads as none.
        let Some(found) = self.bitmap_image(parameter.bitmap, Some(image)).ok()?.1 else {
            return Some(fallback);
        };
        Some(StageTexture {
            source: TextureSource::Bitmap { bitmap: parameter.bitmap, image: image as u32, lod_offset: 0.0 },
            size: None,
            stage_size: Some((found.width, found.height)),
        })
    }

    /// Resolves every (lod, layer) pass list of a shader instance.
    ///
    /// Every tag the material reaches is decoded and validated once, and the
    /// total pass and implementation counts are checked against
    /// [`MAX_MATERIAL_PASSES`] / [`MAX_MATERIAL_IMPLEMENTATIONS`] before
    /// anything is expanded; the records read while resolving are charged
    /// against [`MAX_MATERIAL_WORK`]. Valid ranges may select the same pass
    /// and implementation data many times, so the decoder budgets alone do
    /// not bound the output.
    pub fn material(&mut self, shader_id: TagId, inputs: Option<EngineInputs>) -> Result<Material> {
        let shader = self.shader(shader_id)?;
        let sp = shader.compiled()?;
        sp.validate()?;
        let inputs = inputs.unwrap_or_else(|| EngineInputs::for_shader(&shader));
        let template_id = shader.template.id;
        let tp = self.template(template_id)?.compiled()?;
        tp.validate()?;
        let (mut passes_total, mut imps_total) = (0usize, 0usize);
        let mut lists = Vec::new();
        let mut pass_ids = Vec::new();
        for (lod, l) in tp.lods.iter().enumerate() {
            for layer in 0..usize::from(l.layers.count).min(32) {
                let Some(list) = tp.layer_passes(lod, layer) else {
                    continue;
                };
                if list.is_empty() {
                    continue;
                }
                passes_total += list.len();
                imps_total += list
                    .iter()
                    .map(|p| usize::from(p.implementations.count))
                    .sum::<usize>();
                if passes_total > MAX_MATERIAL_PASSES || imps_total > MAX_MATERIAL_IMPLEMENTATIONS {
                    return Err(Error::Invalid(
                        "material expands past its pass/implementation budget",
                    ));
                }
                pass_ids.extend(list.iter().map(|p| p.pass.id));
                lists.push((lod, layer));
            }
        }
        pass_ids.sort_unstable_by_key(|id| id.0);
        pass_ids.dedup();
        for id in pass_ids {
            self.pass(id)?.compiled()?.validate()?;
        }

        let this = &*self;
        let tp = this.templates[&template_id].compiled()?;
        let mut work = Work(MAX_MATERIAL_WORK);
        let mut layers = Vec::with_capacity(lists.len());
        for (lod, layer) in lists {
            let list = tp.layer_passes(lod, layer).expect("listed above");
            let mut passes = Vec::with_capacity(list.len());
            for (pi, p) in list.iter().enumerate() {
                let spas = this.passes[&p.pass.id].compiled()?;
                if usize::from(p.implementations.count) != spas.implementations.len() {
                    return Err(Error::Invalid(
                        "template binding rows do not match the pass's implementations",
                    ));
                }
                let mut imps = Vec::with_capacity(spas.implementations.len());
                for (k, imp) in spas.implementations.iter().enumerate() {
                    let ib = tp.implementations[usize::from(p.implementations.start) + k];
                    let anim = sp.animation_leaf(lod, layer, pi, k).copied();
                    imps.push(this.resolve(tp, sp, spas, imp, k, ib, anim, &inputs, &mut work, None)?);
                }
                passes.push(ResolvedPass {
                    index: pi,
                    pass: p.pass.id,
                    implementations: imps,
                });
            }
            layers.push(ResolvedLayer { lod, layer, passes });
        }
        Ok(Material {
            shader: shader_id,
            template: template_id,
            inputs,
            layers,
        })
    }

    /// Apply a full parameter rebind to the supplied incoming storage.
    /// Only the selected binding rows execute. An unreadable tag leaves
    /// the incoming state intact. Animated properties need their
    /// evaluator before the static result can be published.
    pub fn selected_implementation(
        &mut self, shader_id: TagId, selection: PassSelection, inputs: Option<&EngineInputs>,
        state: Option<&mut ParameterState>,
    ) -> Result<SelectedImplementation> {
        let shader = self.shader(shader_id)?;
        let defaults = EngineInputs::for_shader(&shader);
        let inputs = inputs.unwrap_or(&defaults);
        self.select(shader.template.id, shader.compiled()?, selection, inputs, state)
    }

    /// What the shader begin draws for the tag `id`: a `shad`, or the
    /// particle tags a particle draw passes in its place
    /// ([`DrawnShader`]).
    pub fn drawn_shader(&self, id: TagId) -> Result<DrawnShader> {
        match self.group_of(id) {
            Some(SHAD) => {
                let shader = self.shader(id)?;
                let inputs = EngineInputs::for_shader(&shader);
                let postprocess = shader.compiled()?.clone();
                Ok(DrawnShader { shader: id, template: shader.template.id, postprocess, inputs })
            }
            Some(PRTM) => {
                let root = self.meta.bytes(self.address(id, PRTM)?, PRTM_ROOT)?;
                let shad = TagId(u32_at(root, PRTM_SHADER + 4));
                let shader = self.shader(shad)?;
                let postprocess = shader.compiled()?.clone();
                Ok(DrawnShader { shader: shad, template: shader.template.id, postprocess, inputs: EngineInputs::default() })
            }
            Some(PRT3) => {
                let postprocess = ShaderPostprocess::decode_particle(&self.meta, self.address(id, PRT3)?)?;
                Ok(DrawnShader { shader: id, template: postprocess.template, postprocess, inputs: EngineInputs::default() })
            }
            Some(_) => Err(Error::Invalid("tag is not a shader or a particle")),
            None => Err(Error::Invalid("tag reference does not resolve")),
        }
    }

    /// [`Self::selected_implementation`] for a [`DrawnShader`], under its
    /// own inputs.
    pub fn selected_drawn(&mut self, drawn: &DrawnShader, selection: PassSelection) -> Result<SelectedImplementation> {
        self.select(drawn.template, &drawn.postprocess, selection, &drawn.inputs, None)
    }

    fn select(
        &mut self, template: TagId, sp: &ShaderPostprocess, selection: PassSelection, inputs: &EngineInputs,
        mut state: Option<&mut ParameterState>,
    ) -> Result<SelectedImplementation> {
        sp.validate()?;
        let tp = self.template(template)?.compiled()?;
        tp.validate()?;
        let lod = tp.lods.get(selection.lod).ok_or(Error::Invalid("missing selected LOD"))?;
        if selection.layer >= usize::from(lod.layers.count) { return Err(Error::Invalid("missing selected layer")); }
        let passes = tp.layer_passes(selection.lod, selection.layer).ok_or(Error::Invalid("missing selected layer"))?;
        let layer_passes = passes.len();
        let pass = *passes.get(selection.pass).ok_or(Error::Invalid("missing selected pass"))?;
        let k = selection.implementation;
        if k >= usize::from(pass.implementations.count) {
            return Err(Error::Invalid("missing selected implementation"));
        }
        let ib = tp.implementations[usize::from(pass.implementations.start) + k];
        self.pass(pass.pass.id)?.compiled()?.validate()?;
        let tp = self.templates[&template].compiled()?;
        let spas = self.passes[&pass.pass.id].compiled()?;
        if usize::from(pass.implementations.count) != spas.implementations.len() {
            return Err(Error::Invalid("template binding rows do not match the pass's implementations"));
        }
        let anim = sp.animation_leaf(selection.lod, selection.layer, selection.pass, k).copied();
        let implementation = self.resolve(tp, sp, spas, &spas.implementations[k], k, ib, anim, inputs,
            &mut Work(MAX_MATERIAL_WORK), state.as_deref_mut())?;
        let render_writes = state.map(|s| s.fixed.switch(pass.pass.id, &implementation)).unwrap_or_default();
        Ok(SelectedImplementation { pass: pass.pass.id, layer_passes, implementation, render_writes })
    }

    fn resolve(
        &self,
        tp: &TemplatePostprocess,
        sp: &ShaderPostprocess,
        spas: &PassPostprocess,
        imp: &Implementation,
        index: usize,
        ib: ImplementationBindings,
        anim: Option<AnimationRanges>,
        inputs: &EngineInputs,
        work: &mut Work,
        state: Option<&mut ParameterState>,
    ) -> Result<ResolvedImplementation> {
        work.charge(
            1 + [
                ib.bitmaps,
                ib.pixel_constants,
                ib.vertex_constants,
                imp.textures,
                imp.render_states,
                imp.stage_states,
                imp.restore_states,
                imp.fragments,
                imp.animated_render_states,
                imp.animated_stage_states,
                imp.external_pixel_constants,
                imp.external_vertex_constants,
            ]
            .iter()
            .map(|r| usize::from(r.count))
            .sum::<usize>(),
        )?;
        // 0x00016f60: bitmap parameters into texture slots, with the raw
        // frame (`+0x18`; 0 for the default texture).
        let mut slots = state.as_ref().map(|s| s.slots.clone()).unwrap_or_default();
        for b in &tp.bindings[ib.bitmaps.indices()] {
            let slot = (b.word & 0xff) as u8;
            if slot >= 64 {
                return Err(Error::Invalid("bitmap binding slot above 63"));
            }
            let p = sp
                .bitmaps
                .get(usize::from(b.source))
                .ok_or(Error::Invalid("bitmap binding names a missing parameter"))?;
            let src = if p.bitmap == TagId::NULL {
                (TextureSource::Default, 0)
            } else {
                (
                    TextureSource::Bitmap {
                        bitmap: p.bitmap,
                        image: p.image.max(0) as u32,
                        lod_offset: p.lod_offset,
                    },
                    p.image,
                )
            };
            slots.insert(slot, src);
        }

        // 0x00017000: instance vectors into vertex defaults (bank 0) and
        // vertex constants (bank 1).
        let mut banks = state.as_ref().map(|s| s.banks).unwrap_or_default();
        let mut written = [[0u8; 16]; 2];
        let mut origin = [[Origin::Instance; 16]; 2];
        for b in &tp.bindings[ib.vertex_constants.indices()] {
            let v = sp
                .vectors
                .get(usize::from(b.source))
                .ok_or(Error::Invalid("vector binding names a missing parameter"))?;
            let reg = usize::from(b.word & 0xf);
            let bank = usize::from((b.word >> 4) & 1);
            let mode = ((b.word >> 5) & 0x1f) as usize;
            let mask = *VECTOR_MODE_MASKS
                .get(mode)
                .ok_or(Error::Invalid("vector binding component mode above 20"))?;
            for c in 0..4 {
                if mask & (1 << c) != 0 {
                    banks[bank][reg][c] = v[c];
                }
            }
            written[bank][reg] |= 0xf; // the upload sends the whole register
        }

        // 0x0001ba00 / 0x00016e81: the pixel shader.
        let variant = !imp.fragments.is_empty();
        let mut ps = initial_definition(&imp.pixel_shader, variant, state.as_ref().map(|s| &s.pixel_shader));
        let mut frag_offset = [0u8; 8];
        if variant {
            build_variant(&mut ps, &mut frag_offset, spas, imp, inputs, work)?;
        }

        // 0x00017170: instance colours into combiner constants.
        let (mut c0w, mut c1w) = (0u16, 0u16);
        for b in &tp.bindings[ib.pixel_constants.indices()] {
            let c = *sp
                .colors
                .get(usize::from(b.source))
                .ok_or(Error::Invalid("colour binding names a missing parameter"))?;
            let mask = CHANNEL_MASKS[usize::from(b.b2 & 7)];
            apply_color(
                &mut ps,
                &mut c0w,
                &mut c1w,
                b.word,
                mask,
                c,
                spas,
                imp,
                &frag_offset,
                inputs,
            )?;
        }

        // The stage takes the slot's size before this binding refreshes it.
        let mut slot_sizes = state.as_ref().map(|s| s.slot_sizes.clone()).unwrap_or_default();
        let mut textures = state.as_ref().map(|s| s.stages).unwrap_or_default();
        // The slot each stage last copied in this implementation, with its
        // bitmap's image count ([`AnimatedTarget::stages`]).
        let mut stage_slots: [Option<(u8, u32)>; 4] = [None; 4];
        for t in &spas.textures[imp.textures.indices()] {
            let stage = textures
                .get_mut(usize::from(t.stage))
                .ok_or(Error::Invalid("texture binding stage above 3"))?;
            if t.slot == 0xff {
                let source = match t.source {
                    0 => TextureSource::None,
                    11 => TextureSource::Lightmap,
                    n => TextureSource::Engine(n),
                };
                stage_slots[usize::from(t.stage)] = None;
                let engine = StageTexture { source, size: None, stage_size: None };
                *stage = Some(match t.source {
                    PROPERTY_BITMAP_TEXTURE => self.property_bitmap(sp).unwrap_or(engine),
                    _ => engine,
                });
                continue;
            }
            let stage_size = slot_sizes.get(&t.slot).copied().unwrap_or(Some((0, 0)));
            let (mut source, frame) = slots
                .get(&t.slot)
                .copied()
                .unwrap_or((TextureSource::UnsetSlot(t.slot), 0));
            let mut size = None;
            stage_slots[usize::from(t.stage)] = Some((t.slot, 0));
            match &mut source {
                TextureSource::Bitmap { bitmap, image, .. } => {
                    let (count, _) = self.bitmap_image(*bitmap, None)?;
                    stage_slots[usize::from(t.stage)] = Some((t.slot, count));
                    // 0x00018b31 compares the raw frame with the image count
                    // and skips the refresh when frame >= count.
                    if t.flags & 2 != 0 && i64::from(frame) < i64::from(count) {
                        let k = usize::try_from(frame).map_err(|_| {
                            Error::Invalid("size refresh of a negative bitmap frame")
                        })?;
                        if let (_, Some(i)) = self.bitmap_image(*bitmap, Some(k))?
                            && i.flags & 0x10 != 0
                        {
                            size = Some((i.width, i.height));
                            slot_sizes.insert(t.slot, size);
                        }
                    }
                    // 0x0001b0a0 wraps the frame for binding, afterwards.
                    if count != 0 {
                        *image %= count;
                    }
                }
                // The rasterizer default texture's images are not decoded.
                TextureSource::Default if t.flags & 2 != 0 => {
                    slot_sizes.insert(t.slot, None);
                }
                _ => {}
            }
            *stage = Some(StageTexture {
                source,
                size,
                stage_size,
            });
        }

        // Every engine binding's target in order, with the vector input it
        // copies (mode 4), for `vector_bindings`.
        let mut engine_writes: Vec<(VectorTarget, Option<u8>)> = Vec::new();
        // The same targets with the scalar input each copies (modes 0..3).
        let mut scalar_writes: Vec<(VectorTarget, Option<u8>)> = Vec::new();

        // 0x00019be0: engine-driven vertex constants (after instance vectors).
        for e in &spas.externals[imp.external_vertex_constants.indices()] {
            let c = EngineVertexConstant::of(*e);
            let (reg, bank, mode, stage) =
                (usize::from(c.reg), usize::from(c.bank), usize::from(c.mode), usize::from(c.stage));
            let dims = stage_dims(&textures, stage);
            eval_engine(
                &mut banks[bank][reg],
                mode,
                0,
                usize::from(e.bytes[3]),
                dims,
                inputs,
            )?;
            written[bank][reg] = 0xf;
            origin[bank][reg] = Origin::Engine;
            let target = VectorTarget::Vertex {
                bank: bank as u8,
                reg: reg as u8,
                components: engine_mode_components(mode),
            };
            engine_writes.push((target, (mode == 4).then_some(e.bytes[3])));
            scalar_writes.push((target, (mode <= 3).then_some(e.bytes[3])));
        }

        // 0x00019ca0: engine-driven pixel constants.
        let pixel_from_engine = !imp.external_pixel_constants.is_empty();
        for e in &spas.externals[imp.external_pixel_constants.indices()] {
            let mode = usize::from(e.bytes[2] & 7);
            let transform = usize::from((e.bytes[2] >> 3) & 7);
            let mut v = [0f32; 4];
            eval_engine(
                &mut v,
                mode,
                transform,
                usize::from(e.bytes[3]),
                (1.0, 1.0),
                inputs,
            )?;
            let half = e.bytes[1] & 8 != 0;
            let packed = pack_engine_colour(v, half);
            let at = apply_color(
                &mut ps,
                &mut c0w,
                &mut c1w,
                e.word(),
                CHANNEL_MASKS[mode],
                packed,
                spas,
                imp,
                &frag_offset,
                inputs,
            )?;
            if let Some((c1, slot)) = at {
                let target = VectorTarget::Pixel {
                    c1,
                    slot,
                    mask: CHANNEL_MASKS[mode],
                    half,
                    transform: transform as u8,
                };
                engine_writes.push((target, (mode == 4).then_some(e.bytes[3])));
                scalar_writes.push((target, (mode <= 3).then_some(e.bytes[3])));
            }
        }

        let mut animated = Vec::new();
        if let Some(a) = anim {
            for (kind, r) in [
                (AnimationKind::Bitmap, a.bitmaps),
                (AnimationKind::RenderState, a.render_states),
                (AnimationKind::StageState, a.stage_states),
                (AnimationKind::PixelConstant, a.pixel_constants),
                (AnimationKind::VertexConstant, a.vertex_constants),
            ] {
                work.charge(usize::from(r.count))?;
                for binding in &sp.animations[r.indices()] {
                    let refs = kind.property_refs(sp, *binding);
                    work.charge(refs.len())?;
                    let pixel_register = if kind == AnimationKind::PixelConstant {
                        let w = u16::from_le_bytes([binding.target, binding.b1]);
                        colour_register(w, spas, imp, &frag_offset, inputs)?
                    } else {
                        None
                    };
                    let stages = if kind == AnimationKind::Bitmap {
                        (0..4u8)
                            .filter_map(|n| match stage_slots[usize::from(n)] {
                                Some((slot, images)) if slot == binding.target => {
                                    Some(FrameStage { stage: n, images })
                                }
                                _ => None,
                            })
                            .collect()
                    } else {
                        Vec::new()
                    };
                    animated.push(AnimatedTarget {
                        pixel_register,
                        kind,
                        binding: *binding,
                        properties: refs.to_vec(),
                        stages,
                    });
                }
            }
        }

        let regs = |bank: usize| {
            (0..16)
                .filter(|&r| written[bank][r] != 0)
                .map(|r| Register {
                    index: if bank == 1 { 18 + r as u8 } else { r as u8 },
                    value: banks[bank][r],
                    written: written[bank][r],
                    origin: origin[bank][r],
                })
                .collect::<Vec<_>>()
        };
        let vertex_constants = regs(1);
        let vertex_defaults = regs(0);
        if let Some(state) = state {
            if !animated.is_empty() {
                return Err(Error::Invalid("animated material requires the property evaluator"));
            }
            state.slots = slots;
            state.slot_sizes = slot_sizes;
            state.stages = textures;
            state.banks = banks;
            state.pixel_shader = ps;
        }
        Ok(ResolvedImplementation {
            index,
            vertex_shader: (!imp.vertex_shader.is_null()).then_some(imp.vertex_shader.id),
            textures,
            stage_states: spas.stage_states[imp.stage_states.indices()].to_vec(),
            render_states: spas.render_states[imp.render_states.indices()].to_vec(),
            restore_states: spas.render_states[imp.restore_states.indices()].to_vec(),
            pixel_shader: ps,
            variant,
            c0_written: c0w,
            c1_written: c1w,
            pixel_constants_from_engine: pixel_from_engine,
            per_draw_inputs: per_draw_inputs(spas, imp),
            input_reads: input_reads(spas, imp),
            vector_bindings: vector_bindings(&engine_writes),
            scalar_bindings: vector_bindings(&scalar_writes),
            vertex_constants,
            vertex_defaults,
            animated,
            engine_render_states: spas.externals[imp.animated_render_states.indices()].to_vec(),
            engine_stage_states: spas.externals[imp.animated_stage_states.indices()].to_vec(),
            dimension_bindings: spas.externals[imp.external_vertex_constants.indices()]
                .iter()
                .map(|&e| EngineVertexConstant::of(e))
                .filter(|c| (5..=20).contains(&c.mode))
                .collect(),
            copies_target: imp.copies_target(),
        })
    }
}

/// The engine scalar inputs the instance supplies: 21, the `shad`'s `+0x44`
/// (`0x0001bbf0` copies it to `0x485a6c`, which `0x000337e0` returns).
pub const INSTANCE_SCALARS: u64 = 1 << 21;
/// The engine integer inputs the instance supplies: 4 and 5, the `shad`'s
/// `+0x3e`/`+0x40` (`0x0001bbf0`, returned by `0x00034060`).
pub const INSTANCE_INTS: u32 = 1 << 4 | 1 << 5;

/// The engine inputs an implementation reads, bit `n` input `n` of each
/// kind: what a caller resolving it with its own [`EngineInputs`] must
/// fill.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputReads {
    pub scalars: u64,
    pub vectors: u64,
    pub ints: u32,
}

/// What `imp` reads: its input masks (spas `+0x104..+0x114`, which
/// `0x0001ae70` evaluates before the draw), the sources of its engine
/// constant bindings (modes 0 to 3 a scalar, 4 a vector) and its fragment
/// selectors (integers).
pub fn input_reads(spas: &PassPostprocess, imp: &Implementation) -> InputReads {
    let mut r = InputReads {
        scalars: imp.scalar_inputs,
        vectors: imp.vector_inputs,
        ints: imp.int_inputs,
    };
    let mut source = |mode: usize, source: u8| match mode {
        0..=3 => r.scalars |= 1u64 << (source & 63),
        4 => r.vectors |= 1u64 << (source & 63),
        _ => {}
    };
    let externals = |range: IndexRange| spas.externals.get(range.indices()).unwrap_or(&[]);
    for e in externals(imp.external_pixel_constants) {
        source(usize::from(e.bytes[2] & 7), e.bytes[3]);
    }
    for e in externals(imp.external_vertex_constants) {
        source(usize::from((e.word() >> 5) & 0x1f), e.bytes[3]);
    }
    for f in spas.fragments.get(imp.fragments.indices()).unwrap_or(&[]) {
        if let Ok(s) = u32::try_from(f.selector)
            && s < 32
        {
            r.ints |= 1 << s;
        }
    }
    r
}

/// Whether `imp` reads an engine input that can change per draw. Its input
/// masks (spas `+0x104..+0x114`) are what `0x0001ae70` evaluates before the
/// draw, bit `n` input `n`; its engine bindings and fragment selectors are
/// checked too, so a binding outside the masks still counts.
pub fn per_draw_inputs(spas: &PassPostprocess, imp: &Implementation) -> bool {
    // Modes 0..3 read a scalar, 4 a vector, 5.. the stage sizes (static).
    let reads = |mode: usize, source: u8| match mode {
        0..=3 => INSTANCE_SCALARS & (1u64 << (source & 63)) == 0,
        4 => true,
        _ => false,
    };
    let masks = imp.scalar_inputs & !INSTANCE_SCALARS != 0 || imp.vector_inputs != 0 || imp.int_inputs & !INSTANCE_INTS != 0;
    let externals = |r: IndexRange| spas.externals.get(r.indices()).unwrap_or(&[]);
    let pixel = externals(imp.external_pixel_constants)
        .iter()
        .any(|e| reads(usize::from(e.bytes[2] & 7), e.bytes[3]));
    let vertex = externals(imp.external_vertex_constants)
        .iter()
        .any(|e| reads(usize::from((e.word() >> 5) & 0x1f), e.bytes[3]));
    let selectors = spas
        .fragments
        .get(imp.fragments.indices())
        .unwrap_or(&[])
        .iter()
        .any(|f| f.selector != -1 && !u32::try_from(f.selector).is_ok_and(|s| s < 32 && INSTANCE_INTS & (1 << s) != 0));
    masks || pixel || vertex || selectors
}

/// Component masks of the 21 vector-binding modes (`0x00017154` byte map
/// into the copy routines at `0x00017130`). Bit 0 = x.
pub const VECTOR_MODE_MASKS: [u8; 21] = [
    0b0001, 0b0010, 0b0100, 0b1000, 0b0111, 0b0001, 0b0010, 0b0100, 0b1000, 0b0011, 0b1100, 0b0011,
    0b1100, 0b1111, 0b1011, 0b1011, 0b0111, 0b0111, 0b1111, 0b1111, 0b1111,
];
/// Byte-lane masks for packed ARGB colours (`0x467068`): R, G, B, A, RGB, R, G, B.
pub const CHANNEL_MASKS: [u32; 8] = [
    0x00ff_0000,
    0x0000_ff00,
    0x0000_00ff,
    0xff00_0000,
    0x00ff_ffff,
    0x00ff_0000,
    0x0000_ff00,
    0x0000_00ff,
];

fn word(ps: &[u8; PSDEF_SIZE], i: usize) -> u32 {
    u32::from_le_bytes(ps[i * 4..i * 4 + 4].try_into().expect("4"))
}
fn set_word(ps: &mut [u8; PSDEF_SIZE], i: usize, v: u32) {
    ps[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
}
// PSDEF dword indices (SDK order).
const PS_ALPHA_IN: usize = 0;
const PS_FINAL_ABCD: usize = 8;
const PS_FINAL_EFG: usize = 9;
const PS_C0: usize = 10;
const PS_C1: usize = 18;
const PS_ALPHA_OUT: usize = 26;
const PS_RGB_IN: usize = 34;
const PS_FINAL_C0: usize = 43;
const PS_FINAL_C1: usize = 44;
const PS_RGB_OUT: usize = 45;
const PS_COMBINER_COUNT: usize = 53;

fn initial_definition(embedded: &[u8; PSDEF_SIZE], variant: bool,
    previous: Option<&[u8; PSDEF_SIZE]>) -> [u8; PSDEF_SIZE] {
    let Some(previous) = previous.filter(|_| variant) else { return *embedded; };
    // 0x1ba00 changes only texture modes, dot mapping and input texture
    // before it builds the selected combiner stages in the persistent buffer.
    let mut ps = *previous;
    ps[54 * 4..57 * 4].copy_from_slice(&embedded[54 * 4..57 * 4]);
    ps
}

/// Assemble the general and final combiners selected by the fragments.
fn build_variant(
    ps: &mut [u8; PSDEF_SIZE],
    frag_offset: &mut [u8; 8],
    spas: &PassPostprocess,
    imp: &Implementation,
    inputs: &EngineInputs,
    work: &mut Work,
) -> Result<()> {
    let mut n = 0usize;
    let mut has_final = false;
    for (k, f) in spas.fragments[imp.fragments.indices()].iter().enumerate() {
        frag_offset[k] = n as u8;
        let mut v = usize::from(f.base);
        if f.selector != -1 {
            let s = usize::try_from(f.selector)
                .ok()
                .filter(|&s| s < 32)
                .ok_or(Error::Invalid(
                    "fragment selector is not an engine integer input",
                ))?;
            v = v.wrapping_add(inputs.ints[s] as usize);
        }
        let var = spas
            .variants
            .get(v)
            .ok_or(Error::Invalid("fragment variant out of range"))?;
        let count = usize::from(var.stages.count);
        work.charge(count)?;
        for (j, st) in spas.stages[var.stages.indices()].iter().enumerate() {
            if var.flags & 1 != 0 && j + 1 == count {
                set_word(ps, PS_FINAL_ABCD, st.rgb_inputs);
                set_word(ps, PS_FINAL_EFG, st.alpha_inputs);
                set_word(ps, PS_FINAL_C0, st.constant0);
                set_word(ps, PS_FINAL_C1, st.constant1);
                has_final = true;
                continue;
            }
            if n >= 8 {
                return Err(Error::Invalid(
                    "variant has more than eight combiner stages",
                ));
            }
            set_word(ps, PS_RGB_IN + n, st.rgb_inputs);
            set_word(ps, PS_RGB_OUT + n, st.rgb_outputs);
            set_word(ps, PS_ALPHA_IN + n, st.alpha_inputs);
            set_word(ps, PS_ALPHA_OUT + n, st.alpha_outputs);
            set_word(ps, PS_C0 + n, st.constant0);
            set_word(ps, PS_C1 + n, st.constant1);
            n += 1;
        }
    }
    if !has_final {
        set_word(ps, PS_FINAL_ABCD, 0xc);
        set_word(ps, PS_FINAL_EFG, 0x2000);
    }
    set_word(ps, PS_COMBINER_COUNT, n.max(1) as u32 | 0x11100);
    Ok(())
}

/// `0x00017170` / `0x00019ca0`: masked write of a packed colour.
fn apply_color(
    ps: &mut [u8; PSDEF_SIZE],
    c0w: &mut u16,
    c1w: &mut u16,
    w: u16,
    mask: u32,
    color: u32,
    spas: &PassPostprocess,
    imp: &Implementation,
    frag_offset: &[u8; 8],
    inputs: &EngineInputs,
) -> Result<Option<(bool, u8)>> {
    let Some(register) = colour_register(w, spas, imp, frag_offset, inputs)? else {
        return Ok(None);
    };
    write_colour(ps, c0w, c1w, register, mask, color)?;
    Ok(Some(register))
}

/// Shared by instance, engine and animated combiner-constant writes.
fn colour_register(
    w: u16,
    spas: &PassPostprocess,
    imp: &Implementation,
    frag_offset: &[u8; 8],
    inputs: &EngineInputs,
) -> Result<Option<(bool, u8)>> {
    let c1 = w & 0x400 != 0;
    let fin = w & 0x3c0 == 0x200;
    let mut reg = usize::from((w >> 6) & 0xf);
    if !imp.fragments.is_empty() {
        let frag = usize::from(w & 7);
        let f = spas.fragments[imp.fragments.indices()]
            .get(frag)
            .ok_or(Error::Invalid("colour binding names a missing fragment"))?;
        if f.selector != -1 {
            let s = usize::try_from(f.selector)
                .ok()
                .filter(|&s| s < 32)
                .ok_or(Error::Invalid(
                    "fragment selector is not an engine integer input",
                ))?;
            if inputs.ints[s] != i32::from((w >> 3) & 7) {
                return Ok(None);
            }
        }
        if !fin {
            reg += usize::from(frag_offset[frag]);
        }
    }
    if !fin && reg >= 8 {
        return Err(Error::Invalid("colour binding register above 8"));
    }
    Ok(Some((c1, if fin { 8 } else { reg as u8 })))
}

fn write_colour(
    ps: &mut [u8; PSDEF_SIZE],
    c0w: &mut u16,
    c1w: &mut u16,
    (c1, slot): (bool, u8),
    mask: u32,
    color: u32,
) -> Result<()> {
    let idx = match slot {
        0..=7 => (if c1 { PS_C1 } else { PS_C0 }) + usize::from(slot),
        8 => if c1 { PS_FINAL_C1 } else { PS_FINAL_C0 },
        _ => return Err(Error::Invalid("colour binding register above 8")),
    };
    let old = word(ps, idx);
    set_word(ps, idx, (old & !mask) | (color & mask));
    if c1 {
        *c1w |= 1 << slot;
    } else {
        *c0w |= 1 << slot;
    }
    Ok(())
}

impl ResolvedImplementation {
    /// Merge the selected ARGB lanes into one resolved combiner constant and
    /// mark the constant for upload. Slot 8 selects the final combiner.
    pub fn write_pixel_constant(&mut self, register: (bool, u8), mask: u32, color: u32) -> Result<()> {
        write_colour(&mut self.pixel_shader, &mut self.c0_written, &mut self.c1_written, register, mask, color)
    }
}

/// `0x00019ca0`'s packing of an engine value into an ARGB combiner
/// constant: with `half`, `(x + 1) / 2` first; each channel clamped to
/// 0..1 and rounded to 8 bits.
pub fn pack_engine_colour(mut v: [f32; 4], half: bool) -> u32 {
    if half {
        for x in &mut v {
            *x = (*x + 1.0) * 0.5;
        }
    }
    let ch = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u32;
    ch(v[3]) << 24 | ch(v[0]) << 16 | ch(v[1]) << 8 | ch(v[2])
}

/// The components `eval_engine` writes in each mode. Bit 0 = x.
fn engine_mode_components(mode: usize) -> u8 {
    match mode {
        0..=3 => 1 << mode,
        4 | 16 | 17 => 0b0111,
        5..=8 => 1 << (mode - 5),
        9 | 11 => 0b0011,
        10 | 12 => 0b1100,
        _ => 0b1111,
    }
}

/// The vector bindings among `writes` (in binding order), each keeping
/// only the components or lanes no later binding of the same register
/// overwrites.
pub(crate) fn vector_bindings(writes: &[(VectorTarget, Option<u8>)]) -> Vec<VectorBinding> {
    let mut out = Vec::new();
    for (i, &(target, source)) in writes.iter().enumerate() {
        let Some(source) = source else { continue };
        let later = &writes[i + 1..];
        let target = match target {
            VectorTarget::Vertex { bank, reg, components } => {
                let over = later
                    .iter()
                    .filter_map(|(t, _)| match *t {
                        VectorTarget::Vertex { bank: b, reg: r, components: c } if (b, r) == (bank, reg) => Some(c),
                        _ => None,
                    })
                    .fold(0, |a, c| a | c);
                VectorTarget::Vertex { bank, reg, components: components & !over }
            }
            VectorTarget::Pixel { c1, slot, mask, half, transform } => {
                let over = later
                    .iter()
                    .filter_map(|(t, _)| match *t {
                        VectorTarget::Pixel { c1: c, slot: s, mask: m, .. } if (c, s) == (c1, slot) => Some(m),
                        _ => None,
                    })
                    .fold(0, |a, m| a | m);
                VectorTarget::Pixel { c1, slot, mask: mask & !over, half, transform }
            }
        };
        let empty = match target {
            VectorTarget::Vertex { components, .. } => components == 0,
            VectorTarget::Pixel { mask, .. } => mask == 0,
        };
        if !empty {
            out.push(VectorBinding { source, target });
        }
    }
    out
}

/// One engine-driven vertex constant (`0x00019be0`, an entry of an
/// implementation's `+0x124` range): the word's low 4 bits the register,
/// bit 4 the bank (1 the constant `c[18 + reg]`, 0 the default
/// `v[reg]`), bits 5..9 the mode (0..3 a scalar into one component, 4 a
/// vector, 5..20 the size of stage `stage - 1`'s texture, stage 0 a size
/// of 1 by 1), bits 13..15 the stage; byte 3 the input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EngineVertexConstant {
    pub bank: u8,
    pub reg: u8,
    pub mode: u8,
    pub stage: u8,
    pub source: u8,
}

impl EngineVertexConstant {
    pub fn of(e: ExternalBinding) -> Self {
        let w = e.word();
        Self {
            bank: ((w >> 4) & 1) as u8,
            reg: (w & 0xf) as u8,
            mode: ((w >> 5) & 0x1f) as u8,
            stage: (w >> 13) as u8,
            source: e.bytes[3],
        }
    }

    /// Whether it reads a stage's size (modes 5..20).
    pub fn reads_size(self) -> bool {
        (5..=20).contains(&self.mode)
    }

    /// The components it writes (bit 0 = x).
    pub fn components(self) -> u8 {
        engine_mode_components(usize::from(self.mode))
    }

    /// Its value written into `row` as `0x00019be0` writes it (only the
    /// mode's components): `size` the stage's texture's (width, height),
    /// read by modes 5..20, `inputs` read by modes 0..4.
    pub fn eval(self, row: &mut [f32; 4], size: (f32, f32), inputs: &EngineInputs) -> Result<()> {
        eval_engine(row, usize::from(self.mode), 0, usize::from(self.source), size, inputs)
    }

    /// For a size mode (5..20), its value written into `row` with `size`
    /// the (width, height) of the texture at stage `stage - 1`; other
    /// modes leave `row` alone.
    pub fn eval_size(self, row: &mut [f32; 4], size: (f32, f32)) {
        if (5..=20).contains(&self.mode) {
            eval_size(row, usize::from(self.mode), size);
        }
    }
}

/// How an engine-driven state's value is converted for its state (byte 3
/// of its entry in [`ANIMATED_RENDER_STATES`] or [`ANIMATED_STAGE_STATES`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineStateKind {
    /// 0: a colour word.
    Colour,
    /// 1: the float's bits.
    Float,
    /// 2: the float rounded to an integer (`fistp`, `0x0001a1f8`).
    Integer,
}

/// Where an engine-driven state's value comes from in the shader draw's
/// context (`0x0001a170`, `0x0001ab50`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineStateSource {
    /// Source byte 1: the scalar at `ctx + 0x5c + 4·slot`, the shader's
    /// evaluated parameter (`0x0001a1cd`).
    Scalar(u8),
    /// Any other source byte: the colour at `ctx + 0x108 + 12·slot`,
    /// packed by `0x00131ed0` into the state's low 24 bits, its alpha byte
    /// kept (`0x0001a89c`).
    Colour(u8),
}

/// One engine-driven render or texture-stage state: an entry of an
/// implementation's `+0x11e` (render) or `+0x120` (stage) range of the
/// `spas` bindings, as `0x0001a170` and `0x0001ab50` read it. A render
/// binding is `[target, source, -, slot]`, a stage binding `[stage,
/// target, source, slot]`; the target indexes [`ANIMATED_RENDER_STATES`]
/// (`0x467020`) or [`ANIMATED_STAGE_STATES`] (`0x46703c`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EngineState {
    /// The D3D render or texture-stage state (the table entry's word).
    pub state: u8,
    /// The texture stage, for a stage state.
    pub stage: Option<u8>,
    pub kind: EngineStateKind,
    pub source: EngineStateSource,
}

impl EngineState {
    /// A render-state binding, or `None` when its target is past the
    /// table (the guest would read past it).
    pub fn render(e: ExternalBinding) -> Option<Self> {
        let [target, source, _, slot] = e.bytes;
        Self::of(&ANIMATED_RENDER_STATES, target, None, source, slot)
    }

    /// A texture-stage-state binding, or `None` when its target is past
    /// the table or its stage past the four stages.
    pub fn stage(e: ExternalBinding) -> Option<Self> {
        let [stage, target, source, slot] = e.bytes;
        if stage >= 4 {
            return None;
        }
        Self::of(&ANIMATED_STAGE_STATES, target, Some(stage), source, slot)
    }

    fn of(table: &[(u8, u8, u8)], target: u8, stage: Option<u8>, source: u8, slot: u8) -> Option<Self> {
        let &(state, _, kind) = table.get(usize::from(target))?;
        let kind = match kind {
            0 => EngineStateKind::Colour,
            1 => EngineStateKind::Float,
            _ => EngineStateKind::Integer,
        };
        let source = if source == 1 { EngineStateSource::Scalar(slot) } else { EngineStateSource::Colour(slot) };
        Some(Self { state, stage, kind, source })
    }

    /// The state's name ([`render_state_name`], [`stage_state_name`]).
    pub fn name(self) -> Option<&'static str> {
        match self.stage {
            None => render_state_name(self.state),
            Some(_) => stage_state_name(u16::from(self.state)),
        }
    }

    /// The state's value for a draw whose engine inputs are `inputs`,
    /// over `current`, the value the state holds (the D3D shadow the guest
    /// reads, `0x00407498` or `0x00404dd0`): `0x0001ae70` fills the
    /// context's scalar slots from `inputs.scalars` and its colour slots
    /// from `inputs.vectors`, and `0x0001a170` and `0x0001ab50` convert
    /// them alike:
    ///
    /// - a scalar of kind [`EngineStateKind::Integer`] is rounded
    ///   (`fistp`, `0x0001a1f8`), of kind [`EngineStateKind::Float`] its
    ///   bits, of kind [`EngineStateKind::Colour`] times [`BYTE_SCALE`],
    ///   rounded, into the top byte over `current`'s low 24 bits
    ///   (`0x0001a801`);
    /// - a colour is packed by `0x00131ed0`, each channel times
    ///   [`BYTE_SCALE`], rounded and masked to a byte, red at bit 16, green
    ///   at 8, blue at 0, into `current`'s low 24 bits, its top byte kept
    ///   (`0x0001a89c`), whatever the kind.
    ///
    /// Rounding is to nearest, ties to even (the x87 default; the
    /// products are exact at 53 bits). `None` when the slot is past the
    /// inputs (the guest would read the next field of its context).
    pub fn value(self, inputs: &EngineInputs, current: u32) -> Option<u32> {
        match self.source {
            EngineStateSource::Scalar(slot) => {
                let x = f64::from(*inputs.scalars.get(usize::from(slot))?);
                Some(match self.kind {
                    EngineStateKind::Integer => fistp(x),
                    EngineStateKind::Float => (x as f32).to_bits(),
                    EngineStateKind::Colour => (current & 0x00ff_ffff) | (fistp(x * f64::from(BYTE_SCALE)) << 24),
                })
            }
            EngineStateSource::Colour(slot) => {
                let v = inputs.vectors.get(usize::from(slot))?;
                Some((current & 0xff00_0000) | pack_colour(*v))
            }
        }
    }
}

/// The fraction-to-byte scale at `0x0045dc38`, 255.
pub const BYTE_SCALE: f32 = 255.0;

/// `fistp m32` under round-to-nearest-even: out of range or NaN gives the
/// integer indefinite, `0x80000000`.
fn fistp(x: f64) -> u32 {
    let r = x.round_ties_even();
    if r.is_nan() || !(-2_147_483_648.0..=2_147_483_647.0).contains(&r) {
        return 0x8000_0000;
    }
    r as i32 as u32
}

/// `0x00131ed0`: a colour as a D3D word's low 24 bits, each channel times
/// [`BYTE_SCALE`], rounded and masked to a byte (no clamp: 1.5 wraps).
fn pack_colour([r, g, b]: [f32; 3]) -> u32 {
    let byte = |c: f32| fistp(f64::from(c) * f64::from(BYTE_SCALE)) & 0xff;
    (byte(r) << 16) | (byte(g) << 8) | byte(b)
}

/// `imp`'s engine-driven render and texture-stage states, in the order
/// `0x0001a170` and `0x0001ab50` apply them, or `None` when one does not
/// decode ([`EngineState`]).
pub fn engine_states(imp: &ResolvedImplementation) -> Option<(Vec<EngineState>, Vec<EngineState>)> {
    let render = imp.engine_render_states.iter().map(|&e| EngineState::render(e)).collect::<Option<Vec<_>>>()?;
    let stage = imp.engine_stage_states.iter().map(|&e| EngineState::stage(e)).collect::<Option<Vec<_>>>()?;
    Some((render, stage))
}

/// `imp`'s engine-driven vertex constants in the order `0x00019be0`
/// applies them.
pub fn engine_vertex_constants(spas: &PassPostprocess, imp: &Implementation) -> Vec<EngineVertexConstant> {
    let r = imp.external_vertex_constants;
    spas.externals.get(r.indices()).unwrap_or(&[]).iter().map(|&e| EngineVertexConstant::of(e)).collect()
}

fn stage_dims(textures: &[Option<StageTexture>; 4], stage: usize) -> (f32, f32) {
    if stage == 0 {
        return (1.0, 1.0);
    }
    match textures
        .get(stage - 1)
        .copied()
        .flatten()
        .and_then(|t| t.stage_size)
    {
        Some((w, h)) => (f32::from(w), f32::from(h)),
        None => (0.0, 0.0),
    }
}

/// `0x0001b2c0`: one engine value into a float4 register.
/// An engine scalar through a binding's transform, as `0x00019ca0` (and
/// the vertex constants' evaluation, with transform 0) applies it: 1 the
/// complement, 2..5 a cosine or sine wave over one period, scaled into
/// 0..1 and clamped; anything else the value itself.
pub fn engine_scalar(x: f32, transform: usize) -> f32 {
    // The guest multiplies two singles on the x87 stack and keeps the
    // angle there through the trig operation. Narrow only the result.
    let angle = f64::from(std::f32::consts::TAU) * f64::from(x);
    let y = match transform {
        1 => return 1.0 - x,
        2 => (angle.cos() + 1.0) * 0.5,
        3 => 0.5 - angle.cos() * 0.5,
        4 => (angle.sin() + 1.0) * 0.5,
        5 => 0.5 - angle.sin() * 0.5,
        _ => return x,
    };
    y.clamp(0.0, 1.0) as f32
}

fn eval_engine(
    out: &mut [f32; 4],
    mode: usize,
    transform: usize,
    source: usize,
    dims: (f32, f32),
    inputs: &EngineInputs,
) -> Result<()> {
    match mode {
        0..=3 => {
            let x = *inputs
                .scalars
                .get(source)
                .ok_or(Error::Invalid("engine scalar index above 63"))?;
            out[mode] = engine_scalar(x, transform);
        }
        4 => {
            let v = inputs
                .vectors
                .get(source)
                .ok_or(Error::Invalid("engine vector index above 63"))?;
            out[..3].copy_from_slice(v);
        }
        5..=20 => eval_size(out, mode, dims),
        _ => return Err(Error::Invalid("engine binding mode above 20")),
    }
    Ok(())
}

/// Modes 5..20 of [`eval_engine`]: the stage's texture size `dims`.
fn eval_size(out: &mut [f32; 4], mode: usize, dims: (f32, f32)) {
    let (w, h) = dims;
    match mode {
        5..=8 => out[mode - 5] = w,
        9 => (out[0], out[1]) = (w, h),
        10 => (out[2], out[3]) = (w, h),
        11 => (out[0], out[1]) = (0.0, 0.0),
        12 => (out[2], out[3]) = (0.0, 0.0),
        13 => *out = [w, h, 0.0, 0.0],
        14 | 18 => *out = [w, 0.0, 0.0, 0.0],
        15 | 19 => *out = [0.0, h, 0.0, 0.0],
        16 => (out[0], out[1], out[2]) = (w, h, 1.0),
        17 => (out[0], out[1], out[2]) = (0.0, 0.0, 0.0),
        _ => *out = [0.0; 4],
    }
}

#[cfg(test)]
mod uses_tests {
    use super::*;

    fn implementation(vertex_shader: Option<TagId>) -> ResolvedImplementation {
        ResolvedImplementation {
            index: 0,
            vertex_shader,
            textures: [None, None, None, None],
            stage_states: Vec::new(),
            render_states: Vec::new(),
            restore_states: Vec::new(),
            pixel_shader: [0; PSDEF_SIZE],
            variant: false,
            c0_written: 0,
            c1_written: 0,
            pixel_constants_from_engine: false,
            per_draw_inputs: false,
            input_reads: InputReads::default(),
            vector_bindings: Vec::new(),
            scalar_bindings: Vec::new(),
            vertex_constants: Vec::new(),
            vertex_defaults: Vec::new(),
            animated: Vec::new(),
            engine_render_states: Vec::new(),
            engine_stage_states: Vec::new(),
            dimension_bindings: Vec::new(),
            copies_target: false,
        }
    }

    /// Invented datums: a shader, its template, two passes (one with no
    /// implementations) and a vertex shader.
    fn material() -> Material {
        Material {
            shader: TagId(0xe000_0001),
            template: TagId(0xe000_0002),
            inputs: EngineInputs::default(),
            layers: vec![
                ResolvedLayer {
                    lod: 0,
                    layer: 0,
                    passes: vec![ResolvedPass { index: 0, pass: TagId(0xe000_0003), implementations: Vec::new() }],
                },
                ResolvedLayer {
                    lod: 0,
                    layer: 1,
                    passes: vec![ResolvedPass {
                        index: 0,
                        pass: TagId(0xe000_0004),
                        implementations: vec![implementation(None), implementation(Some(TagId(0xe000_0005)))],
                    }],
                },
            ],
        }
    }

    #[test]
    fn a_material_uses_its_shader_template_passes_and_vertex_shaders() {
        let m = material();
        for n in 1..=5 {
            assert!(m.uses(TagId(0xe000_0000 + n)), "{n}");
        }
        assert!(!m.uses(TagId(0xe000_0006)));
        assert!(m.uses(TagId(0xf123_0003)), "another salt of the same index");
    }
}
