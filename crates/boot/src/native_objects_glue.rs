//! Third-person object programs for the scene's layer 3.
//!
//! The activation worker reads the mounted tag index and uploads each
//! supported mode once. The sorted scene walk resolves every part of a
//! run before reserving it for every live target copy. With the walk off,
//! the indexed-draw admission still pairs native parts with resolved D3D
//! draws at the original submission site.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

use engine::memory::GuestMemory;
use feature::native_map_work;
use feature::native_objects::{Drawn, Host, ItemMasks, NativeSet, SetPart};
use map::TagId;
use native_render::camera::RenderCamera;
use native_render::gpu::Image;
use native_render::light_gpu::StaticLightTexture;
use native_render::load;
use native_render::look;
use native_render::object_lighting::ShaderValues;
use native_render::object_nodes::{Compression, decompression_rows};
use native_render::object_program_gpu::{EngineStages, LitBuild, LitMaterial, LitModelInput, LitObjects, LitSection, PlannedDraw, Texaccum, lit_model, uniform};
use native_render::object_state::EntryStates;
use native_render::object_streams::{self, StreamData, StreamFormats};
use native_render::object_walk::{Entry, Part as WalkPart};
use tags::model::RenderModelGeometry;
use timeline::native::ObjectWalk;
use native_render::objects::{ItemDraw, ItemKey, ObjectFrame};
use native_render::world_layers::{Geometry, Owner, Site};
use native_render::program::ViewInputs;

use crate::native_map::{self, NativeMap};
use crate::native_world_glue::{Raw, StageDecoder, StageImages, StagePalettes, palettes_hold, scene_load_palettes};

/// One distinct render-model tag, shared by its object definitions.
struct CpuModel {
    mode: TagId,
    lit: LitModelInput,
    normalization: Option<Arc<StaticLightTexture>>,
}

#[derive(Clone, Copy)]
struct PartRange {
    geometry_class: u16,
    node_influences: u8,
    first: u16,
    count: u16,
    triangle_list: bool,
}

impl PartRange {
    fn matches(self, part: WalkPart) -> bool {
        self.geometry_class == part.geometry_class
            && self.node_influences == part.node_influences
            && self.first == part.first_index
            && self.count == part.index_count
            && self.triangle_list == part.triangle_list
    }
}

struct ModelCoverage {
    nodes: usize,
    parts: Vec<((u16, u8), SetPart)>,
    ranges: HashMap<(u16, u8), PartRange>,
}

#[derive(Default)]
struct Models {
    pending: Mutex<Vec<CpuModel>>,
    upload_queued: AtomicBool,
    lit: Mutex<Option<LitObjects>>,
    /// Definition index to render-model datum, shared across definitions.
    index: Mutex<HashMap<u16, u32>>,
    coverage: Mutex<HashMap<TagId, Arc<ModelCoverage>>>,
    palettes: Mutex<StagePalettes>,
}

struct UploadModels {
    models: Arc<Models>,
    map_generation: u64,
}

impl Drop for UploadModels {
    fn drop(&mut self) {
        self.models.upload_queued.store(false, Ordering::Release);
    }
}

impl d3d::ExternalPass for UploadModels {
    fn colour_only(&self) -> bool { true }

    fn writes(&self) -> d3d::ExternalWrites {
        d3d::ExternalWrites::default()
    }

    fn encode(&mut self, target: d3d::ExternalTarget<'_>) {
        if self.map_generation != native_map_work::generation() {
            return;
        }
        let pending = std::mem::take(&mut *self.models.pending.lock().unwrap_or_else(|e| e.into_inner()));
        if pending.is_empty() {
            return;
        }
        let mut lit = self.models.lit.lock().unwrap_or_else(|e| e.into_inner());
        let lit = lit.get_or_insert_with(|| LitObjects::new(target.device));
        for cpu in pending {
            if let Some(texture) = &cpu.normalization {
                lit.set_normalization(target.device, target.queue, texture);
            }
            lit.add_model(target.device, target.queue, cpu.mode.0, cpu.lit);
        }
    }
}

fn queue_upload<B: d3d::backend::Backend>(
    device: &d3d::abi::SharedDevice<B>, models: &Arc<Models>, map_generation: u64,
) {
    if models.pending.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
        || models.upload_queued.swap(true, Ordering::AcqRel)
    {
        return;
    }
    let pass = UploadModels { models: Arc::clone(models), map_generation };
    if let Ok(mut device) = device.lock() {
        device.request_external_pass(Box::new(pass));
    }
}

struct MapState {
    map_generation: u64,
    generation: u64,
    models: Arc<Models>,
    set: Arc<NativeSet>,
}

static GENERATIONS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// The device at the layer-3 call: the render states the items' draws
/// inherit ([`EntryStates`]) and whether its draws are W-buffered.
#[derive(Clone, Debug)]
struct LayerEntry {
    states: EntryStates,
    w_buffered: bool,
}

impl LayerEntry {
    fn read<B: d3d::backend::Backend>(device: &d3d::Device<B>, mem: &dyn GuestMemory) -> Self {
        let table = gfx::xdk::RenderStateTable::default();
        let read = crate::native_world_glue::ReadMemory(mem);
        Self {
            states: EntryStates::read(|t| table.canonical_to_guest(t.index()).and_then(|g| device.render_state(&read, g))),
            w_buffered: device.hw().control0.is_some_and(|c| c & d3d::nv2a::CONTROL0_Z_PERSPECTIVE != 0),
        }
    }
}

/// The host: the device and the current map's models.
pub struct Glue<B: d3d::backend::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
    state: Arc<Mutex<Option<MapState>>>,
    admission: Arc<ObjectAdmission>,
}

/// The worker can publish only into the load it was started for.
fn publishes_to(current: &MapState, models: &Arc<Models>) -> bool {
    Arc::ptr_eq(&current.models, models)
}

impl<B: d3d::backend::Backend + Send + 'static> Glue<B> {
    fn load(&self, mem: &dyn GuestMemory) -> Option<(Arc<NativeSet>, Arc<Models>, u64)> {
        let map_generation = native_map_work::generation();
        let name = crate::health::loaded_map_name(mem).ok()?;
        if name.is_empty() {
            return None;
        }
        let ready = |s: &MapState| s.map_generation == map_generation && s.set.map == name;
        {
            let state = self.state.lock().ok()?;
            if let Some(s) = state.as_ref().filter(|s| ready(s)) {
                return Some((Arc::clone(&s.set), Arc::clone(&s.models), map_generation));
            }
        }
        let palettes = self.device.lock().ok().map(|d| scene_load_palettes(&d, mem)).unwrap_or_default();
        let formats = StreamFormats::read(&Ro(mem));
        let mut state = self.state.lock().ok()?;
        if map_generation != native_map_work::generation() {
            return None;
        }
        if let Some(s) = state.as_ref().filter(|s| ready(s)) {
            return Some((Arc::clone(&s.set), Arc::clone(&s.models), map_generation));
        }
        let generation = GENERATIONS.fetch_add(1, Ordering::Relaxed);
        let models = Arc::new(Models::default());
        let set = Arc::new(NativeSet { map: name.clone(), generation, ..NativeSet::default() });
        *state = Some(MapState {
            map_generation, generation, models: Arc::clone(&models), set: Arc::clone(&set),
        });
        drop(state);

        let device = Arc::downgrade(&self.device);
        spawn_worker(name, map_generation, Arc::clone(&models), Arc::clone(&self.state), palettes, formats,
            move |models| {
                if let Some(device) = device.upgrade() {
                    queue_upload(&device, models, map_generation);
                }
            });
        Some((set, models, map_generation))
    }
}

