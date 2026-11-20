//! The native world's host (docs/graphics/native-world.md, stage 1):
//! loads the active BSP's world from the loaded map's file when the BSP
//! changes, on a worker, and asks the D3D device for the native pass at
//! each view's layer 3 (`render_bridge::native_world`). Inert with
//! `impl.render.native=off` (nothing asks).
//!
//! - **Load once.** On a new (map, BSP) the worker opens the map through
//!   the VFS, finds the scenario BSP whose tag data sits
//!   at the active BSP's address, builds the world (`native_render::load`),
//!   decodes each lightmap page, and translates each material's layer-3
//!   pass. The GPU side is made on the render thread by the first pass.
//!   Until it is ready, or when it fails, the D3D path draws the world.
//! - **Per frame.** The camera and one external pass; the device records it
//!   after the next drain, in order with the draws.
//! - **Material programs.** Each part needs a translated layer-3 pass with decoded
//!   stage bitmaps, the inputs of its vertex classification, and any
//!   sampled lightmap page (`program::part_fit`). The part draws with
//!   its own vertex program, pixel shader, stage textures,
//!   constants and render states, and the device's view constants each
//!   frame. Every other material is left out of the world and keeps its
//!   D3D draws (the skip clears only the items of the parts the world
//!   draws, by owner and part index, whatever shares their shader). The load says
//!   the coverage and each reason once, with counts. A palettized stage
//!   bitmap (the bump map's P8) is expanded through the device palette
//!   bound at its stage when the load started, as the D3D path samples
//!   it; while that palette is no longer bound the D3D path draws the
//!   world.
//! - **Lights**: the lights' part of the world, loaded with it
//!   (`native_lights_glue::load`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use render_bridge::native_map_work::{self, Stage as MapStage};
use render_bridge::native_world::{Host, PartSet};
use map::{Map, TagId};
use native_render::camera::RenderCamera;
use native_render::gpu::{DepthRule, GpuWorld, Image, PassStencil, PassViewport, WorldSelection};
use native_render::light_gpu::{LayeredImage, TextureDim};
use native_render::load::{self, LoadedWorld};
use native_render::look::{self, Stage};
use native_render::program::{self, MaterialProgram, ProgramSet, ViewInputs};
use native_render::skip::StencilPlan;
use native_render::world::{LightmapKey, Owner, World, WorldDraw};

#[path = "native_world_materials.rs"]
mod opaque_materials;

type LayeredImages = HashMap<(TagId, u32), LayeredImage>;

/// What the worker hands the render thread: the world and its images.
pub struct CpuWorld {
    pub world: World,
    pub lightmaps: HashMap<LightmapKey, Image>,
    /// Materials with their own programs, and their stages' images.
    pub programs: HashMap<u16, MaterialProgram>,
    pub stage_images: StageImages,
    layered_images: LayeredImages,
}

/// One loaded world: CPU data until the first pass uploads it.
#[cfg_attr(test, derive(Default))]
pub struct Ready {
    opaque: opaque_materials::Library,
    cpu: Mutex<Option<CpuWorld>>,
    gpu: Mutex<Option<GpuWorld>>,
    /// The parts the native pass draws, with their shaders: what the skip
    /// clears.
    parts: Arc<PartSet>,
    part_bounds: HashMap<(Owner, u8), native_render::world::Aabb>,
    /// How many of the load's materials draw natively, of how many the
    /// world holds: the load's one summary line.
    coverage: (usize, usize),
    rejection_summary: Vec<(String, usize)>,
    failed_sections: u32,
    declared_materials: usize,
    /// The fog of this load (`native_fog_glue`): the compiled layer-16
    /// passes, their fog words and the parts the fog pass draws.
    pub fog: Option<Arc<crate::native_fog_glue::FogView>>,
    /// The water edge blend of this load (`native_water_glue`): the
    /// compiled layer-6 passes, their supplies and the parts the water
    /// pass draws.
    pub water: Option<Arc<crate::native_water_glue::WaterData>>,
    /// What the native lights draw over this world
    /// (`native_lights_glue`; `None` when the world has no native
    /// material).
    pub lights: Option<Arc<crate::native_lights_glue::LightsWorld>>,
    /// The device palettes the load expanded palettized stage bitmaps
    /// through, at the stages it used them (the rest `None`): each ask
    /// checks they are still bound.
    pub palettes: StagePalettes,
}

/// The active BSP. The index and activation distinguish reused addresses
/// within a map and a reload of the same map.
#[derive(Clone, PartialEq, Eq)]
pub struct BspKey {
    map: String,
    bsp: u32,
    index: u16,
    generation: u64,
}

pub type State = Arc<Mutex<(Option<BspKey>, Load)>>;

/// Match the guest's active BSP even if it changed before the next layer 3.
pub fn current_ready<'a>(state: &'a (Option<BspKey>, Load), mem: &dyn GuestMemory) -> Option<&'a Arc<Ready>> {
    let (Some(key), Load::Ready(ready)) = state else { return None };
    (key.generation == native_map_work::generation()
        && mem.u32(render_bridge::native_world::ACTIVE_BSP).ok() == Some(key.bsp)
        && active_bsp_index(mem) == Some(key.index)
        && crate::health::loaded_map_name(mem).ok().as_ref() == Some(&key.map))
        .then_some(ready)
}

pub enum Load {
    Idle,
    Loading(Arc<()>),
    Ready(Arc<Ready>),
    Failed,
}

/// The host: the device and the current world.
pub struct Glue<B: d3d::backend::Backend + Send + 'static> {
    watch: runtime::WatchId,
    device: d3d::abi::SharedDevice<B>,
    state: State,
    switches: hooks::Switches,
    ordered_scene_world: bool,
}

impl<B: d3d::backend::Backend + Send + 'static> Host for Glue<B> {
    fn try_opaque_simple(&self, mem: &mut dyn GuestMemory, header: u32, value: u32) -> bool {
        let _phase = timeline::phase::enter(timeline::phase::Phase::D3d);
        self.device.lock().is_ok_and(|mut d| {
            drain_opaque_writes(mem, &mut d, self.watch).is_ok()
                && d.try_opaque_simple(&crate::native_lights_glue::DeviceMemory(mem), header, value)
        })
    }

    fn load_bsp(&self, mem: &dyn GuestMemory, bsp: u32) -> bool {
        let generation = native_map_work::generation();
        let Ok(map) = crate::health::loaded_map_name(mem) else { return false };
        let Some(index) = active_bsp_index(mem) else { return false };
        if map.is_empty() {
            return false;
        }
        if !render_bridge::native_world::on(&self.switches) {
            return true;
        }
        let key = BspKey { map, bsp, index, generation };
        let job = Arc::new(());
        {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if st.0.as_ref() == Some(&key) {
                return true;
            }
            *st = (Some(key.clone()), Load::Loading(Arc::clone(&job)));
        }
        // The palettes bound now, for the stage bitmaps that are
        // palettized (P8, P8 bump): the D3D path samples them through
        // the device's palette at their stage, which the load expands
        // them through. Read with no other lock held.
        let palettes = self.device.lock().ok().map(|d| scene_load_palettes(&d, mem)).unwrap_or_default();
        let state = Arc::clone(&self.state);
        let started = std::thread::Builder::new()
            .name("native-world load".into())
            .spawn(move || {
                let t0 = std::time::Instant::now();
                let out = load_ready(&key.map, key.bsp, key.index, palettes);
                let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
                if !matches!(&st.1, Load::Loading(current) if Arc::ptr_eq(current, &job))
                    || key.generation != native_map_work::generation()
                {
                    return;
                }
                st.1 = match out {
                    Ok((r, bsp_count)) => {
                        {
                            // One guard, dropped before the print: two
                            // `lock()`s in the macro's arguments both live
                            // to the statement's end, and the second
                            // waited on the first forever (with `state`
                            // held, so the render thread's next ask hung).
                            let (draws, batches) = {
                            let cpu = r.cpu.lock().unwrap_or_else(|e| e.into_inner());
                            cpu.as_ref().map_or((0, 0), |c| (c.world.draws.len(), c.world.batches.len()))
                        };
                        eprintln!(
                            "native-world: {} BSP {} at {:#010x}: {} of {} materials native; {} draws in {} batches, {} parts, in {:.1} s; {} BSP material slots, {} geometry sections skipped; top material rejections: {:?}",
                            key.map,
                            key.index,
                            key.bsp,
                            r.coverage.0,
                            r.coverage.1,
                            draws,
                            batches,
                            r.parts.len(),
                            t0.elapsed().as_secs_f32(),
                            r.declared_materials,
                            r.failed_sections,
                            r.rejection_summary,
                        );
                        }
                        let empty = [
                            (r.parts.is_empty() && r.opaque.is_empty()).then_some(MapStage::World),
                            r.lights.is_none().then_some(MapStage::Lights),
                            r.fog.as_ref().filter(|fog| fog.part_set.is_empty()).map(|_| MapStage::Fog),
                            r.water.as_ref().filter(|water| water.part_set.is_empty()).map(|_| MapStage::Water),
                        ];
                        let index = usize::from(key.index);
                        native_map_work::bsp_loaded(key.generation, &key.map, index, bsp_count, empty);
                        Load::Ready(Arc::new(r))
                    }
                    Err(e) => {
                        eprintln!("native-world: {} BSP {} at {:#010x}: not loaded ({e}); the D3D path draws it", key.map, key.index, key.bsp);
                        Load::Failed
                    }
                };
            });
        if started.is_err() {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            *st = (None, Load::Idle);
            return false;
        }
        true
    }

    fn use_resolved_scene(&self, _mem: &dyn GuestMemory, _bsp: u32) -> bool {
        self.ordered_scene_world
    }

    fn opaque_view(
        &self, mem: &dyn GuestMemory, layer: render_bridge::native_world_layers::Layer,
    ) -> Result<Box<dyn render_bridge::native_world::OpaqueView>, &'static str> {
        if !self.ordered_scene_world {
            return Err("world programs disabled");
        }
        let state = self.state.lock().map_err(|_| "BSP loader unavailable")?;
        let ready = Arc::clone(current_ready(&state, mem).ok_or("BSP load pending")?);
        let key = state.0.as_ref().ok_or("BSP load pending")?;
        Ok(Box::new(OpaqueView {
            watch: self.watch,
            device: Arc::clone(&self.device),
            ready,
            generation: key.generation,
            bsp_index: key.index,
            layer,
            frame: crate::d3d_diag::presents() + 1,
        }))
    }

    fn ask(&self, mem: &mut dyn GuestMemory, bsp: u32, scene_view: &frame_scene::View, plan: &StencilPlan) -> Option<native_render::skip::Cleared> {
        let camera = &scene_view.camera;
        let ready = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            Arc::clone(current_ready(&state, mem)?)
        };
        let mut d = self.device.lock().ok()?;
        if !d.can_external_pass() || !d.begin_external_boundary(&mut crate::native_lights_glue::DeviceMemory(mem)).ok()? {
            return None;
        }
        // With a render thread that records scene stages, everything here
        // is extraction: the guest reads and the checks that can refuse,
        // which decide what the guest skips. The world's own preparation
        // (upload, part slots, pipelines, packets) runs on the render
        // thread (docs/graphics/framescene.md, slice 2).
        let scene = d.queues_scene_stages(timeline::native::Stage::World);
        let metric = if scene { timeline::native::Metric::ExtractNs } else { timeline::native::Metric::PrepareNs };
        let planning = timeline::native::Timer::start(timeline::native::Stage::World, metric);
        // The programs write w as depth and clip w (`xps_w_depth`): a view
        // whose draws store z instead (`SET_CONTROL0` without
        // z-perspective) is the D3D path's.
        if d.hw().control0.is_some_and(|c| c & d3d::nv2a::CONTROL0_Z_PERSPECTIVE == 0) {
            let n = NOT_W_BUFFERED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if n % 600 == 0 {
                eprintln!("native-world: {} views left to the D3D path: not W-buffered (SET_CONTROL0 without z-perspective)", n + 1);
            }
            return None;
        }
        // The palettes the load expanded its palettized stage bitmaps
        // through must still be the ones bound: otherwise the D3D path
        // draws the world (its samples would differ).
        if !palettes_hold(&ready.palettes, &|s| d.stage_palette(&ReadMemory(mem), s)) {
            if !PALETTE_CHANGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                eprintln!("native-world: a stage palette changed since the load expanded its bitmaps through it; the D3D path draws the world");
            }
            return None;
        }
        // The stencil the layer's draws take now, and how its items switch
        // it (`skip::stencil_plan`).
        let Some(stencil) = d.stencil_state(&ReadMemory(mem)) else {
            if !STENCIL_UNREAD.swap(true, std::sync::atomic::Ordering::Relaxed) {
                eprintln!("native-world: the device's stencil state does not read; the D3D path draws the world");
            }
            return None;
        };
        let drained_plan = native_render::skip::stencil_plan(&render_bridge::native_world::Mem(mem), native_render::skip::LIGHTMAP_LAYER)?;
        if &drained_plan != plan { return None; }
        let Some(stencil) = pass_stencil(&stencil, plan) else {
            let n = STENCIL_REFUSED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if n % 600 == 0 {
                eprintln!(
                    "native-world: {} views left to the D3D path: an item switches the stencil and layer 3 starts in another state ({stencil:?})",
                    n + 1
                );
            }
            return None;
        };
        // The cull layer 3 starts in: what the passes that do not set
        // `CULLMODE` inherit (`native_render::look::WorldPass::inherits_cull`).
        let cull = d.cull_state(&ReadMemory(mem)).map(inherited_cull);
        let frustum = GpuWorld::draw_frustum(&camera.view_projection(), true);
        let (masks, cleared, selected) = render_bridge::native_world::plan_parts(
            &render_bridge::native_world::Mem(mem), &ready.parts, |owner, part| {
                ready.part_bounds.get(&(owner, part)).is_some_and(|b| frustum.may_see(b))
            },
        )?;
        let ranges = selected.len();
        if ranges == 0 { return None; }
        let view = mem.u32(engine::render_loop::occlusion::guest::VIEW_INDEX).unwrap_or(u32::MAX);
        let plan = d3d::ExternalPlan { stage: timeline::native::Stage::World, view, ranges, slots: Vec::new() };
        let counted = timeline::phase::enabled().then(|| selected.iter().map(|p| p.0).collect::<Vec<_>>());
        let run = u64::from(d.swap_count()).wrapping_add(1);
        let pass = |selected: Vec<(Owner, u8)>| Box::new(WorldPass {
            selected,
            selection: None,
            ready: Arc::clone(&ready),
            camera: *camera,
            view: None,
            w_clip: None,
            stencil: stencil.clone(),
            cull,
            run,
        });
        drop(planning);
        // A scene stage: queued now, prepared and recorded when the render
        // thread reaches it. Not taken, the pass is reserved here as before.
        let staged = if scene {
            let reserved = d.reserve_scene_stage(&mut crate::native_lights_glue::DeviceMemory(mem), plan.clone(), pass(selected.clone()));
            match reserved {
                Ok(Some(reservation)) => {
                    if !masks.apply(&mut render_bridge::native_world::Mem(mem)) {
                        timeline::native::Stage::World.count(timeline::native::Metric::RefusedMasks, ranges as u64);
                        return None;
                    }
                    reservation.commit();
                    true
                }
                Ok(None) => false,
                Err(_) => return None,
            }
        } else {
            false
        };
        if !staged {
            let reservation = d.prepare_external_bound(&mut crate::native_lights_glue::DeviceMemory(mem), plan, pass(selected), Vec::new()).ok()??;
            if !masks.apply(&mut render_bridge::native_world::Mem(mem)) {
                timeline::native::Stage::World.count(timeline::native::Metric::RefusedMasks, ranges as u64);
                return None;
            }
            reservation.commit();
        }
        if let Some(owners) = counted {
            crate::native_world_layers_glue::count_frontend(crate::d3d_diag::presents() + 1, view, bsp,
                timeline::native::WorldOpaque::Prereserved, owners);
        }
        Some(cleared)
    }
}

