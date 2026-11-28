//! The water edge blend, layer 6 (docs/graphics/native-water-fog.md,
//! "Stage 4b: water and the rest"): the water templates' own layer-6
//! passes, drawn natively between the game's water setup `0x0003ba90`
//! and the water end `0x0003c3c0` (`render_bridge::water_end`; only the
//! layer's items are skipped).
//!
//! What makes water its own module beside [`crate::fog_gpu`] is the
//! stages: a layer-6 pass binds engine textures — the scene itself —
//! which no load can decode. Each is supplied per view ([`Supply`]):
//! the view's own target (id 5) as a copy of the bound colour made in
//! the recording ([`WaterSet::make_copy`]: an attachment cannot be
//! sampled as it is drawn into), the render targets through the
//! reservation's sampled levels (their embedded guest texture headers from
//! Halo's target table `0x004b4b58`, boot's glue). The look keeps only
//! those two kinds ([`crate::fog_gpu::supplied_layer_passes`]);
//! anything else refuses the material, and the census names what a
//! level binds.
//!
//! Pool surfaces use the sorted layer-15 geometry handler. [`sorted_pool`]
//! identifies them at BSP load for the resolved indexed pass. This GPU
//! set holds only materials with a separate layer-6 edge pass.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, hash_map::Entry};

use crate::fog_gpu::{supplied_layer_passes, FogPass, FogProgram, FogWords};
use crate::gpu::{Image, texture};
use crate::look::Stage;
use crate::pass_state::PassState;
use crate::program::{described_pipeline, Gpu, Rigid, PipelineUse, ViewInputs};
use crate::world::{LightmapKey, ModelRows, Owner, World};

/// The layer this module draws.
pub const WATER_LAYER: usize = 6;

/// The shader id the water's packets report to the pass they join (for
/// diagnostics; the pipelines are the passes' own).
const WATER_PROGRAM: gfx::pipeline::ShaderId = gfx::pipeline::ShaderId(u32::MAX - 5);

/// The scratch copy of the bound colour for the stages that sample the
/// view ([`WaterSet::scene_copy`]).
pub(crate) use crate::snapshot_gpu::ColourCopy as SceneCopy;

/// One pass of one selected part, ready to issue ([`WaterSet::items`]).
pub(crate) struct WaterItem {
    pub key: gfx::pipeline::PipelineKey,
    pub pipeline: wgpu::RenderPipeline,
    pub uniforms: wgpu::BindGroup,
    pub offset: Option<u32>,
    pub textures: wgpu::BindGroup,
    pub blend: wgpu::Color,
    pub first: u32,
    pub count: u32,
}

/// `items` as packets for the D3D pass already open on these attachments:
/// the whole target as viewport and the player view as scissor, the world's program vertices
/// and indices ([`crate::gpu::GpuWorld::water_draws`]).
// wgpu compares and hashes pipelines by their stable handle identity.
#[allow(clippy::mutable_key_type)]
pub(crate) fn item_packets(
    items: &[WaterItem],
    cache: &mut HashMap<wgpu::RenderPipeline, std::sync::Arc<gfx::compile::PendingPipeline>>,
    vertices: &wgpu::Buffer,
    indices: &wgpu::Buffer,
    target_size: (u32, u32),
    scissor: gfx::ScissorRect,
) -> Result<Vec<gfx::RecordedDraw>, &'static str> {
    use gfx::{BoundGroup, RecordedDraw};
    let render = gfx::state::RenderState::default();
    let mut packets = Vec::with_capacity(items.len());
    for it in items {
        if (u64::from(it.first) + u64::from(it.count)) * 4 > indices.size() {
            return Err("a selected water part reaches past the indices");
        }
        let pipeline = cache
            .entry(it.pipeline.clone())
            .or_insert_with(|| gfx::compile::Pending::ready(Some(it.pipeline.clone())))
            .clone();
        packets.push(
            RecordedDraw::prepared_indexed(
                &it.key,
                pipeline,
                vec![(vertices.clone(), 0, vertices.size())],
                (indices.clone(), u64::from(it.first) * 4, u64::from(it.count) * 4),
                it.count,
                vec![BoundGroup { index: 0, group: it.uniforms.clone(), offsets: it.offset.into_iter().collect() }, BoundGroup::new(1, it.textures.clone())],
                &render,
                target_size,
                scissor,
            )
            .with_blend_constant(it.blend),
        );
    }
    Ok(packets)
}