impl<B: d3d::backend::Backend + Send + 'static> Host for Glue<B> {
    fn preload(&self, mem: &dyn GuestMemory) {
        if let Some((_, models, generation)) = self.load(mem) {
            queue_upload(&self.device, &models, generation);
        }
    }

    fn native(&self, mem: &mut dyn GuestMemory) -> Option<Arc<NativeSet>> {
        self.admission.0.lock().ok()?.take();
        let (set, models, generation) = self.load(mem)?;
        queue_upload(&self.device, &models, generation);
        let mut device = self.device.lock().ok()?;
        if !device.begin_external_boundary(&mut crate::native_lights_glue::DeviceMemory(mem)).ok()? {
            return None;
        }
        if !model_palettes_hold(&models, &device, mem) {
            return None;
        }
        Some(set)
    }

    fn want(&self, _definition: u32) {
        // The activation worker already visits the entire tag index.
    }

    fn draw_walk(
        &self, mem: &mut dyn GuestMemory, run: &[Entry],
        consume: &mut dyn FnMut(&mut dyn GuestMemory) -> bool,
    ) -> Result<(), ObjectWalk> {
        use timeline::native::{Metric, Stage, Timer};
        let planning = Timer::start(Stage::Objects, Metric::PrepareNs);
        if mem.u8(engine::render_loop::layer::DRAW_FLAG) != Ok(0) {
            // The guest's first shader begin initializes the whole layer's
            // state and clears the cached texture-stage bindings.
            return Err(ObjectWalk::State);
        }
        let (_, models, generation) = self.load(mem).ok_or(ObjectWalk::Model)?;
        let mut device = self.device.lock().map_err(|_| ObjectWalk::Reservation)?;
        if !device.begin_external_boundary(&mut crate::native_lights_glue::DeviceMemory(mem))
            .map_err(|_| ObjectWalk::Reservation)?
        {
            return Err(ObjectWalk::Reservation);
        }
        if !model_palettes_hold(&models, &device, mem) {
            return Err(ObjectWalk::Inputs);
        }
        let camera = RenderCamera::read(&Ro(mem), native_render::camera::RENDER_CAMERA)
            .ok_or(ObjectWalk::Inputs)?;
        let read = crate::native_world_glue::ReadMemory(mem);
        let request = device.external_ready(&read, false).map_err(|_| ObjectWalk::Reservation)?;
        let (width, height) = request.surface_size;
        let (clip, rects) = device.clip_area(&read).ok().flatten().ok_or(ObjectWalk::State)?;
        if !rects.is_empty() || !clip.is_whole(width, height) || request.rect != (0, 0, width, height) {
            return Err(ObjectWalk::State);
        }
        let draws = plan_walk(&models, &device, mem, run)?;
        let ranges = draws.len();
        let view = mem.u32(engine::render_loop::occlusion::guest::VIEW_INDEX).unwrap_or(u32::MAX);
        let promise = d3d::ExternalPlan { stage: Stage::Objects, view, ranges, slots: Vec::new() };
        let pass = Box::new(ObjectsPass {
            models: Arc::clone(&models), camera, objects: Vec::new(), draws,
            owners: Vec::new(), refused: Vec::new(), compare: None, view: None,
            w_clip: None, fallback: None, walk: true, run: device.swap_count().wrapping_add(1),
        });
        drop(planning);
        let ready = device.prepare_external_bound(
            &mut crate::native_lights_glue::DeviceMemory(mem), promise, pass, Vec::new(),
        ).ok().flatten().ok_or(ObjectWalk::Reservation)?;
        let state = self.state.lock().map_err(|_| ObjectWalk::Reservation)?;
        if !state.as_ref().is_some_and(|s| s.map_generation == generation && publishes_to(s, &models))
            || generation != native_map_work::generation()
            || !consume(mem)
        {
            return Err(ObjectWalk::Reservation);
        }
        ready.commit();
        Ok(())
    }

    fn draw(
        &self, mem: &mut dyn GuestMemory, camera: &RenderCamera, generation: u64,
        objects: Vec<ObjectFrame>, items: Vec<ItemDraw>, _masks: ItemMasks,
    ) -> Option<Drawn> {
        use timeline::native::{Metric, Stage, Timer};
        let planning = Timer::start(Stage::Objects, Metric::PrepareNs);
        let models = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            Arc::clone(&st.as_ref().filter(|s| s.generation == generation)?.models)
        };
        let mut device = self.device.lock().ok()?;
        // native() drained before the feature walked the guest list. Resolve the
        // implementation's inherited fields on the device that will reserve it.
        let entry = LayerEntry::read(&device, mem);
        let plan = plan_view(&models, &objects, &items, &entry, device.samples_levels_in_place());
        let ranges = plan.draws.len();
        let refused = (items.len() - ranges) as u64;
        Stage::Objects.count(Metric::Examined, refused);
        Stage::Objects.count(Metric::RefusedInputs, refused);
        if ranges == 0 && plan.compare.is_none() { return None; }
        let probing = probe_at().is_some();
        if plan.compare.is_some() || probing {
            device.request_external_pass(Box::new(ObjectsPass {
                models: Arc::clone(&models), camera: *camera,
                objects: if probing { objects.clone() } else { Vec::new() },
                draws: if probing { plan.draws.clone() } else { Vec::new() },
                owners: if probing { plan.owners.clone() } else { Vec::new() },
                refused: plan.refused, compare: plan.compare,
                view: None, w_clip: None, fallback: None, walk: false, run: 0,
            }));
        }
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !st.as_ref().is_some_and(|s| s.generation == generation && Arc::ptr_eq(&s.models, &models)) {
            Stage::Objects.count(Metric::RefusedInputs, ranges as u64);
            return None;
        }
        drop(planning);
        let object_count = plan.owners.iter().copied().collect::<HashSet<_>>().len();
        let draws: HashMap<_, _> = plan.drawn.into_iter().zip(plan.draws).map(|(index, draw)| {
            let item = &items[index];
            ((objects[item.object].handle, item.key), draw)
        }).collect();
        if draws.is_empty() { return None; }
        let count = draws.len();
        *self.admission.0.lock().ok()? = Some(ObjectLayer {
            frame: crate::d3d_diag::presents() + 1,
            bound: device.bound_target(),
            models,
            camera: *camera,
            draws,
        });
        Some(Drawn::Queued { objects: object_count, items: count })
    }
}

#[derive(Default)]
struct ObjectAdmission(Mutex<Option<ObjectLayer>>);

struct ObjectLayer {
    frame: u64,
    bound: [u32; 8],
    models: Arc<Models>,
    camera: RenderCamera,
    draws: HashMap<(u32, ItemKey), (u32, PlannedDraw)>,
}

fn object_key(geometry: Geometry) -> Option<(u32, ItemKey)> {
    let Owner::Object { handle, section, .. } = geometry.owner else { return None };
    Some((handle, ItemKey {
        record: geometry.record, shader: geometry.shader, section: section.try_into().ok()?,
        part: geometry.part, implementation: usize::from(geometry.implementation),
    }))
}

impl d3d::DrawAdmission for ObjectAdmission {
    fn admit(&self, snapshot: &d3d::DrawSnapshot<'_>) -> Option<(d3d::ExternalPlan, Box<dyn d3d::ExternalPass>)> {
        if feature::native_map_work::Stage::Objects.standing_down() {
            return None;
        }
        let context = crate::native_world_layers_glue::indexed_draw()?;
        let draw = snapshot.draw;
        if context.layer.site != Site::Scene || context.layer.number != 3
            || draw.entry != "D3DDevice_DrawIndexedVertices"
            || context.index_count? as usize != draw.indices.as_ref()?.len()
            || !draw.clip_rects.is_empty()
            || !draw.clip.is_some_and(|r| r.is_whole(draw.target.width, draw.target.height))
        {
            return None;
        }
        let key = object_key(context.geometry?)?;
        let mut pending = self.0.lock().ok()?;
        let layer = pending.as_mut()?;
        if layer.frame != context.frame || layer.bound[..2] != [snapshot.color_header, snapshot.depth_header] {
            return None;
        }
        let (_, planned) = layer.draws.get(&key)?;
        if !object_state_matches(draw, planned)
            || !crate::native_world_layers_glue::matches_triangle_range(draw, planned.indices.len())
        {
            return None;
        }
        let accum = native_render::object_program::TEXACCUM_HEADER;
        if planned.texaccum != (0..4).any(|i| snapshot.sampled_stages[i] && draw.textures[i] == Some(accum)) {
            return None;
        }
        let mut planned = layer.draws.remove(&key)?;
        set_resolved_fog(&mut planned.1, draw)?;
        if planned.1.state.stencil.enable {
            // The legacy resolved replacement's binding only promises
            // the original draw's depth writes. Reserve stencil in walk mode.
            return None;
        }
        let pass = ObjectsPass {
            models: Arc::clone(&layer.models), camera: layer.camera, objects: Vec::new(),
            draws: vec![planned], owners: Vec::new(), refused: Vec::new(), compare: None,
            view: None, w_clip: None, fallback: Some(Arc::new(draw.clone())), walk: false, run: layer.frame,
        };
        Some((d3d::ExternalPlan {
            stage: timeline::native::Stage::Objects, view: context.layer.view, ranges: 1, slots: Vec::new(),
        }, Box::new(pass)))
    }
}

fn object_state_matches(draw: &d3d::ResolvedDraw, planned: &PlannedDraw) -> bool {
    use gfx::state::{CompareFunc, DepthBufferMode, FillMode};
    let r = &draw.state.render;
    let p = planned.state;
    let primitive = gfx::translate::primitive_state(r, wgpu::PrimitiveTopology::TriangleList,
        gfx::translate::Capabilities::from_features(wgpu::Features::empty()));
    let depth = r.depth_mode != DepthBufferMode::Disabled;
    let compare = if depth { gfx::translate::compare_function(r.depth_func) } else { wgpu::CompareFunction::Always };
    let alpha = (r.alpha_test_enable && r.alpha_func != CompareFunc::Always)
        .then(|| (gfx::translate::compare_function(r.alpha_func), r.alpha_ref));
    draw.hw.control0.is_some_and(|c| c & d3d::nv2a::CONTROL0_Z_PERSPECTIVE != 0)
        && draw.target.depth_format.is_some()
        && primitive == p.primitive && compare == p.depth_compare && (depth && r.depth_write) == p.depth_write
        && gfx::translate::color_writes(r.color_write) == p.color_writes && alpha == p.alpha_test
        && r.depth_clamp == p.w_clamp && !r.blend_enable
        && r.stencil.enable == p.stencil.enable && (!r.stencil.enable || r.stencil == p.stencil)
        && r.fill_mode == FillMode::Solid && r.back_fill_mode == FillMode::Solid
        && (!r.solid_offset_enable || (r.depth_bias == 0.0 && r.depth_bias_slope_scale == 0.0))
}