/// The ordinary ABI takes dirty pages on entry. The sorted handler bypasses
/// that wrapper on both resource preparation and submission. A shader call
/// in between can stream a new bitmap or rewrite a vertex/index buffer.
fn drain_opaque_writes<B: d3d::Backend>(
    mem: &mut dyn GuestMemory, device: &mut d3d::Device<B>, watch: runtime::WatchId,
) -> Result<(), &'static str> {
    for range in mem.take_write_watch(watch).map_err(|_| "render memory missing D3D write watch")? {
        device.note_written_pages(range.addr, range.len);
    }
    Ok(())
}

struct OpaqueView<B: d3d::Backend> {
    watch: runtime::WatchId,
    device: d3d::abi::SharedDevice<B>,
    ready: Arc<Ready>,
    generation: u64,
    bsp_index: u16,
    layer: render_bridge::native_world_layers::Layer,
    frame: u64,
}

impl<B: d3d::Backend + Send + 'static> render_bridge::native_world::OpaqueView for OpaqueView<B> {
    fn prepare_opaque(
        &self, mem: &mut dyn GuestMemory, part: &native_render::world_ranges::SortedPart,
    ) -> Result<Box<dyn render_bridge::native_world::OpaqueDraw>, &'static str> {
        if native_map_work::generation() != self.generation || active_bsp_index(mem) != Some(self.bsp_index) {
            return Err("BSP changed during sorted pass");
        }
        let material = self.ready.opaque.get(part)?;
        let layer = self.layer;
        let shader = engine::tags::tag_entry_checked(mem, engine::handle::DatumHandle(part.shader))
            .ok().flatten().and_then(|entry| mem.u32(entry.checked_add(8)?).ok())
            .filter(|&data| data != 0).ok_or("shader tag entry")?;
        let flags = shader.checked_add(0x16).and_then(|at| mem.u16(at).ok()).ok_or("shader flags")?;
        let streams = crate::opaque_streams::Plan::read(mem, part.section, part.geometry,
            part.part_addr, part.program, part.record_addr, part.layer, flags)?;
        let _phase = timeline::phase::enter(timeline::phase::Phase::D3d);
        let mut device = self.device.lock().map_err(|_| "device unavailable")?;
        drain_opaque_writes(mem, &mut device, self.watch)?;
        let accumulation = if layer.number == 3 && material.samples_texaccum() {
            let t = layer.texaccum.ok_or("accumulation level unavailable")?;
            Some((t.surface.ok_or("accumulation surface unavailable")?, t.texture, t.size))
        } else { None };
        let targets = device.prepare_opaque_targets(&ReadMemory(mem), accumulation)
            .ok_or("view attachments unavailable")?;
        if layer.number == 1 && layer.texaccum.and_then(|t| t.surface) != Some(targets.bound[0]) {
            return Err("layer 1 accumulation target");
        }
        let mut ranges = Vec::with_capacity(part.indices.len());
        for r in &part.indices {
            let addr = r.start.checked_mul(2).and_then(|n| part.index_buffer.checked_add(n))
                .ok_or("world index offset")?;
            ranges.push(device.prepare_index_range(&ReadMemory(mem), addr, r.end - r.start)
                .map_err(|_| "world index range")?);
        }
        let owner = match part.owner { Owner::Cluster(n) => n, Owner::Instance(n) => n | (1 << 31) };
        let vertex_key = [part.part_addr, part.program, owner];
        let vertices = device.prepare_section_vertices(&ReadMemory(mem), vertex_key, &ranges, || streams.vertex_inputs(mem))
            .map_err(|_| "world vertex windows")?;
        let textures = material.textures(mem, &mut device, streams.lightmap());
        Ok(Box::new(OpaquePass { watch: self.watch, device: Arc::clone(&self.device), streams, vertices,
            ranges, code: material.code.clone(), textures, layer, targets,
            frame: self.frame }))
    }
}

struct OpaquePass<B: d3d::Backend> {
    watch: runtime::WatchId,
    device: d3d::abi::SharedDevice<B>,
    streams: crate::opaque_streams::Plan,
    vertices: d3d::device::PreparedWorldVertices,
    ranges: Vec<d3d::device::PreparedIndexRange>,
    code: Option<d3d::shader::Microcode>,
    textures: [Option<d3d::device::PreparedStageTexture>; 4],
    layer: render_bridge::native_world_layers::Layer,
    targets: d3d::device::PreparedOpaqueTargets,
    frame: u64,
}

impl<B: d3d::Backend + 'static> render_bridge::native_world::OpaqueDraw for OpaquePass<B> {
    fn bind_streams<'a>(&'a self, cx: &'a hooks::Cx) -> render_bridge::native_world::OpaqueStreams<'a> {
        Box::pin(self.streams.handoff(cx))
    }

    fn submit(self: Box<Self>, mem: &mut dyn GuestMemory, part: &native_render::world_ranges::SortedPart)
        -> Result<(), render_bridge::native_world::OpaqueSubmitError>
    {
        use render_bridge::native_world::OpaqueSubmitError::{BeforeDraw, Device};
        let Self { watch, device, streams: _, vertices, ranges, code, textures, layer, targets, frame } = *self;
        let _phase = timeline::phase::enter(timeline::phase::Phase::D3d);
        let mut device = device.lock().map_err(|_| Device("opaque device mutex poisoned".into()))?;
        drain_opaque_writes(mem, &mut device, watch).map_err(BeforeDraw)?;
        let primitive = if part.strip { gfx::vertex::PrimitiveType::TriangleStrip }
            else { gfx::vertex::PrimitiveType::TriangleList };
        let sent = device.emit_world_ranges(&mut crate::native_lights_glue::DeviceMemory(mem),
            primitive, ranges, vertices, code.as_ref(), &targets, &textures, part.shader,
            |count| crate::native_world_layers_glue::enter_opaque_range(part, layer, frame, count))
            .map_err(|e| Device(e.to_string()))?;
        if sent { Ok(()) } else { Err(BeforeDraw("view changed, callback or capture boundary")) }
    }
}

/// Installs the world host and the other native stages on the device.
pub fn install<B: d3d::backend::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>, watch: runtime::WatchId) -> Option<Arc<dyn d3d::DrawAdmission>> {
    let state: State = Arc::new(Mutex::new((None, Load::Idle)));
    let switches = hooks::Switches::from_env().unwrap_or_default();
    render_bridge::native_world::set_host(Arc::new(Glue {
        watch,
        device: Arc::clone(device),
        state: Arc::clone(&state),
        ordered_scene_world: crate::native_world_layers_glue::owns_scene_world(&switches),
        switches,
    }));
    crate::native_transparents_glue::install(device);
    crate::native_fog_glue::install(device, Arc::clone(&state));
    crate::native_water_glue::install(device, Arc::clone(&state));
    let gels = crate::native_gels_glue::install(device);
    crate::native_lights_glue::install(device, state, gels);
    // The post loads its own screen effects (`native_post_glue::EffectsSlot`).
    crate::native_post_glue::install(device);
    let shadows = crate::native_shadows_glue::install(device);
    crate::native_hud_glue::install(device);
    shadows
}

