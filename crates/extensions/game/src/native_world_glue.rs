//! The native world's host (docs/graphics/native-world.md, stage 1):
//! loads the active BSP's world from the loaded map's file when the BSP
//! changes, on a worker, and pushes each view's world pass as a frame
//! stage at its layer 3 (`render_bridge::native_world`). Inert with
//! `impl.render.native=off` (nothing asks).
//!
//! - **Load once.** On a new (map, BSP) the worker opens the map through
//!   the VFS, finds the scenario BSP whose tag data sits
//!   at the active BSP's address, builds the world (`native_render::load`),
//!   decodes each lightmap page, and translates each material's layer-3
//!   pass. The same worker uploads its buffers and material programs.
//!   Until it is ready, or when it fails, no world draws.
//! - **Per frame.** Completed stages publish before the views. A view supplies
//!   the camera and pushes one stage (`FrameOp::Stage`, slot opaque).
//! - **Material programs.** Each part needs a translated layer-3 pass with decoded
//!   stage bitmaps, the inputs of its vertex classification, and any
//!   sampled lightmap page (`program::part_fit`). The part draws with
//!   its own vertex program, pixel shader, stage textures,
//!   constants and render states, and the view's constants each frame.
//!   Every other material is left out of the world (the skip clears only
//!   the items of the parts the world draws, by owner and part index,
//!   whatever shares their shader). The load says the coverage and each
//!   reason once, with counts. A palettized stage bitmap (the bump map's
//!   P8) is expanded through the palette bound at its stage when the load
//!   started; while that palette is no longer bound the world refuses.
//! - **Lights**: the lights' part of the world, loaded with it
//!   (`native_lights_glue::load`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, mpsc};

use guest_layout::memory::GuestMemory;
use render_bridge::native_map_work::{self, Stage as MapStage};
use render_bridge::native_world::{Host, PartSet};
use map::{Map, TagId};
use native_render::camera::RenderCamera;
use native_render::gpu::{DepthRule, GpuWorld, Image, PassStencil, PassViewport, WorldSelection};
use native_render::light_gpu::{LayeredImage, TextureDim};
use native_render::load::{self, LoadedWorld};
/// The decodes and the draw filter live in the renderer, where the editor
/// and Theater's host view share them.
pub use native_render::load::{decode_first, decode_image, decode_lightmap};
pub use native_render::world::keep_draws;
// The stage decoder moved to native-render, so the editor decodes stage
// images the same way (docs/editor/editor.md).
pub use native_render::stage_images::{StageDecoder, StageImages, StagePalettes};
#[cfg(test)]
use native_render::stage_images::StageRefusal;
use native_render::stage_images::{stage_image, with_blocks};
use native_render::look::{self, Stage};
use native_render::program::{self, MaterialProgram, ProgramSet, ViewInputs};
use native_render::skip::StencilPlan;
use native_render::world::{LightmapKey, Owner, World, WorldDraw};

type LayeredImages = HashMap<(TagId, u32), LayeredImage>;

#[path = "native_world_setup.rs"]
mod setup;
pub use setup::MapState;
use setup::StageReady;

/// One BSP's uploaded geometry. Each additional stage becomes visible at
/// a later frame boundary once its worker-built resources are complete.
#[derive(Default)]
pub struct Ready {
    retire: Option<crate::native_map::RetireQueue>,
    pub(super) sun_world: Option<native_render::sun_casters::SharedWorld>,
    gpu: Mutex<Option<GpuWorld>>,
    /// Published with the GPU bindings. A live edit clears it under the
    /// GPU lock. Testing scene ownership must never depend on lock contention.
    relit: AtomicBool,
    pub(crate) upload_device: Option<(wgpu::Device, wgpu::Queue)>,
    /// The parts the native pass draws, with their shaders: what the skip
    /// clears.
    parts: Arc<PartSet>,
    /// Resident layer-3 parts in each rigid placement or cluster, in section order.
    section_parts: HashMap<Owner, Vec<u8>>,
    /// Each drawn part's world bounds, what the guest's admission culls
    /// by; a live edit's refresh recomputes an edited section's
    /// ([`refresh_world`]).
    part_bounds: RwLock<HashMap<(Owner, u8), native_render::world::Aabb>>,
    /// The structure BSP tag this world was loaded from: what a live
    /// edit's `Section` names.
    bsp_tag: Option<TagId>,
    layer3: Layer3Materials,
    /// The lightmap pages that decoded at the load, which the program
    /// set holds: a refreshed program's parts may sample only these.
    pages: HashSet<LightmapKey>,
    /// The fog of this load (`native_fog_glue`): the compiled layer-16
    /// passes, their fog words and the parts the fog pass draws.
    pub fog: OnceLock<(Arc<crate::native_fog_glue::FogView>, StagePalettes)>,
    /// The water edge blend of this load (`native_water_glue`): the
    /// compiled layer-6 passes, their supplies and the parts the water
    /// pass draws.
    pub water: OnceLock<(Arc<crate::native_water_glue::WaterData>, StagePalettes)>,
    /// The BSP ocean of this load (`native_ocean_glue`): what its layer-3
    /// pass compiles from, or why it cannot.
    pub ocean: OnceLock<Arc<Result<crate::native_ocean_glue::OceanData, String>>>,
    /// The scene's layers 1, 2, 5, 7, 8, 17 and 20 of this load
    /// (`native_layers_glue`), when `frame-scene.layers` is placed.
    pub layers: OnceLock<(Arc<crate::native_layers_glue::LayersView>, StagePalettes)>,
    /// The model decorators' passes of this load (`native_decorators_glue`).
    pub decorators: OnceLock<Arc<crate::native_decorators_glue::DecoratorLooks>>,
    /// The materials of the map's skies (`native_sky_glue`): what the sky
    /// strip's constants are evaluated from.
    pub sky: OnceLock<Arc<crate::native_sky_glue::SkyMaterials>>,
    /// What the native lights draw over this world
    /// (`native_lights_glue`), once the light resource worker publishes.
    pub lights: OnceLock<Arc<crate::native_lights_glue::LightsWorld>>,
    /// The device palettes the load expanded palettized stage bitmaps
    /// through, at the stages it used them (the rest `None`): each ask
    /// checks they are still bound.
    pub palettes: StagePalettes,
    /// The BSP's lightmap bitmap and the palettes its pages expand
    /// through, when it has pages: what a live edit of a page decodes
    /// again ([`decode_pages`]).
    lightmaps: Option<(TagId, Vec<[u32; 256]>)>,
}

impl Drop for Ready {
    fn drop(&mut self) {
        // Detach the sender first. The empty shell here and the worker's
        // payload both drop normally without queueing another retirement.
        if let Some(send) = self.retire.take() {
            crate::native_map::retire(send, timeline::setup::Stage::World, self);
        }
    }
}

impl Ready {
    fn owns_display_lightmaps(&self) -> bool {
        self.relit.load(Ordering::Acquire)
    }

    pub(crate) fn gpu(&self) -> std::sync::MutexGuard<'_, Option<GpuWorld>> {
        self.gpu.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The GPU world with the renderer's device and scene formats, for a
    /// stage's prewarm before any guest mask is cleared. A busy GPU world
    /// prepares nothing.
    pub(crate) fn with_pipeline_target<T>(
        &self,
        prepare: impl FnOnce(&mut GpuWorld, &wgpu::Device, wgpu::TextureFormat, wgpu::TextureFormat) -> Option<T>,
    ) -> Option<T> {
        let (device, color, depth) = pipeline_target();
        let mut guard = self.gpu.try_lock().ok()?;
        prepare(guard.as_mut()?, &device, color, depth)
    }
}

/// The renderer's device and queue, set once when the renderer is
/// installed (`render_glue::install_game`).
static RENDER_DEVICE: OnceLock<(wgpu::Device, wgpu::Queue)> = OnceLock::new();

/// Records the renderer's device and queue for the loads and stages.
pub(crate) fn set_render_device(device: wgpu::Device, queue: wgpu::Queue) {
    let _ = RENDER_DEVICE.set((device, queue));
}

/// The renderer's device and its scene colour and depth formats.
pub(crate) fn pipeline_target() -> (wgpu::Device, wgpu::TextureFormat, wgpu::TextureFormat) {
    let (device, _) = upload_device();
    use native_render::frame_graph::targets::{COLOUR_FORMAT, DEPTH_FORMAT};
    (device, COLOUR_FORMAT, DEPTH_FORMAT)
}

/// The renderer's device and queue, which a load uploads on.
pub(crate) fn upload_device() -> (wgpu::Device, wgpu::Queue) {
    RENDER_DEVICE.get().cloned().unwrap_or_else(|| unimplemented!("world: a load with no renderer installed"))
}

/// The active BSP. The index and activation distinguish reused addresses
/// within a map and a reload of the same map.
#[derive(Clone, PartialEq, Eq)]
pub struct BspKey {
    map: String,
    bsp: u32,
    index: u16,
    generation: u64,
    bsp_generation: u64,
}

impl BspKey {
    /// The name of the map this world loaded from: what a glue that
    /// decodes a live edit's images again opens.
    pub(crate) fn map(&self) -> &str {
        &self.map
    }

    /// Whether the world this key made is still the loaded one.
    pub(crate) fn current(&self) -> bool {
        self.generation == native_map_work::generation()
            && self.bsp_generation == native_map_work::bsp_generation()
    }
}

pub type State = Arc<Mutex<MapState>>;