/// D3D's specular bit selects the colour term in its implicit final combiner.
const SPECULAR_ENABLE: u32 = 103;
const STIPPLE_ENABLE: u32 = 83;
const TWO_SIDED_LIGHTING: u32 = 141;
const SCENE_STENCIL_MODE: u32 = 0x0046_7010;

/// Pointers and lists read by the shader begin and its pass-switch code.
const CACHED_PASS_DESCRIPTOR: u32 = engine::render_loop::layer::SETUP_CONTEXT + 0x20;
const CACHED_IMPLEMENTATION: u32 = engine::render_loop::layer::SETUP_CONTEXT + 0x24;
const IMPLEMENTATION_RESTORE: u32 = 0x11c;
const DESCRIPTOR_STATES: u32 = 0x14;
const STATE_SIZE: u32 = 5;

fn object_fog(
    words: [u32; 7], default_final: bool, specular: Option<u32>,
) -> Option<([f32; 4], [f32; 4])> {
    let (fog, color) = crate::native_fog_glue::fog_of(&words);
    let mut uniform = fog.uniform();
    if default_final {
        uniform[3] = f32::from(u8::from(specular? != 0));
    }
    Some((uniform, color))
}

fn set_fog(draw: &mut PlannedDraw, words: [u32; 7], specular: Option<u32>) -> Option<()> {
    let (fog, color) = object_fog(words, draw.default_final, specular)?;
    draw.pixel.fog = fog;
    draw.pixel.fog_color = color;
    Some(())
}

fn set_resolved_fog(draw: &mut PlannedDraw, original: &d3d::ResolvedDraw) -> Option<()> {
    let table = gfx::xdk::RenderStateTable::default();
    let raw = |state| {
        let slot = table.canonical_to_guest(state)?;
        original.raw.render.get(slot as usize).copied()
    };
    let words = native_render::fog_gpu::FogWords::default()
        .resolve(&native_render::fog_gpu::FOG_WORD_SLOTS.map(raw))?;
    set_fog(draw, words, raw(SPECULAR_ENABLE))
}

/// Commit only state-preserving runs. A later guest handler inherits the
/// same render states it would inherit if the skipped programs had run.
/// The guest's own shader/binding cache tracks the last real guest call.
fn plan_walk<B: d3d::backend::Backend>(
    models: &Models, device: &d3d::Device<B>, mem: &dyn GuestMemory, run: &[Entry],
) -> Result<Vec<(u32, PlannedDraw)>, ObjectWalk> {
    use gfx::state::RenderStateType as R;
    let mut entry = LayerEntry::read(device, mem);
    if !entry.w_buffered {
        return Err(ObjectWalk::State);
    }
    let read = crate::native_world_glue::ReadMemory(mem);
    let flags = mem.u32(native_render::skip::STENCIL_OFF_FLAGS).map_err(|_| ObjectWalk::Inputs)?;
    let stencil_during_draw = flags & 0x00ff_ff00 == 0;
    if !stencil_during_draw {
        // The handler temporarily applies stencil mode 0 and leaves
        // mode 1. Skipping it is state-neutral only if it already holds 1.
        if mem.u16(SCENE_STENCIL_MODE) != Ok(1) || device.stencil_state(&read) != Some(scene_stencil()) {
            return Err(ObjectWalk::State);
        }
        entry.states.0.retain(|(r, _)| *r != R::StencilEnable);
        entry.states.0.push((R::StencilEnable, 0));
    }
    let fog_words = crate::native_fog_glue::device_fog_words(device, mem);
    let table = gfx::xdk::RenderStateTable::default();
    let device_state = |slot| device.render_state(&read, slot);
    let raw = |state| table.canonical_to_guest(state).and_then(device_state);
    let specular = raw(SPECULAR_ENABLE);
    if device.hw().stipple_control.or_else(|| raw(STIPPLE_ENABLE)) != Some(0)
        || raw(TWO_SIDED_LIGHTING) != Some(0)
        || raw(R::ShadeMode.index()) != Some(gfx::state::ShadeMode::Gouraud.xbox_value())
    {
        return Err(ObjectWalk::State);
    }
    let cached = mem.u32(CACHED_IMPLEMENTATION).map_err(|_| ObjectWalk::State)?;
    let mut checked_transition = false;
    let index = models.index.try_lock().map_err(|_| ObjectWalk::Model)?;
    let coverage = models.coverage.try_lock().map_err(|_| ObjectWalk::Model)?;
    let lit = models.lit.try_lock().map_err(|_| ObjectWalk::Model)?;
    let lit = lit.as_ref().ok_or(ObjectWalk::Model)?;
    let guest = Ro(mem);
    let mut objects = HashMap::new();
    let mut draws = Vec::with_capacity(run.len());
    for &item in run {
        let (handle, part) = match item {
            Entry::Excluded => continue,
            Entry::Object { handle, part: Some(part) } => (handle, part),
            _ => return Err(ObjectWalk::Unsupported),
        };
        let frame = objects.entry(handle)
            .or_insert_with(|| native_render::objects::read_object(&guest, handle))
            .as_ref().ok_or(ObjectWalk::Inputs)?;
        if compare().is_some_and(|c| c.definition == frame.definition as u16) {
            return Err(ObjectWalk::Unsupported);
        }
        let model = *index.get(&(frame.definition as u16)).ok_or(ObjectWalk::Model)?;
        let cover = coverage.get(&TagId(model)).ok_or(ObjectWalk::Model)?;
        if model != part.mode || frame.nodes.len() != cover.nodes || !lit.has(model) {
            return Err(ObjectWalk::Model);
        }
        if !cover.ranges.get(&(part.key.section, part.key.part)).is_some_and(|range| range.matches(part)) {
            return Err(ObjectWalk::Unsupported);
        }
        let rows = part.rows(&guest).ok_or(ObjectWalk::Inputs)?;
        let mut draw = lit.plan_scene(model, frame, &part.key, &rows, &entry.states).map_err(|why| {
            count_refusal(&why);
            ObjectWalk::Inputs
        })?;
        let pass = lit.planned_pass(model, &draw).ok_or(ObjectWalk::Model)?;
        if pass.implementation.vertex_shader != Some(TagId(part.vertex_shader))
            || !stable_render_states(&pass.implementation.render_states, device_state)
        {
            return Err(ObjectWalk::State);
        }
        if part.program != cached {
            if !stable_render_states(&pass.implementation.restore_states, device_state)
                || (!checked_transition && stable_cached_restore(mem, cached, device_state) != Some(true))
            {
                return Err(ObjectWalk::State);
            }
            checked_transition = true;
        }
        let fog = native_render::fog_gpu::FogWords::from_states(&pass.implementation.render_states)
            .resolve(&fog_words).ok_or(ObjectWalk::Inputs)?;
        set_fog(&mut draw, fog, specular).ok_or(ObjectWalk::Inputs)?;
        if draw.texaccum && (!device.samples_levels_in_place()
            || !native_render::object_nodes::texaccum_bound(&guest))
        {
            return Err(ObjectWalk::Inputs);
        }
        draws.push((model, draw));
    }
    Ok(draws)
}

fn scene_stencil() -> gfx::state::StencilState {
    use gfx::state::{CompareFunc, StencilOp, StencilState};
    StencilState {
        enable: true, func: CompareFunc::Always, pass: StencilOp::Replace, ..Default::default()
    }
}

fn stable_cached_restore(
    mem: &dyn GuestMemory, implementation: u32, read: impl Fn(u32) -> Option<u32>,
) -> Option<bool> {
    let descriptor = mem.u32(CACHED_PASS_DESCRIPTOR).ok()?;
    if implementation == 0 || descriptor == 0 {
        return Some(true);
    }
    let range = u32::from(mem.u16(implementation.checked_add(IMPLEMENTATION_RESTORE)?).ok()?);
    let table = mem.u32(descriptor.checked_add(DESCRIPTOR_STATES)?).ok()?;
    let start = range & 0x1ff;
    for index in start..start + (range >> 9) {
        let at = table.checked_add(STATE_SIZE * index)?;
        let state = u32::from(mem.u8(at).ok()?);
        if read(state) != Some(mem.u32(at.checked_add(1)?).ok()?) {
            return Some(false);
        }
    }
    Some(true)
}

fn stable_render_states(
    states: &[tags::shader::RenderStateValue], read: impl Fn(u32) -> Option<u32>,
) -> bool {
    states.iter().all(|s| read(u32::from(s.state)) == Some(s.value))
}

/// One view's plan ([`plan_view`]).
#[derive(Default)]
struct ViewPlan {
    /// The items drawn (their places in the items handed in).
    drawn: Vec<usize>,
    /// Their draws, in the list's order, each with its model index, and
    /// the object each belongs to.
    draws: Vec<(u32, PlannedDraw)>,
    owners: Vec<usize>,
    /// The items refused, with their object and why (the probe's).
    refused: Vec<(usize, String)>,
    /// The comparison's part, planned: model, draw and label.
    compare: Option<(u32, PlannedDraw, String)>,
}