/// The world's GPU side, uploaded from its CPU side by the first pass that
/// needs it (on the render thread). `None` inside when it was taken and
/// is gone.
pub fn upload<'a>(ready: &'a Ready, t: &d3d::ExternalTarget<'_>) -> std::sync::MutexGuard<'a, Option<GpuWorld>> {
    upload_on(ready, t.device, t.queue)
}

/// [`upload`] on pinned attachments' device and queue (packet passes,
/// `d3d::ExternalPass::record_draws`).
pub fn upload_on<'a>(ready: &'a Ready, device: &wgpu::Device, queue: &wgpu::Queue) -> std::sync::MutexGuard<'a, Option<GpuWorld>> {
    let mut gpu = ready.gpu.lock().unwrap_or_else(|e| e.into_inner());
    if gpu.is_some() {
        return gpu;
    }
    let Some(cpu) = ready.cpu.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return gpu;
    };
    let CpuWorld {
        world,
        lightmaps,
        programs,
        stage_images,
        layered_images,
    } = cpu;
    let mut stage = |s: Stage| match s {
        Stage::Bitmap { bitmap, image } => stage_images.get(&(bitmap, image)).cloned(),
        Stage::Empty | Stage::Lightmap | Stage::Engine(_) => None,
    };
    // Set even when empty: a batch without a program is never drawn with
    // stage 1's.
    let set = ProgramSet::with_layered(device, queue, &world, programs, &mut stage,
        &mut |k| lightmaps.get(&k).cloned(), &layered_images);
    eprintln!(
        "native-world: {} materials draw with their own programs, {} of them their placements with their rigid programs",
        set.len(),
        set.rigid_len()
    );
    let mut g = GpuWorld::with_programs(device, queue, world, set);
    // The fog set rides the same upload (`native_fog_glue`), after the
    // world's images are in.
    if let Some(fog) = ready.fog.as_ref().filter(|f| !f.programs.is_empty()) {
        crate::native_fog_glue::upload_fog(fog, device, queue, &mut g, &mut stage, &mut |k| lightmaps.get(&k).cloned());
    }
    // The water set rides the same upload (`native_water_glue`).
    if let Some(water) = ready.water.as_ref().filter(|w| !w.programs.is_empty()) {
        crate::native_water_glue::upload_water(water, device, queue, &mut g, &mut stage, &mut |k| lightmaps.get(&k).cloned());
    }
    *gpu = Some(g);
    gpu
}

/// One view's world pass.
struct WorldPass {
    selected: Vec<(Owner, u8)>,
    selection: Option<WorldSelection>,
    ready: Arc<Ready>,
    camera: RenderCamera,
    /// The device's view constants at the pass (material programs).
    view: Option<[[f32; 4]; d3d::VIEW_CONSTANTS]>,
    /// The device's W-buffer clip at the pass (`d3d::DepthClip`): the range
    /// and clamp the D3D path draws the layer with. Without it the
    /// programs culled what lies past `z_max`, where a cinematic's range
    /// reaches further and clamps (03a's intro: distant buildings left as
    /// sky-coloured slivers).
    w_clip: Option<([f32; 2], bool)>,
    /// The stencil the pass draws with ([`pass_stencil`]).
    stencil: (gfx::state::StencilState, Vec<Owner>),
    /// The device's cull at the layer-3 call ([`inherited_cull`]; `None`:
    /// it did not read).
    cull: Option<native_render::program::Cull>,
    /// The swap's run, shared with the object runs: adjacent reservations
    /// reuse one pin of the attachments (`d3d::ExternalPass::batch_key`).
    run: u64,
}

/// `CULLMODE` and `FRONTFACE` as the D3D path's pipeline takes them
/// (`gfx::translate::primitive_state`).
pub fn inherited_cull((cull, front): (gfx::state::CullMode, gfx::state::FrontFace)) -> native_render::program::Cull {
    let rs = gfx::state::RenderState {
        cull_mode: cull,
        front_face: front,
        ..Default::default()
    };
    let p = gfx::translate::primitive_state(
        &rs,
        wgpu::PrimitiveTopology::TriangleList,
        gfx::translate::Capabilities::from_features(wgpu::Features::empty()),
    );
    (p.cull_mode, p.front_face)
}

impl WorldPass {
    fn configure(&self, gpu: &mut GpuWorld, depth_format: wgpu::TextureFormat) {
        gpu.set_inherited_cull(native_render::program::InheritedCull {
            device: self.cull,
            placements: true,
            clusters: true,
        });
        let (state, marked) = &self.stencil;
        let mut stencil = PassStencil::from_device(state, depth_format);
        if stencil != PassStencil::default() {
            stencil.marked = marked.clone();
        }
        gpu.set_stencil(stencil);
    }
}

impl d3d::ExternalPass for WorldPass {
    /// Packets need no worker round trip; the pin holds while no other
    /// command intervenes (`d3d::ThreadedBackend`).
    fn batch_key(&self) -> Option<u64> { Some(self.run) }

    fn view_constants(&mut self, constants: &[[f32; 4]; d3d::VIEW_CONSTANTS]) {
        self.view = Some(*constants);
    }

    fn depth_clip(&mut self, clip: d3d::DepthClip) {
        self.w_clip = Some((clip.range, clip.clamp));
    }

    fn prepare_copy(&mut self, t: &d3d::ExternalTarget<'_>) -> Result<(), &'static str> {
        let mut guard = upload(&self.ready, t);
        let gpu = guard.as_mut().ok_or("world geometry unavailable")?;
        if gpu.has_programs() && self.view.is_none() { return Err("world view constants unavailable"); }
        self.configure(gpu, t.depth_format);
        if self.selection.is_none() {
            self.selection = gpu.select_parts(&self.selected, &self.camera);
        }
        let selection = self.selection.as_ref().ok_or("world part unavailable")?;
        gpu.prepare_selection(t.device, t.color_format, t.depth_format, selection)
            .then_some(()).ok_or("world part or program binding unavailable")
    }

    fn writes(&self) -> d3d::ExternalWrites {
        d3d::ExternalWrites { color: true, depth_stencil: true }
    }

    /// Material programs' parts join the D3D render pass already open on
    /// these attachments, their view rows written ahead of them
    /// (`ExternalDraws::writes`): no pass, command buffer or worker round
    /// trip of their own. The fixed stage records on its own encoder (`None`).
    fn record_draws(
        &mut self, t: &d3d::backend::ExternalAttachments, request: d3d::ExternalPassRequest,
    ) -> Option<Result<d3d::backend::ExternalDraws, &'static str>> {
        let mut guard = upload_on(&self.ready, &t.device, &t.queue);
        let Some(gpu) = guard.as_mut() else { return Some(Err("world geometry unavailable")) };
        if !gpu.has_programs() { return None; }
        let Some(constants) = self.view else { return Some(Err("world view constants unavailable")) };
        self.configure(gpu, t.depth_format);
        if self.selection.is_none() {
            self.selection = gpu.select_parts(&self.selected, &self.camera);
        }
        let Some(selection) = self.selection.as_ref() else { return Some(Err("world part unavailable")) };
        if !gpu.prepare_selection(&t.device, t.color_format, t.depth_format, selection) {
            return Some(Err("world part or program binding unavailable"));
        }
        let view = ViewInputs {
            constants, guest_size: request.surface_size, size: t.size, z_scale: request.z_scale, w_clip: self.w_clip,
        };
        Some(gpu.selection_draws(t.color_format, t.depth_format, t.size, selection, &view))
    }

    fn encode(&mut self, t: d3d::ExternalTarget<'_>) { self.record_ranges(t); }

    fn record_ranges(&mut self, mut t: d3d::ExternalTarget<'_>) -> usize {
        let mut gpu = upload(&self.ready, &t);
        let Some(gpu) = gpu.as_mut() else { return 0 };
        self.configure(gpu, t.depth_format);
        let Some(selection) = &self.selection else { return 0 };
        let view = self.view.map(|constants| ViewInputs {
            constants, guest_size: t.guest_size, size: t.size, z_scale: t.z_scale, w_clip: self.w_clip,
        });
        let depth = DepthRule { full_scale: t.z_scale, floor: true };
        if !gpu.write_selection(&mut t.encoder_uploads(), selection, view.as_ref(), &self.camera, depth) {
            return 0;
        }
        let [x, y, width, height] = t.viewport;
        let recorded = gpu.encode_selection(
            t.encoder,
            t.color,
            t.color_format,
            t.depth,
            t.depth_format,
            PassViewport { x, y, width, height },
            t.size,
            selection,
        );
        let unbound = gpu.unbound_batches();
        if unbound > 0 && !UNBOUND.swap(true, std::sync::atomic::Ordering::Relaxed) {
            eprintln!("native-world: {unbound} batch draws left out: their material program could not be bound");
        }
        recorded
    }
}

static NOT_W_BUFFERED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Said once when a material program could not be bound.
static UNBOUND: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static PALETTE_CHANGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static STENCIL_UNREAD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static STENCIL_REFUSED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The stencil the native pass draws layer 3 with, from the device's state
/// as the layer starts (`now`) and how its items switch it (`plan`,
/// `0x0004d640`/`0x0004d720`): the state and the sections drawn with mode
/// 2's reference; `None` when that cannot be exact.
///
/// The native pass draws its parts before the layer's items and clears
/// them, so their switches do not run. Without any switch, every item
/// draws with `now`. With one, an item leaves mode 1, so `now` must be
/// mode 1 for the items left to D3D to see what they would have: then the
/// flags' mode 0 is the stencil off for every item, and otherwise a marked
/// section draws with mode 1's state and reference `0xff` (mode 2), the
/// rest with mode 1. Each part writes what its own item would have where
/// it is drawn (`ALWAYS`; the depth test decides which part is last).
fn pass_stencil(now: &gfx::state::StencilState, plan: &StencilPlan) -> Option<(gfx::state::StencilState, Vec<Owner>)> {
    if !plan.switches {
        return Some((*now, Vec::new()));
    }
    if !PassStencil::is_structure_mode(now) {
        return None;
    }
    if plan.off {
        return Some((gfx::state::StencilState { enable: false, ..*now }, Vec::new()));
    }
    Some((*now, plan.marked.clone()))
}

/// The device palette at each texture stage (`A8R8G8B8` entries), or
/// `None`: what a palettized stage bitmap (P8, P8 bump) is sampled through
/// on the D3D path. A world keeps those of the stages it expanded
/// bitmaps through.
pub type StagePalettes = [Option<Vec<u32>>; 4];

/// The palettes bound at stages 0..3 now.
pub fn stage_palettes<B: d3d::backend::Backend>(d: &d3d::Device<B>, mem: &dyn GuestMemory) -> StagePalettes {
    std::array::from_fn(|s| d.stage_palette(&ReadMemory(mem), s))
}

/// Guest pointer to the palette header read by the scene's texture reset (0x00015b70).
const DEFAULT_SCENE_PALETTE: u32 = 0x0048_4dbc;

/// A loading/UI boundary may not have the scene's palette bound yet.
/// The scene reset binds this palette; each native draw still checks its bindings.
pub fn scene_load_palettes<B: d3d::backend::Backend>(d: &d3d::Device<B>, mem: &dyn GuestMemory) -> StagePalettes {
    let mut bound = stage_palettes(d, mem);
    supply_scene_palette(&mut bound, || d.palette_entries(&ReadMemory(mem), mem.u32(DEFAULT_SCENE_PALETTE).ok()?));
    bound
}

fn supply_scene_palette(bound: &mut StagePalettes, default: impl FnOnce() -> Option<Vec<u32>>) {
    let scene_stages = &mut bound[..3];
    let incomplete = |p: &Option<Vec<u32>>| p.as_ref().is_none_or(|p| p.len() != 256);
    if scene_stages.iter().any(incomplete)
        && let Some(default) = default().filter(|p| p.len() == 256)
    {
        for palette in scene_stages.iter_mut().filter(|p| incomplete(p)) {
            *palette = Some(default.clone());
        }
    }
}

/// Whether each palette a world expanded through is still the one bound
/// at its stage (`bound`).
pub fn palettes_hold(used: &StagePalettes, bound: &dyn Fn(usize) -> Option<Vec<u32>>) -> bool {
    used.iter().enumerate().all(|(s, p)| p.is_none() || bound(s) == *p)
}

/// The hook's guest memory as the device reads it, read-only.
pub struct ReadMemory<'a>(pub &'a dyn GuestMemory);

