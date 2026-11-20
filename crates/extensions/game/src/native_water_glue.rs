//! The native water's host (docs/graphics/native-water-fog.md, "Stage 4b:
//! water and the rest"): over the native world's loaded BSP, it compiles
//! the transparent parts' layer-6 passes at the load ([`load`]), and at
//! the layer-6 call puts the view's water in the frame step's scene
//! (`frame_scene::WaterState`) and draws the scene's copy as one pass per
//! view (`render_bridge::native_water`): a scene stage the render thread
//! prepares and records when it takes them, else reserved here
//! (`native_fog_glue::commit_late`, docs/graphics/framescene.md, slice 5).
//!
//! Sampled render targets use the texture header embedded at `+4` in the
//! target descriptor (`0x004b4b58`, stride `0x98`). The resolver at
//! `0x0001dcc0` requires a live surface at `+0x88` and a clear byte at
//! `+0x95`. Engine texture 5 samples a copy of the bound colour for the
//! view's own target.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use engine::memory::GuestMemory;
use engine::screen_effect::{TARGET_STRIDE, TARGETS};
use render_bridge::native_water::Host;
use map::{Map, TagId};
use native_render::fog_gpu::{FogProgram, FogWords};
use native_render::look::{self, Stage};
use native_render::program::ViewInputs;
use native_render::water_gpu::{self, Supply, WaterSet, WATER_LAYER};
use native_render::world::{LightmapKey, Owner, World};
use frame_scene::LatePart;

use crate::native_world_glue::{ReadMemory, Ready, StageDecoder, StageImages, State, current_ready, palettes_hold};

/// The water of one loaded world: what compiled, the parts the native
/// pass draws, and the engine-texture supplies it needs per view.
pub struct WaterData {
    /// Each material's resolved layer-6 passes (with the engine stages
    /// kept) and their compiled programs, in draw order.
    pub passes: HashMap<u16, Vec<native_render::fog_gpu::FogPass>>,
    pub programs: HashMap<u16, Vec<FogProgram>>,
    /// The fog words each material's passes set themselves (a water pass
    /// that reads the fog register gets the device's fog).
    pub words: HashMap<u16, FogWords>,
    /// The parts the pass draws, in the transparents' order: owner and
    /// part, index range, lightmap page and material.
    pub parts: Vec<((Owner, u8), (u32, u32, Option<LightmapKey>, u16))>,
    /// The skip's table: each part with its material's shader by tag
    /// index.
    pub part_set: HashMap<(Owner, u8), u16>,
    /// The distinct render targets the passes sample (their engine
    /// stages' `Target` supplies), ascending.
    pub targets: Vec<u32>,
    /// Whether any pass samples the view's own target.
    pub samples_view: bool,
}

/// The embedded texture header returned by `0x0001dcc0` for a live target.
fn target_header(mem: &dyn GuestMemory, target: u32) -> Option<u32> {
    let descriptor = TARGETS.checked_add(TARGET_STRIDE.checked_mul(target)?)?;
    let disabled = descriptor.checked_add(0x95)?;
    if mem.u32(descriptor + 0x88).ok()? == 0 || mem.u8(disabled).ok()? != 0 {
        return None;
    }
    Some(descriptor + 4)
}