/// Plans each of `items` against the uploaded models: drawn only when its
/// model is on the GPU, its part and implementation are native, every
/// per-object constant and every state field resolves
/// (`LitObjects::plan`), the device draws W-buffered, and, for a part
/// sampling the texture accumulation target, the device samples levels in
/// place (`levels`). A shadow item is planned for the comparison only.
fn plan_view(models: &Models, objects: &[ObjectFrame], items: &[ItemDraw], entry: &LayerEntry, levels: bool) -> ViewPlan {
    let mut plan = ViewPlan::default();
    // Access the definition lookup before the lit GPU cache.
    let Ok(index) = models.index.try_lock() else { return plan };
    let Ok(lit) = models.lit.try_lock() else { return plan };
    let compared = compare();
    for (k, item) in items.iter().enumerate() {
        let o = &objects[item.object];
        let planned = (|| -> Result<(u32, PlannedDraw), String> {
            if !entry.w_buffered {
                return Err("the device's draws are not W-buffered (SET_CONTROL0 without z-perspective)".into());
            }
            let model = *index.get(&(o.definition as u16)).ok_or("its model is not uploaded")?;
            let lit = lit.as_ref().ok_or("no model is uploaded")?;
            let d = lit.plan_scene(model, o, &item.key, &item.rows, &entry.states)?;
            if d.texaccum && !levels {
                return Err("it samples the texture accumulation target and the device does not sample levels in place".into());
            }
            Ok((model, d))
        })();
        match planned {
            Ok((model, d)) if item.shadow => {
                if plan.compare.is_none() && compared.is_some_and(|c| c.definition == o.definition as u16 && c.part.picks(d.part, item.key.shader)) {
                    let label = format!("object {:#010x} definition {:#010x} part {} record {}", o.handle, o.definition, d.part, item.key.record);
                    plan.compare = Some((model, d, label));
                }
            }
            Ok((model, d)) => {
                plan.drawn.push(k);
                plan.draws.push((model, d));
                plan.owners.push(item.object);
            }
            Err(why) => {
                if item.shadow && compared.is_some() {
                    eprintln!("native-objects compare: definition {:#010x} record {}: {why}", o.definition, item.key.record);
                }
                count_refusal(&why);
                plan.refused.push((item.object, format!("record {} (section {} part {} implementation {}): {why}", item.key.record, item.key.section, item.key.part, item.key.implementation)));
            }
        }
    }
    plan
}

/// Items the plan gave back to the D3D path, and the first of each reason
/// (logged once).
static REFUSED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static REASONS: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn count_refusal(why: &str) {
    REFUSED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut seen = REASONS.lock().unwrap_or_else(|e| e.into_inner());
    let seen = seen.get_or_insert_with(HashSet::new);
    if seen.len() < 64 && seen.insert(why.to_owned()) {
        eprintln!("native-objects: an item keeps its D3D draw: {why} (each reason logged once)");
    }
}

static PALETTE_CHANGED: AtomicBool = AtomicBool::new(false);

fn model_palettes_hold<B: d3d::backend::Backend>(
    models: &Models, device: &d3d::Device<B>, mem: &dyn GuestMemory,
) -> bool {
    let used = models.palettes.lock().unwrap_or_else(|e| e.into_inner());
    let hold = palettes_hold(&used, &|s| device.stage_palette(&crate::native_world_glue::ReadMemory(mem), s));
    if !hold && !PALETTE_CHANGED.swap(true, Ordering::Relaxed) {
        eprintln!("native-objects: the stage palette changed since a base map was expanded");
    }
    hold
}

/// Guest memory, read only.
struct Ro<'a>(&'a dyn GuestMemory);

impl native_render::camera::GuestRead for Ro<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

/// Installs the host on the device (beside the world's).
pub fn install<B: d3d::backend::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    let switches = feature::Switches::from_env().unwrap_or_default();
    if !feature::native_objects::on(&switches) {
        return;
    }
    let admission = Arc::new(ObjectAdmission::default());
    if !feature::native_objects::walk_on(&switches) && let Ok(mut device) = device.lock() {
        device.add_draw_admission(admission.clone());
    }
    feature::native_objects::set_host(Arc::new(Glue {
        device: Arc::clone(device),
        state: Arc::new(Mutex::new(None)),
        admission,
    }));
}

/// Visit every object definition in the mounted tag index, including
/// definitions whose first instance will spawn after activation.
fn spawn_worker(
    name: String,
    map_generation: u64,
    models: Arc<Models>,
    state: Arc<Mutex<Option<MapState>>>,
    palettes: StagePalettes,
    formats: Option<StreamFormats>,
    upload: impl Fn(&Arc<Models>) + Send + 'static,
) {
    let launched = std::thread::Builder::new().name("native-objects load".into()).spawn(move || {
        let mut opened = match open(&name, palettes, formats) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("native-objects: {name}: map not opened ({e})");
                return;
            }
        };
        let definitions: Vec<TagId> = opened.map.tags().iter()
            .filter(|tag| tags::object::is_object_group(tag.group))
            .map(|tag| tag.id)
            .collect();
        if definitions.is_empty() {
            native_map_work::map_stage_empty(map_generation, &name, native_map_work::Stage::Objects);
            return;
        }
        let mut loaded = 0;
        let mut refused = 0;
        let mut coverage = HashMap::<TagId, Result<Arc<ModelCoverage>, String>>::new();
        for definition in definitions {
            if native_map_work::generation() != map_generation {
                return;
            }
            let result = (|| -> Result<_, String> {
                if excepted(definition.0) {
                    return Err("H2_NATIVE_OBJECTS_EXCEPT".into());
                }
                let model = opened.ctx.resolve_object(definition).map_err(|e| e.to_string())?
                    .model.ok_or("the object has no model")?;
                let mode = model.render_model.as_ref().ok_or("the model has no render model")?.id;
                let native = coverage.entry(mode).or_insert_with(|| {
                    let (cpu, cover) = load_model(&mut opened, mode).map_err(|e| e.to_string())?;
                    let cover = Arc::new(cover);
                    models.palettes.lock().unwrap_or_else(|e| e.into_inner()).clone_from(opened.decoder.used());
                    models.coverage.lock().unwrap_or_else(|e| e.into_inner()).insert(mode, Arc::clone(&cover));
                    models.pending.lock().unwrap_or_else(|e| e.into_inner()).push(cpu);
                    upload(&models);
                    Ok(cover)
                }).as_ref().map_err(Clone::clone)?;
                if model.node_count as usize != native.nodes {
                    return Err("model and render model disagree on the node count".into());
                }
                Ok((mode, Arc::clone(native)))
            })();
            match result {
                Ok((mode, native)) => {
                    let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
                    let Some(s) = state.as_mut().filter(|s| {
                        publishes_to(s, &models) && s.map_generation == native_map_work::generation()
                    }) else { return };
                    let index = definition.0 as u16;
                    models.index.lock().unwrap_or_else(|e| e.into_inner()).insert(index, mode.0);
                    let set = Arc::make_mut(&mut s.set);
                    set.definitions.insert(index, native.nodes);
                    set.parts.extend(native.parts.iter().map(|&((section, part), p)| ((index, section, part), p)));
                    if compare().is_some_and(|c| c.definition == index) {
                        set.shadowed.insert(index);
                    }
                    loaded += 1;
                }
                Err(why) => {
                    refused += 1;
                    if refused == 1 || feature::native_verbose() {
                        eprintln!("native-objects: {name}: definition {:#010x} left to D3D ({why})", definition.0);
                    }
                }
            }
        }
        eprintln!("native-objects: {name}: {loaded} definitions ready, {refused} left to D3D");
        upload(&models);
    });
    if let Err(e) = launched {
        eprintln!("native-objects: could not start map loader ({e})");
    }
}

/// The map open for model loads.
struct Opened {
    map: NativeMap,
    ctx: tags::object::Context,
    raw: Raw,
    /// Base maps as their stages sample them (palettized ones expanded
    /// through the stage's palette), shared by every model of the map.
    decoder: StageDecoder,
    images: StageImages,
    /// The shader tags, read at the first material that needs them.
    lib: Option<Option<tags::material::ShaderLibrary>>,
    /// The vector normalization cube, loaded as the lights load it at the
    /// first lit model (`Some(None)`: it does not load).
    normalization: Option<Option<Arc<StaticLightTexture>>>,
    /// The game's stream formats (`None`: lit materials stay on D3D).
    formats: Option<StreamFormats>,
    /// What an object's draw samples for the lightmap (the globals'
    /// rasterizer bitmap at `+0x64`, `object_program::OBJECT_LIGHTMAP`),
    /// read at the first lit model: the image when uniform, else why not.
    lightmap: Option<Result<Image, String>>,
}

fn open(name: &str, palettes: StagePalettes, formats: Option<StreamFormats>) -> Result<Opened, load::Error> {
    let (mut map, files) = native_map::open(name)?;
    let ctx = tags::object::Context::read(&mut map)?;
    Ok(Opened {
        map,
        ctx,
        raw: Raw(files),
        decoder: StageDecoder::new(palettes),
        images: StageImages::new(),
        lib: None,
        normalization: None,
        lightmap: None,
        formats,
    })
}