impl native_render::camera::GuestRead for ReadMemory<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> { self.0.u32(addr).ok() }
}

impl d3d::GuestMemory for ReadMemory<'_> {
    fn read(&self, addr: u32, out: &mut [u8]) -> Result<(), d3d::GuestError> {
        self.0.read(addr, out).map_err(|f| d3d::GuestError { addr: f.addr, len: f.len })
    }
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), d3d::GuestError> {
        Err(d3d::GuestError {
            addr,
            len: data.len() as u32,
        })
    }
}

/// Raw resource reads over the map and its shared maps.
pub struct Raw(pub assets::ResourceFiles);

impl tags::geometry::RawSource for Raw {
    fn read_raw(&mut self, pointer: map::RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        self.0
            .read(pointer.file, pointer.offset, size)
            .map_err(|_| tags::Error::MissingResource(pointer.file))
    }
}

/// Loads the world of map `name`'s BSP whose tag data sits at `bsp`.
/// The active structure BSP's index in the scenario's BSP block: the
/// game globals' (`engine::points::GAME_GLOBALS`) `i16` at `+2`, as
/// `0x0011c1b0` and `0x00137d40` index the block with it
/// (`live_telemetry::layout::GLOBALS_ACTIVE_BSP`). `None` when negative
/// or unread.
pub fn active_bsp_index(mem: &dyn GuestMemory) -> Option<u16> {
    let globals = mem.u32(engine::points::GAME_GLOBALS.base).ok().filter(|&g| g != 0)?;
    u16::try_from(mem.u16(globals + 2).ok()? as i16).ok()
}

/// Which of the scenario's BSPs (`addresses`: each one's load address, in
/// block order) is the active one: the game's index, when that BSP loads
/// at the active BSP's address. Not the first BSP at that address: a
/// level's BSPs share it.
fn active_scenario_bsp(addresses: &[Option<u32>], index: u16, bsp: u32) -> Option<usize> {
    let i = usize::from(index);
    (addresses.get(i).copied().flatten() == Some(bsp)).then_some(i)
}

fn load_ready(name: &str, bsp: u32, active: u16, palettes: StagePalettes) -> Result<(Ready, usize), load::Error> {
    let (mut map, files) = crate::native_map::open(name)?;
    let bsps = load::scenario_bsps(&mut map)?;
    let addresses: Vec<Option<u32>> = bsps
        .iter()
        .map(|(id, _)| map.tag(*id).ok().and_then(|t| t.location).map(|l| l.address))
        .collect();
    let index = active_scenario_bsp(&addresses, active, bsp).ok_or("the active BSP index is not a scenario BSP at the active BSP's address")?;
    let mut raw = Raw(files);
    let LoadedWorld {
        world,
        shaders,
        lightmap_bitmap,
        lightmap_error,
        palettes: lightmap_palettes,
        failed_sections,
        ..
    } = load::load_world(&mut map, &mut raw, index)?;
    let mut files = raw.0;

    // Lightmap pages through their palettes, with their mip chains.
    let mut lightmaps = HashMap::new();
    let mut rejected_pages = HashMap::new();
    let page_keys = world.lightmap_keys();
    if !page_keys.is_empty() {
        let bitmap = lightmap_bitmap
            .ok_or_else(|| "the BSP has no matching lightmap bitmap".to_owned())
            .and_then(|id| {
                let tag = tags::DecodedTag::read(&mut map, id)
                    .map_err(|e| format!("the BSP lightmap bitmap does not read ({e})"))?;
                match tag.data {
                    tags::TagData::Bitmap(bitmap) => Ok(bitmap),
                    _ => Err("the BSP lightmap tag is not a bitmap".into()),
                }
            });
        for key in page_keys {
            let page = bitmap.as_ref().map_err(Clone::clone).and_then(|b| {
                let image = b.images.get(usize::from(key.page)).ok_or("no such lightmap bitmap page")?;
                let palette = lightmap_palettes.get(usize::from(key.palette)).ok_or("no such lightmap palette")?;
                decode_lightmap(image, palette, &mut files, true)
            });
            match page {
                Ok(page) => {
                    lightmaps.insert(key, page);
                }
                Err(why) => {
                    rejected_pages.insert(key, why);
                }
            }
        }
    }
    let page_ready = |key| {
        if lightmaps.contains_key(&key) {
            Ok(())
        } else {
            let why = rejected_pages.get(&key).map_or("page unavailable", String::as_str);
            Err(format!("a part's lightmap page does not decode ({why})"))
        }
    };
    // What draws natively: a part whose layer-3 program translates and
    // whose streams and images supply that program.
    let mut decoder = StageDecoder::new(palettes);
    let mut layered_images = LayeredImages::new();
    let out = match lightmap_error {
        Some(reason) => Programs {
            refused: BTreeMap::from([(format!("lightmap group does not decode ({reason})"), world.materials())]),
            ..Default::default()
        },
        None => material_programs(&mut map, &mut files, &shaders, &world, &page_ready, &mut decoder,
            lightmap_bitmap.is_none(), &mut layered_images),
    };
    let native: HashSet<u16> = out.programs.keys().copied().collect();
    let prepared_parts = out.parts;
    let palettized = out.palettized;
    let rigid_refused = out.rigid_refused;
    let (programs, mut stage_images, refused) = (out.programs, out.images, out.refused);
    let accepts = |d: &WorldDraw| prepared_parts.contains(&(d.owner, d.part));
    if hooks::native_verbose() {
        log_coverage(&mut map, &world, &shaders, &native, &refused, prepared_parts.len());
        let placed = world.draws.iter().filter(|d| native.contains(&d.material) && matches!(d.owner, Owner::Instance(_)));
        let (with, without) = placed.fold((0, 0), |(w, wo), d| if accepts(d) { (w + 1, wo) } else { (w, wo + 1) });
        eprintln!(
            "native-world: {with} instanced-geometry parts draw natively with their rigid programs (the placement's rows in c[50..52] per draw, as 0x0004d830 uploads them); D3D keeps {without} of native materials without one"
        );
        for (why, ms) in &rigid_refused {
            let parts = world.draws.iter().filter(|d| ms.contains(&d.material) && matches!(d.owner, Owner::Instance(_))).count();
            eprintln!("native-world: D3D keeps the placements of {} materials ({parts} parts): {why}", ms.len());
        }
        if !palettized.is_empty() {
            // The materials whose look depends on a device palette (the
            // bump maps' normals): the ones to compare first in an A/B.
            let parts = world.draws.iter().filter(|d| palettized.contains(&d.material)).count();
            eprintln!(
                "native-world: {} native materials ({parts} parts) sample a palettized stage bitmap (P8 bump or P8) through the device's stage palette: {:?}",
                palettized.len(),
                palettized
            );
        }
    }
    // The sorted geometry entries identify individual cluster parts and their full shader datums.
    let lights = if !native.is_empty() {
        crate::native_lights_glue::load(&mut map, &mut files, &shaders, &world, &accepts, &decoder.palettes)
            .map(Arc::new)
    } else {
        None
    };
    // The screen effects are not the world's: `native_post_glue` loads
    // them itself, with or without the native world.
    // The fog (layer 16), over the whole world before its draws are kept:
    // a material that stays on the D3D path for layer 3 may still fog
    // natively.
    let fog = Some(crate::native_fog_glue::load(&mut map, &mut files, &shaders, &world, &lightmaps, &mut stage_images, &mut decoder));
    // The water edge blend (layer 6), over the world's transparent parts
    // before its draws are kept.
    let water = Some(crate::native_water_glue::load(&mut map, &mut files, &shaders, &world, &lightmaps, &mut stage_images, &mut decoder));
    // Draw only a native material's clusters, and its placements with its
    // rigid program: the rest keeps its D3D draws.
    let opaque = opaque_materials::Library::load(&mut map, &shaders);
    let coverage = (native.len(), world.materials().len());
    let world = keep_draws(world, &accepts);
    let parts = part_set(&world, &shaders);
    let part_bounds = world.draws.iter().map(|d| ((d.owner, d.part), d.bounds)).collect();
    Ok((Ready {
        opaque, part_bounds, coverage,
        rejection_summary: top_rejections(&refused),
        failed_sections,
        declared_materials: shaders.len(),
        cpu: Mutex::new(Some(CpuWorld {
            world,
            lightmaps,
            programs,
            stage_images,
            layered_images,
        })),
        gpu: Mutex::new(None),
        parts: Arc::new(parts),
        fog,
        water,
        lights,
        palettes: decoder.used().clone(),
    }, bsps.len()))
}

pub fn decode_lightmap(
    image: &tags::Image,
    palette: &[u32; 256],
    files: &mut assets::ResourceFiles,
    refuse_linear: bool,
) -> Result<Image, String> {
    if image.format != tags::PixelFormat::P8 {
        return Err(format!("unsupported lightmap format {:?}", image.format));
    }
    let decoded = assets::decode_image(image, files).map_err(|e| e.to_string())?;
    if refuse_linear && decoded.desc.format.layout() == gfx::format::Layout::Linear {
        return Err("linear lightmap coordinates are in texels".into());
    }
    expand_levels(decoded.levels, palette).ok_or_else(|| "no usable lightmap mip levels".into())
}

/// A P8 page's decoded levels (the index in every channel) through
/// `palette`, largest first, as one image with its mip chain.
fn expand_levels(levels: Vec<assets::Level>, palette: &[u32; 256]) -> Option<Image> {
    let mut levels: Vec<_> = levels.into_iter().filter(|l| l.face == 0 && l.depth <= 1).collect();
    levels.sort_by_key(|l| l.level);
    Image::from_levels(
        levels
            .into_iter()
            .map(|l| {
                // P8 decodes as L8: the index in every channel.
                let indices: Vec<u8> = l.rgba.chunks_exact(4).map(|p| p[0]).collect();
                (l.width, l.height, load::expand_page(&indices, palette))
            })
            .collect(),
    )
}

/// The skip's table: each part the world draws (by owner and part index),
/// with its material's full shader datum. `world` holds only native
/// materials' draws ([`keep_draws`]).
fn part_set(world: &World, shaders: &[TagId]) -> PartSet {
    world
        .draws
        .iter()
        .filter_map(|d| Some(((d.owner, d.part), shaders.get(usize::from(d.material))?.0)))
        .collect()
}