/// How one engine-texture stage is supplied per view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Supply {
    /// Engine texture 5, the view's bound target: a copy of the bound
    /// colour made in the recording ([`WaterSet::make_copy`]), before
    /// the pass draws into it.
    View,
    /// A render target by index (the engine-texture table's `Target`):
    /// the reservation's sampled level of the descriptor's texture header.
    Target(u32),
}

/// A stage's supply, when it binds an engine texture (`Supply` of the id
/// `crate::screen_look::engine_texture` maps).
pub fn supply_of(stage: Stage) -> Option<Supply> {
    match stage {
        Stage::Engine(id) => match crate::screen_look::engine_texture(id) {
            crate::screen_look::EngineTexture::View => Some(Supply::View),
            crate::screen_look::EngineTexture::Target(t) => Some(Supply::Target(t)),
            _ => None,
        },
        _ => None,
    }
}

/// The water materials the native pass draws, their parts' index ranges,
/// and the copy of the bound colour the view stages sample.
pub struct WaterSet {
    ready_formats: HashSet<(wgpu::TextureFormat, wgpu::TextureFormat)>,
    materials: HashMap<u16, WaterMaterial>,
    /// Each water part, by owner and part index: its index range,
    /// lightmap page and material.
    parts: HashMap<(Owner, u8), (u32, u32, Option<LightmapKey>, u16)>,
    /// The distinct render targets the materials sample (the supplies'
    /// `Target`s), ascending: the order of the sampled levels the
    /// reservation hands [`WaterSet::draw`].
    pub targets: Vec<u32>,
    /// Whether any material samples the view's own target.
    pub samples_view: bool,
    /// The lightmap pages the parts use, as views.
    pages: HashMap<LightmapKey, Option<wgpu::TextureView>>,
    /// Scratch views for the Reference and Display scene copies.
    scene_copies: crate::snapshot_gpu::ColourCopies,
}

struct WaterMaterial {
    passes: Vec<(Gpu, PassState, Option<Rigid>)>,
    /// Per stage: the engine supply ([`supply_of`]).
    supplies: [Option<Supply>; 4],
    words: FogWords,
}

impl WaterSet {
    /// Every texture the passes' stages sample as image `image` of
    /// `bitmap`, with the stage and its signed channels, each once.
    pub fn held(&self, bitmap: map::TagId, image: u32) -> Vec<crate::gpu::Held> {
        let want = Stage::Bitmap { bitmap, image };
        let mut materials: Vec<_> = self.materials.iter().collect();
        materials.sort_by_key(|(m, _)| **m);
        let mut out = Vec::new();
        for (_, m) in materials {
            for (g, _, _) in &m.passes {
                g.held(want, &mut out);
            }
        }
        out
    }

    /// The textures that hold lightmap page `key`.
    pub fn page_textures(&self, key: LightmapKey) -> Vec<wgpu::Texture> {
        self.pages.get(&key).and_then(|v| Some(v.as_ref()?.texture().clone())).into_iter().collect()
    }