fn material(o: &mut Opened, shader: TagId) -> Option<tags::material::Material> {
    if o.lib.is_none() {
        let lib = tags::material::ShaderLibrary::read(&mut o.map);
        if let Err(e) = &lib {
            eprintln!("native-objects: the shader tags do not read ({e}); every object stays on D3D");
        }
        o.lib = Some(lib.ok());
    }
    o.lib.as_mut()?.as_mut()?.material(shader, None).ok()
}

/// Why an opaque material stays on D3D: no layer-3 implementation reads
/// only what the object path gives (`object_program::program_refusal`),
/// or it does not resolve. One that does is drawn through its programs
/// ([`lit_material`]) or refused there; never with a stand-in.
fn program_refusal(o: &mut Opened, shader: TagId) -> Option<String> {
    let Some(m) = material(o, shader) else {
        return Some("the material does not resolve".into());
    };
    native_render::object_program::program_refusal(&m)
}

/// Compile all sections, including runtime-selected damage variants,
/// permutations and LODs. The live render record selects the section.
fn load_model(o: &mut Opened, mode: TagId) -> Result<(CpuModel, ModelCoverage), load::Error> {
    let geo = RenderModelGeometry::read_in(&mut o.map, mode, &mut o.raw, Some(&o.ctx.meta))?;
    let mut materials = HashMap::new();
    for (index, m) in geo.materials.iter().enumerate() {
        if program_refusal(o, m.shader.id).is_none()
            && let Ok(lit) = lit_material(o, m.shader.id)
        {
            materials.insert(index as u16, lit);
        }
    }
    let LitBuild { input: lit, refused, .. } = lit_parts(o, &geo, &materials);
    if lit.is_empty() {
        let why = refused.into_iter().map(|(_, reason)| reason).collect::<Vec<_>>().join(", ");
        return Err(format!("no supported layer-3 parts ({why})").into());
    }
    let parts = set_parts(&lit);
    let ranges = parts.iter().filter_map(|&((section, part), _)| {
        let section_data = geo.sections.get(section as usize)?;
        let p = section_data.geometry.parts.get(part as usize)?;
        Some(((section, part), PartRange {
            geometry_class: section_data.wire.geometry_classification.0,
            node_influences: section_data.wire.unknown_014,
            first: p.first_index,
            count: p.index_count,
            triangle_list: p.is_triangle_list(),
        }))
    }).collect();
    let cover = ModelCoverage { nodes: geo.nodes.len(), parts, ranges };
    let cpu = CpuModel { mode, lit, normalization: o.normalization.clone().flatten() };
    Ok((cpu, cover))
}

/// What an opaque material's programs draw with: the material and its
/// shader's values (`shad +0x44`, `+0x48`), or why they do not read.
fn lit_material(o: &mut Opened, shader: TagId) -> Result<LitMaterial, String> {
    let material = material(o, shader).ok_or("the material does not resolve")?;
    let read = || -> Result<LitMaterial, String> {
        if o.formats.is_none() {
            return Err("the game's stream formats do not read".into());
        }
        let lib = o.lib.as_ref().and_then(Option::as_ref).ok_or("the shader tags do not read")?;
        let address = o.map.tag(shader).ok().and_then(|t| t.location).map(|l| l.address).ok_or("its shader tag has no address")?;
        let values = lib.metadata().bytes(address + SHADER_VALUES, 8).map_err(|_| "its shader values do not read")?;
        let word = |k: usize| u32::from_le_bytes([values[k], values[k + 1], values[k + 2], values[k + 3]]);
        Ok(LitMaterial {
            shader: ShaderValues { scalar_21: word(0), vector_24: word(4) },
            material: material.clone(),
        })
    };
    read()
}

/// The shader instance's values the object inputs scale by (`shad +0x44`
/// scalar 21, `+0x48`).
const SHADER_VALUES: u32 = 0x44;

/// The lit parts of `loaded`'s drawn sections for `materials`
/// ([`lit_model`]): its vertex programs per section classification read
/// from the shader tags, its stage bitmaps through the stage decoder.
fn lit_parts(o: &mut Opened, geo: &RenderModelGeometry, materials: &HashMap<u16, LitMaterial>) -> LitBuild {
    let mut sections = Vec::new();
    // Every detail level: a distant object's records draw a lower one.
    for (si, s) in geo.sections.iter().enumerate() {
        let flags = s.wire.compression_flags.0;
        let bounds = geo.compression.as_ref();
        let compression = Compression {
            position: bounds.filter(|_| flags & 1 != 0).map(|c| c.position_bounds),
            texcoord: bounds.filter(|_| flags & 2 != 0).map(|c| c.texcoord_bounds),
        };
        let g = &s.geometry;
        let slots = g.vertex_buffers.iter().map(|b| b.slot + 1).max().unwrap_or(0);
        let mut streams = vec![None; slots];
        for b in &g.vertex_buffers {
            streams[b.slot] = Some((b.type_id, StreamData { bytes: g.data(b), stride: usize::from(b.stride) }));
        }
        sections.push(LitSection {
            index: si as u16,
            geometry: s.wire.geometry_classification.0,
            influences: s.wire.unknown_014,
            decompression: decompression_rows(&compression),
            streams,
            vertex_count: g.vertex_count(),
            parts: g
                .parts
                .iter()
                .enumerate()
                .filter(|(i, p)| *i < 16 && p.is_drawn() && p.kind != load::PART_TRANSPARENT)
                .map(|(k, p)| (k as u8, p.material, g.part_triangles(p)))
                .collect(),
        });
    }
    // The programs each section's classification takes, read first (the
    // stage decoder borrows the map after).
    let mut programs: HashMap<(TagId, u16, u8), Result<(Vec<u8>, Vec<u16>), String>> = HashMap::new();
    for lit in materials.values() {
        let vertex_shaders = lit
            .material
            .layers
            .iter()
            .filter(|l| l.lod == 0 && l.layer == look::LIGHTMAP_LAYER)
            .flat_map(|l| &l.passes)
            .flat_map(|p| &p.implementations)
            .filter_map(|i| i.vertex_shader);
        for vs in vertex_shaders {
            for s in &sections {
                let key = (vs, s.geometry, s.influences);
                if !programs.contains_key(&key) {
                    let p = object_vertex_program(o, vs, s.geometry, s.influences);
                    programs.insert(key, p);
                }
            }
        }
    }
    // The normalization cube, as the lights load it (once per map).
    if o.normalization.is_none() {
        let cube = match o.lib.as_ref().and_then(Option::as_ref) {
            Some(lib) => crate::native_lights_glue::light_textures(&mut o.map, lib.metadata(), &mut o.raw.0).0,
            None => None,
        };
        o.normalization = Some(cube);
    }
    if o.lightmap.is_none() {
        let l = object_lightmap(o);
        match &l {
            Ok(i) => {
                if feature::native_verbose() {
                    eprintln!("native-objects: the objects' lightmap stage loads ({}x{}, uniform)", i.width, i.height);
                }
            }
            Err(why) => eprintln!("native-objects: the objects' lightmap stage stays on D3D: {why}"),
        }
        o.lightmap = Some(l);
    }
    let Some(formats) = o.formats.clone() else {
        return LitBuild {
            refused: materials.keys().map(|&m| (m, "the game's stream formats do not read".to_string())).collect(),
            ..LitBuild::default()
        };
    };
    let mut vertex = |vs: TagId, geometry: u16, influences: u8| {
        programs.get(&(vs, geometry, influences)).cloned().unwrap_or_else(|| Err("its vertex program was not read".into()))
    };
    let mut image = |stage: usize, bitmap: TagId, image: u32| -> Option<Image> {
        o.decoder.decode(&mut o.map, &mut o.raw.0, &mut o.images, bitmap, image, stage, true).ok()?;
        o.images.get(&(bitmap, image)).cloned()
    };
    let engine = EngineStages {
        texaccum: true,
        normalization: o.normalization.as_ref().is_some_and(Option::is_some),
        lightmap: o.lightmap.clone().and_then(Result::ok),
    };
    lit_model(&sections, materials, &formats, &engine, &mut vertex, &mut image)
}

/// What an object's layer-3 draw samples where its pass reads the
/// lightmap (`native_render::object_program::OBJECT_LIGHTMAP`): the
/// globals' rasterizer bitmap at `+0x64`, decoded as stage 3 samples it,
/// when it is uniform (then the coordinates the draw adds do not matter).
fn object_lightmap(o: &mut Opened) -> Result<Image, String> {
    let lib = o.lib.as_ref().and_then(Option::as_ref).ok_or("the shader tags do not read")?;
    let matg = o
        .map
        .tags()
        .iter()
        .find(|t| t.group.0 == *b"matg")
        .and_then(|t| t.location.as_ref().map(|l| l.address))
        .ok_or("no matg")?;
    let read = |a: u32, n: usize| lib.metadata().bytes(a, n).ok().map(<[u8]>::to_vec);
    let bitmap = crate::native_lights_glue::rasterizer_bitmap(&read, matg, native_render::object_program::OBJECT_LIGHTMAP)
        .ok_or("the globals' rasterizer data names no bitmap at +0x64")?;
    o.decoder.decode(&mut o.map, &mut o.raw.0, &mut o.images, bitmap, 0, 3, false)?;
    let image = o.images.get(&(bitmap, 0)).cloned().ok_or("its image does not decode")?;
    if !uniform(&image) {
        return Err(format!("the globals' rasterizer bitmap {bitmap:?} ({}x{}) is not uniform, so the coordinates the draw adds (the BSP's stream 0x2e) would matter", image.width, image.height));
    }
    Ok(image)
}