/// Match the guest's active BSP even if it changed before the next layer 3.
pub fn current_ready<'a>(state: &'a MapState, mem: &dyn GuestMemory) -> Option<&'a Arc<Ready>> {
    let ready = state.ready.as_ref()?;
    let key = state.key.as_ref()?;
    (key.current()
        && mem.u32(render_bridge::native_world::ACTIVE_BSP).ok() == Some(key.bsp)
        && active_bsp_index(mem) == Some(key.index)
        && crate::health::read_map_name(mem, &mut [0; 32]).ok().as_deref() == Some(key.map.as_str()))
        .then_some(ready)
}

/// [`current_ready`] with its key, for a glue that applies a live edit to
/// the world's copies on a worker (it opens the key's map and checks
/// [`BspKey::current`] before it writes).
pub fn current_ready_keyed(state: &MapState, mem: &dyn GuestMemory) -> Option<(Arc<Ready>, BspKey)> {
    let ready = current_ready(state, mem)?;
    Some((Arc::clone(ready), state.key.clone()?))
}

/// The host: the current world.
pub struct Glue {
    state: State,
    switches: hooks::Switches,
    relight_requested: bool,
    /// A live edit's refresh is running ([`refresh_world`]); edits wait
    /// in the queue until it ends, so they apply in order.
    refreshing: Arc<AtomicBool>,
}

impl Host for Glue {
    fn retire_bsp(&self) {
        self.retire_stale_bsp();
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
        let key = BspKey { map, bsp, index, generation, bsp_generation: native_map_work::bsp_generation() };
        let (finished, pending) = mpsc::channel();
        let retired = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !key.current() {
                return false;
            }
            if state.key.as_ref() == Some(&key) {
                return true;
            }
            std::mem::replace(&mut *state, MapState::loading(key.clone(), pending))
        };
        retired.retire();
        let device = upload_device();
        let palettes = scene_load_palettes(mem);
        crate::native_objects_glue::install_function_tables(mem);
        let relight_requested = self.relight_requested;
        let launched = background::spawn("native-world load", move || {
            if let Err(e) = load_ready(&key, device, palettes, relight_requested, &finished)
                && key.current()
            {
                eprintln!("native-world: {} BSP {} at {:#010x}: not loaded ({e})", key.map, key.index, key.bsp);
            }
        });
        if let Err(error) = launched {
            eprintln!("native-world: could not start map loader ({error})");
            self.state.lock().unwrap_or_else(|e| e.into_inner()).key = None;
            return false;
        }
        true
    }

    fn begin_frame(&self) {
        let retired = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let retired = state.take_stale();
            state.publish();
            retired
        };
        if let Some(retired) = retired {
            retired.retire();
        }
        self.refresh_edits();
    }

    fn ask(&self, mem: &mut dyn GuestMemory, bsp: u32, scene_view: &frame_scene::View, plan: &StencilPlan, claim_guest: bool) -> Result<native_render::skip::Cleared, &'static str> {
        let ready = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            Arc::clone(current_ready(&state, mem).ok_or("world_pending")?)
        };
        // Everything here is extraction: the guest reads and the checks
        // that can refuse, which decide what the guest skips. The render
        // thread binds the worker-built resources and records the stage.
        let planning = timeline::native::Timer::start(timeline::native::Stage::World, timeline::native::Metric::ExtractNs);
        // The palettes the load expanded its palettized stage bitmaps
        // through must still be the ones bound.
        if !palettes_hold(&ready.palettes, &|s| stage_palette(&*mem, s)) {
            return Err("palettes");
        }
        // The stencil the layer's draws take now, and how its items switch
        // it (`skip::stencil_plan`).
        let stencil = layer_stencil(&*mem).ok_or("stencil")?;
        let drained_plan = native_render::skip::stencil_plan(&render_bridge::native_world::Mem(mem), native_render::skip::LIGHTMAP_LAYER)
            .ok_or("stencil_plan")?;
        if &drained_plan != plan { return Err("stencil_plan"); }
        let stencil = pass_stencil(&stencil, plan).ok_or("stencil")?;
        // The cull layer 3 starts in: what the passes that do not set
        // `CULLMODE` inherit (`native_render::look::WorldPass::inherits_cull`).
        let cull = layer_cull(&*mem).map(inherited_cull);
        let factor = projection_factor();
        let cull_camera = native_render::camera::display_camera(scene_view.camera, factor);
        let frustum = GpuWorld::draw_frustum(&cull_camera.view_projection(), true);
        let view = mem.u32(engine::render_loop::occlusion::guest::VIEW_INDEX).unwrap_or(u32::MAX);
        let run = crate::presents::presents().wrapping_add(1);
        let (device, color, depth) = pipeline_target();
        let time = native_render::layer_pass::render_time(&ReadMemory(mem));
        let accumulation = accumulation_target(&ReadMemory(mem));
        // The parts' selection, or why it does not draw: the GPU world not
        // uploaded, a part or texaccum it cannot bind, an animation that
        // does not read, pipelines still warming.
        let select = |parts: &[(Owner, u8)]| -> Result<Arc<WorldSelection>, &'static str> {
            let mut guard = ready.gpu();
            let gpu = guard.as_mut().ok_or("gpu_pending")?;
            configure_world(gpu, depth, cull, &stencil);
            // The guest's list whole: the GPU culls it by the lens.
            let mut selection = gpu.select_unculled(parts).ok_or("selection")?;
            if !gpu.select_texaccum(&mut selection, accumulation) { return Err("texaccum"); }
            gpu.animate_selection(&mut selection, time).map_err(|_| "animation")?;
            gpu.prewarm_world(&device, color, depth, &selection).then(|| Arc::new(selection)).ok_or("warming")
        };
        let push = |selection| {
            native_render::stage::frame_builder().push(native_render::stage::FrameOp::Stage {
                slot: frame_types::SlotAt { slot: frame_types::Slot::Opaque, view: view as u8 },
                stage: Box::new(WorldPass {
                    selection,
                    ready: Arc::clone(&ready),
                    camera: scene_view.camera,
                    cull_camera,
                    stencil: stencil.clone(),
                    cull,
                    token: view_token(run, view),
                }),
            });
        };
        // Sections only the wider display lens sees, flooded in.
        if factor < 1.0 && let Some(visibility) = &scene_view.display_visibility && (!visibility.clusters.is_empty() || !visibility.placements.is_empty()) {
            let sections = visibility.clusters.iter().map(|&id| Owner::Cluster(u32::from(id)))
                .chain(visibility.placements.iter().copied().filter(|&id| {
                    engine::render_loop::world_walk::fully_visible_placement(mem, bsp, id) == Some(true)
                }).map(|id| Owner::Instance(u32::from(id))));
            let parts = display_section_parts(sections, &ready.section_parts,
                &ready.part_bounds.read().unwrap_or_else(|e| e.into_inner()), &frustum);
            // Whole sections only, and none that samples an accumulation
            // no stage wrote (`GpuWorld::display_complete_parts`).
            let parts = ready.gpu().as_ref().map(|gpu| gpu.display_complete_parts(&parts)).unwrap_or_default();
            if !parts.is_empty() && let Ok(selection) = select(&parts) {
                push(selection);
            }
        }
        if !claim_guest { return Err("display_only"); }
        let (masks, cleared, selected) = {
            let bounds = ready.part_bounds.read().unwrap_or_else(|e| e.into_inner());
            render_bridge::native_world::plan_parts(
                // Every loaded part of the guest's list: the GPU culls them.
                &render_bridge::native_world::Mem(mem), &ready.parts, |owner, part| {
                    bounds.contains_key(&(owner, part))
                },
            )
            .ok_or("masks")?
        };
        if selected.is_empty() { return Err("part"); }
        let counted = timeline::phase::enabled().then(|| selected.iter().map(|p| p.0).collect::<Vec<_>>());
        let selection = select(&selected)?;
        drop(planning);
        if !masks.apply(&mut render_bridge::native_world::Mem(mem)) {
            timeline::native::Stage::World.count(timeline::native::Metric::RefusedMasks, selected.len() as u64);
            return Err("masks");
        }
        push(selection);
        if let Some(owners) = counted {
            crate::native_world_layers_glue::count_frontend(run, view, bsp,
                timeline::native::WorldOpaque::Prereserved, owners);
        }
        Ok(cleared)
    }
}

impl Glue {
    fn retire_stale_bsp(&self) {
        let retired = self.state.lock().unwrap_or_else(|e| e.into_inner()).take_stale();
        if let Some(retired) = retired {
            retired.retire();
        }
    }
}

impl Glue {
    /// Starts a live edit's refresh of the current world
    /// (docs/editor/realtime-edits.md, "The native refresh") once the
    /// world is resident and no refresh is running: the queued edits are
    /// decoded again from the map file, which the raw overlay serves
    /// edited, on a worker, and applied under the world's GPU lock
    /// ([`refresh_world`]). While the world loads they wait: the load
    /// may have read the file before the edit.
    fn refresh_edits(&self) {
        use render_bridge::native_edits::{self, Consumer};
        if !native_edits::pending(Consumer::World) || self.refreshing.load(Ordering::Acquire) {
            return;
        }
        let (ready, key) = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            match (state.ready.as_ref(), state.key.as_ref()) {
                (Some(ready), Some(key)) if key.current() => (Arc::clone(ready), key.clone()),
                _ => return,
            }
        };
        let edits = native_edits::drain(Consumer::World);
        if edits.is_empty() {
            return;
        }
        self.refreshing.store(true, Ordering::Release);
        let refreshing = Arc::clone(&self.refreshing);
        let launched = background::spawn("native-world edits", move || {
            refresh_world(&key, &ready, &edits);
            refreshing.store(false, Ordering::Release);
        });
        if let Err(e) = launched {
            self.refreshing.store(false, Ordering::Release);
            eprintln!("native-world: could not start the live edit refresh ({e}); the world reloads");
            native_map_work::reload_bsp();
        }
    }
}