/// Loads the water edge blend's passes over the world's transparent
/// parts (the water surfaces), called from the world's worker before its
/// draws are kept. One line says how many translated and why the rest
/// stay on the D3D path.
pub fn load<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    files: &mut assets::ResourceFiles,
    shaders: &[TagId],
    world: &World,
    lightmaps: &HashMap<LightmapKey, native_render::gpu::Image>,
    images: &mut StageImages,
    decoder: &mut StageDecoder,
) -> Arc<WaterData> {
    let mut passes: HashMap<u16, Vec<native_render::fog_gpu::FogPass>> = HashMap::new();
    let mut programs: HashMap<u16, Vec<FogProgram>> = HashMap::new();
    let mut words: HashMap<u16, FogWords> = HashMap::new();
    let mut parts: Vec<((Owner, u8), (u32, u32, Option<LightmapKey>, u16))> = Vec::new();
    let materials: Vec<u16> = world.transparents.iter().map(|t| t.material).collect::<BTreeSet<_>>().into_iter().collect();
    if !materials.is_empty() {
        if let Ok(mut lib) = tags::material::ShaderLibrary::read(map) {
            let mut refused: HashMap<String, usize> = HashMap::new();
            for &m in &materials {
                let Some(&shader) = shaders.get(usize::from(m)) else { continue };
                let compiled = lib
                    .material(shader, None)
                    .map_err(|e| format!("material: {e}"))
                    .and_then(|mat| water_gpu::water_passes(&mat).map_err(|r| r.describe()))
                    .and_then(|ps| {
                        ps.iter()
                            .map(|p| {
                                let code = look::world_program(map, lib.metadata(), p.look.vertex_shader)
                                    .ok_or("no world-space vertex program")?;
                                FogProgram::compile(p, &code)
                            })
                            .collect::<Result<Vec<_>, String>>()
                            .map(|compiled| (ps, compiled))
                    })
                    .and_then(|(ps, compiled)| {
                        // Every sampled stage's image, or the material
                        // stays on the D3D path (the compiled program's
                        // dims name what it samples).
                        for (p, c) in ps.iter().zip(&compiled) {
                            for (n, s) in p.look.textures.iter().enumerate() {
                                if let Stage::Bitmap { bitmap, image } = *s
                                    && c.program.dims[n].is_some()
                                {
                                    decoder.decode(map, files, images, bitmap, image, n, false)?;
                                }
                            }
                        }
                        Ok((ps, compiled))
                    });
                match compiled {
                    Ok((ps, compiled)) => {
                        words.insert(m, compiled.first().map(|p| p.words).unwrap_or_default());
                        passes.insert(m, ps);
                        programs.insert(m, compiled);
                    }
                    Err(why) => *refused.entry(why).or_default() += 1,
                }
            }
            let mut unlit = 0;
            for t in &world.transparents {
                // This lane has no rigid water program for placement-local streams.
                if matches!(t.owner, Owner::Instance(_)) {
                    continue;
                }
                let Some(compiled) = programs.get(&t.material) else { continue };
                let samples_lightmap = compiled.iter().any(|c| {
                    c.program.stages.iter().zip(&c.program.dims).any(|(s, d)| matches!(s, Stage::Lightmap) && d.is_some())
                });
                if samples_lightmap && !t.lightmap.is_some_and(|k| lightmaps.contains_key(&k)) {
                    unlit += 1;
                    continue;
                }
                parts.push(((t.owner, t.part), (t.first_index, t.index_count, t.lightmap, t.material)));
            }
            let mut why: Vec<_> = refused.into_iter().collect();
            why.sort();
            eprintln!(
                "native-water: {} of {} water materials' layer-6 passes translate, {} parts native ({unlit} without their lightmap page); on the D3D path: {why:?}",
                programs.len(),
                materials.len(),
                parts.len(),
            );
        }
    }
    let mut targets: Vec<u32> = passes
        .values()
        .flat_map(|ps| {
            ps.iter()
                .flat_map(|p| p.look.textures.iter().filter_map(|s| water_gpu::supply_of(*s)))
                .filter_map(|s| match s { Supply::Target(t) => Some(t), Supply::View => None })
        })
        .collect();
    targets.sort_unstable();
    targets.dedup();
    let samples_view = passes
        .values()
        .any(|ps| ps.iter().any(|p| p.look.textures.iter().any(|s| water_gpu::supply_of(*s) == Some(Supply::View))));
    let part_set: HashMap<(Owner, u8), u16> = parts
        .iter()
        .filter_map(|&(k, (_, _, _, m))| Some((k, shaders.get(usize::from(m))?.0 as u16)))
        .collect();
    Arc::new(WaterData {
        passes,
        programs,
        words,
        parts,
        part_set,
        targets,
        samples_view,
    })
}

/// Uploads the water set onto `gpu` (the world's `upload`, once its own
/// is done).
pub fn upload_water(
    water: &WaterData,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    gpu: &mut native_render::gpu::GpuWorld,
    stage_image: &mut dyn FnMut(Stage) -> Option<native_render::gpu::Image>,
    lightmap_image: &mut dyn FnMut(LightmapKey) -> Option<native_render::gpu::Image>,
) {
    let mut programs = HashMap::new();
    for (&m, compiled) in &water.programs {
        if let Some(ps) = water.passes.get(&m) {
            programs.insert(m, (ps.clone(), compiled.clone()));
        }
    }
    let set = WaterSet::new(
        device, queue, water.parts.clone(), programs, water.targets.clone(), water.samples_view,
        stage_image, lightmap_image,
    );
    if hooks::native_verbose() {
        eprintln!(
            "native-water: {} materials' layer-6 passes uploaded, {} parts, targets {:?}, samples the view: {}",
            set.len(),
            water.parts.len(),
            water.targets,
            water.samples_view
        );
    }
    gpu.set_water(set);
}

