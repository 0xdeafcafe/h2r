//! The water edge blend, layer 6 (docs/graphics/native-renderer.md,
//! "Stage 4b: water and the rest"): the water templates' own layer-6
//! passes, drawn natively inside the window between the game's water
//! setup `0x0003ba90` and its teardown `0x0003c3c0` (whose alpha-only
//! clear `0x3c515` stays in guest order — the wrappers are never
//! skipped, only the layer's items).
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
//! The geometry is the water surfaces themselves, the kind-4 transparent
//! parts ([`crate::world::World::transparents`]): one item draws them in
//! layer 15 and again in layer 6, and only layer 6 is taken here.

use std::collections::HashMap;

use crate::fog_gpu::{supplied_layer_passes, FogPass, FogProgram, FogWords};
use crate::gpu::{Image, texture};
use crate::look::Stage;
use crate::pass_state::PassState;
use crate::program::{described_pipeline, Gpu, ViewInputs};
use crate::world::{LightmapKey, Owner};

/// The layer this module draws.
pub const WATER_LAYER: usize = 6;

/// The shader id the water's packets report to the pass they join (for
/// diagnostics; the pipelines are the passes' own).
const WATER_PROGRAM: gfx::pipeline::ShaderId = gfx::pipeline::ShaderId(u32::MAX - 5);

/// The scratch copy of the bound colour for the stages that sample the
/// view ([`WaterSet::scene_copy`]): copy `from` into `to` (`extent`) before
/// the pass; `view` is `to`'s.
pub(crate) struct SceneCopy {
    pub from: wgpu::Texture,
    pub to: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub extent: wgpu::Extent3d,
}

/// One pass of one selected part, ready to issue ([`WaterSet::items`]).
pub(crate) struct WaterItem {
    pub key: gfx::pipeline::PipelineKey,
    pub pipeline: wgpu::RenderPipeline,
    pub uniforms: wgpu::BindGroup,
    pub textures: wgpu::BindGroup,
    pub blend: wgpu::Color,
    pub first: u32,
    pub count: u32,
}

/// `items` as packets for the D3D pass already open on these attachments:
/// the whole target as viewport and scissor, the world's program vertices
/// and indices ([`crate::gpu::GpuWorld::water_draws`]).
pub(crate) fn item_packets(
    items: &[WaterItem],
    cache: &mut HashMap<wgpu::RenderPipeline, std::sync::Arc<gfx::compile::PendingPipeline>>,
    vertices: &wgpu::Buffer,
    indices: &wgpu::Buffer,
    target_size: (u32, u32),
) -> Result<Vec<gfx::RecordedDraw>, &'static str> {
    use gfx::{BoundGroup, RecordedDraw, ScissorRect};
    let scissor = ScissorRect::full(target_size.0, target_size.1);
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
                vec![BoundGroup::new(0, it.uniforms.clone()), BoundGroup::new(1, it.textures.clone())],
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
    scene_copies: [Option<wgpu::TextureView>; 2],
}

struct WaterMaterial {
    passes: Vec<(Gpu, PassState)>,
    /// Per stage: the engine supply ([`supply_of`]).
    supplies: [Option<Supply>; 4],
    words: FogWords,
}