/// What one drain of live edits asks of the world loaded from BSP tag
/// `bsp` ([`refresh_plan`]).
#[derive(Debug, Default, PartialEq)]
struct RefreshPlan {
    /// Why the world reloads instead, if it does.
    reload: Option<String>,
    /// Sections of this world replaced in place.
    sections: Vec<load::SectionOwner>,
    /// Images whose textures are rewritten in place.
    images: Vec<(TagId, u32)>,
    /// Shader, template, pass and vertex shader tags whose programs are
    /// refreshed ([`refresh_shaders`]).
    shaders: Vec<u32>,
}

/// Sorts `edits` for the world loaded from `bsp`: a section of it is
/// replaced in place, or reloads the world when its counts grew (or its
/// kind is unknown); another BSP's section waits for that BSP's own load,
/// which reads the edited file; an image is rewritten; an edited shader
/// refreshes the programs built from it. Tags match by index, as
/// `native_map::tag_of` does.
fn refresh_plan(bsp: Option<TagId>, edits: &[render_bridge::native_edits::Edit]) -> RefreshPlan {
    use render_bridge::native_edits::{Edit, KIND_CLUSTER, KIND_INSTANCE_DEF};
    let ours = |tag: u32| bsp.is_some_and(|b| b.0 & 0xffff == tag & 0xffff);
    let mut plan = RefreshPlan::default();
    for &edit in edits {
        match edit {
            Edit::Section { bsp, .. } if !ours(bsp) => {}
            Edit::Section { kind, index, grown: true, .. } => {
                plan.reload.get_or_insert(format!("section {kind}:{index} changed its counts"));
            }
            Edit::Section { kind: KIND_CLUSTER, index, .. } => plan.sections.push(load::SectionOwner::Cluster(index)),
            Edit::Section { kind: KIND_INSTANCE_DEF, index, .. } => {
                plan.sections.push(load::SectionOwner::InstanceDefinition(index));
            }
            Edit::Section { kind, index, .. } => {
                plan.reload.get_or_insert(format!("section {kind}:{index} is of an unknown kind"));
            }
            Edit::Image { bitmap, image } => plan.images.push((TagId(bitmap), image)),
            Edit::Shader { tag } => plan.shaders.push(tag),
            // Models are the objects' copy. Another tag's field write
            // changes nothing this copy reads from metadata live.
            Edit::Model { .. } | Edit::Tag { .. } => {}
        }
    }
    plan
}

/// The pages of the BSP's lightmap bitmap that `images` edits, each
/// decoded again through every palette the load held, as the load decodes
/// them ([`decode_lightmap`]); a page that does not decode says why.
/// Another bitmap's image is not a page.
fn decode_pages<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>, files: &mut assets::ResourceFiles, ready: &Ready, images: &[(TagId, u32)],
) -> Vec<(LightmapKey, Result<Image, String>)> {
    let Some((bitmap, palettes)) = ready.lightmaps.as_ref() else { return Vec::new() };
    let edited: Vec<u16> = images
        .iter()
        .filter(|(tag, _)| tag.0 & 0xffff == bitmap.0 & 0xffff)
        .filter_map(|&(_, image)| u16::try_from(image).ok())
        .collect();
    if edited.is_empty() {
        return Vec::new();
    }
    let read = tags::DecodedTag::read(map, *bitmap)
        .map_err(|e| format!("the BSP lightmap bitmap does not read ({e})"))
        .and_then(|tag| match tag.data {
            tags::TagData::Bitmap(b) => Ok(b),
            _ => Err("the BSP lightmap tag is not a bitmap".to_owned()),
        });
    let mut out = Vec::new();
    for page in edited {
        let image = read
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|b| b.images.get(usize::from(page)).ok_or_else(|| "no such lightmap bitmap page".to_owned()));
        // A colour page (the converted Halo 1 pages) has no palette.
        if image.as_ref().is_ok_and(|i| i.format != tags::PixelFormat::P8) {
            let decoded = image.and_then(|i| decode_lightmap(i, None, files, true));
            out.push((LightmapKey { page, palette: None }, decoded));
            continue;
        }
        for (n, palette) in palettes.iter().enumerate() {
            let Ok(palette_index) = u8::try_from(n) else { break };
            let decoded = image.clone().and_then(|i| decode_lightmap(i, Some(palette), files, true));
            out.push((LightmapKey { page, palette: Some(palette_index) }, decoded));
        }
    }
    out
}

/// Applies one drain of live edits to `ready`, the world of `key`: the
/// sections decoded again and replaced in place with their bounds
/// recomputed (the GPU world's and the admission's), the images decoded
/// again at each stage that samples them and their textures rewritten.
/// A section the world cannot take in place reloads the world
/// (`native_map_work::reload_bsp`): until the reload publishes, its parts
/// keep their guest draws from guest memory. An image whose size, levels or
/// format changed keeps its pixels until the map reloads, and says so.
/// Edited shaders go first ([`refresh_shaders`]).
fn refresh_world(key: &BspKey, ready: &Ready, edits: &[render_bridge::native_edits::Edit]) {
    let reload = |why: String| {
        eprintln!("native-world: {} BSP {}: a live edit reloads the world ({why})", key.map, key.index);
        native_map_work::reload_bsp();
    };
    let plan = refresh_plan(ready.bsp_tag, edits);
    if ready.owns_display_lightmaps()
        && (plan.reload.is_some() || !plan.sections.is_empty() || !plan.shaders.is_empty() || !plan.images.is_empty())
    {
        let mut guard = ready.gpu();
        if let Some(gpu) = guard.as_mut() {
            gpu.discard_relight();
        }
        ready.relit.store(false, Ordering::Release);
        eprintln!("native-world: {} BSP {}: live edit invalidates reconstructed pages, using retail", key.map, key.index);
    }
    if let Some(why) = &plan.reload {
        return reload(why.clone());
    }
    if !plan.shaders.is_empty() {
        if let Err(why) = refresh_shaders(key, ready, &plan.shaders) {
            return reload(why);
        }
    }
    if plan.sections.is_empty() && plan.images.is_empty() {
        return;
    }
    let Some((device, queue)) = ready.upload_device.as_ref() else { return };
    let (mut map, files) = match crate::native_map::open(&key.map) {
        Ok(opened) => opened,
        Err(e) => return reload(format!("the map does not open: {e}")),
    };
    let mut raw = Raw(files);
    let sections = match ready.bsp_tag.filter(|_| !plan.sections.is_empty()) {
        Some(bsp) => match load::load_sections(&mut map, &mut raw, bsp, &plan.sections) {
            Ok(sections) => sections,
            Err(e) => return reload(e.to_string()),
        },
        None => Vec::new(),
    };
    // Which stages sample each image, read under the lock; the decodes
    // run without it.
    let stages: Vec<((TagId, u32), Vec<usize>)> = {
        let gpu = ready.gpu();
        let Some(gpu) = gpu.as_ref() else { return };
        plan.images.iter().map(|&(bitmap, image)| ((bitmap, image), gpu.image_stages(bitmap, image))).collect()
    };
    let decoded: Vec<_> = stages
        .into_iter()
        .map(|((bitmap, image), at)| {
            ((bitmap, image), native_render::stage_images::decode_stages(&mut map, &mut raw.0, &ready.palettes, bitmap, image, &at))
        })
        .collect();
    let pages = decode_pages(&mut map, &mut raw.0, ready, &plan.images);
    // A gel is image 0 of its bitmap, uploaded as the lights' load decoded
    // it (no palette).
    let gels: Vec<(TagId, Image)> = plan
        .images
        .iter()
        .filter(|&&(bitmap, image)| image == 0 && ready.lights.get().is_some_and(|l| l.has_gel(bitmap.0)))
        .filter_map(|&(bitmap, _)| Some((bitmap, light_image(&mut map, &mut raw.0, bitmap, 0, 0, &None)?.0)))
        .collect();
    if !key.current() {
        return;
    }
    let mut guard = ready.gpu();
    let Some(gpu) = guard.as_mut() else { return };
    let mut edited = HashSet::new();
    for (owner, input) in &sections {
        match gpu.replace_section(queue, *owner, input) {
            Ok(_) => {
                edited.insert(*owner);
            }
            // It did not decode at load: the guest draws it.
            Err(native_render::world::SectionRefusal::NoSection) => {}
            Err(why) => {
                drop(guard);
                return reload(format!("{owner:?}: {why:?}"));
            }
        }
    }
    for ((bitmap, image), pixels) in &decoded {
        if let Err(why) = gpu.rewrite_image(queue, *bitmap, *image, pixels) {
            eprintln!(
                "native-world: {} BSP {}: bitmap {:#010x} image {image} keeps its pixels until the map reloads ({why:?})",
                key.map, key.index, bitmap.0
            );
        }
    }
    for (page, decoded) in &pages {
        match decoded {
            Ok(pixels) => {
                if let Err(why) = gpu.rewrite_page(queue, *page, pixels) {
                    eprintln!(
                        "native-world: {} BSP {}: lightmap page {} palette {:?} keeps its pixels until the map reloads ({why:?})",
                        key.map, key.index, page.page, page.palette
                    );
                }
            }
            // It did not decode at load either, or no longer does: the
            // the guest draws what used it.
            Err(why) if !gpu.page_textures(*page).is_empty() => {
                eprintln!(
                    "native-world: {} BSP {}: lightmap page {} palette {:?} does not decode ({why}); it keeps its pixels",
                    key.map, key.index, page.page, page.palette
                );
            }
            Err(_) => {}
        }
    }
    for (bitmap, pixels) in &gels {
        if let Err(why) = gpu.rewrite_gel(device, queue, bitmap.0, pixels) {
            eprintln!(
                "native-world: {} BSP {}: gel {:#010x} keeps its pixels until the map reloads ({why:?})",
                key.map, key.index, bitmap.0
            );
        }
    }
    if !edited.is_empty() {
        let mut bounds = ready.part_bounds.write().unwrap_or_else(|e| e.into_inner());
        for d in gpu.world().draws.iter().filter(|d| edited.contains(&d.owner)) {
            bounds.insert((d.owner, d.part), d.bounds);
        }
    }
    drop(guard);
    // Flush the writes before the next admitted draw.
    queue.submit([]);
}