struct Glue<B: d3d::backend::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
    state: State,
}

impl<B: d3d::backend::Backend + Send + 'static> Host for Glue<B> {
    fn ask(&self, mem: &mut dyn GuestMemory) -> Option<native_render::skip::Cleared> {
        use timeline::native::{Metric, Stage};
        let ready = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            Arc::clone(current_ready(&state, mem)?)
        };
        let water = ready.water.clone()?;
        let Ok(mut d) = self.device.lock() else { return None };
        if !d.can_external_pass()
            || !d.begin_external_boundary(&mut crate::native_lights_glue::DeviceMemory(mem)).ok()?
        {
            return None;
        }
        let read = ReadMemory(mem);
        if !d.stencil_state(&read).is_some_and(|s| !s.enable)
            || !palettes_hold(&ready.palettes, &|n| d.stage_palette(&read, n))
        {
            return None;
        }
        // The render targets the passes sample (the reflection inputs):
        // their embedded texture headers, resolved by the reservation.
        let mut headers = Vec::with_capacity(water.targets.len());
        for &t in &water.targets {
            let Some(header) = target_header(mem, t) else {
                if !TARGET_UNREADABLE.swap(true, Ordering::Relaxed) {
                    eprintln!("native-water: render target {t} is unavailable; the D3D path draws the water");
                }
                return None;
            };
            headers.push(header);
        }
        // With a render thread that records scene stages, everything here
        // is extraction; the water's preparation and recording run on the
        // render thread (docs/graphics/framescene.md, slice 5).
        let scene = d.queues_scene_stages(Stage::Water);
        let planning = timeline::native::Timer::start(Stage::Water, if scene { Metric::ExtractNs } else { Metric::PrepareNs });
        // The fog state, as the fog's (a water pass that reads the fog
        // register gets the device's).
        let device_words = crate::native_fog_glue::device_fog_words(&d, mem);
        let materials = crate::native_fog_glue::resolve_materials(&water.words, &device_words)?;
        // The skip over a preview: nothing touches the guest until the
        // reservation has recorded on every copy.
        let read = ReadMemory(mem);
        let mut preview = native_render::skip::MaskPreview::new(&read)?;
        let (selected, cleared) = native_render::fog::clear_fog(&mut preview, WATER_LAYER as u32, &|item| {
            let accepted = water.part_set.get(&(item.owner, item.part)) == Some(&(item.shader));
            if !accepted {
                Stage::Water.count(Metric::Examined, 1);
                Stage::Water.count(Metric::RefusedOwnership, 1);
            }
            accepted
        })?;
        let masks = preview.finish()?;
        if selected.is_empty() {
            return None;
        }
        // The view's water into the frame step's scene; the stage draws
        // the scene's copy. A view the scene cannot take stays on the D3D
        // path.
        let parts: Vec<LatePart> = selected.iter().map(|item| LatePart { owner: item.owner, part: item.part }).collect();
        let late = render_bridge::scene_step::late_layer(&*mem, WATER_LATE_LAYER, true, device_words, &materials, &parts)?;
        let fogs = crate::native_fog_glue::late_fogs(&late.materials);
        let selected = crate::native_fog_glue::late_selected(&late.parts);
        let ranges = selected.len();
        let writes_depth = water
            .programs
            .values()
            .any(|passes| passes.iter().any(|p| p.state.depth_write));
        let view = mem.u32(engine::render_loop::occlusion::guest::VIEW_INDEX).unwrap_or(u32::MAX);
        let plan = d3d::ExternalPlan { stage: Stage::Water, view, ranges, slots: Vec::new() };
        let run = u64::from(d.swap_count()).wrapping_add(1);
        let pass = || -> Box<dyn d3d::ExternalPass> {
            Box::new(WaterViewPass {
                ready: Arc::clone(&ready),
                selected: selected.clone(),
                fogs: fogs.clone(),
                headers: headers.clone(),
                samples_view: water.samples_view,
                view: None,
                w_clip: None,
                writes_depth,
                run,
            })
        };
        drop(planning);
        if !crate::native_fog_glue::commit_late(&mut d, mem, &self.state, &ready, scene, plan, &pass, &masks) {
            return None;
        }
        render_bridge::scene_step::late_drawn(late.key, WATER_LATE_LAYER);
        Some(cleared)
    }
}