    /// Uploads `programs` (each material's layer-6 passes and their
    /// compiled programs) for `parts` (the transparents' parts list:
    /// owner and part, index range, lightmap page and material).
    /// `stage_image` decodes a stage's bitmap image, `lightmap_image` a
    /// page. A material is kept only when every pass uploads and its
    /// passes agree on their supplies.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        world: &World,
        parts: Vec<((Owner, u8), (u32, u32, Option<LightmapKey>, u16))>,
        programs: HashMap<u16, (Vec<FogPass>, Vec<FogProgram>)>,
        targets: Vec<u32>,
        samples_view: bool,
        stage_image: &mut dyn FnMut(Stage) -> Option<Image>,
        lightmap_image: &mut dyn FnMut(LightmapKey) -> Option<Image>,
    ) -> Self {
        let mut placements: HashMap<u16, BTreeMap<u32, ModelRows>> = HashMap::new();
        for &((owner, _), (_, _, _, material)) in &parts {
            if let Owner::Instance(i) = owner && let Some(rows) = world.rows(i) {
                placements.entry(material).or_default().insert(i, *rows);
            }
        }
        let mut materials = HashMap::new();
        'material: for (material, (passes, compiled)) in programs {
            if passes.is_empty() || passes.len() != compiled.len() {
                continue;
            }
            let supplies = passes[0].look.textures.map(supply_of);
            if passes.iter().any(|p| p.look.textures.map(supply_of) != supplies) {
                continue; // the passes disagree on their supplies
            }
            let words = compiled[0].words;
            if compiled.iter().any(|p| p.words != words) {
                continue;
            }
            let mut gpus = Vec::with_capacity(compiled.len());
            for mut p in compiled {
                p.program.rigid = p.rigid;
                let Some(g) = Gpu::new(device, queue, p.program, stage_image) else {
                    continue 'material;
                };
                let rigid = placements.get(&material).and_then(|rows| {
                    Rigid::with_slots(device, &g, rows.iter().map(|(&i, &r)| (i, r)).collect())
                });
                gpus.push((g, p.state, rigid));
            }
            materials.insert(material, WaterMaterial { passes: gpus, supplies, words });
        }
        let mut pages = HashMap::new();
        for &(_, (_, _, lightmap, material)) in &parts {
            if materials.contains_key(&material)
                && let Some(k) = lightmap
                && !pages.contains_key(&k)
            {
                pages.insert(k, lightmap_image(k).map(|i| texture(device, queue, &i, "native water lightmap")));
            }
        }
        Self { ready_formats: HashSet::new(), materials, parts: parts.into_iter().collect(), targets, samples_view, pages, scene_copies: Default::default() }
    }

    /// How many materials draw natively.
    pub fn len(&self) -> usize {
        self.materials.len()
    }

    pub fn is_empty(&self) -> bool {
        self.materials.is_empty()
    }

    /// Whether the set draws `part` of `owner`: the `prepare_copy` check
    /// that every selected part will record.
    pub fn draws_part(&self, owner: Owner, part: u8) -> bool {
        let Some(&(_, count, page, material)) = self.parts.get(&(owner, part)) else { return false };
        count != 0 && self.materials.get(&material).is_some_and(|m| {
            let has_page = page.and_then(|k| self.pages.get(&k)).is_some_and(Option::is_some);
            m.passes.iter().all(|(g, _, rigid)| {
                if let Owner::Instance(i) = owner && rigid.as_ref().and_then(|r| r.offset(i)).is_none() {
                    return false;
                }
                has_page || !g.program.stages.iter().zip(&g.program.dims)
                    .any(|(s, dim)| *s == Stage::Lightmap && dim.is_some())
            })
        })
    }

    /// The fog words of `material`'s passes, to resolve over the device's
    /// (`FogWords::resolve`): a water pass that reads the fog register
    /// gets the device's fog like a fog pass does.
    pub fn words(&self, material: u16) -> Option<&FogWords> {
        self.materials.get(&material).map(|m| &m.words)
    }

    /// The frame's uniforms for every pass (each material's fog resolved
    /// over the device's words, as the fog set's), through the
    /// submission's upload arena.
    pub fn prepare(
        &self,
        uploads: &mut gfx::ring::EncoderUploads<'_>,
        view: &ViewInputs,
        fogs: &HashMap<u16, (shaders::psh::fog::Fog, [f32; 4])>,
    ) {
        for (&material, m) in &self.materials {
            let Some((fog, color)) = fogs.get(&material) else { continue };
            for (g, _, rigid) in &m.passes {
                let mut pixel = g.program.pixel;
                pixel.fog = fog.uniform();
                pixel.fog_color = *color;
                g.prepare_with(uploads, view, &g.program.vertex, &pixel);
                if let Some(r) = rigid { r.prepare(uploads, view); }
            }
        }
    }

    /// Makes the pipelines for these target formats (before the pass).
    pub fn pipelines(&mut self, device: &wgpu::Device, color: wgpu::TextureFormat, depth: wgpu::TextureFormat) {
        self.prepare_pipelines(device, color, depth, PipelineUse::Direct);
    }

    pub(crate) fn prepare_pipelines(
        &mut self, device: &wgpu::Device, color: wgpu::TextureFormat, depth: wgpu::TextureFormat, usage: PipelineUse,
    ) -> bool {
        if self.ready_formats.contains(&(color, depth)) { return true; }
        let stencil = wgpu::StencilState::default();
        let mut ready = true;
        for m in self.materials.values_mut() {
            for (g, state, rigid) in &mut m.passes {
                if let Some(r) = rigid {
                    ready &= r.pipelines.prepare(device, color, depth, Some(state), None, &stencil, usage);
                }
                ready &= g.pipelines.prepare(device, color, depth, Some(state), None, &stencil, usage);
            }
        }
        // Only an admission check observes the frame boundary. Direct rendering
        // can use a pipeline as soon as the compiler finishes.
        if ready && matches!(usage, PipelineUse::Preflight) {
            self.ready_formats.insert((color, depth));
        }
        ready
    }

    /// The scene copy needs the full, single-sampled colour attachment
    /// ([`crate::snapshot_gpu::can_copy`]).
    pub fn can_copy(color: &wgpu::TextureView, size: (u32, u32)) -> bool {
        crate::snapshot_gpu::can_copy(color, size)
    }

    /// Copies the bound colour for the stages that sample the view's own
    /// target into this target copy's scratch texture
    /// ([`crate::snapshot_gpu::ColourCopies`]), before the pass draws into
    /// the target, so the stages read the scene as the wrapper's own draws
    /// found it.
    pub fn make_copy(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        copy: gfx::external::TargetCopy,
    ) -> wgpu::TextureView {
        let scene = self.scene_copy(device, color, copy);
        scene.record(encoder);
        scene.view
    }

    /// [`WaterSet::make_copy`] without the command, for a recording that
    /// issues it later.
    pub(crate) fn scene_copy(&mut self, device: &wgpu::Device, color: &wgpu::TextureView, copy: gfx::external::TargetCopy) -> SceneCopy {
        self.scene_copies.take(device, color, copy)
    }

    /// The texture group of `material`'s passes for one lightmap page,
    /// with the supplies' per-view views: `copy` the view copy, `sampled`
    /// the render targets' views in [`WaterSet::targets`]' order. `None`
    /// when a supply has no view or a group does not bind.
    fn groups_for(
        &self,
        device: &wgpu::Device,
        m: &WaterMaterial,
        lightmap: Option<LightmapKey>,
        copy: Option<&wgpu::TextureView>,
        sampled: &[Option<wgpu::TextureView>],
    ) -> Option<Vec<wgpu::BindGroup>> {
        let page = match lightmap {
            Some(k) => self.pages.get(&k).cloned().flatten(),
            None => None,
        };
        // The per-view views: the supplies' stages.
        let mut supplied: HashMap<usize, wgpu::TextureView> = HashMap::new();
        for (stage, s) in m.supplies.iter().enumerate() {
            match s {
                Some(Supply::View) => {
                    supplied.insert(stage, copy?.clone());
                }
                Some(Supply::Target(t)) => {
                    let i = self.targets.iter().position(|&want| want == *t)?;
                    supplied.insert(stage, sampled.get(i)?.as_ref()?.clone());
                }
                None => {}
            }
        }
        m.passes.iter()
            .map(|(g, _, _)| g.textures_supplied(device, page.as_ref(), &supplied))
            .collect()
    }

    /// Every pass of every selected part, in reservation order, with its
    /// pipeline, groups, blend constant and index range: what
    /// [`WaterSet::draw`] issues, as data (the pipelines are made,
    /// [`WaterSet::pipelines`]). Refused when a part, pass, supply or
    /// group is missing, as `draw` records nothing then.
    pub(crate) fn items(
        &self,
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        copy: Option<&wgpu::TextureView>,
        selected: &[(Owner, u8)],
        sampled: &[Option<wgpu::TextureView>],
    ) -> Result<Vec<WaterItem>, &'static str> {
        let mut groups: HashMap<(u16, Option<LightmapKey>), Vec<wgpu::BindGroup>> = HashMap::new();
        let stencil = wgpu::StencilState::default();
        let mut items = Vec::new();
        for key in selected {
            let &(first, count, lightmap, material) = self.parts.get(key).ok_or("a selected water part did not bind")?;
            if count == 0 {
                return Err("a selected water part draws nothing");
            }
            let m = self.materials.get(&material).ok_or("a selected water material did not bind")?;
            let views = match groups.entry((material, lightmap)) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => {
                    let views = self
                        .groups_for(device, m, lightmap, copy, sampled)
                        .ok_or("a water part's textures or supplied views did not bind")?;
                    entry.insert(views)
                }
            };
            for ((g, state, rigid), textures) in m.passes.iter().zip(views.iter()) {
                let (pipelines, uniforms, offset) = match key.0 {
                    Owner::Cluster(_) => (&g.pipelines, &g.uniforms, None),
                    Owner::Instance(i) => {
                        let r = rigid.as_ref().ok_or("a water placement has no rigid program")?;
                        (&r.pipelines, &r.uniforms, Some(r.offset(i).ok_or("a water placement has no slot")?))
                    }
                };
                let pipeline = pipelines.get(color_format, depth_format, &wgpu::StencilState::default(), None).ok_or("a water pipeline is not made")?;
                items.push(WaterItem {
                    key: described_pipeline(WATER_PROGRAM, color_format, depth_format, Some(state), None, stencil.clone()),
                    pipeline: pipeline.clone(),
                    uniforms: uniforms.clone(),
                    offset,
                    textures: textures.clone(),
                    blend: state.blend_constant,
                    first,
                    count,
                });
            }
        }
        if items.is_empty() {
            return Err("water selection draws nothing");
        }
        Ok(items)
    }

    /// [`WaterSet::prepare`]'s bytes for the selected parts' materials, as
    /// (buffer, offset, bytes): the uniforms the items read.
    pub(crate) fn writes(
        &self,
        view: &ViewInputs,
        fogs: &HashMap<u16, (shaders::psh::fog::Fog, [f32; 4])>,
        selected: &[(Owner, u8)],
    ) -> Result<Vec<(wgpu::Buffer, u64, Vec<u8>)>, &'static str> {
        let mut used: BTreeMap<u16, BTreeSet<u32>> = BTreeMap::new();
        for key in selected {
            let slots = used.entry(self.parts.get(key).ok_or("a selected water part did not bind")?.3).or_default();
            if let Owner::Instance(i) = key.0 { slots.insert(i); }
        }
        let mut writes = Vec::new();
        for (material, slots) in used {
            let m = self.materials.get(&material).ok_or("a selected water material did not bind")?;
            let (fog, color) = fogs.get(&material).ok_or("a selected water material has no resolved fog")?;
            for (g, _, rigid) in &m.passes {
                let mut pixel = g.program.pixel;
                pixel.fog = fog.uniform();
                pixel.fog_color = *color;
                writes.extend(g.prepare_writes(view, &g.program.vertex, &pixel));
                if let Some(r) = rigid { writes.extend(r.prepare_writes(view, slots.iter().copied(), None)); }
            }
        }
        Ok(writes)
    }

    /// Records the selected items in order into `pass` (the world's
    /// vertex and index buffers bound): `copy` the view copy
    /// ([`WaterSet::make_copy`], when a material samples the view), and
    /// `sampled` the render targets' views in [`WaterSet::targets`]'
    /// order. Returns how many parts were drawn; a part whose supply has
    /// no view records nothing (its material was refused at the ask).
    pub fn draw(
        &self,
        device: &wgpu::Device,
        pass: &mut wgpu::RenderPass<'_>,
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        copy: Option<&wgpu::TextureView>,
        selected: &[(Owner, u8)],
        sampled: &[Option<wgpu::TextureView>],
    ) -> usize {
        let Ok(items) = self.items(device, color_format, depth_format, copy, selected, sampled) else { return 0 };
        for it in items {
            pass.set_pipeline(&it.pipeline);
            pass.set_bind_group(0, &it.uniforms, it.offset.as_slice());
            pass.set_bind_group(1, &it.textures, &[]);
            pass.set_blend_constant(it.blend);
            pass.draw_indexed(it.first..it.first + it.count, 0, 0..1);
        }
        selected.len()
    }
}