/// Materials left to the D3D path, by reason.
type Refused = BTreeMap<String, Vec<u16>>;

fn top_rejections(refused: &Refused) -> Vec<(String, usize)> {
    const LIMIT: usize = 5;
    let mut counts: Vec<_> = refused.iter().map(|(why, materials)| (why, materials.len())).collect();
    counts.sort_by(|(a, n), (b, m)| m.cmp(n).then_with(|| a.cmp(b)));
    counts.into_iter().take(LIMIT).map(|(why, count)| (why.clone(), count)).collect()
}

/// The coverage of one load on stderr: how many materials (and parts)
/// draw natively, then one line per reason the rest stay on the D3D path,
/// most parts first, with a material that has it.
fn log_coverage<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    world: &World,
    shaders: &[TagId],
    native: &HashSet<u16>,
    refused: &Refused,
    drawn_parts: usize,
) {
    let mut parts: HashMap<u16, usize> = HashMap::new();
    for d in &world.draws {
        *parts.entry(d.material).or_default() += 1;
    }
    let count = |ms: &[u16]| ms.iter().map(|m| parts.get(m).copied().unwrap_or(0)).sum::<usize>();
    let materials = world.materials();
    eprintln!(
        "native-world: {} of {} materials draw natively with their own programs ({} of {} parts); {} stay on the D3D path for {} reasons",
        native.len(),
        materials.len(),
        drawn_parts,
        world.draws.len(),
        materials.len().saturating_sub(native.len()),
        refused.len()
    );
    let mut lines: Vec<_> = refused.iter().map(|(why, ms)| (count(ms), why, ms)).collect();
    lines.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    for (n, why, ms) in lines {
        let example = ms
            .first()
            .and_then(|&m| shaders.get(usize::from(m)))
            .and_then(|&s| map.tag(s).ok().and_then(|t| t.name.clone()))
            .unwrap_or_default();
        eprintln!("native-world: D3D keeps {} materials ({n} parts): {why}; e.g. {example}", ms.len());
    }
}

/// The world with only the draws `keep` takes, batched again (the draws
/// stay in their order, so a batch is a run of one material and page).
pub fn keep_draws(mut world: World, keep: &dyn Fn(&WorldDraw) -> bool) -> World {
    world.draws.retain(|d| keep(d));
    world.batches.clear();
    for (i, d) in world.draws.iter_mut().enumerate() {
        match world.batches.last_mut() {
            Some(b) if b.material == d.material && b.lightmap == d.lightmap => b.draw_count += 1,
            _ => world.batches.push(native_render::world::Batch {
                material: d.material,
                lightmap: d.lightmap,
                first_draw: i as u32,
                draw_count: 1,
            }),
        }
        d.batch = world.batches.len() as u32 - 1;
    }
    world
}

/// Stage images by (bitmap, image index).
pub type StageImages = HashMap<(TagId, u32), Image>;

/// What [`material_programs`] gives.
#[derive(Default)]
struct Programs {
    parts: HashSet<(Owner, u8)>,
    programs: HashMap<u16, MaterialProgram>,
    images: StageImages,
    refused: Refused,
    /// Native materials whose placements stay on the D3D path, by reason
    /// (their rigid program does not translate or fit).
    rigid_refused: Refused,
    /// Materials with a palettized stage bitmap.
    palettized: Vec<u16>,
}

/// The world's layer-3 programs, the parts that supply their inputs
/// (`program::part_fit`), the images their stages sample, and the
/// others by the reason they stay on the D3D path. Stage images go
/// through `decoder` (a palettized one expanded through its stage's
/// palette, [`StageDecoder`]).
fn material_programs<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    files: &mut assets::ResourceFiles,
    shaders: &[TagId],
    world: &World,
    page_ready: &dyn Fn(LightmapKey) -> Result<(), String>,
    decoder: &mut StageDecoder,
    no_lightmap_group: bool,
    layered: &mut LayeredImages,
) -> Programs {
    let mut out = Programs::default();
    let Programs {
        parts,
        programs,
        images,
        refused,
        rigid_refused,
        palettized,
    } = &mut out;
    let placed: HashSet<u16> = world
        .draws
        .iter()
        .filter(|d| matches!(d.owner, Owner::Instance(_)))
        .map(|d| d.material)
        .collect();
    let materials = world.materials();
    let mut lib = match tags::material::ShaderLibrary::read(map) {
        Ok(l) => l,
        Err(e) => {
            refused.insert(format!("the shader tags do not read ({e})"), materials);
            return out;
        }
    };
    let normalization = map.tags().iter().find(|tag| tag.group.0 == *b"matg")
        .and_then(|tag| tag.location.as_ref())
        .and_then(|loc| crate::native_lights_glue::matg_light_bitmaps(lib.metadata(), loc.address).0);
    let mut dimensions = HashMap::new();
    for m in materials {
        let mut uses_palette = false;
        let Some(&shader) = shaders.get(usize::from(m)) else {
            refused.entry("no shader".into()).or_default().push(m);
            continue;
        };
        let pass = if no_lightmap_group {
            let selection = tags::material::PassSelection { lod: 0, layer: look::LIGHTMAP_LAYER, pass: 0,
                implementation: look::UNLIT_BSP_IMPLEMENTATION };
            lib.selected_implementation(shader, selection, None, None).map_err(|e| format!("material: {e}"))
                .and_then(|pass| look::unlit_bsp_pass(&pass, normalization).map_err(|r| format!("layer 3: {r:?}")))
        } else {
            lib.material(shader, None).map_err(|e| format!("material: {e}"))
                .and_then(|mat| look::layer3_pass(&mat).map_err(|r| format!("layer 3: {r:?}")))
        };
        let compiled = pass
            .and_then(|pass| {
                let code = look::world_program(map, lib.metadata(), pass.look.vertex_shader).ok_or("no world-space vertex program")?;
                let mut dims = [None; 4];
                for (slot, stage) in dims.iter_mut().zip(pass.look.textures) {
                    *slot = match stage {
                        Stage::Bitmap { bitmap, image } => Some(*dimensions.entry((bitmap, image))
                            .or_insert_with(|| bitmap_dimension(map, files, bitmap, image, layered))
                            .as_ref().map_err(Clone::clone)?),
                        Stage::Lightmap => Some(TextureDim::D2),
                        Stage::Empty => None,
                        Stage::Engine(id) => return Err(format!("unbound engine texture {id}")),
                    };
                }
                let mut p = program::compile_section(&pass, &code, dims)?;
                if placed.contains(&m) {
                    // Its placements: the pass with the rigid
                    // classification, or they stay on the D3D path.
                    let rigid = look::rigid_program(map, lib.metadata(), pass.look.vertex_shader)
                        .ok_or_else(|| "no rigid vertex program".to_string())
                        .and_then(|code| program::compile_section_rigid(&pass.look, pass.state.alpha_test, &code, dims));
                    match rigid {
                        Ok(r) => p.rigid = Some(r),
                        Err(why) => rigid_refused.entry(why).or_default().push(m),
                    }
                }
                Ok(p)
            })
            .and_then(|p| {
                for (n, s) in p.stages.iter().enumerate() {
                    let Stage::Bitmap { bitmap, image } = *s else { continue };
                    if p.dims[n].is_none() || layered.contains_key(&(bitmap, image)) {
                        continue;
                    }
                    // A linear texture is addressed in texels on the D3D
                    // path (`tex_scale`); the programs take normalised
                    // coordinates.
                    uses_palette |= decoder.decode(map, files, images, bitmap, image, n, true)?;
                }
                Ok(p)
            });
        match compiled {
            Ok(p) => {
                let mut any = false;
                let mut reasons = BTreeMap::<String, usize>::new();
                for d in world.draws.iter().filter(|d| d.material == m) {
                    let inputs = match d.owner {
                        Owner::Cluster(_) => Ok(p.inputs),
                        Owner::Instance(i) if world.rows(i).is_none() => Err("placement rows unavailable".to_owned()),
                        Owner::Instance(_) => p.rigid.as_ref().map(|r| r.inputs)
                            .ok_or_else(|| "rigid vertex program unavailable".to_owned()),
                    };
                    match inputs.and_then(|inputs| program::part_fit(d, inputs, &p, page_ready)) {
                        Ok(()) => { parts.insert((d.owner, d.part)); any = true; }
                        Err(why) => *reasons.entry(why).or_default() += 1,
                    }
                }
                if !any {
                    for why in reasons.into_keys() {
                        refused.entry(why).or_default().push(m);
                    }
                    continue;
                }
                if hooks::native_verbose() {
                    for (why, count) in reasons {
                        eprintln!("native-world: material {m} leaves {count} parts to D3D: {why}");
                    }
                }
                programs.insert(m, p);
                if uses_palette {
                    palettized.push(m);
                }
            }
            Err(why) => refused.entry(why).or_default().push(m),
        }
    }
    palettized.sort_unstable();
    out
}

/// Keep cube faces and volume slices together. Passing face zero to the
/// two-dimensional image decoder changes both the fetch and its coordinates.
fn bitmap_dimension<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>, files: &mut assets::ResourceFiles, bitmap: TagId, index: u32,
    images: &mut LayeredImages,
) -> Result<TextureDim, String> {
    let tags::TagData::Bitmap(tag) = tags::DecodedTag::read(map, bitmap).map_err(|e| e.to_string())?.data else {
        return Err("stage datum is not a bitmap".into());
    };
    let image = tag.images.get(index as usize).ok_or("bitmap has no such image")?;
    let dim = match image.kind {
        tags::TextureKind::D2 => return Ok(TextureDim::D2),
        tags::TextureKind::D3 => TextureDim::D3,
        tags::TextureKind::Cube => TextureDim::Cube,
        tags::TextureKind::Unknown(kind) => return Err(format!("unknown bitmap shape {kind}")),
    };
    if matches!(image.format, tags::PixelFormat::P8 | tags::PixelFormat::P8Bump) {
        return Err("layered bitmap requires a stage palette".into());
    }
    let decoded = assets::bitmap::decode_image(image, files).map_err(|e| e.to_string())?;
    if decoded.desc.format.layout() == gfx::format::Layout::Linear {
        return Err("layered bitmap uses linear addressing".into());
    }
    let pixels = crate::native_lights_glue::layered(decoded.levels, dim)
        .ok_or("incomplete layered bitmap")?;
    images.insert((bitmap, index), pixels);
    Ok(dim)
}

/// Stage images as their stages sample them, for every native pass that
/// loads with the world (its layer 3, the transparents; the objects can
/// take it too): a palettized bitmap (P8, P8 bump) is expanded through the
/// device palette bound at its stage when the load started
/// ([`stage_image`]), once per image, and another stage may share that
/// expansion only when it holds the same palette. Records which palettes
/// it used ([`Ready`] checks they stay bound) and which images do not
/// decode.
#[derive(Default)]
pub struct StageDecoder {
    /// The palettes bound at each stage when the load started.
    palettes: StagePalettes,
    /// Those an expansion used.
    used: StagePalettes,
    /// The stage each palettized image was expanded at.
    expanded_at: HashMap<(TagId, u32), usize>,
    /// Images that do not decode at any stage.
    failed: HashMap<(TagId, u32), String>,
    /// Cached images whose D3D coordinates are in texels.
    linear: HashSet<(TagId, u32)>,
}