/// The water's layer, as the scene names it.
const WATER_LATE_LAYER: u8 = WATER_LAYER as u8;

static TARGET_UNREADABLE: AtomicBool = AtomicBool::new(false);

/// Installs the host over the world's state (`native_world_glue::install`).
pub fn install<B: d3d::backend::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>, state: State) {
    render_bridge::native_water::set_host(Arc::new(Glue {
        device: Arc::clone(device),
        state,
    }));
}

/// One view's water replacement.
struct WaterViewPass {
    ready: Arc<Ready>,
    /// The parts the masks surrendered, in the layer's item order.
    selected: Vec<(Owner, u8)>,
    /// Each material's fog unit and colour, resolved at the ask.
    fogs: HashMap<u16, (shaders::psh::fog::Fog, [f32; 4])>,
    /// The render targets' guest texture headers ([`WaterData::targets`]'
    /// order), for the reservation's sampled levels.
    headers: Vec<u32>,
    samples_view: bool,
    view: Option<[[f32; 4]; d3d::VIEW_CONSTANTS]>,
    w_clip: Option<([f32; 2], bool)>,
    writes_depth: bool,
    /// The swap the view is in: the run's key, so attachments are pinned
    /// once when no render target is sampled
    /// (`d3d::ExternalPass::batch_key`).
    run: u64,
}

impl WaterViewPass {
    /// What a copy must hold for the pass to draw there: the sampled
    /// render targets (live, not attachments, filterable) and, when a
    /// material samples the view, a bound colour that can be copied and
    /// filtered.
    fn preflight(
        &self,
        sampled: &[Option<d3d::backend::SampledLevel>],
        color: &wgpu::TextureView,
        color_format: wgpu::TextureFormat,
        depth: &wgpu::TextureView,
        size: (u32, u32),
        features: wgpu::Features,
    ) -> Result<(), &'static str> {
        if self.view.is_none() || self.w_clip.is_none() {
            return Err("water view constants or W-buffer clip unavailable");
        }
        if sampled.len() != self.headers.len() {
            return Err("a water target sample is unavailable on this copy");
        }
        for level in sampled {
            let level = level.as_ref().ok_or("a water target sample is unavailable on this copy")?;
            let texture = level.view.texture();
            if texture == color.texture() || texture == depth.texture() {
                return Err("a water sample aliases an attachment");
            }
            if !gfx::program::filterable(texture.format(), features) {
                return Err("a water sample is not filterable");
            }
        }
        if self.samples_view {
            if !gfx::program::filterable(color_format, features) {
                return Err("the bound colour is not filterable for the scene stages");
            }
            if color.texture().format() != color_format || !WaterSet::can_copy(color, size) {
                return Err("the bound colour cannot be copied for the scene stages");
            }
        }
        Ok(())
    }

    /// The view inputs and sampled views of a pinned copy, after its
    /// preflight.
    fn inputs(
        &self,
        t: &d3d::backend::ExternalAttachments,
        request: d3d::ExternalPassRequest,
    ) -> Result<(ViewInputs, Vec<Option<wgpu::TextureView>>), &'static str> {
        self.preflight(&t.sampled, &t.color, t.color_format, &t.depth, t.size, t.device.features())?;
        let constants = self.view.ok_or("water view constants unavailable")?;
        let view = ViewInputs {
            constants,
            guest_size: request.surface_size,
            size: t.size,
            z_scale: request.z_scale,
            w_clip: self.w_clip,
        };
        let sampled = t.sampled.iter().map(|s| s.as_ref().map(|s| s.view.clone())).collect();
        Ok((view, sampled))
    }
}

