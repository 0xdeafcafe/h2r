//! Third-person object programs for the scene's layer 3.
//!
//! The activation worker reads the mounted tag index and uploads each
//! supported mode once. The sorted scene walk resolves every part of a
//! run before reserving it for every live target copy.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

use engine::memory::GuestMemory;
use hooks::native_map_work;
use hooks::native_objects::{Host, NativeSet, SetPart};
use map::TagId;
use frame_scene::{FrameScene, InstanceKey, PartFlags, View};
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

static STATE_TABLE: std::sync::LazyLock<gfx::xdk::RenderStateTable> =
    std::sync::LazyLock::new(gfx::xdk::RenderStateTable::default);

impl LayerEntry {
    fn read<B: d3d::backend::Backend>(device: &d3d::Device<B>, mem: &dyn GuestMemory) -> Self {
        let table = &*STATE_TABLE;
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

    fn draw_walk(
        &self, mem: &mut dyn GuestMemory, view: Option<(&FrameScene, &View)>, first: usize, run: &[Entry],
        consume: &mut dyn FnMut(&mut dyn GuestMemory) -> bool,
    ) -> Result<(), ObjectWalk> {
        use timeline::native::{Metric, Stage, Timer};
        // With a render thread that records scene stages, the planning here
        // is extraction: the guest and device reads and every check that
        // can refuse, which decide the run the guest skips. The packets,
        // pipelines and uploads are made on the render thread
        // (docs/graphics/framescene.md, slice 2).
        let staging = self.device.lock().is_ok_and(|d| d.queues_scene_stages(Stage::Objects));
        let planning = Timer::start(Stage::Objects, if staging { Metric::ExtractNs } else { Metric::PrepareNs });
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
        // The view needs a render camera that reads, as the layer's draws
        // do: the frame step opened it with one (hooks::scene_step).
        let (scene, scene_view) = view.ok_or(ObjectWalk::Inputs)?;
        let read = crate::native_world_glue::ReadMemory(mem);
        let request = device.external_ready(&read, false).map_err(|_| ObjectWalk::Reservation)?;
        let (width, height) = request.surface_size;
        let (clip, rects) = device.clip_area(&read).ok().flatten().ok_or(ObjectWalk::State)?;
        if !rects.is_empty() || !clip.is_whole(width, height) || request.rect != (0, 0, width, height) {
            return Err(ObjectWalk::State);
        }
        let draws = plan_walk(&models, &device, mem, scene, scene_view, first, run)?;
        let ranges = draws.len();
        let view = mem.u32(engine::render_loop::occlusion::guest::VIEW_INDEX).unwrap_or(u32::MAX);
        let promise = d3d::ExternalPlan { stage: Stage::Objects, view, ranges, slots: Vec::new() };
        let batch = u64::from(device.swap_count()).wrapping_add(1);
        let pass = |draws: Vec<(u32, PlannedDraw)>| {
            Box::new(ObjectsPass { models: Arc::clone(&models), draws, view: None, w_clip: None, run: batch })
        };
        drop(planning);
        // The load the run was planned against, held to the commit.
        let current = || match self.state.lock() {
            Ok(state)
                if state.as_ref().is_some_and(|s| s.map_generation == generation && publishes_to(s, &models))
                    && generation == native_map_work::generation() =>
            {
                Ok(state)
            }
            _ => Err(ObjectWalk::Reservation),
        };
        // A scene stage: queued now, its packets made when the render
        // thread reaches it. Not taken, the run is reserved here as before.
        if staging {
            let reserved = device.reserve_scene_stage(
                &mut crate::native_lights_glue::DeviceMemory(mem), promise.clone(), pass(draws.clone()),
            );
            if let Some(reservation) = reserved.map_err(|_| ObjectWalk::Reservation)? {
                let _state = current()?;
                if !consume(mem) {
                    return Err(ObjectWalk::Reservation);
                }
                reservation.commit();
                return Ok(());
            }
        }
        let ready = device.prepare_external_bound(
            &mut crate::native_lights_glue::DeviceMemory(mem), promise, pass(draws), Vec::new(),
        ).ok().flatten().ok_or(ObjectWalk::Reservation)?;
        let _state = current()?;
        if !consume(mem) {
            return Err(ObjectWalk::Reservation);
        }
        ready.commit();
        Ok(())
    }
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

/// Commit only state-preserving runs. A later guest handler inherits the
/// same render states it would inherit if the skipped programs had run.
/// The guest's own shader/binding cache tracks the last real guest call.
///
/// Plans a run of the walk: the state checks against the device and
/// the guest's handler, and each part's draw from the frame step's view
/// (`scene`, `view`): its object's frame and its rows as extraction read
/// them. `first` is the run's first slot.
#[allow(clippy::too_many_arguments)]
fn plan_walk<B: d3d::backend::Backend>(
    models: &Models, device: &d3d::Device<B>, mem: &dyn GuestMemory,
    scene: &FrameScene, view: &View, first: usize, run: &[Entry],
) -> Result<Vec<(u32, PlannedDraw)>, ObjectWalk> {
    use gfx::state::RenderStateType as R;
    let mut entry = LayerEntry::read(device, mem);
    if !entry.w_buffered {
        return Err(ObjectWalk::State);
    }
    let read = crate::native_world_glue::ReadMemory(mem);
    let flags = mem.u32(native_render::skip::STENCIL_OFF_FLAGS).map_err(|_| ObjectWalk::Inputs)?;
    let stencil_during_draw = flags & 0x00ff_ff00 == 0;
    let in_mode_1 = || mem.u16(SCENE_STENCIL_MODE) == Ok(1) && device.stencil_state(&read) == Some(scene_stencil());
    if !stencil_during_draw {
        // The handler temporarily applies stencil mode 0 and leaves
        // mode 1. Skipping it is state-neutral only if it already holds 1.
        if !in_mode_1() {
            return Err(ObjectWalk::State);
        }
        entry.states.0.retain(|(r, _)| *r != R::StencilEnable);
        entry.states.0.push((R::StencilEnable, 0));
    }
    // A marked part's handler draws in stencil mode 2 and leaves mode 1
    // (`frame_scene::PartFlags::STENCIL_MARK`). Skipping it is
    // state-neutral, and the parts around it draw over the same stencil,
    // only if the device already holds mode 1. With the stencil off the
    // mark changes nothing.
    let marks = stencil_during_draw
        && run.iter().enumerate().any(|(n, e)| {
            matches!(e, Entry::Object { .. })
                && scene.part_at(view, (first + n) as u32).is_some_and(|(_, p)| p.flags.contains(PartFlags::STENCIL_MARK))
        });
    if marks && !in_mode_1() {
        return Err(ObjectWalk::State);
    }
    let fog_words = crate::native_fog_glue::device_fog_words(device, mem);
    let table = &*STATE_TABLE;
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
    // A model upload holds the models for long: the run keeps its D3D
    // draws rather than wait. Otherwise the holder is a scene stage making
    // its packets on the render thread, which is short: wait for it.
    let lit = if models.upload_queued.load(Ordering::Acquire) {
        models.lit.try_lock().map_err(|_| ObjectWalk::Model)?
    } else {
        models.lit.lock().map_err(|_| ObjectWalk::Model)?
    };
    let lit = lit.as_ref().ok_or(ObjectWalk::Model)?;
    let guest = Ro(mem);
    let mut draws = Vec::with_capacity(run.len());
    for (slot, &item) in (first..).zip(run) {
        let (handle, part) = match item {
            Entry::Excluded => continue,
            Entry::Object { handle, part: Some(part) } => (handle, part),
            _ => return Err(ObjectWalk::Unsupported),
        };
        let instance = scene.instance(view, InstanceKey(handle)).ok_or(ObjectWalk::Inputs)?;
        let model = *index.get(&(instance.definition as u16)).ok_or(ObjectWalk::Model)?;
        let cover = coverage.get(&TagId(model)).ok_or(ObjectWalk::Model)?;
        if model != part.mode || instance.nodes.len() != cover.nodes || !lit.has(model) {
            return Err(ObjectWalk::Model);
        }
        if !cover.ranges.get(&(part.key.section, part.key.part)).is_some_and(|range| range.matches(part)) {
            return Err(ObjectWalk::Unsupported);
        }
        let (_, scene_part) = scene.part_at(view, slot as u32)
            .filter(|(owner, p)| owner.key == instance.key && p.key == part.key)
            .ok_or(ObjectWalk::Inputs)?;
        let mut draw = lit.plan_part_of(model, scene, instance, scene_part, &entry.states).map_err(|why| {
            count_refusal(&why);
            ObjectWalk::Inputs
        })?;
        if stencil_during_draw && scene_part.flags.contains(PartFlags::STENCIL_MARK) {
            native_render::object_state::mark(&mut draw.state);
        }
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

/// Stencil mode 1, which layer 3's stencil switches leave.
fn scene_stencil() -> gfx::state::StencilState {
    native_render::object_state::structure_stencil(0)
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
    let switches = hooks::Switches::from_env().unwrap_or_default();
    if !hooks::native_objects::on(&switches) {
        return;
    }
    hooks::native_objects::set_host(Arc::new(Glue {
        device: Arc::clone(device),
        state: Arc::new(Mutex::new(None)),
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
                    loaded += 1;
                }
                Err(why) => {
                    refused += 1;
                    if refused == 1 || hooks::native_verbose() {
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
                if hooks::native_verbose() {
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

/// One sorted run's objects: the draws planned at the walk, in the list's
/// order.
struct ObjectsPass {
    models: Arc<Models>,
    /// The planned draws with their model index.
    draws: Vec<(u32, PlannedDraw)>,
    /// The view's constants, for the lit parts' programs.
    view: Option<[[f32; 4]; d3d::VIEW_CONSTANTS]>,
    /// The device's W-buffer clip at the pass (`d3d::DepthClip`), as the
    /// world's: the range the D3D path draws the layer with (the clamp is
    /// each draw's, `object_state`). Without it the programs clipped w at
    /// 0, not at the view's near (03a: 256 depth units), and drew the part
    /// of an object nearer than that, which the D3D path culls.
    w_clip: Option<([f32; 2], bool)>,
    /// Groups adjacent reservations within a swap: they share one pin of
    /// the attachments (`d3d::ExternalPass::batch_key`).
    run: u64,
}

impl d3d::ExternalPass for ObjectsPass {
    fn batch_key(&self) -> Option<u64> { Some(self.run) }

    fn view_constants(&mut self, constants: &[[f32; 4]; d3d::VIEW_CONSTANTS]) {
        self.view = Some(*constants);
    }

    fn depth_clip(&mut self, clip: d3d::DepthClip) {
        self.w_clip = Some((clip.range, clip.clamp));
    }

    fn sampled(&self) -> Vec<u32> {
        if self.draws.iter().any(|(_, d)| d.texaccum) {
            vec![native_render::object_program::TEXACCUM_HEADER]
        } else {
            Vec::new()
        }
    }

    fn writes(&self) -> d3d::ExternalWrites {
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

    /// The run's packets join the D3D render pass already open on these
    /// attachments: no pass, command buffer or bundle of its own.
    fn record_draws(
        &mut self, t: &d3d::backend::ExternalAttachments, request: d3d::ExternalPassRequest,
    ) -> Option<Result<d3d::backend::ExternalDraws, &'static str>> {
        Some(self.packets(t, request))
    }
}

impl ObjectsPass {
    fn packets(
        &self, t: &d3d::backend::ExternalAttachments, request: d3d::ExternalPassRequest,
    ) -> Result<d3d::backend::ExternalDraws, &'static str> {
        let constants = self.view.ok_or("object view constants unavailable")?;
        let texaccum = texaccum_level(&t.sampled, request.surface_size, t.size);
        if self.draws.iter().any(|(_, d)| d.texaccum) && texaccum.is_none() {
            return Err("object texture accumulation unavailable");
        }
        if let Some(accum) = &texaccum
            && (accum.view.texture() == t.color.texture() || accum.view.texture() == t.depth.texture())
        {
            return Err("object texture accumulation is an attachment");
        }
        let mut lit = self.models.lit.lock().unwrap_or_else(|e| e.into_inner());
        let lit = lit.as_mut().ok_or("planned object geometry unavailable")?;
        if !self.draws.iter().all(|(model, d)| lit.has(*model) && !d.indices.is_empty()) {
            return Err("planned object geometry unavailable");
        }
        let view = ViewInputs {
            constants,
            guest_size: request.surface_size,
            size: t.size,
            z_scale: request.z_scale,
            w_clip: self.w_clip,
        };
        lit.draws(&t.device, &t.queue, &t.uploads, t.color_format, t.depth_format, t.size, &view, &self.draws, texaccum)
    }
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
    fn the_lit_parts_clip_w_as_the_device_draws_the_layer() {
        use d3d::ExternalPass;
        let mut pass = ObjectsPass {
            models: Arc::new(Models::default()),
            draws: Vec::new(),
            view: None,
            w_clip: None,
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