impl StageDecoder {
    pub fn new(palettes: StagePalettes) -> Self {
        Self {
            palettes,
            ..Self::default()
        }
    }

    /// The palettes the expansions used, at their stages (the rest
    /// `None`).
    pub fn used(&self) -> &StagePalettes {
        &self.used
    }

    /// Image `image` of `bitmap` as stage `stage` samples it, put in
    /// `images` unless there already: `true` when it is palettized (and
    /// so went through the stage's palette), or why the stage cannot have
    /// it, as `stage <n> bitmap <why>`. `refuse_linear`: a linear texture
    /// is refused (it is not cached as failed: another pass may take it).
    #[allow(clippy::too_many_arguments)]
    pub fn decode<R: std::io::Read + std::io::Seek>(
        &mut self,
        map: &mut Map<R>,
        files: &mut assets::ResourceFiles,
        images: &mut StageImages,
        bitmap: TagId,
        image: u32,
        stage: usize,
        refuse_linear: bool,
    ) -> Result<bool, String> {
        let key = (bitmap, image);
        if refuse_linear && self.linear.contains(&key) {
            return Err(format!("stage {stage} bitmap is a linear texture"));
        }
        let mut linear = false;
        let decoded = self.take(images, key, stage, &mut |palette| {
            let (image, expanded, layout) = decode_stage(map, files, bitmap, image as usize, stage, palette)?;
            linear = layout == gfx::format::Layout::Linear;
            if refuse_linear && linear {
                return Err(StageRefusal::Here("is a linear texture"));
            }
            Ok((image, expanded))
        });
        if linear && decoded.is_ok() {
            self.linear.insert(key);
        }
        decoded
    }

    /// [`decode`](Self::decode) with the decode itself as `decode` (given
    /// the stage's palette).
    fn take(
        &mut self,
        images: &mut StageImages,
        key: (TagId, u32),
        stage: usize,
        decode: &mut dyn FnMut(&Option<Vec<u32>>) -> Result<(Image, bool), StageRefusal>,
    ) -> Result<bool, String> {
        let palette = self.palettes.get(stage).ok_or_else(|| format!("stage {stage} does not exist"))?;
        if let Some(&at) = self.expanded_at.get(&key) {
            // One expansion per image: this stage must hold the same
            // palette (and stage 3 holds the lightmap's).
            if stage == 3 || *palette != self.palettes[at] {
                return Err(format!("stage {stage} bitmap is palettized and was expanded through another stage's palette"));
            }
            self.used[stage].clone_from(palette);
            return Ok(true);
        }
        if images.contains_key(&key) {
            return Ok(false);
        }
        if let Some(why) = self.failed.get(&key) {
            return Err(format!("stage {stage} bitmap {why}"));
        }
        match decode(palette) {
            Ok((i, expanded)) => {
                images.insert(key, i);
                if expanded {
                    self.expanded_at.insert(key, stage);
                    self.used[stage].clone_from(palette);
                }
                Ok(expanded)
            }
            Err(StageRefusal::Image(why)) => {
                let message = format!("stage {stage} bitmap {why}");
                self.failed.insert(key, why);
                Err(message)
            }
            // Not the image's fault: it may do at another stage or pass.
            Err(StageRefusal::Here(why)) => Err(format!("stage {stage} bitmap {why}")),
        }
    }
}

/// Why a stage cannot have a bitmap's image: the image itself, or only
/// here (this stage's palette, or a linear texture this pass refuses).
#[derive(Debug, PartialEq, Eq)]
enum StageRefusal {
    Image(String),
    Here(&'static str),
}

/// Image `index` of a stage's bitmap at stage `stage`, and whether it was
/// expanded through the stage's palette; or why the stage cannot have it:
/// it does not decode, or it is palettized and the stage has no palette
/// for it ([`stage_image`]). The caller uses the layout to refuse linear
/// textures where the native path cannot use the D3D texel coordinates.
fn decode_stage<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    files: &mut assets::ResourceFiles,
    id: TagId,
    index: usize,
    stage: usize,
    palette: &Option<Vec<u32>>,
) -> Result<(Image, bool, gfx::format::Layout), StageRefusal> {
    let tags::TagData::Bitmap(b) = tags::DecodedTag::read(map, id).map_err(|_| StageRefusal::Image("does not read".into()))?.data else {
        return Err(StageRefusal::Image("is not a bitmap".into()));
    };
    let image = b.images.get(index).ok_or_else(|| StageRefusal::Image("has no such image".into()))?;
    let (d, blocks) = assets::decode_image_blocks(image, files)
        .map_err(|e| StageRefusal::Image(format!("does not decode ({e})")))?;
    let layout = d.desc.format.layout();
    let (image, palettized) = stage_image(image.format, d.levels, stage, palette)?;
    Ok((with_blocks(image, blocks), palettized, layout))
}

/// `image` with its BC blocks, when the D3D path uploads the bitmap as BC
/// (`assets::decode_image_blocks`): the native pass then samples the same
/// GPU-decoded texels. The CPU decoder (`gfx::dxt`) truncates the
/// interpolated colours, the GPU rounds them, which left native lit
/// surfaces one level darker than the D3D path's on a few percent of
/// pixels.
fn with_blocks(image: Image, blocks: Option<assets::BlockLevels>) -> Image {
    let Some(b) = blocks else { return image };
    let mut levels: Vec<_> = b.levels.into_iter().filter(|l| l.face == 0 && l.depth <= 1).collect();
    levels.sort_by_key(|l| l.level);
    image.with_blocks(b.host.to_wgpu(), levels.into_iter().map(|l| l.rgba).collect())
}

/// A stage bitmap's decoded levels as the stage samples them, and whether
/// they went through its palette. A palettized format (P8, P8 bump)
/// decodes as its index in every channel, and the D3D path samples it
/// through the device palette bound at its stage: so it is expanded
/// through `palette`, and refused at stage 3 (where the structure draw
/// binds each lightmap's own palette, `0x00015c90`) or without a full
/// palette. For the world's layer 3, stages 0..2 hold the device's default
/// palette (bound on every stage at `0x00015b70`; only the lightmap binder
/// binds another, at stage 3).
fn stage_image(
    format: tags::PixelFormat,
    levels: Vec<assets::Level>,
    stage: usize,
    palette: &Option<Vec<u32>>,
) -> Result<(Image, bool), StageRefusal> {
    let palette: Option<[u32; 256]> = if matches!(format, tags::PixelFormat::P8 | tags::PixelFormat::P8Bump) {
        if stage == 3 {
            return Err(StageRefusal::Here("is palettized at stage 3 (the lightmap's palette is bound there)"));
        }
        let p = palette
            .as_ref()
            .ok_or(StageRefusal::Here("is palettized and no palette is bound at its stage"))?;
        Some(
            p.as_slice()
                .try_into()
                .map_err(|_| StageRefusal::Here("is palettized and its stage's palette has fewer than 256 entries"))?,
        )
    } else {
        None
    };
    let mut levels: Vec<_> = levels.into_iter().filter(|l| l.face == 0 && l.depth <= 1).collect();
    levels.sort_by_key(|l| l.level);
    let image = Image::from_levels(
        levels
            .into_iter()
            .map(|l| match &palette {
                // Decoded as L8: the index in every channel.
                Some(p) => {
                    let indices: Vec<u8> = l.rgba.chunks_exact(4).map(|t| t[0]).collect();
                    (l.width, l.height, load::expand_page(&indices, p))
                }
                None => (l.width, l.height, l.rgba),
            })
            .collect(),
    )
    .ok_or_else(|| StageRefusal::Image("does not decode".into()))?;
    Ok((image, palette.is_some()))
}

/// A light stage uses normalized 2D coordinates. Decode all mip levels
/// with the same palette and BC rules as the world's material pass.
pub fn light_image<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    files: &mut assets::ResourceFiles,
    id: TagId,
    index: usize,
    stage: usize,
    palette: &Option<Vec<u32>>,
) -> Option<(Image, bool)> {
    let tags::TagData::Bitmap(b) = tags::DecodedTag::read(map, id).ok()?.data else { return None };
    let image = b.images.get(index)?;
    if image.kind != tags::TextureKind::D2 {
        return None;
    }
    let (d, blocks) = assets::decode_image_blocks(image, files).ok()?;
    if d.desc.format.layout() == gfx::format::Layout::Linear {
        return None;
    }
    let (image, palettized) = stage_image(image.format, d.levels, stage, palette).ok()?;
    Some((with_blocks(image, blocks), palettized))
}

/// The first image of a bitmap as RGBA, with its mip chain.
pub fn decode_first<R: std::io::Read + std::io::Seek>(map: &mut Map<R>, files: &mut assets::ResourceFiles, id: TagId) -> Option<Image> {
    decode_image(map, files, id, 0)
}