impl WaterSet {
    /// Uploads `programs` (each material's layer-6 passes and their
    /// compiled programs) for `parts` (the transparents' parts list:
    /// owner and part, index range, lightmap page and material).
    /// `stage_image` decodes a stage's bitmap image, `lightmap_image` a
    /// page. A material is kept only when every pass uploads and its
    /// passes agree on their supplies.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        parts: Vec<((Owner, u8), (u32, u32, Option<LightmapKey>, u16))>,
        programs: HashMap<u16, (Vec<FogPass>, Vec<FogProgram>)>,
        targets: Vec<u32>,
        samples_view: bool,
        stage_image: &mut dyn FnMut(Stage) -> Option<Image>,
        lightmap_image: &mut dyn FnMut(LightmapKey) -> Option<Image>,
    ) -> Self {
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
            for p in compiled {
                let Some(g) = Gpu::new(device, queue, p.program, stage_image) else {
                    continue 'material;
                };
                gpus.push((g, p.state));
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
        Self { materials, parts: parts.into_iter().collect(), targets, samples_view, pages, scene_copies: [None, None] }
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
            m.passes.iter().all(|(g, _)| {
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
            for (g, _) in &m.passes {
                let mut pixel = g.program.pixel;
                pixel.fog = fog.uniform();
                pixel.fog_color = *color;
                g.prepare_with(uploads, view, &pixel);
            }
        }
    }

    /// Makes the pipelines for these target formats (before the pass).
    pub fn pipelines(&mut self, device: &wgpu::Device, color: wgpu::TextureFormat, depth: wgpu::TextureFormat) {
        for m in self.materials.values_mut() {
            for (g, state) in &mut m.passes {
                g.pipeline(device, color, depth, Some(&*state));
            }
        }
    }

    /// The scene copy needs the full, single-sampled colour attachment.
    pub fn can_copy(color: &wgpu::TextureView, size: (u32, u32)) -> bool {
        let texture = color.texture();
        texture.usage().contains(wgpu::TextureUsages::COPY_SRC)
            && texture.dimension() == wgpu::TextureDimension::D2
            && texture.sample_count() == 1
            && texture.depth_or_array_layers() == 1
            && (texture.width(), texture.height()) == size
    }

    /// Copies the bound colour for the stages that sample the view's own
    /// target into this target copy's scratch texture, remade when its
    /// extent or format changes. The copy is recorded before
    /// the pass draws into the target, so the stages read the scene as
    /// the wrapper's own draws found it.
    pub fn make_copy(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        copy: d3d::TargetCopy,
    ) -> wgpu::TextureView {
        let scene = self.scene_copy(device, color, copy);
        encoder.copy_texture_to_texture(scene.from.as_image_copy(), scene.to.as_image_copy(), scene.extent);
        scene.view
    }

    /// [`WaterSet::make_copy`] without the command: the scratch texture
    /// made (or kept) for this target copy and the copy to record before
    /// the pass, for a recording that issues it later.
    pub(crate) fn scene_copy(
        &mut self,
        device: &wgpu::Device,
        color: &wgpu::TextureView,
        copy: d3d::TargetCopy,
    ) -> SceneCopy {
        let texture = color.texture();
        let extent = texture.size();
        let format = texture.format();
        let scratch = match copy {
            d3d::TargetCopy::Reference => &mut self.scene_copies[0],
            d3d::TargetCopy::Display => &mut self.scene_copies[1],
        };
        if scratch.as_ref().is_some_and(|view| {
            let texture = view.texture();
            texture.size() != extent || texture.format() != format
        }) {
            *scratch = None;
        }
        let view = scratch.get_or_insert_with(|| {
            let usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("native water view copy"),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            }).create_view(&Default::default())
        });
        SceneCopy { from: texture.clone(), to: view.texture().clone(), view: view.clone(), extent }
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
            .map(|(g, _)| g.textures_supplied(device, page.as_ref(), &supplied))
            .collect()
    }

    /// Every pass of every selected part, in reservation order, with its
    /// pipeline, groups, blend constant and index range: what
    /// [`WaterSet::draw`] issues, as data (the pipelines are made,
    /// [`WaterSet::pipelines`]). Refused when a part, pass, supply or
    /// group is missing, as `draw` records nothing then.
    #[allow(clippy::too_many_arguments)]
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
            let at = (material, lightmap);
            if !groups.contains_key(&at) {
                let views = self.groups_for(device, m, lightmap, copy, sampled)
                    .ok_or("a water part's textures or supplied views did not bind")?;
                groups.insert(at, views);
            }
            for ((g, state), textures) in m.passes.iter().zip(&groups[&at]) {
                let pipeline = g.pipelines.get(&(color_format, depth_format)).ok_or("a water pipeline is not made")?;
                items.push(WaterItem {
                    key: described_pipeline(WATER_PROGRAM, color_format, depth_format, Some(state), None, stencil.clone()),
                    pipeline: pipeline.clone(),
                    uniforms: g.uniforms.clone(),
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
        let mut used = std::collections::BTreeSet::new();
        for key in selected {
            used.insert(self.parts.get(key).ok_or("a selected water part did not bind")?.3);
        }
        let mut writes = Vec::new();
        for material in used {
            let m = self.materials.get(&material).ok_or("a selected water material did not bind")?;
            let (fog, color) = fogs.get(&material).ok_or("a selected water material has no resolved fog")?;
            for (g, _) in &m.passes {
                let mut pixel = g.program.pixel;
                pixel.fog = fog.uniform();
                pixel.fog_color = *color;
                writes.extend(g.prepare_writes(view, &pixel));
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
        let mut groups: HashMap<(u16, Option<LightmapKey>), Vec<wgpu::BindGroup>> = HashMap::new();
        for key in selected {
            let Some(&(first_index, index_count, lightmap, material)) = self.parts.get(key) else { return 0 };
            let Some(m) = self.materials.get(&material) else { return 0 };
            let key = (material, lightmap);
            if !groups.contains_key(&key) {
                let Some(views) = self.groups_for(device, m, lightmap, copy, sampled) else { return 0 };
                groups.insert(key, views);
            }
            for ((g, state), textures) in m.passes.iter().zip(&groups[&key]) {
                let Some(pipeline) = g.pipelines.get(&(color_format, depth_format)) else { return 0 };
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &g.uniforms, &[]);
                pass.set_bind_group(1, textures, &[]);
                pass.set_blend_constant(state.blend_constant);
                pass.draw_indexed(first_index..first_index + index_count, 0, 0..1);
            }
        }
        selected.len()
    }
}

/// The layer-6 passes of `material` with the per-view supplied engine
/// stages kept: [`supplied_layer_passes`] at [`WATER_LAYER`].
pub fn water_passes(material: &tags::material::Material) -> Result<Vec<FogPass>, crate::fog_gpu::FogRefusal> {
    supplied_layer_passes(material, WATER_LAYER)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fog_gpu::tests::{fogs, parts, pass, render, selected, view, world, SIZE};
    use crate::gpu::GpuWorld;
    use wgpu::TextureFormat as F;

    /// A water set over the synthetic scene: `samples_view` makes its
    /// stage 0 the view's own target (engine texture 5), else it samples
    /// nothing.
    fn scene(ctx: &gfx::GfxContext, samples_view: bool) -> GpuWorld {
        let world = world();
        let parts = parts(&world);
        let mut gpu = GpuWorld::new(&ctx.device, &ctx.queue, world, &mut |_| None, &mut |_| None);
        let stage0 = if samples_view { Stage::Engine(5) } else { Stage::Empty };
        let (look, program) = pass(stage0);
        let set = WaterSet::new(
            &ctx.device, &ctx.queue, parts, [(0, (vec![look], vec![program]))].into(), Vec::new(), samples_view,
            &mut |_| None, &mut |_| None,
        );
        assert_eq!(set.len(), 1);
        gpu.set_water(set);
        gpu
    }

    /// Without a view copy the water's packets with their uniforms
    /// written at the front of the submission draw what `encode_water`
    /// draws on its own pass after `prepare_water`.
    #[test]
    fn water_packets_draw_what_the_water_pass_draws_with_the_uniforms_written_up_front() {
        let Ok(ctx) = gfx::GfxContext::headless() else { return };
        let mut gpu = scene(&ctx, false);
        let (selected, view, fogs) = (selected(), view(), fogs());
        let mut images = Vec::new();
        for packets in [true, false] {
            images.push(render(&ctx, |enc, arena, target| {
                let draws = gpu
                    .water_draws(&ctx.device, F::Rgba8Unorm, F::Depth32Float, (SIZE, SIZE), &selected, &[], &view, &fogs)
                    .unwrap();
                assert_eq!((draws.ranges, draws.packets.len()), (2, 2));
                assert_eq!(draws.writes.len(), 2, "the pass's vertex and pixel blocks");
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
                        &ctx.device, enc, target.color, F::Rgba8Unorm, target.depth.unwrap(), F::Depth32Float, (SIZE, SIZE),
                        d3d::TargetCopy::Display, &selected, &[],
                    );
                    assert_eq!(recorded, 2);
                }
            }));
        }
        assert!(images[0].0.iter().any(|&b| b != 0), "the water drew something");
        assert_eq!(images[0], images[1]);
    }

    /// A set that samples the view: the encoded commands (the staged
    /// uniforms, the scene copy and the pass) draw what `encode_water`
    /// does, and run again unchanged (they run twice, a validation dry run
    /// and the real one). Packets are not offered for it.
    #[test]
    fn water_that_samples_the_view_records_as_encoded_commands() {
        let Ok(ctx) = gfx::GfxContext::headless() else { return };
        let mut gpu = scene(&ctx, true);
        let (selected, view, fogs) = (selected(), view(), fogs());
        assert!(gpu
            .water_draws(&ctx.device, F::Rgba8Unorm, F::Depth32Float, (SIZE, SIZE), &selected, &[], &view, &fogs)
            .is_err());
        let mut images = Vec::new();
        for encoded in [true, false] {
            images.push(render(&ctx, |enc, arena, target| {
                if encoded {
                    let commands = gpu
                        .water_encoded(
                            &ctx.device, &ctx.queue, arena, target.color, F::Rgba8Unorm, target.depth.unwrap(), F::Depth32Float,
                            (SIZE, SIZE), d3d::TargetCopy::Display, &selected, &[], &view, &fogs,
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
                        &ctx.device, enc, target.color, F::Rgba8Unorm, target.depth.unwrap(), F::Depth32Float, (SIZE, SIZE),
                        d3d::TargetCopy::Display, &selected, &[],
                    );
                    assert_eq!(recorded, 2);
                }
            }));
        }
        assert!(images[0].0.iter().any(|&b| b != 0), "the water drew something");
        assert_eq!(images[0], images[1]);
    }
}