impl d3d::ExternalPass for WaterViewPass {
    // The selection and constants belong to this view. Ready pins the BSP,
    // and each recording binds the target copy passed to it by the renderer.
    fn replays_scene(&self) -> bool { true }

    /// Pinned attachments only when no render target is sampled: the pin
    /// carries no sampled levels (`d3d::ThreadedBackend`).
    fn batch_key(&self) -> Option<u64> {
        self.headers.is_empty().then_some(self.run)
    }

    fn view_constants(&mut self, constants: &[[f32; 4]; d3d::VIEW_CONSTANTS]) {
        self.view = Some(*constants);
    }

    fn depth_clip(&mut self, clip: d3d::DepthClip) {
        self.w_clip = (clip.w_buffered == Some(true)).then_some((clip.range, clip.clamp));
    }

    /// The render targets' texture headers, resolved at the ask: the
    /// reflection inputs, resolved to the live levels (a header that
    /// cannot be resolved refuses the whole replacement).
    fn sampled(&self) -> Vec<u32> {
        self.headers.clone()
    }

    fn prepare_copy(&mut self, t: &d3d::ExternalTarget<'_>) -> Result<(), &'static str> {
        self.preflight(t.sampled, t.color, t.color_format, t.depth, t.size, t.device.features())?;
        let guard = crate::native_world_glue::upload(&self.ready, t);
        let gpu = guard.as_ref().ok_or("world geometry unavailable")?;
        let set = gpu.water().ok_or("water materials unavailable")?;
        for &(owner, part) in &self.selected {
            if !set.draws_part(owner, part) {
                return Err("a selected water part did not bind");
            }
        }
        Ok(())
    }

    fn writes(&self) -> d3d::ExternalWrites {
        d3d::ExternalWrites { color: true, depth_stencil: self.writes_depth }
    }

    /// Without a scene copy the water's draws join the D3D render pass
    /// already open on these attachments, their uniforms written ahead of
    /// them (`ExternalDraws::writes`). A set that samples the view's own
    /// target needs a copy first: [`WaterViewPass::record_encoded`].
    fn record_draws(
        &mut self, t: &d3d::backend::ExternalAttachments, request: d3d::ExternalPassRequest,
    ) -> Option<Result<d3d::backend::ExternalDraws, &'static str>> {
        if self.samples_view {
            return None;
        }
        let (view, sampled) = match self.inputs(t, request) {
            Ok(inputs) => inputs,
            Err(why) => return Some(Err(why)),
        };
        let mut guard = crate::native_world_glue::upload_on(&self.ready, &t.device, &t.queue);
        let Some(gpu) = guard.as_mut() else { return Some(Err("world geometry unavailable")) };
        Some(gpu.water_draws(&t.device, t.color_format, t.depth_format, t.size, &self.selected, &sampled, &view, &self.fogs))
    }

