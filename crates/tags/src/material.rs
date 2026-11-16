//! Resolved materials: a `shad` turned into the ordered passes the engine
//! draws, each with its texture stages, constants and render state, in the
//! order and with the semantics of the draw-time setup `0x00016b90`.
//!
//! Engine-driven values (game time, object functions, lighting) are taken
//! from an [`EngineInputs`] snapshot; the defaults are zero except the three
//! inputs the instance root supplies. Animated instance parameters are listed
//! with their targets but not evaluated (the mapping-function evaluator
//! `0x0013b390` is not reimplemented here). See `docs/formats/shader-instances.md`.
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

/// An animated target, not evaluated.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimatedTarget {
    pub kind: AnimationKind,
    pub binding: AnimationBinding,
    /// The property components feeding it.
    pub properties: Vec<PropertyRef>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimationKind {
    Bitmap,
    RenderState,
    StageState,
    PixelConstant,
    VertexConstant,
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
    /// Hardware vertex constants (`c[18 + i]`, `0x00018eab`).
    pub vertex_constants: Vec<Register>,
    /// Vertex attribute defaults `v[i]` (`SET_VERTEX_DATA4F_M`, `0x00018e3b`).
    pub vertex_defaults: Vec<Register>,
    pub animated: Vec<AnimatedTarget>,
    /// Engine-driven render-state / stage-state bindings, raw.
    pub engine_render_states: Vec<ExternalBinding>,
    pub engine_stage_states: Vec<ExternalBinding>,
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

/// Tag lookup plus decoded caches over one map's metadata.
pub struct ShaderLibrary {
    meta: Metadata,
    tags: HashMap<TagId, (FourCC, u32)>,
    templates: HashMap<TagId, ShaderTemplate>,
    passes: HashMap<TagId, ShaderPass>,
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
        }
    }
    /// Loads the metadata region and tag table of a map.
    pub fn read<R: std::io::Read + std::io::Seek>(map: &mut map::Map<R>) -> Result<Self> {
        let meta = Metadata::read(map)?;
        let tags: Vec<_> = map
            .tags()
            .iter()
            .filter_map(|t| t.location.map(|l| (t.id, t.group, l.address)))
            .collect();
        Ok(Self::new(meta, tags))
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
                    imps.push(this.resolve(tp, sp, spas, imp, k, ib, anim, &inputs, &mut work)?);
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

    #[allow(clippy::too_many_arguments)]
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
        let mut slots: HashMap<u8, (TextureSource, i32)> = HashMap::new();
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
        let mut banks = [[[0f32; 4]; 16]; 2];
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
        let mut ps = imp.pixel_shader;
        let mut frag_offset = [0u8; 8];
        let variant = !imp.fragments.is_empty();
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

        // 0x00018a90: texture stage bindings. A slot binding first copies the
        // whole slot to the stage (0x00018afb), then, when it asks, refreshes
        // the *slot's* size (0x00018b31..0x00018ba2), so the stage keeps the
        // slot's earlier size. Each implementation starts from zero slot
        // sizes (fresh state; see the stale-state note in the docs).
        let mut slot_sizes: HashMap<u8, Option<(u16, u16)>> = HashMap::new();
        let mut textures: [Option<StageTexture>; 4] = [None; 4];
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
                *stage = Some(StageTexture {
                    source,
                    size: None,
                    stage_size: None,
                });
                continue;
            }
            let stage_size = slot_sizes.get(&t.slot).copied().unwrap_or(Some((0, 0)));
            let (mut source, frame) = slots
                .get(&t.slot)
                .copied()
                .unwrap_or((TextureSource::UnsetSlot(t.slot), 0));
            let mut size = None;
            match &mut source {
                TextureSource::Bitmap { bitmap, image, .. } => {
                    let (count, _) = self.bitmap_image(*bitmap, None)?;
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

        // 0x00019be0: engine-driven vertex constants (after instance vectors).
        for e in &spas.externals[imp.external_vertex_constants.indices()] {
            let w = e.word();
            let reg = usize::from(w & 0xf);
            let bank = usize::from((w >> 4) & 1);
            let mode = usize::from((w >> 5) & 0x1f);
            let stage = (w >> 13) as usize;
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
            if e.bytes[1] & 8 != 0 {
                for x in &mut v {
                    *x = (*x + 1.0) * 0.5;
                }
            }
            let ch = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u32;
            let packed = ch(v[3]) << 24 | ch(v[0]) << 16 | ch(v[1]) << 8 | ch(v[2]);
            apply_color(
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
                    let pr = sp.property_ranges[usize::from(binding.properties)];
                    work.charge(usize::from(pr.count))?;
                    animated.push(AnimatedTarget {
                        kind,
                        binding: *binding,
                        properties: sp.property_refs[pr.indices()].to_vec(),
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
            vertex_constants: regs(1),
            vertex_defaults: regs(0),
            animated,
            engine_render_states: spas.externals[imp.animated_render_states.indices()].to_vec(),
            engine_stage_states: spas.externals[imp.animated_stage_states.indices()].to_vec(),
        })
    }
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

/// `0x0001ba00`: assemble the general/final combiners from fragments.
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
#[allow(clippy::too_many_arguments)]
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
) -> Result<()> {
    let c1 = w & 0x400 != 0;
    let fin = w & 0x3c0 == 0x200;
    let mut reg = usize::from((w >> 6) & 0xf);
    if !imp.fragments.is_empty() {
        let frag = usize::from(w & 7);
        let f = spas
            .fragments
            .get(usize::from(imp.fragments.start) + frag)
            .ok_or(Error::Invalid("colour binding names a missing fragment"))?;
        if f.selector != -1 {
            let s = usize::try_from(f.selector)
                .ok()
                .filter(|&s| s < 32)
                .ok_or(Error::Invalid(
                    "fragment selector is not an engine integer input",
                ))?;
            if inputs.ints[s] != i32::from((w >> 3) & 7) {
                return Ok(());
            }
        }
        if !fin {
            reg += usize::from(frag_offset[frag]);
        }
    }
    let (idx, bit) = if fin {
        (if c1 { PS_FINAL_C1 } else { PS_FINAL_C0 }, 8)
    } else if reg < 8 {
        (if c1 { PS_C1 } else { PS_C0 } + reg, reg)
    } else {
        return Err(Error::Invalid("colour binding register above 8"));
    };
    let old = word(ps, idx);
    set_word(ps, idx, (old & !mask) | (color & mask));
    if c1 {
        *c1w |= 1 << bit;
    } else {
        *c0w |= 1 << bit;
    }
    Ok(())
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
fn eval_engine(
    out: &mut [f32; 4],
    mode: usize,
    transform: usize,
    source: usize,
    dims: (f32, f32),
    inputs: &EngineInputs,
) -> Result<()> {
    use std::f32::consts::TAU;
    match mode {
        0..=3 => {
            let x = *inputs
                .scalars
                .get(source)
                .ok_or(Error::Invalid("engine scalar index above 63"))?;
            out[mode] = match transform {
                1 => 1.0 - x,
                2 => ((TAU * x).cos() + 1.0) * 0.5,
                3 => 0.5 - (TAU * x).cos() * 0.5,
                4 => ((TAU * x).sin() + 1.0) * 0.5,
                5 => 0.5 - (TAU * x).sin() * 0.5,
                _ => x,
            };
            if (2..=5).contains(&transform) {
                out[mode] = out[mode].clamp(0.0, 1.0);
            }
        }
        4 => {
            let v = inputs
                .vectors
                .get(source)
                .ok_or(Error::Invalid("engine vector index above 63"))?;
            out[..3].copy_from_slice(v);
        }
        5..=20 => {
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
        _ => return Err(Error::Invalid("engine binding mode above 20")),
    }
    Ok(())
}