/// Vertex shader `vs`'s program and input semantics for a section of
/// geometry classification `geometry` and influences `influences`, as
/// `0x0001cb20` picks its classification.
fn object_vertex_program(o: &mut Opened, vs: TagId, geometry: u16, influences: u8) -> Result<(Vec<u8>, Vec<u16>), String> {
    let address = o.map.tag(vs).ok().and_then(|t| t.location).map(|l| l.address).ok_or("the vertex shader tag has no address")?;
    let lib = o.lib.as_ref().and_then(Option::as_ref).ok_or("the shader tags do not read")?;
    let meta = lib.metadata();
    let count = look::classification_count(meta, address).ok_or("the vertex shader's classifications do not read")?;
    let class = object_streams::classification(geometry, influences, count).ok_or_else(|| format!("geometry classification {geometry}"))?;
    let code = look::vertex_program(meta, address, class).ok_or_else(|| format!("classification {class} has no program"))?;
    let semantics = look::vertex_semantics(meta, address, class).ok_or_else(|| format!("classification {class}'s inputs do not read"))?;
    Ok((code, semantics))
}

/// The native set's entries of a model's lit parts, by (section, part):
/// the part's shader (tag index), its implementations, and those whose
/// program samples the texture accumulation target.
fn set_parts(lit: &LitModelInput) -> Vec<((u16, u8), SetPart)> {
    lit.parts
        .iter()
        .map(|p| {
            let mut e = SetPart { shader: p.shader_tag as u16, ..SetPart::default() };
            for (n, slot) in p.implementations.iter().enumerate() {
                let Some((program, _)) = slot else { continue };
                e.implementations |= 1 << n;
                if lit.programs.get(*program).is_some_and(|x| x.pass.texaccum.contains(&true)) {
                    e.texaccum |= 1 << n;
                }
            }
            ((p.section, p.part), e)
        })
        .collect()
}

/// One view's objects: the draws planned at the hook, in the list's order.
struct ObjectsPass {
    models: Arc<Models>,
    camera: RenderCamera,
    /// The frames handed with the items (the probe's spheres).
    objects: Vec<ObjectFrame>,
    /// The planned draws with their model index, and each one's object.
    draws: Vec<(u32, PlannedDraw)>,
    owners: Vec<usize>,
    /// The items left to D3D, with their object and why (the probe's).
    refused: Vec<(usize, String)>,
    /// The comparison's part, armed at the encode (with the view).
    compare: Option<(u32, PlannedDraw, String)>,
    /// The view's constants, for the lit parts' programs.
    view: Option<[[f32; 4]; d3d::VIEW_CONSTANTS]>,
    /// The device's W-buffer clip at the pass (`d3d::DepthClip`), as the
    /// world's: the range the D3D path draws the layer with (the clamp is
    /// each draw's, `object_state`). Without it the programs clipped w at
    /// 0, not at the view's near (03a: 256 depth units), and drew the part
    /// of an object nearer than that, which the D3D path culls.
    w_clip: Option<([f32; 2], bool)>,
    fallback: Option<Arc<d3d::ResolvedDraw>>,
    walk: bool,
    run: u64,
}

impl d3d::ExternalPass for ObjectsPass {
    fn batch_key(&self) -> Option<u64> { self.walk.then_some(self.run) }

    fn uses_validated_resources(&self) -> bool { self.fallback.is_some() }

    fn resolved_fallback(&self) -> Option<Arc<d3d::ResolvedDraw>> {
        self.fallback.clone()
    }

    fn view_constants(&mut self, constants: &[[f32; 4]; d3d::VIEW_CONSTANTS]) {
        self.view = Some(*constants);
    }

    fn depth_clip(&mut self, clip: d3d::DepthClip) {
        self.w_clip = Some((clip.range, clip.clamp));
    }

    fn sampled(&self) -> Vec<u32> {
        if self.draws.iter().any(|(_, d)| d.texaccum) || self.compare.as_ref().is_some_and(|c| c.1.texaccum) {
            vec![native_render::object_program::TEXACCUM_HEADER]
        } else {
            Vec::new()
        }
    }

    fn writes(&self) -> d3d::ExternalWrites {
        if self.fallback.is_none() && !self.walk {
            return d3d::ExternalWrites::default();
        }
        d3d::ExternalWrites {
            color: self.draws.iter().any(|(_, d)| !d.state.color_writes.is_empty()),
            depth_stencil: self.draws.iter().any(|(_, d)| {
                use gfx::state::StencilOp;
                let stencil = d.state.stencil;
                d.state.depth_write || (stencil.enable && stencil.write_mask != 0
                    && [stencil.fail, stencil.depth_fail, stencil.pass].iter().any(|&op| op != StencilOp::Keep))
            }),
        }
    }

    fn prepare_copy(&mut self, t: &d3d::ExternalTarget<'_>) -> Result<(), &'static str> {
        if self.view.is_none() { return Err("object view constants unavailable"); }
        if self.draws.iter().any(|(_, d)| d.texaccum) && texaccum_level(t.sampled, t.guest_size, t.size).is_none() {
            return Err("object texture accumulation unavailable");
        }
        let lit = self.models.lit.lock().unwrap_or_else(|e| e.into_inner());
        if lit.as_ref().is_some_and(|l| self.draws.iter().all(|(model, d)| l.has(*model) && !d.indices.is_empty())) {
            Ok(())
        } else {
            Err("planned object geometry unavailable")
        }
    }

    fn record_ranges(&mut self, t: d3d::ExternalTarget<'_>) -> usize {
        let Some(constants) = self.view else { return 0 };
        let mut lit = self.models.lit.lock().unwrap_or_else(|e| e.into_inner());
        let Some(lit) = lit.as_mut() else { return 0 };
        let texaccum = texaccum_level(t.sampled, t.guest_size, t.size);
        let view = self.view_inputs(constants, &t);
        if let Some((model, d, label)) = &self.compare {
            arm_compare(lit, *model, d, label, &view, texaccum);
        }
        if let Some(at) = probe_at()
            && probe_due()
        {
            let vp = self.camera.view_projection();
            let size = (t.guest_size.0 as f32, t.guest_size.1 as f32);
            for (k, o) in self.objects.iter().enumerate() {
                if sphere_covers(&vp, o.sphere, size, at).is_none() {
                    continue;
                }
                for ((model, d), _) in self.draws.iter().zip(&self.owners).filter(|(_, owner)| **owner == k) {
                    eprintln!("native-objects probe at {at:?}: definition {:#010x} handle {:#010x}: drawn: {}", o.definition, o.handle, lit.describe(*model, d));
                }
                for (_, why) in self.refused.iter().filter(|(owner, _)| *owner == k) {
                    eprintln!("native-objects probe at {at:?}: definition {:#010x} handle {:#010x}: left to D3D: {why}", o.definition, o.handle);
                }
            }
            eprintln!(
                "native-objects probe at {at:?}: texture accumulation level {:?} (guest {:?}, copy {:?})",
                t.sampled.first().map(|l| l.as_ref().map(|l| l.size)),
                t.guest_size,
                t.size
            );
        }
        0
    }

    fn record_bundles(&mut self, t: &mut d3d::ExternalTarget<'_>) -> Option<d3d::backend::ExternalBundles> {
        let constants = self.view?;
        let view = self.view_inputs(constants, t);
        let mut lit = self.models.lit.lock().ok()?;
        let texaccum = texaccum_level(t.sampled, t.guest_size, t.size);
        if let Some(accum) = &texaccum
            && (accum.view.texture() == t.color.texture() || accum.view.texture() == t.depth.texture())
        {
            return None;
        }
        lit.as_mut()?.bundles(t.device, t.queue, t.uploads, t.color_format, t.depth_format,
            t.size, &view, &self.draws, texaccum, self.run)
    }
}

/// `H2_NATIVE_OBJECTS_PROBE=x,y` (guest pixels): on the first passes and
/// every 250th after, each object of the pass whose bounding sphere covers
/// that pixel, with the sections its records draw, the sections its
/// native parts come from, and the texture accumulation level's size.
fn probe_at() -> Option<(f32, f32)> {
    static AT: std::sync::OnceLock<Option<(f32, f32)>> = std::sync::OnceLock::new();
    *AT.get_or_init(|| {
        let at = std::env::var("H2_NATIVE_OBJECTS_PROBE").ok().and_then(|v| parse_probe(&v));
        if let Some(at) = at {
            eprintln!("native-objects: probing guest pixel {at:?}");
        }
        at
    })
}

impl ObjectsPass {
    /// The view the lit parts' programs draw with: the device's constants,
    /// the target's sizes, and its W-buffer clip.
    fn view_inputs(&self, constants: [[f32; 4]; d3d::VIEW_CONSTANTS], t: &d3d::ExternalTarget<'_>) -> ViewInputs {
        ViewInputs {
            constants,
            guest_size: t.guest_size,
            size: t.size,
            z_scale: t.z_scale,
            w_clip: self.w_clip,
        }
    }
}

/// `H2_NATIVE_OBJECTS_COMPARE=definition:part[:after]`: one lit part
/// compared with the D3D path's draw of it (`d3d::uniform_compare`, as
/// the world's `H2_NATIVE_WORLD_DUMP`). The definition's items keep their
/// D3D draws (`NativeSet::shadowed`) and are planned as the pass would
/// draw them; on the pass after `after` (default 0) that plans its part
/// `part` (an index into the model's lit parts, one per section and
/// geometry part; or, written `0x...`, the first item whose shader is that
/// tag, e.g. `0xe92207ad:0xe5da0465:30` for 03a's rebar), that draw is
/// armed, and
/// the D3D draws with the same node rows are compared (the part's own
/// program field by field; the object's other draws, its other layers
/// among them, named with their cull and depth).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Compare {
    definition: u16,
    part: Part,
    after: u32,
}