/// Pool surfaces use LOD 0's sorted range in the compiled template.
pub fn sorted_pool(template: Option<&str>, material: &tags::shader::TemplatePostprocess) -> bool {
    template.is_some_and(|name| name.rsplit('\\').next().is_some_and(|leaf|
        leaf.eq_ignore_ascii_case("water_pool") || leaf.eq_ignore_ascii_case("water_pool_static")))
        && material.lods.first().is_some_and(|lod| usize::from(lod.layers.count) > 15)
        && material.layer_passes(0, 15).is_some_and(|passes| !passes.is_empty())
}

/// Edge-blend passes and their per-view engine texture supplies.
pub fn water_passes(material: &tags::material::Material, rasterizer: Option<map::TagId>) -> Result<Vec<FogPass>, crate::fog_gpu::FogRefusal> {
    let passes = supplied_layer_passes(material, WATER_LAYER, rasterizer)?;
    // The water pass draws under the device's stencil only.
    for p in &passes {
        p.state.device_stencil_only("water passes");
    }
    Ok(passes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fog_gpu::tests::{assert_split_images, fogs, parts, pass, player_viewport, render, selected, view, world, SIZE};
    use crate::gpu::GpuWorld;
    use wgpu::TextureFormat as F;

    #[test]
    fn pool_uses_the_first_lods_sorted_range() {
        use tags::shader::{IndexRange, TemplateLod, TemplatePass, TemplatePostprocess, TagRef};
        let mut template = TemplatePostprocess {
            lods: vec![TemplateLod { layers: IndexRange { start: 1, count: 16 },
                layer_mask: 1 << 15, unknown_6: 0 }],
            layers: vec![IndexRange::default(); 17],
            passes: vec![TemplatePass { pass: TagRef { group: map::FourCC(*b"spas"), id: map::TagId(3) },
                implementations: IndexRange { start: 0, count: 1 } }],
            implementations: vec![Default::default()],
            bindings: vec![],
        };
        template.layers[16] = IndexRange { start: 0, count: 1 };
        let pool = Some(r"shaders\shader_templates\water\water_pool");
        assert!(sorted_pool(pool, &template));
        assert!(!sorted_pool(Some("glass"), &template));
        assert!(template.layer_passes(0, WATER_LAYER).is_none());
        template.lods[0].layers.count = 15;
        assert!(!sorted_pool(pool, &template));
    }

    /// A water set over the synthetic scene: `samples_view` makes its
    /// stage 0 the view's own target (engine texture 5), else it samples
    /// nothing.
    fn scene(ctx: &gfx::GfxContext, samples_view: bool, translation: f32) -> GpuWorld {
        let mut world = world();
        world.placements[0].1[1][3] = translation;
        let parts = parts(&world);
        let mut gpu = GpuWorld::new(&ctx.device, &ctx.queue, world, &mut |_| None, &mut |_| None);
        let stage0 = if samples_view { Stage::Engine(5) } else { Stage::Empty };
        let (look, program) = pass(stage0);
        let set = WaterSet::new(
            &ctx.device, &ctx.queue, gpu.world(), parts, [(0, (vec![look], vec![program]))].into(), Vec::new(), samples_view,
            &mut |_| None, &mut |_| None,
        );
        assert_eq!(set.len(), 1);
        gpu.set_water(set);
        gpu
    }

    #[test]
    fn water_and_fog_reach_the_presented_edge_for_a_cluster_outside_retail() {
        let Ok(ctx) = gfx::GfxContext::headless() else { return };
        let mut gpu = scene(&ctx, false, 0.);
        let (_, program) = pass(Stage::Empty);
        let set = crate::fog_gpu::FogSet::new(
            &ctx.device, &ctx.queue, gpu.world(), parts(gpu.world()), [(0, vec![program])].into(),
            &mut |_| None, &mut |_| None,
        );
        gpu.set_fog(set);
        let selected = [(Owner::Cluster(0), 0)];
        let fog = fogs();
        for water in [false, true] {
            for factor in [1.0, 0.75] {
                let mut camera = view();
                // The test shader writes guest window pixels. At 4:3 its
                // quad spans -2.5..-0.5. Widen around the same viewport centre.
                let centre = SIZE as f32 / 2.;
                camera.constants[0] = [0., -0.5 * factor, 0., centre + (-1.5 - centre) * factor];
                let (color, _) = render(&ctx, |enc, arena, t| {
                    let mut uploads = gfx::ring::EncoderUploads { device: &ctx.device, queue: &ctx.queue, arena, encoder: enc };
                    let viewport = player_viewport(false);
                    let depth = t.depth.unwrap();
                    let n = if water {
                        gpu.prepare_water(&mut uploads, &camera, &fog);
                        gpu.encode_water(&ctx.device, enc, t.color, F::Rgba8Unorm, depth, F::Depth32Float,
                            t.size, viewport, gfx::external::TargetCopy::Display, &selected, &[])
                    } else {
                        gpu.prepare_fog(&mut uploads, &camera, &fog, &crate::fog_gpu::LayerSamples::new());
                        gpu.encode_fog(&ctx.device, enc, t.color, F::Rgba8Unorm, depth, F::Depth32Float,
                            t.size, viewport, &selected)
                    };
                    assert_eq!(n, 1);
                });
                let mut covered = false;
                for (i, pixel) in color.chunks_exact(4).enumerate() {
                    if pixel != [0; 4] {
                        assert!(i % (SIZE as usize) < (SIZE / 8) as usize, "outside the added left strip");
                        covered = true;
                    }
                }
                assert_eq!(covered, factor < 1., "water={water}, factor={factor}");
            }
        }
    }

    #[test]
    fn a_missing_rigid_pass_or_model_rows_leaves_only_the_placement_on_guest() {
        let Ok(ctx) = gfx::GfxContext::headless() else { return };
        for (has_rows, has_program) in [(false, true), (true, false), (true, true)] {
            let mut world = world();
            if !has_rows { world.placements.clear(); }
            let (look, mut program) = pass(Stage::Empty);
            if !has_program { program.rigid = None; }
            let set = WaterSet::new(
                &ctx.device, &ctx.queue, &world, parts(&world), [(0, (vec![look], vec![program]))].into(),
                Vec::new(), false, &mut |_| None, &mut |_| None,
            );
            assert!(set.draws_part(Owner::Cluster(0), 0));
            assert_eq!(set.draws_part(Owner::Instance(5), 0), has_rows && has_program);
        }
    }

    #[test]
    fn a_water_placement_moves_with_its_own_model_rows() {
        let Ok(ctx) = gfx::GfxContext::headless() else { return };
        let draw = |translation| {
            let mut gpu = scene(&ctx, false, translation);
            render(&ctx, |enc, arena, target| {
                let mut uploads = gfx::ring::EncoderUploads { device: &ctx.device, queue: &ctx.queue, arena, encoder: enc };
                gpu.prepare_water(&mut uploads, &view(), &fogs());
                let count = gpu.encode_water(
                    &ctx.device, enc, target.color, F::Rgba8Unorm, target.depth.unwrap(), F::Depth32Float,
                    (SIZE, SIZE), player_viewport(false), gfx::external::TargetCopy::Display, &[(Owner::Instance(5), 0)], &[],
                );
                assert_eq!(count, 1);
            }).0
        };
        let original = draw(0.0);
        let shifted = draw(3.0);
        assert!(original.iter().any(|&b| b != 0) && shifted.iter().any(|&b| b != 0));
        assert_ne!(original, shifted);
    }

    /// Without a view copy the water's packets with their uniforms
    /// written at the front of the submission draw what `encode_water`
    /// draws on its own pass after `prepare_water`.
    #[test]
    fn water_packets_draw_what_the_water_pass_draws_with_the_uniforms_written_up_front() {
        let Ok(ctx) = gfx::GfxContext::headless() else { return };
        let mut gpu = scene(&ctx, false, 3.0);
        let (selected, view, fogs) = (selected(), view(), fogs());
        let mut images = Vec::new();
        for (packets, split) in [(true, false), (false, false), (true, true), (false, true)] {
            images.push(render(&ctx, |enc, arena, target| {
                let draws = gpu
                    .water_draws(&ctx.device, F::Rgba8Unorm, F::Depth32Float, (SIZE, SIZE), player_viewport(split), &selected, &[], &view, &fogs)
                    .unwrap();
                assert_eq!((draws.ranges, draws.packets.len()), (2, 2));
                assert_eq!(draws.writes.len(), 4, "cluster uniforms and placement view rows");
                if packets {
                    for (buffer, offset, bytes) in &draws.writes {
                        arena.write_front(&ctx.device, &ctx.queue, buffer, *offset, bytes);
                    }
                    gfx::Renderer::encode_pass(&ctx, enc, target, gfx::Clear::NONE, &draws.packets, None).unwrap();
                } else {
                    let mut uploads = gfx::ring::EncoderUploads { device: &ctx.device, queue: &ctx.queue, arena, encoder: enc };
                    gpu.prepare_water(&mut uploads, &view, &fogs);
                    assert!(draws.writes.iter().all(|(buffer, ..)| arena.wrote_in_order(buffer)));
                    let recorded = gpu.encode_water(
                        &ctx.device, enc, target.color, F::Rgba8Unorm, target.depth.unwrap(), F::Depth32Float, (SIZE, SIZE), player_viewport(split),
                        gfx::external::TargetCopy::Display, &selected, &[],
                    );
                    assert_eq!(recorded, 2);
                }
            }));
        }
        assert!(images[0].0.iter().any(|&b| b != 0), "the water drew something");
        assert_split_images(&images);
    }

    /// A set that samples the view: the encoded commands (the staged
    /// uniforms, the scene copy and the pass) draw what `encode_water`
    /// does, and run again unchanged (they run twice, a validation dry run
    /// and the real one). Packets are not offered for it.
    #[test]
    fn water_that_samples_the_view_records_as_encoded_commands() {
        let Ok(ctx) = gfx::GfxContext::headless() else { return };
        let mut gpu = scene(&ctx, true, 3.0);
        let (selected, view, fogs) = (selected(), view(), fogs());
        assert!(gpu
            .water_draws(&ctx.device, F::Rgba8Unorm, F::Depth32Float, (SIZE, SIZE), player_viewport(false), &selected, &[], &view, &fogs)
            .is_err());
        let mut images = Vec::new();
        for (encoded, split) in [(true, false), (false, false), (true, true), (false, true)] {
            images.push(render(&ctx, |enc, arena, target| {
                if encoded {
                    let commands = gpu
                        .water_encoded(
                            &ctx.device, &ctx.queue, arena, target.color, F::Rgba8Unorm, target.depth.unwrap(), F::Depth32Float,
                            (SIZE, SIZE), player_viewport(split), gfx::external::TargetCopy::Display, &selected, &[], &view, &fogs,
                        )
                        .unwrap();
                    assert_eq!((commands.ranges, commands.passes), (2, 1));
                    let mut scratch = ctx.device.create_command_encoder(&Default::default());
                    (commands.commands)(&mut scratch);
                    drop(scratch.finish());
                    (commands.commands)(enc);
                } else {
                    let mut uploads = gfx::ring::EncoderUploads { device: &ctx.device, queue: &ctx.queue, arena, encoder: enc };
                    gpu.prepare_water(&mut uploads, &view, &fogs);
                    let recorded = gpu.encode_water(
                        &ctx.device, enc, target.color, F::Rgba8Unorm, target.depth.unwrap(), F::Depth32Float, (SIZE, SIZE), player_viewport(split),
                        gfx::external::TargetCopy::Display, &selected, &[],
                    );
                    assert_eq!(recorded, 2);
                }
            }));
        }
        assert!(images[0].0.iter().any(|&b| b != 0), "the water drew something");
        assert_split_images(&images);
    }
}