/// Whether shader `shader` is built from one of the `edited` tags (matched
/// by index): it is one, or its material uses one
/// (`tags::material::Material::uses`). A shader that no longer resolves
/// counts as built from them: the edit may be what broke it. Each answer
/// is kept in `known`.
pub(crate) fn shader_edited(
    lib: &mut tags::material::ShaderLibrary,
    known: &mut HashMap<TagId, bool>,
    edited: &[TagId],
    shader: TagId,
) -> bool {
    if let Some(&known) = known.get(&shader) {
        return known;
    }
    let same = edited.iter().any(|t| t.0 & 0xffff == shader.0 & 0xffff);
    let uses = same || lib.material(shader, None).map(|m| edited.iter().any(|&t| m.uses(t))).unwrap_or(true);
    known.insert(shader, uses);
    uses
}

/// The world materials (by index into `shaders`) whose shader `edited`
/// says is built from an edited tag, ascending.
fn edited_materials(shaders: &[TagId], edited: &mut dyn FnMut(TagId) -> bool) -> Vec<u16> {
    shaders
        .iter()
        .enumerate()
        .filter_map(|(m, &shader)| u16::try_from(m).ok().filter(|_| edited(shader)))
        .collect()
}

/// Refreshes the world's material programs built from the edited
/// `tags` (`Edit::Shader`: shaders, templates, passes, vertex shaders;
/// docs/editor/realtime-edits.md, "The native refresh"). The shaders are
/// read again from the map file, which the metadata overlay serves
/// edited; each native material built from an edited tag is compiled
/// again as the load compiled it and put in place
/// (`GpuWorld::refresh_programs`): its uniform buffers rewritten when
/// only constants changed, its programs rebuilt otherwise. Materials the
/// load left on the guest stay there; their draws read the edit from
/// guest memory. Err says why the world reloads instead: another of this
/// load's consumers (the fog, the water, the layers, the lights, the
/// decorators or the opaque pass) draws an edited shader, or a refreshed
/// material would draw other parts than it does.
fn refresh_shaders(key: &BspKey, ready: &Ready, tags: &[u32]) -> Result<(), String> {
    let Some((device, queue)) = ready.upload_device.as_ref() else { return Ok(()) };
    let (mut map, mut files) = crate::native_map::open(&key.map).map_err(|e| format!("the map does not open: {e}"))?;
    let mut lib = tags::material::ShaderLibrary::read(&mut map).map_err(|e| format!("the shader tags do not read ({e})"))?;
    let edited: Vec<TagId> = tags.iter().map(|&t| TagId(t)).collect();
    let mut known = HashMap::new();
    let affected = edited_materials(&ready.layer3.shaders, &mut |s| shader_edited(&mut lib, &mut known, &edited, s));
    if affected.is_empty() {
        return Ok(());
    }

    // The consumers that compile their own copy of a shader reload with
    // the world.
    let mut users = Vec::new();
    let draws = |programs: &dyn Fn(u16) -> bool| affected.iter().any(|&m| programs(m));
    if ready.fog.get().is_some_and(|(fog, _)| draws(&|m| fog.programs.contains_key(&m))) {
        users.push("the fog");
    }
    if ready.water.get().is_some_and(|(water, _)| draws(&|m| water.programs.contains_key(&m))) {
        users.push("the water");
    }
    if ready.layers.get().is_some_and(|(view, _)| draws(&|m| view.layers.values().any(|d| d.programs.contains_key(&m)))) {
        users.push("the layers");
    }
    if ready.lights.get().is_some_and(|lights| draws(&|m| lights.draws_material(m))) {
        users.push("the lights");
    }
    if let Some(decorators) = ready.decorators.get() {
        let ids: Vec<TagId> = decorators
            .shaders()
            .into_iter()
            .filter_map(|index| map.tags().get(usize::from(index)).map(|t| t.id))
            .collect();
        if ids.into_iter().any(|s| shader_edited(&mut lib, &mut known, &edited, s)) {
            users.push("the decorators");
        }
    }
    if !users.is_empty() {
        return Err(format!("{} draw an edited shader", users.join(", ")));
    }

    // The native materials among them, and the parts they draw.
    let (world, native, before) = {
        let gpu = ready.gpu();
        let Some(gpu) = gpu.as_ref() else { return Ok(()) };
        let native: HashSet<u16> = affected.iter().copied().filter(|&m| gpu.material_program(m).is_some()).collect();
        let w = gpu.world();
        let world = World {
            draws: w.draws.iter().filter(|d| native.contains(&d.material)).cloned().collect(),
            batches: w.batches.iter().filter(|b| native.contains(&b.material)).cloned().collect(),
            placements: w.placements.clone(),
            ..Default::default()
        };
        let before: HashSet<(Owner, u8)> = world.draws.iter().map(|d| (d.owner, d.part)).collect();
        (world, native, before)
    };
    if native.is_empty() {
        return Ok(());
    }
    let page_ready = |page| {
        if ready.pages.contains(&page) {
            Ok(())
        } else {
            Err("a part's lightmap page does not decode (page unavailable)".to_owned())
        }
    };
    let mut decoder = StageDecoder::new(ready.palettes.clone());
    let mut layered = LayeredImages::new();
    let out = material_programs(&mut map, &mut files, &ready.layer3, &world, &page_ready, &mut decoder,
        &mut layered, key, Some(&native));
    if let Some((why, _)) = out.refused.iter().next() {
        return Err(format!("an edited material no longer compiles ({why})"));
    }
    if out.parts != before {
        return Err("an edited material would draw other parts".into());
    }
    if !key.current() {
        return Ok(());
    }
    let images = out.images;
    let mut guard = ready.gpu();
    let Some(gpu) = guard.as_mut() else { return Ok(()) };
    let done = gpu.refresh_programs(device, queue, out.programs, &mut |stage| match stage {
        Stage::Bitmap { bitmap, image } => images.get(&(bitmap, image)).cloned(),
        _ => None,
    }, &layered)?;
    drop(guard);
    // Flush the writes before the next admitted draw.
    queue.submit([]);
    eprintln!(
        "native-world: {} BSP {}: a shader edit refreshed {} materials' constants and rebuilt {} ({} unchanged)",
        key.map, key.index, done.constants.len(), done.rebuilt.len(), done.unchanged.len()
    );
    Ok(())
}

/// Installs the world host and the other native stages.
pub fn install() {
    let state: State = Arc::default();
    let switches = hooks::Switches::from_env().unwrap_or_default();
    render_bridge::native_world::set_host(Arc::new(Glue {
        state: Arc::clone(&state),
        relight_requested: native_render::lighting::lighting().is_ok_and(native_render::lighting::Lighting::is_relight),
        switches,
        refreshing: Arc::default(),
    }));
    crate::native_transparents_glue::install(Arc::clone(&state));
    crate::native_fog_glue::install(Arc::clone(&state));
    crate::native_water_glue::install(Arc::clone(&state));
    crate::native_ocean_glue::install(Arc::clone(&state));
    crate::native_layers_glue::install(Arc::clone(&state));
    crate::native_decorators_glue::install(Arc::clone(&state));
    crate::native_sky_glue::install(Arc::clone(&state));
    crate::native_gels_glue::install();
    crate::native_lights_glue::install(state);
    // The post loads its own screen effects (`native_post_glue::EffectsSlot`).
    crate::native_post_glue::install();
    crate::native_shadows_glue::install();
    crate::native_hud_glue::install();
    #[cfg(feature = "movie-playback")]
    crate::movie_display_glue::install();
}

/// The flood supplies newly visible clusters and fully opaque rigid placements.
fn display_section_parts(
    sections: impl Iterator<Item = Owner>, resident: &HashMap<Owner, Vec<u8>>,
    bounds: &HashMap<(Owner, u8), native_render::world::Aabb>, frustum: &native_render::cull::Frustum,
) -> Vec<(Owner, u8)> {
    let mut selected = Vec::new();
    for owner in sections {
        for &part in resident.get(&owner).into_iter().flatten() {
            if bounds.get(&(owner, part)).is_some_and(|b| frustum.may_see(b)) {
                selected.push((owner, part));
            }
        }
    }
    selected
}

/// The accumulation level layer 3 samples (target 1), when the bound
/// surfaces hold one of the descriptor's size.
fn accumulation_target(_mem: &ReadMemory<'_>) -> Option<native_render::world_layers::Texaccum> {
    unimplemented!("world: accumulation level from the bound targets")
}

/// One view's world pass.
struct WorldPass {
    selection: Arc<WorldSelection>,
    ready: Arc<Ready>,
    camera: RenderCamera,
    /// The lens the GPU culls the selection by (`display_camera` at the
    /// view's projection factor).
    cull_camera: RenderCamera,
    /// The stencil the pass draws with ([`pass_stencil`]).
    stencil: (gfx::state::StencilState, Vec<Owner>),
    /// The cull at the layer-3 call ([`inherited_cull`]; `None`: it did
    /// not read).
    cull: Option<native_render::program::Cull>,
    /// This game view in this frame ([`view_token`]): its later layers'
    /// culls read the pyramid its cull builds.
    token: u64,
}