/// Which of the compared definition's parts ([`Compare`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    /// An index into the model's lit parts.
    Index(usize),
    /// The first item whose shader has this tag index.
    Shader(u16),
}

impl Part {
    fn picks(self, part: usize, shader: u32) -> bool {
        match self {
            Part::Index(i) => i == part,
            Part::Shader(s) => s == shader as u16,
        }
    }
}

fn compare() -> Option<Compare> {
    static C: std::sync::OnceLock<Option<Compare>> = std::sync::OnceLock::new();
    *C.get_or_init(|| {
        let c = std::env::var("H2_NATIVE_OBJECTS_COMPARE").ok().and_then(|v| parse_compare(&v));
        if let Some(c) = c {
            eprintln!("native-objects: comparing {c:?} with the D3D path's draws (its objects left to D3D)");
        }
        c
    })
}

fn parse_compare(v: &str) -> Option<Compare> {
    let mut it = v.trim().split(':');
    let definition = *parse_definitions(it.next()?).first()?;
    let part = match it.next().map(str::trim) {
        None => Part::Index(0),
        Some(p) => match p.strip_prefix("0x").or_else(|| p.strip_prefix("0X")) {
            Some(hex) => Part::Shader(u32::from_str_radix(hex, 16).ok()? as u16),
            None => Part::Index(p.parse().ok()?),
        },
    };
    let after = it.next().map_or(Some(0), |a| a.trim().parse().ok())?;
    Some(Compare { definition, part, after })
}

/// Arms the comparison once, on the pass after the comparison's `after`
/// that planned its part: `d` of model `model`, named `label`.
fn arm_compare(lit: &LitObjects, model: u32, d: &PlannedDraw, label: &str, view: &ViewInputs, texaccum: Option<Texaccum<'_>>) {
    static SEEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let Some(c) = compare() else { return };
    if SEEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed) != c.after {
        return;
    }
    let label = label.to_owned();
    match lit.part_uniforms(model, d, view, texaccum) {
        Ok(u) => {
            eprintln!(
                "native-objects compare {label}: {}; {} program words, rows c[50..] {:?}, decompression c[170..173] {:?}, textures {:?}",
                lit.describe(model, d),
                u.words.len(),
                &u.vertex.constants[50..53],
                &u.vertex.constants[170..173],
                u.textures
            );
            d3d::uniform_compare::arm(d3d::uniform_compare::Expected {
                label,
                words: u.words,
                vertex: u.vertex,
                pixel: u.pixel,
                options: u.options,
                def: u.def,
                pixel_options: u.pixel_options,
                primitive: u.primitive,
                depth: (u.depth.0, u.depth.1, Default::default()),
                color: None,
                textures: u.textures,
                samplers: u.samplers,
                draws: 4,
            });
        }
        Err(e) => eprintln!("native-objects compare {label}: {e}"),
    }
}

/// `H2_NATIVE_OBJECTS_EXCEPT=0xe92207ad,...`: definitions (tag datums,
/// or their low words) left to the D3D path, to tell which object a
/// difference belongs to.
fn excepted(definition: u32) -> bool {
    static EXCEPT: std::sync::OnceLock<Vec<u16>> = std::sync::OnceLock::new();
    EXCEPT
        .get_or_init(|| std::env::var("H2_NATIVE_OBJECTS_EXCEPT").map(|v| parse_definitions(&v)).unwrap_or_default())
        .contains(&(definition as u16))
}

/// A comma-separated list of definitions, hex (`0x` optional) datums or
/// tag indices: their tag indices.
fn parse_definitions(v: &str) -> Vec<u16> {
    v.split(',')
        .filter_map(|d| {
            let d = d.trim();
            let d = d.strip_prefix("0x").or_else(|| d.strip_prefix("0X")).unwrap_or(d);
            u32::from_str_radix(d, 16).ok().map(|x| x as u16)
        })
        .collect()
}

fn probe_due() -> bool {
    static PROBED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = PROBED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    n < 8 || n % 250 == 0
}

fn parse_probe(v: &str) -> Option<(f32, f32)> {
    let (x, y) = v.split_once(',')?;
    let (x, y) = (x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?);
    (x.is_finite() && y.is_finite()).then_some((x, y))
}

/// Whether `sphere` (centre, radius), seen through `view_proj` on a
/// screen of `size` pixels, covers pixel `at`: the centre's distance in
/// pixels, the projected radius (the farthest of its six axis points from
/// the centre on screen) and the centre's view depth `w`. A camera inside
/// or at the sphere covers every pixel (radius infinite).
fn sphere_covers(view_proj: &native_render::camera::Mat4, sphere: ([f32; 3], f32), size: (f32, f32), at: (f32, f32)) -> Option<(f32, f32, f32)> {
    let (c, r) = sphere;
    let clip = |p: [f32; 3]| native_render::camera::transform(view_proj, [p[0], p[1], p[2], 1.0]);
    let screen = |v: [f32; 4]| ((v[0] / v[3] + 1.0) * 0.5 * size.0, (1.0 - v[1] / v[3]) * 0.5 * size.1);
    let centre = clip(c);
    let (px, py) = (at.0 + 0.5, at.1 + 0.5);
    if centre[3] <= r {
        return Some((0.0, f32::INFINITY, centre[3]));
    }
    let (sx, sy) = screen(centre);
    let mut radius = 0f32;
    for axis in 0..3 {
        for sign in [-1.0, 1.0] {
            let mut p = c;
            p[axis] += sign * r;
            let v = clip(p);
            if v[3] <= 0.0 {
                return Some((0.0, f32::INFINITY, centre[3]));
            }
            let (x, y) = screen(v);
            radius = radius.max(((x - sx).powi(2) + (y - sy).powi(2)).sqrt());
        }
    }
    let distance = ((px - sx).powi(2) + (py - sy).powi(2)).sqrt();
    (distance <= radius).then_some((distance, radius, centre[3]))
}

/// The texture accumulation level the pass was given, with its size in
/// guest texels (this copy's texels over the colour's scale).
fn texaccum_level(sampled: &[Option<d3d::SampledLevel>], guest_size: (u32, u32), size: (u32, u32)) -> Option<Texaccum<'_>> {
    let level = sampled.first()?.as_ref()?;
    Some(Texaccum { view: &level.view, guest_size: guest_texels(level.size, guest_size, size) })
}