/// Image `index` of a bitmap as RGBA, with its mip chain.
pub fn decode_image<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    files: &mut assets::ResourceFiles,
    id: TagId,
    index: usize,
) -> Option<Image> {
    let tags::TagData::Bitmap(b) = tags::DecodedTag::read(map, id).ok()?.data else {
        return None;
    };
    let d = assets::decode_image(b.images.get(index)?, files).ok()?;
    let mut levels: Vec<_> = d.levels.into_iter().filter(|l| l.face == 0 && l.depth <= 1).collect();
    levels.sort_by_key(|l| l.level);
    // The whole decoded chain: the native pass samples with mipmaps, as
    // the D3D path does.
    Image::from_levels(levels.into_iter().map(|l| (l.width, l.height, l.rgba)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_render::world::{Owner, PartInput, Placement, SectionInput, WorldBuilder};

    /// Whether a native material's part was kept: a
    /// cluster's, and a placement's when the material has a rigid program
    /// (`rigid`). The passes after layer 3 (shadows 11, lights 12 and 13, fog
    /// 16, overlays 7) test `ZFUNC` equal against layer 3's depth, so the
    /// native pass must write the D3D path's bits: a cluster goes through the
    /// world program with the same position bits on both paths, and the D3D
    /// path draws a placement with the rigid classification of the pass's
    /// vertex program, which places the definition's vertices on the GPU with
    /// the placement's rows in `c[50..52]` (`0x0004d830`: forward, left and up
    /// by column and the position, copied from the placement the render
    /// record points at, its scale not applied). The native pass does the
    /// same, with the same rows per placement (`program::rigid_vertex`).
    fn drawn_natively(d: &WorldDraw, rigid: &HashSet<u16>) -> bool {
        match d.owner {
            Owner::Cluster(_) => true,
            Owner::Instance(_) => rigid.contains(&d.material),
        }
    }

    #[test]
    fn opaque_resource_boundaries_drain_the_same_runtime_watch_as_d3d() {
        use d3d::device::{DeviceConfig, GuestRange};
        use runtime::guest::Permissions;
        use runtime::watch::CONTIGUOUS_BASE;

        let mut mem = runtime::Guest::new();
        let va = 0x10000;
        let pa = 0x40000;
        let folded = CONTIGUOUS_BASE | pa;
        mem.map(va, 0x2000, pa, Permissions::RW).unwrap();
        let watch = mem.subscribe_writes(crate::d3d_glue::WATCH_OWNER);
        let foreign = mem.subscribe_writes("other observer");
        for id in [watch, foreign] { mem.watch_writes(id, folded, 0x2000).unwrap(); }

        let range = GuestRange { addr: 0x60000, len: 0x10000 };
        let mut device = d3d::Device::new(DeviceConfig::halo2(range, range), d3d::RecordingBackend::new());
        device.tracker.watch(folded, 0x2000);
        let initial = device.tracker.generation();
        mem.set_u32(va, 42).unwrap();
        drain_opaque_writes(&mut mem, &mut device, watch).unwrap();
        assert!(device.tracker.dirty_since(folded, 4, initial));
        assert!(!mem.has_dirty(watch));

        // A material or resource-destroy callback writes another page after
        // prepare. Its store must be visible to the later ordered emit.
        let prepared = device.tracker.generation();
        mem.set_u32(va + 0x1004, 81).unwrap();
        drain_opaque_writes(&mut mem, &mut device, watch).unwrap();
        assert!(!device.tracker.dirty_since(folded, 4, prepared));
        assert!(device.tracker.dirty_since(folded + 0x1004, 4, prepared));
        assert!(!mem.has_dirty(watch));
        assert_eq!(mem.take_dirty(foreign).unwrap(), [runtime::DirtyRange { addr: folded, len: 0x2000 }]);
    }

    #[test]
    fn material_rejection_summary_counts_and_ranks_the_top_reasons() {
        let refused = [
            ("missing page", vec![0, 1]),
            ("no palette", vec![2, 3, 4]),
            ("no shader", vec![5, 6]),
            ("linear", vec![7]),
            ("no program", vec![8]),
            ("unsupported stage", vec![9]),
        ];
        let refused = refused.into_iter().map(|(why, materials)| (why.into(), materials)).collect();
        let top = top_rejections(&refused);
        assert_eq!(
            top.iter().map(|(why, count)| (why.as_str(), *count)).collect::<Vec<_>>(),
            [("no palette", 3), ("missing page", 2), ("no shader", 2), ("linear", 1), ("no program", 1)],
        );
    }

    #[test]
    fn materials_left_to_the_d3d_path_leave_the_world() {
        let tri = |m| SectionInput {
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            parts: vec![PartInput {
                material: m,
                transparent: false,
                triangles: vec![[0, 1, 2]],
            }],
            ..Default::default()
        };
        let mut b = WorldBuilder::new();
        for (i, m) in [1u16, 2, 2, 3].into_iter().enumerate() {
            b.add_section(Owner::Cluster(i as u32), &tri(m), &Placement::IDENTITY, None);
        }
        let native: HashSet<u16> = [2, 3].into_iter().collect();
        let w = keep_draws(b.finish(), &|d| native.contains(&d.material));
        let batches: Vec<_> = w.batches.iter().map(|b| (b.material, b.first_draw, b.draw_count)).collect();
        assert_eq!(batches, vec![(2, 0, 2), (3, 2, 1)]);
        assert_eq!(w.draws.iter().map(|d| (d.material, d.batch)).collect::<Vec<_>>(), vec![(2, 0), (2, 0), (3, 1)]);
    }

    #[test]
    fn an_untranslated_material_keeps_its_d3d_draws_and_a_translated_one_is_skipped() {
        // The world as the loader leaves it with programs on: material 7
        // translated, materials 8 and 9 refused, 9 sharing 7's shader. 8
        // and 9 are neither drawn natively nor skipped, so the D3D path
        // still draws them; 7's cluster part is drawn natively and
        // skipped, the shader's other parts not, and 7's part on an
        // instanced placement stays on the D3D path without a rigid
        // program (the D3D path places it on the GPU), and is drawn
        // natively with one.
        let tri = |m| PartInput {
            material: m,
            transparent: false,
            triangles: vec![[0, 1, 2]],
        };
        let section = |parts| SectionInput {
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            parts,
            ..Default::default()
        };
        let mut b = WorldBuilder::new();
        b.add_section(Owner::Cluster(0), &section(vec![tri(7), tri(9)]), &Placement::IDENTITY, None);
        b.add_section(Owner::Instance(3), &section(vec![tri(8), tri(7)]), &Placement::IDENTITY, None);
        let world = b.finish();
        let mut shaders = vec![TagId(0xe000_0000); 10];
        shaders[7] = TagId(0xe000_0107);
        shaders[8] = TagId(0xe000_0108);
        shaders[9] = TagId(0xe000_0107);
        let translated: HashSet<u16> = [7].into_iter().collect();
        let w = keep_draws(world.clone(), &|d| translated.contains(&d.material) && drawn_natively(d, &HashSet::new()));
        assert!(w.draws.iter().all(|d| d.material == 7), "8 and 9 are not drawn natively (no stage 1 look)");
        let parts = part_set(&w, &shaders);
        let want: PartSet = [((Owner::Cluster(0), 0), 0xe000_0107)].into_iter().collect();
        assert_eq!(parts, want, "7's cluster part by owner and index, with its shader; not 9's, which shares it, nor the placement's");
        // Batched again over what is left: one batch, one draw.
        let batches: Vec<_> = w.batches.iter().map(|b| (b.material, b.first_draw, b.draw_count)).collect();
        assert_eq!(batches, vec![(7, 0, 1)]);
        assert_eq!(w.draws[0].batch, 0);
        // With 7's rigid program its placement's part is drawn and skipped
        // too, in the same batch after the cluster's.
        let rigid: HashSet<u16> = [7].into_iter().collect();
        let w = keep_draws(world, &|d| translated.contains(&d.material) && drawn_natively(d, &rigid));
        let parts = part_set(&w, &shaders);
        let want: PartSet = [((Owner::Cluster(0), 0), 0xe000_0107), ((Owner::Instance(3), 1), 0xe000_0107)].into_iter().collect();
        assert_eq!(parts, want);
        let owners: Vec<_> = w.draws.iter().map(|d| (d.owner, d.batch)).collect();
        assert_eq!(owners, vec![(Owner::Cluster(0), 0), (Owner::Instance(3), 0)]);
    }

    /// 03a's intro cinematic: the level's BSPs load at one address, and the
    /// game's index says which is active. The first BSP at the address is
    /// not it (the address alone gave the play space's world, whose parts
    /// matched some of the cinematic BSP's items by cluster, part and
    /// shader: those items were skipped and nothing drew them).
    #[test]
    fn the_active_bsp_is_the_game_s_index_not_the_first_at_its_address() {
        let at = 0x800e_6010;
        let addresses = [Some(at), Some(at), Some(0x8010_0000), None];
        assert_eq!(active_scenario_bsp(&addresses, 1, at), Some(1));
        assert_eq!(active_scenario_bsp(&addresses, 0, at), Some(0));
        assert_eq!(active_scenario_bsp(&addresses, 2, at), None, "loads elsewhere");
        assert_eq!(active_scenario_bsp(&addresses, 3, at), None, "no location");
        assert_eq!(active_scenario_bsp(&addresses, 4, at), None, "past the block");
    }

    #[test]
    fn the_inherited_cull_is_the_device_s_as_the_d3d_pipeline_takes_it() {
        // The D3D draws of placement 282 on 03a: `CULL_CW`, front face `CW`.
        use gfx::state::{CullMode, FrontFace};
        assert_eq!(inherited_cull((CullMode::Cw, FrontFace::Cw)), (Some(wgpu::Face::Front), wgpu::FrontFace::Cw));
        assert_eq!(inherited_cull((CullMode::Ccw, FrontFace::Cw)), (Some(wgpu::Face::Back), wgpu::FrontFace::Cw));
        assert_eq!(inherited_cull((CullMode::None, FrontFace::Ccw)), (None, wgpu::FrontFace::Ccw));
    }

    #[test]
    fn the_active_bsp_index_reads_the_game_globals() {
        let mut m = engine::memory::FlatMemory::new(0x0040_0000, 0x0021_0000);
        assert_eq!(active_bsp_index(&m), None, "no globals yet");
        let globals = 0x0050_0000;
        m.set_u32(engine::points::GAME_GLOBALS.base, globals).unwrap();
        m.set_u32(globals, 0x0001_0000).unwrap();
        assert_eq!(active_bsp_index(&m), Some(1));
        m.set_u32(globals, 0xffff_0000).unwrap();
        assert_eq!(active_bsp_index(&m), None, "-1: none active");
    }

    #[test]
    fn the_pass_stencil_follows_the_items_switches() {
        use gfx::state::{CompareFunc, StencilOp, StencilState};
        let mode1 = StencilState {
            enable: true,
            func: CompareFunc::Always,
            reference: 0,
            read_mask: 0xff,
            write_mask: 0xff,
            fail: StencilOp::Keep,
            depth_fail: StencilOp::Keep,
            pass: StencilOp::Replace,
        };
        let other = StencilState { enable: true, func: CompareFunc::Equal, reference: 3, ..mode1 };
        let none = StencilPlan::default();
        assert_eq!(pass_stencil(&other, &none), Some((other, vec![])), "no switch: the state as it is");
        let marked = StencilPlan { off: false, marked: vec![Owner::Cluster(4)], switches: true };
        assert_eq!(pass_stencil(&mode1, &marked), Some((mode1, vec![Owner::Cluster(4)])));
        assert_eq!(pass_stencil(&other, &marked), None, "a switch leaves mode 1, which the layer did not start in");
        let object = StencilPlan { switches: true, ..StencilPlan::default() };
        assert_eq!(pass_stencil(&mode1, &object), Some((mode1, vec![])), "an object's switch: mode 1 throughout");
        let off = StencilPlan { off: true, marked: vec![], switches: true };
        let (state, marked) = pass_stencil(&mode1, &off).unwrap();
        assert!(!state.enable && marked.is_empty(), "mode 0: the stencil off");
    }

    /// The device's stencil state as `0x00015370` sets it for `mode`
    /// (`engine::render::stencil::for_mode`); mode 0 only clears the
    /// enable, over `before`.
    fn device_mode(mode: u16, before: gfx::state::StencilState) -> gfx::state::StencilState {
        use engine::render::stencil::{Compare, Op, for_mode};
        use gfx::state::{CompareFunc, StencilOp, StencilState};
        let op = |o| match o {
            Op::Keep => StencilOp::Keep,
            Op::Replace => StencilOp::Replace,
        };
        match for_mode(mode).expect("a mode the helper takes") {
            None => StencilState { enable: false, ..before },
            Some(s) => StencilState {
                enable: s.enable,
                func: match s.func {
                    Compare::Equal => CompareFunc::Equal,
                    Compare::NotEqual => CompareFunc::NotEqual,
                    Compare::Always => CompareFunc::Always,
                },
                reference: s.reference,
                read_mask: s.mask,
                write_mask: s.write_mask,
                fail: op(s.fail),
                depth_fail: op(s.depth_fail),
                pass: op(s.pass),
            },
        }
    }

    /// The stencil value a pixel keeps after the native pass draws
    /// `owner`'s part over `stored` (depth passing), as the pass's
    /// pipeline evaluates it: the device's state translated
    /// (`PassStencil::from_device`, a D24S8 depth) with the owner's
    /// reference (`PassStencil::reference_of`), `WorldPass::encode`'s.
    fn native_write(state: &gfx::state::StencilState, marked: &[Owner], owner: Owner, stored: u8) -> u8 {
        let mut s = PassStencil::from_device(state, wgpu::TextureFormat::Depth24PlusStencil8);
        if s == PassStencil::default() {
            return stored;
        }
        s.marked = marked.to_vec();
        let reference = s.reference_of(owner) as u8;
        let face = s.state.front;
        let (read, write) = (s.state.read_mask as u8, s.state.write_mask as u8);
        let passes = match face.compare {
            wgpu::CompareFunction::Always => true,
            wgpu::CompareFunction::Equal => reference & read == stored & read,
            wgpu::CompareFunction::NotEqual => reference & read != stored & read,
            other => panic!("not a compare the device's modes use: {other:?}"),
        };
        let op = if passes { face.pass_op } else { face.fail_op };
        match op {
            wgpu::StencilOperation::Keep => stored,
            wgpu::StencilOperation::Replace => (stored & !write) | (reference & write),
            other => panic!("not an op the device's modes use: {other:?}"),
        }
    }

    /// What the game's layer 3 leaves in the stencil where a section's
    /// item draws (`0x0004d640`, the draw, `0x0004d720`), against what the
    /// native pass writes for its part, over every stored value; and the
    /// device's state after the layer, which the native pass does not set
    /// (the D3D items left in the layer and the passes after it see the
    /// state the layer started in): the same as the game's after its last
    /// switching item, mode 1.
    #[test]
    fn the_native_stencil_writes_and_leaves_what_the_game_s_layer_3_does() {
        let mode1 = device_mode(1, gfx::state::StencilState::default());
        let marked = Owner::Cluster(4);
        let plain = [Owner::Cluster(1), Owner::Instance(2)];
        let cases = [
            // (the layer's plan, the game's mode for the marked section and
            // for the rest)
            (StencilPlan { off: false, marked: vec![marked], switches: true }, 2, 1),
            (StencilPlan { switches: true, ..StencilPlan::default() }, 1, 1),
            (StencilPlan { off: true, marked: vec![], switches: true }, 0, 0),
            (StencilPlan::default(), 1, 1),
        ];
        for (plan, marked_mode, plain_mode) in cases {
            let (state, sections) = pass_stencil(&mode1, &plan).expect("mode 1 at the layer's start: drawn");
            for stored in 0..=255u8 {
                for (owner, mode) in std::iter::once((marked, marked_mode)).chain(plain.iter().map(|&o| (o, plain_mode))) {
                    let game = device_mode(mode, mode1);
                    let expect = if game.enable {
                        let s = engine::render::stencil::for_mode(mode).unwrap().unwrap();
                        if s.passes(stored) { s.after_pass(stored) } else { stored }
                    } else {
                        stored
                    };
                    assert_eq!(
                        native_write(&state, &sections, owner, stored),
                        expect,
                        "{plan:?}: {owner:?} over {stored:#x} (game mode {mode})"
                    );
                }
            }
            // After the layer: the game's switching items end in mode 1
            // (0x0004d720), and without a switch the state is the start's.
            // The native pass sets no device state, so the device holds
            // `mode1`, the start: equal either way.
            let game_after = if plan.switches { device_mode(1, mode1) } else { mode1 };
            assert_eq!(game_after, mode1, "{plan:?}");
        }
        // Mode 2's write is 0xff, mode 1's 0: the reference alone differs.
        assert_eq!(device_mode(2, mode1), gfx::state::StencilState { reference: PassStencil::MARK as u8, ..mode1 });
    }

    /// A 2x2 level and its 1x1 mip as `assets` decodes a P8 image: the
    /// index in every channel.
    fn indexed(indices: [u8; 4]) -> Vec<assets::Level> {
        let level = |level, width, idx: &[u8]| assets::Level {
            face: 0,
            level,
            width,
            height: width,
            depth: 1,
            rgba: idx.iter().flat_map(|&i| [i, i, i, 0xff]).collect(),
        };
        vec![level(1, 1, &indices[..1]), level(0, 2, &indices)]
    }

    /// A palette of tangent-space normals packed as colours, `A8R8G8B8`.
    fn normals() -> Vec<u32> {
        (0..256u32).map(|i| 0xff00_0000 | ((i ^ 0x80) << 16) | ((0xff - i) << 8) | 0xfe).collect()
    }

    #[test]
    fn scene_palette_supplies_an_unbound_stage_but_the_draw_still_checks_it() {
        let default = normals();
        let known = vec![0x1122_3344; 256];
        let lightmap = vec![0x5566_7788; 256];
        let mut bound = [Some(known.clone()), Some(vec![0; 64]), None, Some(lightmap.clone())];
        let before = bound.clone();
        supply_scene_palette(&mut bound, || Some(vec![0; 64]));
        assert_eq!(bound, before, "an incomplete default cannot supply a stage");
        supply_scene_palette(&mut bound, || Some(default.clone()));
        assert_eq!(bound, [Some(known), Some(default.clone()), Some(default.clone()), Some(lightmap)]);
        let mut decoder = StageDecoder::new(bound);
        let mut images = StageImages::new();
        let key = (TagId(0xe000_0001), 0);
        let p8 = |pal: &Option<Vec<u32>>| stage_image(tags::PixelFormat::P8Bump, indexed([1; 4]), 2, pal);
        assert_eq!(decoder.take(&mut images, key, 2, &mut |pal| p8(pal)), Ok(true));
        assert!(!palettes_hold(decoder.used(), &|stage| before[stage].clone()));
        assert!(palettes_hold(decoder.used(), &|stage| (stage == 2).then(|| default.clone())));
    }

    #[test]
    fn a_palettized_bump_map_samples_its_stage_palette_not_its_index() {
        // The 03a A/B: a P8 bump map sampled as its index (grey) gave the
        // bump term a wrong normal, so `r0` clamped to black on most walls
        // and over-brightened others. The stage samples the palette entry,
        // as the D3D path does through the palette bound at that stage.
        let (image, expanded) = stage_image(tags::PixelFormat::P8Bump, indexed([0, 1, 0x80, 0xff]), 2, &Some(normals())).unwrap();
        assert!(expanded);
        assert_eq!((image.width, image.height), (2, 2));
        let texel = |i: usize| <[u8; 4]>::try_from(&image.rgba[4 * i..4 * i + 4]).unwrap();
        assert_eq!(texel(0), [0x80, 0xff, 0xfe, 0xff]);
        assert_eq!(texel(1), [0x81, 0xfe, 0xfe, 0xff]);
        assert_eq!(texel(2), [0x00, 0x7f, 0xfe, 0xff]);
        assert_eq!(texel(3), [0x7f, 0x00, 0xfe, 0xff]);
        // The mip goes through the same palette.
        assert_eq!(image.mips, vec![vec![0x80, 0xff, 0xfe, 0xff]]);
        // Plain P8 the same.
        assert!(stage_image(tags::PixelFormat::P8, indexed([0; 4]), 0, &Some(normals())).unwrap().1);
    }

    #[test]
    fn a_palettized_stage_without_its_palette_stays_on_the_d3d_path() {
        let p8 = tags::PixelFormat::P8Bump;
        let palette = |e: StageRefusal| matches!(e, StageRefusal::Here(_));
        assert!(palette(stage_image(p8, indexed([0; 4]), 1, &None).unwrap_err()));
        // Stage 3 holds each lightmap's palette.
        assert!(palette(stage_image(p8, indexed([0; 4]), 3, &Some(normals())).unwrap_err()));
        // A palette of fewer entries than the indices can reach.
        assert!(palette(stage_image(p8, indexed([0; 4]), 0, &Some(normals()[..32].to_vec())).unwrap_err()));
        // A direct format keeps its colours and needs no palette.
        let (image, expanded) = stage_image(tags::PixelFormat::Argb8888, indexed([9, 8, 7, 6]), 3, &None).unwrap();
        assert!(!expanded);
        assert_eq!(&image.rgba[..4], &[9, 9, 9, 0xff]);
    }

    #[test]
    fn a_world_draws_only_while_the_palettes_it_expanded_through_are_bound() {
        let used: StagePalettes = [None, None, Some(normals()), None];
        assert!(palettes_hold(&used, &|s| (s == 2).then(normals)));
        // Other stages are not looked at.
        assert!(palettes_hold(&used, &|s| if s == 2 { Some(normals()) } else { Some(vec![1]) }));
        assert!(!palettes_hold(&used, &|_| None));
        let mut other = normals();
        other[5] = 0;
        assert!(!palettes_hold(&used, &|_| Some(other.clone())));
        assert!(palettes_hold(&StagePalettes::default(), &|_| None));
    }

    #[test]
    fn passes_share_a_palettized_image_only_through_the_same_stage_palette() {
        // Stages 1 and 2 hold the default palette, stage 0 none: as at
        // 03a's layer 3. The world's pass expands a P8 bump map at stage
        // 2; a transparent pass (sharing the decoder and its images)
        // takes it at stage 1, and is refused it at stage 0 and 3.
        let mut d = StageDecoder::new([None, Some(normals()), Some(normals()), Some(normals())]);
        let mut images = StageImages::new();
        let bump = (TagId(0xe000_0040), 0);
        let mut decodes = 0;
        let p8 = |stage: usize| {
            move |palette: &Option<Vec<u32>>| stage_image(tags::PixelFormat::P8Bump, indexed([1, 2, 3, 4]), stage, palette)
        };
        assert_eq!(d.take(&mut images, bump, 2, &mut |pal| { decodes += 1; p8(2)(pal) }), Ok(true));
        assert_eq!(images[&bump].rgba[..4], [0x81, 0xfe, 0xfe, 0xff], "palette entry 1, not grey 1");
        assert_eq!(d.used(), &[None, None, Some(normals()), None]);
        assert_eq!(d.take(&mut images, bump, 1, &mut |pal| { decodes += 1; p8(1)(pal) }), Ok(true));
        assert_eq!(decodes, 1, "expanded once");
        assert_eq!(d.used(), &[None, Some(normals()), Some(normals()), None]);
        assert!(!palettes_hold(d.used(), &|s| (s == 2).then(normals)));
        assert!(d.take(&mut images, bump, 0, &mut |pal| p8(0)(pal)).is_err(), "no palette at stage 0");
        assert!(d.take(&mut images, bump, 3, &mut |pal| p8(3)(pal)).is_err(), "the lightmap's stage");

        // A transparent's own palettized bitmap at a stage without a
        // palette is refused, and not cached as failed: it still expands
        // at a stage that has one.
        let other = (TagId(0xe000_0041), 0);
        let err = d.take(&mut images, other, 0, &mut |pal| p8(0)(pal)).unwrap_err();
        assert!(err.starts_with("stage 0 bitmap is palettized"), "{err}");
        assert!(!images.contains_key(&other));
        assert_eq!(d.take(&mut images, other, 1, &mut |pal| p8(1)(pal)), Ok(true));
        assert_eq!(d.used()[1], Some(normals()));

        // A direct format is no palette's business; an image that does
        // not decode fails at every stage after.
        let plain = (TagId(0xe000_0042), 0);
        let argb = |pal: &Option<Vec<u32>>| stage_image(tags::PixelFormat::Argb8888, indexed([5; 4]), 0, pal);
        assert_eq!(d.take(&mut images, plain, 0, &mut |pal| argb(pal)), Ok(false));
        let broken = (TagId(0xe000_0043), 0);
        let why = format!("does not decode ({})", assets::Error::MissingResource(map::ResourceFile::MainMenu));
        let err = d.take(&mut images, broken, 1, &mut |_| Err(StageRefusal::Image(why.clone()))).unwrap_err();
        assert_eq!(err, format!("stage 1 bitmap {why}"));
        let err = d.take(&mut images, broken, 2, &mut |_| unreachable!("cached as failed")).unwrap_err();
        assert_eq!(err, format!("stage 2 bitmap {why}"));
    }
}