/// A game view in one frame, for the pyramid its layer 3 builds
/// (`GpuWorld::culled_selection_draws`) and its later layers read
/// (`GpuWorld::culled_layer_draws`): the frame's run and the guest's view
/// index.
pub(crate) fn view_token(run: u64, view: u32) -> u64 {
    (run << 32) | u64::from(view)
}

/// `CULLMODE` and `FRONTFACE` as a pipeline takes them
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

fn configure_world(
    gpu: &mut GpuWorld, depth: wgpu::TextureFormat, cull: Option<native_render::program::Cull>,
    (state, marked): &(gfx::state::StencilState, Vec<Owner>),
) {
    gpu.set_inherited_cull(native_render::program::InheritedCull {
        device: cull,
        placements: true,
        clusters: true,
    });
    let mut stencil = PassStencil::from_device(state, depth);
    if stencil != PassStencil::default() {
        stencil.marked = marked.clone();
    }
    gpu.set_stencil(stencil);
}

/// The stencil state layer 3 starts in: the guest's render-state shadow
/// (`native_targets_glue::stencil_state`). `None` when it does not read.
fn layer_stencil(mem: &dyn GuestMemory) -> Option<gfx::state::StencilState> {
    crate::native_targets_glue::stencil_state(mem)
}

/// The `CULLMODE` and `FRONTFACE` layer 3 starts in, from the guest's
/// render-state shadow (`native_targets_glue::cull_state`).
fn layer_cull(mem: &dyn GuestMemory) -> Option<(gfx::state::CullMode, gfx::state::FrontFace)> {
    crate::native_targets_glue::cull_state(mem)
}

/// The display lens over the retail one, for the one copy drawn: the
/// horizontal factor the present camera's views carry
/// (`render_glue::ui_state`, as the retail callbacks last set it).
pub(crate) fn projection_factor() -> f32 {
    crate::render_glue::ui_state().projection_factor
}

/// The view's rectangle in guest pixels: the whole target, as there is
/// no guest viewport.
pub(crate) fn view_rect(view: &ViewInputs) -> (u32, u32, u32, u32) {
    (0, 0, view.guest_size.0, view.guest_size.1)
}

/// The viewport `(x, y, width, height)` in `texels` of a view whose
/// rectangle is in guest pixels of `view.guest_size`.
pub(crate) fn stage_viewport(view: &native_render::program::ViewInputs, texels: (u32, u32)) -> [f32; 4] {
    let Some(scale) = gfx::raster::RenderScale::between(view.guest_size, texels) else {
        return [0.0; 4];
    };
    let (x, y, w, h) = view_rect(view);
    scale.rect(x, y, w, h).map(|n| n as f32)
}

impl native_render::stage::Stage for WorldPass {
    fn name(&self) -> &'static str { "world" }

    /// The material programs' parts as commands for the frame's encoder:
    /// the view rows and the guest cull's inputs copied in draw order, the
    /// cull (`GpuWorld::culled_selection_draws`), then the packets in a
    /// pass that loads and keeps the attachments.
    fn record(
        &mut self, t: &gfx::external::ExternalAttachments, request: &native_render::program::ViewInputs,
    ) -> native_render::stage::StageWork {
        let mut guard = self.ready.gpu();
        let Some(gpu) = guard.as_mut() else { unimplemented!("world: geometry not uploaded at the stage") };
        if !gpu.has_programs() { unimplemented!("world: fixed stage with no material programs") }
        configure_world(gpu, t.depth_format, self.cull, &self.stencil);
        let selection = &self.selection;
        if !gpu.prepare_selection(&t.device, t.color_format, t.depth_format, selection) {
            unimplemented!("world: part or program binding unavailable");
        }
        let view = ViewInputs { size: t.size, ..*request };
        if selection.texaccum.is_some() {
            unimplemented!("world: accumulation level from the targets");
        }
        // The pyramid's screen: the lens into the request's rectangle.
        let lens = native_render::camera::display_camera(self.camera, projection_factor());
        let [x, y, width, height] = stage_viewport(request, t.size);
        let occlusion = Some((t.depth.texture(), &lens, PassViewport { x, y, width, height }, self.token));
        let culled = gpu.culled_selection_draws(&t.uploads, &t.device, &t.queue, t.color_format, t.depth_format, t.size,
            selection, &view, &self.cull_camera, t.lighting, None, occlusion);
        let encoded = culled.map(|culled| encoded_world(t, culled.draws, culled.late, culled.dispatch));
        match encoded {
            Ok(e) => native_render::stage::StageWork::Encoded(e),
            Err(why) => unimplemented!("world: {why}"),
        }
    }
}