/// `texels` of a level in a copy whose colour target is `size` texels for
/// `guest` guest pixels, in guest texels.
fn guest_texels(texels: (u32, u32), guest: (u32, u32), size: (u32, u32)) -> (u32, u32) {
    let scale = |t: u32, g: u32, s: u32| (u64::from(t) * u64::from(g) / u64::from(s.max(1))) as u32;
    (scale(texels.0, guest.0, size.0), scale(texels.1, guest.1, size.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lit parts are W-buffered programs as the world's: they clip w
    /// to the range the device draws the layer with, not from 0, or an
    /// object nearer than the view's near is drawn where the D3D path
    /// culls it.
    #[test]
    fn an_object_draw_is_keyed_by_owner_record_shader_part_and_implementation() {
        let geometry = Geometry {
            shader: 0xe000_0001, variant: 0, implementation: 2, sorted_slot: 16,
            record: 5, part: 3, owner: Owner::Object { handle: 9, model: 18, section: 4 },
        };
        assert_eq!(object_key(geometry), Some((9, ItemKey {
            record: 5, shader: 0xe000_0001, section: 4, part: 3, implementation: 2,
        })));
        assert!(object_key(Geometry { owner: Owner::Cluster(4), ..geometry }).is_none());
    }

    #[test]
    fn the_lit_parts_clip_w_as_the_device_draws_the_layer() {
        use d3d::ExternalPass;
        let camera = RenderCamera {
            position: [0.0; 3],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov: 1.0,
            viewport: [0, 0, 480, 640],
            near: 0.01,
            far: 100.0,
        };
        let mut pass = ObjectsPass {
            models: Arc::new(Models::default()),
            camera,
            objects: Vec::new(),
            draws: Vec::new(),
            owners: Vec::new(),
            refused: Vec::new(),
            compare: None,
            view: None,
            w_clip: None,
            fallback: None,
            walk: false,
            run: 0,
        };
        pass.depth_clip(d3d::DepthClip { range: [256.0, 16_777_215.0], clamp: false, w_buffered: Some(true) });
        assert_eq!(pass.w_clip, Some(([256.0, 16_777_215.0], false)));
        assert!(pass.sampled().is_empty(), "no draw samples the texture accumulation level");
        let constants = [[0.0; 4]; d3d::VIEW_CONSTANTS];
        let view = ViewInputs { constants, guest_size: (640, 480), size: (640, 480), z_scale: 16_777_215.0, w_clip: pass.w_clip };
        let u = native_render::program::frame_vertex(&Default::default(), &view);
        assert_eq!(u.w_clip, Some([256.0, 16_777_215.0]), "near at the device's 256, not 0");
    }

    #[test]
    fn a_comparison_names_a_definition_a_part_and_a_pass() {
        assert_eq!(parse_compare("0xe92207ad:2:30"), Some(Compare { definition: 0x07ad, part: Part::Index(2), after: 30 }));
        assert_eq!(parse_compare("e92207ad"), Some(Compare { definition: 0x07ad, part: Part::Index(0), after: 0 }));
        assert_eq!(parse_compare("0xe92207ad:0xe5da0465:30"), Some(Compare { definition: 0x07ad, part: Part::Shader(0x0465), after: 30 }));
        assert_eq!(parse_compare("0xe92207ad:x"), None);
        assert!(Part::Shader(0x0465).picks(7, 0xe5da_0465));
        assert!(!Part::Index(2).picks(7, 0xe5da_0465));
    }

    #[test]
    fn excepted_definitions_parse_as_tag_indices() {
        assert_eq!(parse_definitions("0xe92207ad, e703058e,zz,0X07FF"), vec![0x07ad, 0x058e, 0x07ff]);
        assert!(parse_definitions("").is_empty());
    }

    #[test]
    fn the_probe_names_the_objects_whose_sphere_covers_its_pixel() {
        assert_eq!(parse_probe(" 575, 410"), Some((575.0, 410.0)));
        assert_eq!(parse_probe("575"), None);
        // Identity but w = 2: x and y over 2 across the screen.
        let mut vp = [[0.0f32; 4]; 4];
        for (i, c) in vp.iter_mut().enumerate() {
            c[i] = 1.0;
        }
        vp[3][3] = 2.0;
        let size = (640.0, 480.0);
        // Centre at (320, 240); radius 0.2 over w 2: 32 px across, 24 down.
        let (d, r, w) = sphere_covers(&vp, ([0.0, 0.0, 0.0], 0.2), size, (330.0, 240.0)).expect("covered");
        assert!((d - 10.5).abs() < 0.1 && (r - 32.0).abs() < 0.1 && w == 2.0, "{d} {r} {w}");
        assert_eq!(sphere_covers(&vp, ([0.0, 0.0, 0.0], 0.2), size, (400.0, 240.0)), None, "outside");
        assert_eq!(sphere_covers(&vp, ([0.0, 0.0, 0.0], 3.0), size, (0.0, 0.0)).map(|c| c.1), Some(f32::INFINITY), "the camera inside");
    }

    #[test]
    fn a_levels_texels_under_the_render_scale_are_its_guest_size() {
        assert_eq!(guest_texels((640, 480), (640, 480), (640, 480)), (640, 480), "1x");
        assert_eq!(guest_texels((1280, 960), (640, 480), (1280, 960)), (640, 480), "2x");
        assert_eq!(guest_texels((320, 240), (640, 480), (1280, 960)), (160, 120), "a smaller level, scaled");
    }

    fn map_state(name: &str, generation: u64, models: &Arc<Models>) -> MapState {
        MapState {
            map_generation: 0,
            generation,
            models: Arc::clone(models),
            set: Arc::new(NativeSet { map: name.to_owned(), generation, ..NativeSet::default() }),
        }
    }

    /// A worker publishes only to the load it was started for: after A,
    /// B and A again, the first A's worker (still finishing) finds a load
    /// of the same name but other models, and publishes nothing there.
    #[test]
    fn a_worker_publishes_only_to_its_own_load() {
        let first = Arc::new(Models::default());
        let again = Arc::new(Models::default());
        assert!(publishes_to(&map_state("03a_oldmombasa", 1, &first), &first));
        assert!(!publishes_to(&map_state("03a_oldmombasa", 3, &again), &first), "the same name, another load");
        assert!(publishes_to(&map_state("03a_oldmombasa", 3, &again), &again));
    }

    fn frame(definition: u32) -> ObjectFrame {
        ObjectFrame {
            handle: 0x8001_0001,
            definition,
            nodes: Vec::new(),
            sphere: ([0.0; 3], 1.0),
            lighting: None,
            defaults: None,
        }
    }

    fn item(object: usize, record: u16, shadow: bool) -> ItemDraw {
        ItemDraw {
            index: object,
            object,
            key: native_render::objects::ItemKey { record, shader: 0xe000_0010, section: 0, part: 0, implementation: 0 },
            rows: vec![[0.0; 4]; 3],
            shadow,
        }
    }

    #[test]
    fn sorted_walk_only_skips_render_states_that_are_already_live() {
        use tags::shader::RenderStateValue;
        let states = [
            RenderStateValue { state: 11, value: 13 },
            RenderStateValue { state: 19, value: 20 },
        ];
        assert!(stable_render_states(&states, |slot| match slot {
            11 => Some(13), 19 => Some(20), _ => None,
        }));
        assert!(!stable_render_states(&states, |slot| (slot == 11).then_some(13)));
        assert!(!stable_render_states(&states, |slot| (slot == 19).then_some(20)));
        assert!(!stable_render_states(&states, |_| Some(12)));
    }

    #[test]
    fn object_walk_supplies_fog_and_specular_for_the_implicit_combiner() {
        let mut words = [1, 3, 10.0f32.to_bits(), 30.0f32.to_bits(), 0, 0, 0xff12_3456];
        let (fog, color) = object_fog(words, true, Some(1)).unwrap();
        assert_eq!(shaders::psh::fog::factor_from_uniform(fog, 25.0), 0.25);
        assert_eq!(fog[3], 1.0);
        assert_eq!(color, shaders::psh::argb(words[6]));
        words[0] = 0;
        let (fog, _) = object_fog(words, true, Some(0)).unwrap();
        assert_eq!(shaders::psh::fog::factor_from_uniform(fog, 25.0), 1.0);
        assert_eq!(fog[3], 0.0);
        assert!(object_fog(words, true, None).is_none());
        assert!(object_fog(words, false, None).is_some());
    }

    /// Every item the plan cannot resolve is refused, with why, before the
    /// hook returns: a model not uploaded yet, or a device not
    /// W-buffered. Nothing is drawn then, and a shadow item never is.
    #[test]
    fn an_item_whose_draw_does_not_resolve_is_refused_at_the_hook() {
        let models = Models::default();
        let objects = vec![frame(0xe000_0100)];
        let entry = LayerEntry { states: EntryStates::default(), w_buffered: true };
        let plan = plan_view(&models, &objects, &[item(0, 1, false), item(0, 2, true)], &entry, true);
        assert!(plan.drawn.is_empty() && plan.draws.is_empty() && plan.compare.is_none());
        assert_eq!(plan.refused.len(), 2);
        assert!(plan.refused[0].1.contains("record 1") && plan.refused[0].1.contains("not uploaded"), "{:?}", plan.refused);
        models.index.lock().unwrap().insert(0x0100, 7);
        let plan = plan_view(&models, &objects, &[item(0, 1, false)], &entry, true);
        assert!(plan.refused[0].1.contains("no model is uploaded"), "{:?}", plan.refused);
        let z = LayerEntry { w_buffered: false, ..entry };
        let plan = plan_view(&models, &objects, &[item(0, 1, false)], &z, true);
        assert!(plan.refused[0].1.contains("W-buffered"), "{:?}", plan.refused);
    }

    /// The set is keyed by what a record names: (section, part), each
    /// with its shader, its implementations and those sampling the
    /// texture accumulation target.
    #[test]
    fn a_models_lit_parts_enter_the_set_by_section_and_part() {
        use native_render::object_program_gpu::LitPart;
        let part = |section, part, implementations| LitPart {
            section,
            part,
            decompression: [[0.0; 4]; 3],
            shader_tag: 0xe5da_0465,
            shader: Default::default(),
            indices: 0..3,
            implementations,
        };
        let lit = LitModelInput {
            parts: vec![part(2, 0, [Some((0, 0)), None, Some((1, 0)), None, None]), part(2, 3, [None, Some((2, 0)), None, None, None])],
            ..LitModelInput::default()
        };
        let parts: HashMap<(u16, u8), SetPart> = set_parts(&lit).into_iter().collect();
        assert_eq!(parts[&(2, 0)], SetPart { shader: 0x0465, implementations: 0b101, texaccum: 0 });
        assert_eq!(parts[&(2, 3)].implementations, 0b010);
        assert_eq!(parts.len(), 2);
    }
}