    /// The set that samples the view: the scene copy and the pass as
    /// commands in the frame's open encoder, their uniforms staged now
    /// (`GpuWorld::water_encoded`): no command buffer of their own.
    fn record_encoded(
        &mut self, t: &d3d::backend::ExternalAttachments, request: d3d::ExternalPassRequest,
    ) -> Option<Result<d3d::backend::ExternalEncoded, &'static str>> {
        if !self.samples_view {
            return None;
        }
        let (view, sampled) = match self.inputs(t, request) {
            Ok(inputs) => inputs,
            Err(why) => return Some(Err(why)),
        };
        let mut guard = crate::native_world_glue::upload_on(&self.ready, &t.device, &t.queue);
        let Some(gpu) = guard.as_mut() else { return Some(Err("world geometry unavailable")) };
        Some(gpu.water_encoded(
            &t.device, &t.queue, &t.uploads, &t.color, t.color_format, &t.depth, t.depth_format,
            t.size, t.copy, &self.selected, &sampled, &view, &self.fogs,
        ))
    }

    fn record_ranges(&mut self, mut t: d3d::ExternalTarget<'_>) -> usize {
        let Some(constants) = self.view else { return 0 };
        let view = ViewInputs {
            constants,
            guest_size: t.guest_size,
            size: t.size,
            z_scale: t.z_scale,
            w_clip: self.w_clip,
        };
        let sampled: Vec<Option<wgpu::TextureView>> = t.sampled.iter().map(|s| s.as_ref().map(|s| s.view.clone())).collect();
        let mut gpu = crate::native_world_glue::upload(&self.ready, &t);
        let Some(gpu) = gpu.as_mut() else { return 0 };
        {
            let mut uploads = t.encoder_uploads();
            gpu.prepare_water(&mut uploads, &view, &self.fogs);
        }
        gpu.encode_water(
            t.device, t.encoder, t.color, t.color_format, t.depth, t.depth_format,
            t.size, t.copy, &self.selected, &sampled,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use d3d::{ExternalPass, TargetCopy, backend::SampledLevel};
    use engine::memory::FlatMemory;
    use wgpu::{TextureFormat, TextureUsages};

    #[test]
    fn target_header_uses_the_embedded_texture_of_a_live_target() {
        let mut mem = FlatMemory::new(TARGETS, (2 * TARGET_STRIDE) as usize);
        let descriptor = TARGETS + TARGET_STRIDE;
        assert_eq!(target_header(&mem, 1), None);

        mem.set_u32(descriptor + 0x88, 0x1000).unwrap();
        mem.set_u8(descriptor + 0x94, 1).unwrap();
        assert_eq!(target_header(&mem, 1), Some(native_render::object_nodes::TEXACCUM_HEADER));

        mem.set_u8(descriptor + 0x95, 1).unwrap();
        assert_eq!(target_header(&mem, 1), None);
        assert_eq!(target_header(&mem, 2), None);
    }

    #[test]
    fn prepare_copy_refuses_attachment_aliases_and_unfilterable_formats() {
        let Ok(ctx) = gfx::GfxContext::headless() else {
            eprintln!("SKIPPED water preflight test: no headless adapter");
            return;
        };
        let size = (4, 4);
        let make_view = |format, usage| {
            ctx.device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            }).create_view(&Default::default())
        };
        let color_usage = TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC | TextureUsages::TEXTURE_BINDING;
        let color = make_view(TextureFormat::Rgba8Unorm, color_usage);
        let depth = make_view(TextureFormat::Depth32Float, TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING);
        let integer = make_view(TextureFormat::Rgba8Uint, color_usage);
        let valid = make_view(TextureFormat::Rgba8Unorm, TextureUsages::TEXTURE_BINDING);
        let uploads = gfx::ring::UploadPool::default().begin();
        for copy in [TargetCopy::Reference, TargetCopy::Display] {
            let prepare = |color: &wgpu::TextureView, sampled: Option<&wgpu::TextureView>, samples_view| {
                let mut encoder = ctx.device.create_command_encoder(&Default::default());
                let sampled: Vec<_> = sampled
                    .map(|v| Some(SampledLevel { view: v.texture().create_view(&Default::default()), size }))
                    .into_iter()
                    .collect();
                let mut pass = WaterViewPass {
                    ready: Arc::new(Ready::default()),
                    selected: Vec::new(),
                    fogs: HashMap::new(),
                    headers: vec![native_render::object_nodes::TEXACCUM_HEADER; sampled.len()],
                    samples_view,
                    view: Some([[0.0; 4]; d3d::VIEW_CONSTANTS]),
                    w_clip: Some(([0.0, 1.0], false)),
                    writes_depth: false,
                    run: 0,
                };
                pass.prepare_copy(&d3d::ExternalTarget {
                    device: &ctx.device,
                    queue: &ctx.queue,
                    encoder: &mut encoder,
                    color,
                    color_format: color.texture().format(),
                    depth: &depth,
                    depth_format: depth.texture().format(),
                    viewport: [0.0, 0.0, 4.0, 4.0],
                    z_scale: 1.0,
                    guest_size: size,
                    size,
                    sampled: &sampled,
                    slots: &[],
                    uploads: &uploads,
                    copy,
                    stages: [None; 4],
                    on_submit: None,
                })
            };
            assert_eq!(prepare(&color, Some(&color), false), Err("a water sample aliases an attachment"));
            assert_eq!(prepare(&color, Some(&depth), false), Err("a water sample aliases an attachment"));
            assert_eq!(prepare(&color, Some(&integer), false), Err("a water sample is not filterable"));
            assert_eq!(prepare(&integer, None, true), Err("the bound colour is not filterable for the scene stages"));
            assert_eq!(prepare(&color, Some(&valid), true), Err("world geometry unavailable"));
        }
    }
}