/// A game view's culled world parts as commands for the frame's encoder
/// ([`WorldPass::record`]): the view rows staged now and copied in
/// draw order, the early cull, the early packets in a pass over the
/// attachments (loaded and kept), then with the pyramid its build from
/// that depth, the late cull and the late packets in a second pass.
fn encoded_world(
    t: &gfx::external::ExternalAttachments, draws: gfx::external::ExternalDraws, late: Vec<gfx::RecordedDraw>,
    dispatch: native_render::cull_gpu::GuestDispatch,
) -> gfx::external::ExternalEncoded {
    let writes: Vec<_> = draws.writes.iter()
        .map(|(dst, offset, bytes)| (t.uploads.stage(&t.device, &t.queue, bytes), dst.clone(), *offset, bytes.len() as u64))
        .collect();
    let (color, depth) = (t.color.clone(), t.depth.clone());
    let has_stencil = t.depth_format.has_stencil_aspect();
    let size = t.size;
    let ranges = draws.ranges;
    let packets = draws.packets;
    let late = dispatch.has_late().then_some(late);
    let count = packets.len() + late.as_ref().map_or(0, Vec::len);
    let passes = 1 + usize::from(late.is_some());
    let draw = move |encoder: &mut wgpu::CommandEncoder, packets: &[gfx::RecordedDraw]| {
        fn keep<T>() -> wgpu::Operations<T> { wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("native world culled"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &color, depth_slice: None, resolve_target: None, ops: keep(),
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth,
                depth_ops: Some(keep()),
                stencil_ops: has_stencil.then(keep),
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_viewport(0.0, 0.0, size.0 as f32, size.1 as f32, 0.0, 1.0);
        // Checked when the packets were made: every pipeline bound, none
        // counted.
        let _ = gfx::renderer::encode_in_pass(&mut pass, packets);
    };
    let commands = move |encoder: &mut wgpu::CommandEncoder| {
        for ((src, at), dst, offset, len) in &writes {
            encoder.copy_buffer_to_buffer(src, *at, dst, *offset, *len);
        }
        dispatch.record(encoder);
        draw(encoder, &packets);
        if let Some(late) = &late {
            dispatch.record_late(encoder);
            draw(encoder, late);
        }
    };
    gfx::external::ExternalEncoded { ranges, draws: count, passes, commands: Box::new(commands) }
}

/// The stencil the native pass draws layer 3 with, from the device's state
/// as the layer starts (`now`) and how its items switch it (`plan`,
/// `0x0004d640`/`0x0004d720`): the state and the sections drawn with mode
/// 2's reference; `None` when that cannot be exact.
///
/// The native pass draws its parts before the layer's items and clears
/// them, so their switches do not run. Without any switch, every item
/// draws with `now`. With one, an item leaves mode 1, so `now` must be
/// mode 1 for the items left to the guest to see what they would have: then the
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

/// The palette bound at stage `stage` now, as the game records it
/// (`native_targets_glue::stage_palette`).
pub fn stage_palette(mem: &dyn GuestMemory, stage: usize) -> Option<Vec<u32>> {
    crate::native_targets_glue::stage_palette(mem, stage)
}

/// The palettes bound at stages 0..3 now.
pub fn stage_palettes(mem: &dyn GuestMemory) -> StagePalettes {
    std::array::from_fn(|s| stage_palette(mem, s))
}

/// Guest pointer to the palette header read by the scene's texture reset (0x00015b70).
const DEFAULT_SCENE_PALETTE: u32 = crate::native_targets_glue::DEFAULT_PALETTE;

/// A loading/UI boundary may not have the scene's palette bound yet.
/// The scene reset binds this palette; each native draw still checks its bindings.
pub fn scene_load_palettes(mem: &dyn GuestMemory) -> StagePalettes {
    let mut bound = stage_palettes(mem);
    supply_scene_palette(&mut bound, || palette_entries(mem, mem.u32(DEFAULT_SCENE_PALETTE).ok()?));
    bound
}

/// The entries of the palette whose header is `header`
/// (`native_targets_glue::palette_entries`).
fn palette_entries(mem: &dyn GuestMemory, header: u32) -> Option<Vec<u32>> {
    crate::native_targets_glue::palette_entries(mem, header)
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

/// The hook's guest memory, read-only.
pub struct ReadMemory<'a>(pub &'a dyn GuestMemory);

impl native_render::camera::GuestRead for ReadMemory<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> { self.0.u32(addr).ok() }
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
/// game globals' (`guest_layout::points::GAME_GLOBALS`) `i16` at `+2`, as
/// `0x0011c1b0` and `0x00137d40` index the block with it
/// (`live_telemetry::layout::GLOBALS_ACTIVE_BSP`). `None` when negative
/// or unread.
pub fn active_bsp_index(mem: &dyn GuestMemory) -> Option<u16> {
    let globals = mem.u32(guest_layout::points::GAME_GLOBALS.base).ok().filter(|&g| g != 0)?;
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

fn load_ready(
    key: &BspKey, (device, queue): (wgpu::Device, wgpu::Queue), palettes: StagePalettes,
    relight_requested: bool, finished: &mpsc::Sender<StageReady>,
) -> Result<(), load::Error> {
    use timeline::setup::Stage as Setup;
    let Some(_load) = crate::native_map::begin_load(Setup::World, || key.current()) else {
        return Ok(());
    };
    let started = std::time::Instant::now();
    let world_setup = Setup::World.worker();
    let _scope = mem_watch::scope("native.world.load");
    if !key.current() { return Ok(()); }
    let retire = crate::native_map::retire_queue()?;
    let (mut map, files) = crate::native_map::open(&key.map)?;
    let bsps = load::scenario_bsps(&mut map)?;
    let addresses: Vec<Option<u32>> = bsps
        .iter()
        .map(|(id, _)| map.tag(*id).ok().and_then(|t| t.location).map(|l| l.address))
        .collect();
    let index = active_scenario_bsp(&addresses, key.index, key.bsp).ok_or("the active BSP index is not a scenario BSP at the active BSP's address")?;
    let bsp_tag = bsps[index].0;
    let mut raw = Raw(files);
    let LoadedWorld {
        checksum,
        mut world,
        per_vertex_placements,
        shaders,
        lightmap_bitmap,
        lightmap_error,
        palettes: lightmap_palettes,
        failed_sections,
        ..
    } = load::load_world(&mut map, &mut raw, index)?;
    let mut files = raw.0;
    if !key.current() { return Ok(()); }

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
        for page_key in page_keys {
            if !key.current() { return Ok(()); }
            let page = bitmap.as_ref().map_err(Clone::clone).and_then(|b| {
                let image = b.images.get(usize::from(page_key.page)).ok_or("no such lightmap bitmap page")?;
                let palette = native_render::load::page_palette(&lightmap_palettes, page_key)?;
                decode_lightmap(image, palette, &mut files, true)
            });
            match page {
                Ok(page) => {
                    lightmaps.insert(page_key, page);
                }
                Err(why) => {
                    rejected_pages.insert(page_key, why);
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
    // The later shader walks use part descriptors and placement rows. Keep
    // those before filtering layer 3, without copying any vertex or index data.
    let stage_world = World {
        light_parts: world.light_parts.clone(),
        draws: world.draws.clone(),
        batches: world.batches.clone(),
        transparents: world.transparents.clone(),
        placements: world.placements.clone(),
        ..Default::default()
    };
    // Every opaque part, before the layer-3 filter: E3's sun tiles draw
    // object shadows on the structure (docs/graphics/e3-lighting.md, "Sun
    // shadows from objects"). Retail never reads it.
    let sun_world = native_render::sun_casters::SunWorld::new(&world);
    let layer3 = Layer3Materials::group(&mut world, &shaders, lightmap_bitmap.is_some(), &per_vertex_placements);
    let declared = world.materials().len();
    let out = match lightmap_error {
        Some(reason) => Programs {
            refused: BTreeMap::from([(format!("lightmap group does not decode ({reason})"), world.materials())]),
            ..Default::default()
        },
        None => material_programs(&mut map, &mut files, &layer3, &world, &page_ready, &mut decoder,
            &mut layered_images, key, None),
    };
    if !key.current() { return Ok(()); }
    let native: HashSet<u16> = out.programs.keys().copied().collect();
    let prepared_parts = out.parts;
    let palettized = out.palettized;
    let rigid_refused = out.rigid_refused;
    let (programs, mut stage_images, refused) = (out.programs, out.images, out.refused);
    let accepts = |d: &WorldDraw| prepared_parts.contains(&(d.owner, d.part));
    if hooks::native_verbose() {
        log_coverage(&mut map, &world, &layer3.shaders, &native, &refused, prepared_parts.len());
        let placed = world.draws.iter().filter(|d| native.contains(&d.material) && matches!(d.owner, Owner::Instance(_)));
        let (with, without) = placed.fold((0, 0), |(w, wo), d| if accepts(d) { (w + 1, wo) } else { (w, wo + 1) });
        eprintln!(
            "native-world: {with} instanced-geometry parts draw natively with their rigid programs (the placement's rows in c[50..52] per draw, as 0x0004d830 uploads them); the guest keeps {without} of native materials without one"
        );
        for (why, ms) in &rigid_refused {
            let parts = world.draws.iter().filter(|d| ms.contains(&d.material) && matches!(d.owner, Owner::Instance(_))).count();
            eprintln!("native-world: the guest keeps the placements of {} materials ({parts} parts): {why}", ms.len());
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
    if !key.current() { return Ok(()); }
    let missed_lit_parts = if relight_requested {
        world.draws.iter().filter(|d| d.lightmap.is_some() && !accepts(d)).count()
    } else { 0 };
    let world = keep_draws(world, &accepts);
    let parts = part_set(&world, &layer3.shaders);
    let mut section_parts = HashMap::<Owner, Vec<u8>>::new();
    for &(owner, part) in parts.keys() {
        section_parts.entry(owner).or_default().push(part);
    }
    for group in section_parts.values_mut() { group.sort_unstable(); }
    let part_bounds = RwLock::new(world.draws.iter().map(|d| ((d.owner, d.part), d.bounds)).collect());
    let empty_world = parts.is_empty();
    let stage_bitmap = |images: &StageImages, stage: Stage| match stage {
        Stage::Bitmap { bitmap, image } => images.get(&(bitmap, image)).cloned(),
        _ => None,
    };
    let page_set: HashSet<LightmapKey> = lightmaps.keys().copied().collect();
    let gpu = {
        let mut set = ProgramSet::with_layered(&device, &queue, &world, programs, &mut |s| stage_bitmap(&stage_images, s),
            &mut |k| lightmaps.get(&k).cloned(), &layered_images);
        if relight_requested {
            let header = map.header();
            let loaded = crate::relight::load(&header.name, key.index, checksum, header.checksum, &lightmaps)
                .and_then(|pages| {
                    let Some(pages) = pages else { return Ok(false) };
                    if failed_sections != 0 || missed_lit_parts != 0 || !rejected_pages.is_empty() {
                        return Err(format!("{failed_sections} unread sections, {missed_lit_parts} unowned page-lit parts, {} unread pages", rejected_pages.len()));
                    }
                    set.install_relight(&device, &queue, &world, &pages).map_err(str::to_owned)?;
                    Ok(true)
                });
            match loaded {
                Ok(true) => eprintln!("native-world: {} BSP {}: both reconstructed display lightmaps ready", key.map, key.index),
                Ok(false) => {},
                Err(why) => eprintln!("native-world: {} BSP {}: using retail lightmaps ({why})", key.map, key.index),
            }
        }
        eprintln!(
            "native-world: {} BSP {} at {:#010x}: {} of {} materials native; {} draws in {} batches, {} parts, in {:.1} s; {} BSP material slots, {} geometry sections skipped; top material rejections: {:?}",
            key.map, key.index, key.bsp, native.len(), declared, world.draws.len(), world.batches.len(), parts.len(),
            started.elapsed().as_secs_f32(), shaders.len(), failed_sections, top_rejections(&refused),
        );
        GpuWorld::with_programs(&device, &queue, world, set)
    };
    let send = |stage| {
        if !key.current() { return false; }
        // Flush writes on the producing queue before the first admitted draw.
        queue.submit([]);
        finished.send(stage).is_ok()
    };
    let bsp = key.clone();
    let sun_world = Arc::new(sun_world.with_current(move || bsp.current()));
    if !send(StageReady::World(Arc::new(Ready {
        sun_world: Some(sun_world),
        relit: AtomicBool::new(gpu.has_relight()),
        retire: Some(retire), part_bounds, bsp_tag: Some(bsp_tag), gpu: Mutex::new(Some(gpu)), parts: Arc::new(parts), section_parts,
        layer3, pages: page_set,
        upload_device: Some((device.clone(), queue.clone())), palettes: decoder.used().clone(),
        fog: OnceLock::new(), water: OnceLock::new(), ocean: OnceLock::new(), layers: OnceLock::new(),
        decorators: OnceLock::new(), sky: OnceLock::new(), lights: OnceLock::new(),
        lightmaps: lightmap_bitmap.map(|bitmap| (bitmap, lightmap_palettes.clone())),
    }))) { return Ok(()); }
    drop(world_setup);
    drop(layered_images);

    // Each result owns all of its GPU resources before it enters the mailbox.
    // No map walk or upload takes a lock used by an admitted draw.
    let empty_lights = {
        let _scope = mem_watch::scope("native.lights.load");
        let _setup = Setup::Lights.worker();
        let lights = crate::native_lights_glue::load(&mut map, &mut files, &shaders, &stage_world, decoder.palettes());
        if let Some(lights) = lights {
            let lights = Arc::new(lights);
            let set = lights.upload_engine_textures(&device, &queue);
            if !send(StageReady::Lights(lights, Box::new(set))) {
                return Ok(());
            }
            false
        } else {
            true
        }
    };
    if !key.current() { return Ok(()); }

    let empty_fog = {
        let _scope = mem_watch::scope("native.fog.load");
        let _setup = Setup::Fog.worker();
        let mut fog = crate::native_fog_glue::load(
            &mut map, &mut files, &shaders, &stage_world, &lightmaps, &mut stage_images, &mut decoder,
        );
        let mut empty = fog.part_set.is_empty();
        if !empty {
            let set = crate::native_fog_glue::upload_fog(&fog, &device, &queue, &stage_world,
                &mut |s| stage_bitmap(&stage_images, s), &mut |k| lightmaps.get(&k).cloned());
            fog.part_set.retain(|&(owner, part), _| set.draws_part(owner, part));
            empty = fog.part_set.is_empty();
            if !empty && !send(StageReady::Fog(Arc::new(fog), decoder.used().clone(), set)) {
                return Ok(());
            }
        }
        empty
    };
    if !key.current() { return Ok(()); }

    let empty_water = {
        let _scope = mem_watch::scope("native.water.load");
        let _setup = Setup::Water.worker();
        let mut water = crate::native_water_glue::load(
            &mut map, &mut files, &shaders, &stage_world, &lightmaps, &mut stage_images, &mut decoder,
        );
        let mut empty = water.part_set.is_empty();
        if !empty || !water.pools.is_empty() {
            let set = crate::native_water_glue::upload_water(&water, &device, &queue, &stage_world,
                &mut |s| stage_bitmap(&stage_images, s), &mut |k| lightmaps.get(&k).cloned());
            water.part_set.retain(|&(owner, part), _| set.draws_part(owner, part));
            empty = water.part_set.is_empty();
            if (!empty || !water.pools.is_empty()) && !send(StageReady::Water(Arc::new(water), decoder.used().clone(), Box::new(set))) {
                return Ok(());
            }
        }
        empty
    };
    if !key.current() { return Ok(()); }

    {
        let _scope = mem_watch::scope("native.ocean.load");
        if let Some(ocean) = crate::native_ocean_glue::load(&mut map, &mut files, bsp_tag, &mut stage_images, &mut decoder)
            && !send(StageReady::Ocean(Arc::new(ocean)))
        {
            return Ok(());
        }
    }
    if !key.current() { return Ok(()); }

    if render_bridge::native_layers::placed() {
        let _scope = mem_watch::scope("native.layers.load");
        let _setup = Setup::Layers.worker();
        let mut layers = crate::native_layers_glue::load(
            &mut map, &mut files, &shaders, &stage_world, &lightmaps, &mut stage_images, &mut decoder,
        );
        let sets = crate::native_layers_glue::upload_layers(&mut layers, &device, &queue, &stage_world,
            &mut |s| stage_bitmap(&stage_images, s), &mut |k| lightmaps.get(&k).cloned());
        if !send(StageReady::Layers(Arc::new(layers), decoder.used().clone(), sets)) {
            return Ok(());
        }
    }
    if !key.current() { return Ok(()); }

    {
        let _scope = mem_watch::scope("native.sky.load");
        if !send(StageReady::Sky(crate::native_sky_glue::load(&mut map, &mut files, &mut decoder))) {
            return Ok(());
        }
    }
    if !key.current() { return Ok(()); }

    drop(lightmaps);
    let decorators = {
        let _scope = mem_watch::scope("native.decorators.load");
        let _setup = Setup::Decorators.worker();
        crate::native_decorators_glue::load(&mut map, &mut files, &mut decoder, stage_images)
    };
    if key.current() && finished.send(StageReady::Decorators(decorators)).is_ok() {
        native_map_work::bsp_loaded(key.generation, &key.map, index, bsps.len(), [
            empty_world.then_some(MapStage::World), empty_lights.then_some(MapStage::Lights),
            empty_fog.then_some(MapStage::Fog), empty_water.then_some(MapStage::Water),
        ]);
    }
    Ok(())
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

/// Materials left to the guest, by reason.
type Refused = BTreeMap<String, Vec<u16>>;

fn top_rejections(refused: &Refused) -> Vec<(String, usize)> {
    const LIMIT: usize = 5;
    let mut counts: Vec<_> = refused.iter().map(|(why, materials)| (why, materials.len())).collect();
    counts.sort_by(|(a, n), (b, m)| m.cmp(n).then_with(|| a.cmp(b)));
    counts.into_iter().take(LIMIT).map(|(why, count)| (why.clone(), count)).collect()
}

/// The coverage of one load on stderr: how many materials (and parts)
/// draw natively, then one line per reason the rest stay on the guest,
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
        "native-world: {} of {} materials draw natively with their own programs ({} of {} parts); {} stay on the guest for {} reasons",
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
        eprintln!("native-world: the guest keeps {} materials ({n} parts): {why}; e.g. {example}", ms.len());
    }
}

/// Layer 3 batches by the program the render record selects. BSP material
/// indices stay valid for the other passes and for live shader edits. Only
/// a material drawn with both lighting modes needs an extra program slot.
#[derive(Default)]
struct Layer3Materials {
    shaders: Vec<TagId>,
    implementations: Vec<usize>,
}

impl Layer3Materials {
    fn group(world: &mut World, shaders: &[TagId], has_group: bool, per_vertex: &[bool]) -> Self {
        let mut slots = Self {
            shaders: shaders.to_vec(),
            implementations: vec![if has_group { 0 } else { look::UNLIT_BSP_IMPLEMENTATION }; shaders.len()],
        };
        if !has_group { return slots; }
        let vertex_lit = |d: &WorldDraw| matches!(d.owner, Owner::Instance(i) if per_vertex.get(i as usize) == Some(&true));
        let per_pixel: HashSet<_> = world.draws.iter().filter(|d| !vertex_lit(d)).map(|d| d.material).collect();
        let mut mixed = HashMap::new();
        for draw in &mut world.draws {
            let m = usize::from(draw.material);
            // Keep a malformed section index from aliasing an appended slot.
            if m >= shaders.len() || draw.material == u16::MAX {
                draw.material = u16::MAX;
                continue;
            }
            if !vertex_lit(draw) { continue; }
            if per_pixel.contains(&draw.material) {
                draw.material = *mixed.entry(draw.material).or_insert_with(|| {
                    // Leave only this variant to the sorted handler if all
                    // representable material slots are occupied.
                    let Ok(next) = u16::try_from(slots.shaders.len()) else { return u16::MAX };
                    if next != u16::MAX {
                        slots.shaders.push(shaders[m]);
                        slots.implementations.push(1);
                    }
                    next
                });
            } else {
                slots.implementations[m] = 1;
            }
        }
        world.regroup_draws();
        slots
    }
}

/// What [`material_programs`] gives.
#[derive(Default)]
struct Programs {
    parts: HashSet<(Owner, u8)>,
    programs: HashMap<u16, MaterialProgram>,
    images: StageImages,
    refused: Refused,
    /// Native materials whose placements stay on the guest, by reason
    /// (their rigid program does not translate or fit).
    rigid_refused: Refused,
    /// Materials with a palettized stage bitmap.
    palettized: Vec<u16>,
}

/// The world's layer-3 programs, the parts that supply their inputs
/// (`program::part_fit`), the images their stages sample, and the
/// others by the reason they stay on the guest. Stage images go
/// through `decoder` (a palettized one expanded through its stage's
/// palette, [`StageDecoder`]).
fn material_programs<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    files: &mut assets::ResourceFiles,
    layer3: &Layer3Materials,
    world: &World,
    page_ready: &dyn Fn(LightmapKey) -> Result<(), String>,
    decoder: &mut StageDecoder,
    layered: &mut LayeredImages,
    key: &BspKey,
    only: Option<&HashSet<u16>>,
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
    let mut materials = world.materials();
    if let Some(only) = only {
        materials.retain(|m| only.contains(m));
    }
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
        if !key.current() { break; }
        let mut uses_palette = false;
        let Some(&shader) = layer3.shaders.get(usize::from(m)).filter(|_| m != u16::MAX) else {
            refused.entry("no layer 3 material slot".into()).or_default().push(m);
            continue;
        };
        // An animated pass compiles at clock 0 and keeps its animation,
        // sampled at the render clock each frame (`GpuWorld::animate_selection`).
        let properties = native_render::layer_pass::instance_properties(&lib, shader);
        let implementation = layer3.implementations[usize::from(m)];
        let selection = tags::material::PassSelection { lod: 0, layer: look::LIGHTMAP_LAYER, pass: 0, implementation };
        let pass = lib.shader(shader).map_err(|e| format!("material: {e}"))
            .and_then(|s| {
                // The flagged draw skips the bucket bind and runs the sorted
                // handler's additional per-record shader setup.
                if implementation == 1 && s.raw[0x16] & 1 != 0 {
                    return Err("vertex-lit shader requires sorted BSP parameter setup".to_owned());
                }
                let inputs = tags::material::EngineInputs::for_shader(&s);
                let pass = lib.selected_implementation(shader, selection, Some(&inputs), None)
                    .map_err(|e| format!("material: {e}"))?;
                look::selected_bsp_pass_sampled(&pass, &inputs, normalization, &properties)
                    .map_err(|r| format!("layer 3: {r:?}"))
            });
        let compiled = pass
            .and_then(|(pass, animation)| {
                let vrtx = map.tag(pass.look.vertex_shader).ok().and_then(|t| t.location)
                    .ok_or("vertex shader tag unavailable")?.address;
                let code = look::vertex_program(lib.metadata(), vrtx, look::WORLD_CLASSIFICATION)
                    .ok_or("no world-space vertex program")?;
                let mut dims = [None; 4];
                for (slot, stage) in dims.iter_mut().zip(pass.look.textures) {
                    *slot = match stage {
                        Stage::Bitmap { bitmap, image } => Some(*dimensions.entry((bitmap, image))
                            .or_insert_with(|| bitmap_dimension(map, files, bitmap, image, layered))
                            .as_ref().map_err(Clone::clone)?),
                        Stage::Lightmap | Stage::Engine(native_render::object_program::TEXACCUM_TEXTURE) => Some(TextureDim::D2),
                        Stage::Empty => None,
                        Stage::Engine(id) => return Err(format!("unbound engine texture {id}")),
                        Stage::Host(h) => return Err(format!("a host stage in a tag's pass ({h:?})")),
                    };
                }
                let required_streams = |inputs, class| {
                    if implementation != 1 { return Ok(inputs); }
                    let semantics = look::vertex_semantics(lib.metadata(), vrtx, class)
                        .ok_or_else(|| format!("unreadable vertex-lit input declaration for class {class}"))?;
                    program::vertex_lit_inputs(inputs, &semantics)
                };
                let mut p = program::compile_section(&pass, &code, dims)?;
                p.inputs = required_streams(p.inputs, look::WORLD_CLASSIFICATION)?;
                p.animation = animation;
                if placed.contains(&m) {
                    // Its placements: the pass with the rigid
                    // classification, or they stay on the guest.
                    let rigid = look::vertex_program(lib.metadata(), vrtx, look::RIGID_CLASSIFICATION)
                        .ok_or_else(|| "no rigid vertex program".to_string())
                        .and_then(|code| {
                            let mut rigid = program::compile_section_rigid(&pass.look, pass.state.alpha_test, &code, dims)?;
                            rigid.inputs = required_streams(rigid.inputs, look::RIGID_CLASSIFICATION)?;
                            Ok(rigid)
                        });
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
                    // A linear texture is addressed in texels on the guest
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
                        eprintln!("native-world: material {m} leaves {count} parts to the guest: {why}");
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

#[cfg(test)]
mod tests {
    use super::*;
    use native_render::world::{Owner, PartInput, Placement, SectionInput, WorldBuilder};

    #[test]
    fn the_display_flood_selects_only_resident_parts_inside_its_frustum() {
        use native_render::{cull::Frustum, world::Aabb};
        let resident = HashMap::from([(Owner::Cluster(2), vec![0, 1, 2]), (Owner::Cluster(3), vec![0])]);
        let point = |x| Aabb { min: [x, 0., 0.5], max: [x, 0., 0.5] };
        let bounds = HashMap::from([
            ((Owner::Cluster(2), 0), point(1.25)),
            ((Owner::Cluster(2), 1), point(1.75)),
            ((Owner::Cluster(3), 0), point(1.25)),
        ]);
        let frustum = |scale| Frustum::from_view_projection(&[
            [scale, 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [0., 0., 0., 1.],
        ]);
        assert!(display_section_parts([Owner::Cluster(2)].into_iter(), &resident, &bounds, &frustum(1.)).is_empty());
        assert_eq!(display_section_parts([Owner::Cluster(2)].into_iter(), &resident, &bounds, &frustum(0.75)), [(Owner::Cluster(2), 0)]);
    }

    #[test]
    fn relit_bsp_ownership_survives_an_encoding_worker_holding_the_gpu() {
        let ready = Ready::default();
        ready.relit.store(true, Ordering::Release);
        let _encoding = ready.gpu();
        assert!(ready.owns_display_lightmaps());
        ready.relit.store(false, Ordering::Release);
        assert!(!ready.owns_display_lightmaps());
    }

    /// A live edit's drain, sorted for the world of one BSP: its sections
    /// in place, another BSP's left for that BSP's load, a grown section
    /// (or one of an unknown kind) reloading the world, images rewritten,
    /// models left to the objects.
    #[test]
    fn a_drain_of_live_edits_sorts_into_in_place_refreshes_and_reloads() {
        use render_bridge::native_edits::{Edit, KIND_CLUSTER, KIND_INSTANCE_DEF};
        let bsp = Some(TagId(0xe2b4_0012));
        let section = |bsp, kind, index, grown| Edit::Section { bsp, kind, index, grown };
        let plan = refresh_plan(bsp, &[
            section(0xe2b4_0012, KIND_CLUSTER, 4, false),
            section(0x0000_0012, KIND_INSTANCE_DEF, 2, false),
            section(0xe2b4_0013, KIND_CLUSTER, 9, true),
            Edit::Image { bitmap: 0xe000_0100, image: 3 },
            Edit::Model { tag: 0xe000_0200 },
            Edit::Shader { tag: 0xe000_0300 },
            Edit::Tag { group: u32::from_be_bytes(*b"weap"), tag: 0xe000_0400 },
        ]);
        assert_eq!(plan, RefreshPlan {
            reload: None,
            sections: vec![load::SectionOwner::Cluster(4), load::SectionOwner::InstanceDefinition(2)],
            images: vec![(TagId(0xe000_0100), 3)],
            shaders: vec![0xe000_0300],
        });
        let grown = refresh_plan(bsp, &[section(0xe2b4_0012, KIND_CLUSTER, 4, true)]);
        assert!(grown.reload.is_some());
        let unknown = refresh_plan(bsp, &[section(0xe2b4_0012, 9, 4, false)]);
        assert!(unknown.reload.is_some());
        assert_eq!(refresh_plan(None, &[section(0xe2b4_0012, KIND_CLUSTER, 4, true)]), RefreshPlan::default(),
            "no world loaded from a BSP: nothing of it to refresh");
    }

    /// A template edit picks every world material whose shader is built
    /// from it, and only those, each shader asked once.
    #[test]
    fn an_edited_template_picks_the_materials_built_from_it() {
        let shaders = [TagId(0xe000_0010), TagId(0xe000_0011), TagId(0xe000_0010), TagId(0xe000_0012)];
        let mut asked = Vec::new();
        let mut known = HashMap::new();
        let picked = edited_materials(&shaders, &mut |s| {
            *known.entry(s).or_insert_with(|| {
                asked.push(s);
                s.0 != 0xe000_0011
            })
        });
        assert_eq!(picked, vec![0, 2, 3]);
        assert_eq!(asked, vec![TagId(0xe000_0010), TagId(0xe000_0011), TagId(0xe000_0012)]);
    }

    /// Whether a native material's part was kept: a
    /// cluster's, and a placement's when the material has a rigid program
    /// (`rigid`). The passes after layer 3 (shadows 11, lights 12 and 13, fog
    /// 16, overlays 7) test `ZFUNC` equal against layer 3's depth, so the
    /// native pass must write the guest's bits: a cluster goes through the
    /// world program with the same position bits on both paths, and the guest
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
    fn one_shader_batches_the_lightmap_and_vertex_lighting_implementations_separately() {
        let tri = |material| SectionInput {
            positions: vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
            parts: vec![PartInput { material, triangles: vec![[0, 1, 2]], ..Default::default() }],
            ..Default::default()
        };
        let mut builder = WorldBuilder::new();
        // Absence of UVs itself never changes the implementation selected by the record.
        builder.add_section(Owner::Cluster(3), &tri(0), &Placement::IDENTITY, None);
        builder.add_section(Owner::Instance(0), &tri(0), &Placement::IDENTITY, None);
        builder.add_section(Owner::Instance(1), &tri(0), &Placement::IDENTITY, None);
        builder.add_section(Owner::Instance(2), &tri(1), &Placement::IDENTITY, None);
        let mut world = builder.finish();
        let mut menu = world.clone();
        let shaders = [TagId(100), TagId(200)];
        let modes = [false, true, true];
        let slots = Layer3Materials::group(&mut world, &shaders, true, &modes);
        assert_eq!(slots.shaders, [TagId(100), TagId(200), TagId(100)]);
        assert_eq!(slots.implementations, [0, 1, 1]);
        let draws: Vec<_> = world.draws.iter().map(|d| {
            assert_eq!(world.batches[d.batch as usize].material, d.material);
            (d.owner, slots.implementations[d.material as usize])
        }).collect();
        assert_eq!(draws, [(Owner::Cluster(3), 0), (Owner::Instance(0), 0),
            (Owner::Instance(2), 1), (Owner::Instance(1), 1)]);
        assert_eq!(part_set(&world, &slots.shaders)[&(Owner::Instance(1), 0)], shaders[0].0);
        let mut full = menu.clone();
        let all_slots = vec![TagId(100); usize::from(u16::MAX)];
        let full_slots = Layer3Materials::group(&mut full, &all_slots, true, &modes);
        assert_eq!(full_slots.shaders.len(), all_slots.len());
        assert_eq!(full.draws.iter().find(|d| d.owner == Owner::Instance(1)).unwrap().material, u16::MAX);
        assert_eq!(full.draws.iter().find(|d| d.owner == Owner::Cluster(3)).unwrap().material, 0);
        let unlit = Layer3Materials::group(&mut menu, &shaders, false, &modes);
        assert_eq!(unlit.shaders, shaders);
        assert_eq!(unlit.implementations, [3, 3]);
    }

    #[test]
    fn materials_left_to_the_guest_leave_the_world() {
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
    fn an_untranslated_material_keeps_its_guest_draws_and_a_translated_one_is_skipped() {
        // The world as the loader leaves it with programs on: material 7
        // translated, materials 8 and 9 refused, 9 sharing 7's shader. 8
        // and 9 are neither drawn natively nor skipped, so the guest
        // still draws them; 7's cluster part is drawn natively and
        // skipped, the shader's other parts not, and 7's part on an
        // instanced placement stays on the guest without a rigid
        // program (the guest places it on the GPU), and is drawn
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
    fn the_inherited_cull_is_the_pipeline_s() {
        // Placement 282 on 03a: `CULL_CW`, front face `CW`.
        use gfx::state::{CullMode, FrontFace};
        assert_eq!(inherited_cull((CullMode::Cw, FrontFace::Cw)), (Some(wgpu::Face::Front), wgpu::FrontFace::Cw));
        assert_eq!(inherited_cull((CullMode::Ccw, FrontFace::Cw)), (Some(wgpu::Face::Back), wgpu::FrontFace::Cw));
        assert_eq!(inherited_cull((CullMode::None, FrontFace::Ccw)), (None, wgpu::FrontFace::Ccw));
    }

    #[test]
    fn the_active_bsp_index_reads_the_game_globals() {
        let mut m = guest_layout::memory::FlatMemory::new(0x0040_0000, 0x0021_0000);
        assert_eq!(active_bsp_index(&m), None, "no globals yet");
        let globals = 0x0050_0000;
        m.set_u32(guest_layout::points::GAME_GLOBALS.base, globals).unwrap();
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
    /// (the guest items left in the layer and the passes after it see the
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
