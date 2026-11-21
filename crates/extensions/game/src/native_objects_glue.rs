//! Third-person object programs for the scene's layer 3.
//!
//! The activation worker reads the mounted tag index and uploads each
//! supported mode once. The sorted scene walk resolves every part of a
//! run before reserving it for every live target copy.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

use engine::memory::GuestMemory;
use render_bridge::native_map_work;
use render_bridge::native_objects::Host;
use map::TagId;
use frame_scene::{FrameScene, InstanceKey, PartFlags, View};
use native_render::gpu::Image;
use native_render::light_gpu::StaticLightTexture;
use native_render::load;
use native_render::look;
use native_render::object_lighting::ShaderValues;
use native_render::object_nodes::{Compression, decompression_rows};
use native_render::object_program_gpu::{
    EngineStages, LitBuild, LitMaterial, LitModelInput, LitObjects, LitSection, PlannedDraw, Texaccum, lit_model, lit_subparts, uniform,
};
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
    /// The part's first subpart and subpart count, which a record choosing
    /// subparts names (`frame_scene::Subparts`).
    first_subpart: u16,
    subpart_count: u16,
}

impl PartRange {
    fn matches(self, part: WalkPart) -> bool {
        self.geometry_class == part.geometry_class
            && self.node_influences == part.node_influences
            && self.first == part.first_index
            && self.count == part.index_count
            && self.triangle_list == part.triangle_list
            && part.subparts.is_none_or(|s| s.first == self.first_subpart && u16::from(s.count) == self.subpart_count)
    }
}

struct ModelCoverage {
    nodes: usize,
    ranges: HashMap<(u16, u8), PartRange>,
}

#[derive(Default)]
struct Models {
    retire: Option<native_map::RetireQueue>,
    lit: Mutex<Option<LitObjects>>,
    /// Definition index to render-model datum, shared across definitions.
    index: HashMap<u16, u32>,
    coverage: HashMap<TagId, ModelCoverage>,
    palettes: StagePalettes,
}

impl Drop for Models {
    fn drop(&mut self) {
        if let Some(send) = self.retire.take() {
            native_map::retire(send, timeline::setup::Stage::Objects, self);
        }
    }
}

struct MapState {
    name: String,
    generation: u64,
    job: Arc<()>,
    /// The frame during which the worker finished, and its resident models.
    published: Option<(u64, Arc<Models>)>,
}

impl MapState {
    fn resident(&self, frame: u64) -> Option<Arc<Models>> {
        let (finished, models) = self.published.as_ref()?;
        (frame > *finished).then(|| Arc::clone(models))
    }

    fn owns(&self, job: &Arc<()>, generation: u64) -> bool {
        self.generation == generation && Arc::ptr_eq(&self.job, job)
    }
}

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
    gpu: (wgpu::Device, wgpu::Queue),
}

impl<B: d3d::backend::Backend + Send + 'static> Glue<B> {
    fn load(&self, mem: &dyn GuestMemory) -> Option<(Arc<Models>, u64)> {
        let generation = native_map_work::generation();
        let mut field = [0; 32];
        let name = crate::health::read_map_name(mem, &mut field).ok()?;
        if name.is_empty() {
            return None;
        }
        let current = |s: &MapState| s.generation == generation && s.name == name;
        {
            let state = self.state.lock().ok()?;
            if let Some(s) = state.as_ref().filter(|s| current(s)) {
                return Some((s.resident(timeline::frames())?, generation));
            }
        }
        let _dispatch = timeline::setup::dispatch();
        let palettes = self.device.lock().ok().map(|d| scene_load_palettes(&d, mem)).unwrap_or_default();
        let formats = StreamFormats::read(&Ro(mem));
        let mut state = self.state.lock().ok()?;
        if generation != native_map_work::generation() {
            return None;
        }
        if let Some(s) = state.as_ref().filter(|s| current(s)) {
            return Some((s.resident(timeline::frames())?, generation));
        }
        let name = name.into_owned();
        let job = Arc::new(());
        let retired = state.replace(MapState {
            name: name.clone(),
            generation,
            job: Arc::clone(&job),
            published: None,
        });
        drop(state);
        let state = Arc::clone(&self.state);
        let gpu = self.gpu.clone();
        let launched = background::spawn("native-objects load", move || {
            let _timer = timeline::setup::Stage::Objects.worker();
            // Destruction of the old map's textures and buffers also belongs
            // off the guest thread. Already committed passes own their models.
            drop(retired);
            load_objects(&name, generation, &job, &state, palettes, formats, gpu);
        });
        if let Err(e) = launched {
            eprintln!("native-objects: could not start map loader ({e})");
        }
        None
    }
}

impl<B: d3d::backend::Backend + Send + 'static> Host for Glue<B> {
    fn preload(&self, mem: &dyn GuestMemory) {
        let _ = self.load(mem);
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
        let (models, generation) = self.load(mem).ok_or(ObjectWalk::Model)?;
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
        // do: the frame step opened it with one (render_bridge::scene_step).
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
                if state.as_ref().is_some_and(|s| {
                    s.generation == generation
                        && s.published.as_ref().is_some_and(|(_, current)| Arc::ptr_eq(current, &models))
                }) && generation == native_map_work::generation() =>
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
    // A part toggling the auxiliary state draws with the view's rows and
    // clip on (`frame_scene::AuxDepth`); its epilogue leaves them off.
    // Skipping its handler is state-neutral only if the device already
    // holds them off.
    let aux = run.iter().enumerate().any(|(n, e)| {
        matches!(e, Entry::Object { .. })
            && scene.part_at(view, (first + n) as u32).is_some_and(|(_, p)| p.flags.contains(PartFlags::AUX_DEPTH))
    });
    if aux && !view.aux_depth.is_some_and(|a| aux_depth_at_rest(device, mem, &a)) {
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
    let mut lit = models.lit.try_lock().map_err(|_| ObjectWalk::Model)?;
    let lit = lit.as_mut().ok_or(ObjectWalk::Model)?;
    let guest = Ro(mem);
    let mut draws = Vec::with_capacity(run.len());
    for (slot, &item) in (first..).zip(run) {
        let (handle, part) = match item {
            Entry::Excluded => continue,
            Entry::Object { handle, part: Some(part) } => (handle, part),
            _ => return Err(ObjectWalk::Unsupported),
        };
        let instance = scene.instance(view, InstanceKey(handle)).ok_or(ObjectWalk::Inputs)?;
        let model = *models.index.get(&(instance.definition as u16)).ok_or(ObjectWalk::Model)?;
        let cover = models.coverage.get(&TagId(model)).ok_or(ObjectWalk::Model)?;
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
        if scene_part.flags.contains(PartFlags::AUX_DEPTH) {
            draw.aux_depth = Some(view.aux_depth.ok_or(ObjectWalk::Inputs)?);
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
    let (gpu, color, depth) = device.external_pipeline_target(&read).ok_or(ObjectWalk::Reservation)?;
    if !lit.prewarm_draws(&gpu, color, depth, &draws) {
        return Err(ObjectWalk::Reservation);
    }
    Ok(draws)
}

/// Whether the device holds what the auxiliary state toggle leaves off
/// (`0x00012d50` with BL 0, `aux.rest_*`): the cached byte 0, `c[0..3]`
/// the rows off, and the program clip range off, owned by the game and
/// pushed. Every comparison is of the bits.
fn aux_depth_at_rest<B: d3d::backend::Backend>(
    device: &d3d::Device<B>, mem: &dyn GuestMemory, aux: &frame_scene::AuxDepth,
) -> bool {
    use d3d::layout::device_field::{CLIP_FLAGS, CLIP_PROGRAM};
    let bits = |r: &[f32]| r.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    let layout = d3d::layout::GuestLayout::HALO2;
    let stored = mem.u32(layout.p_device).ok().and_then(|d| {
        Some((
            [mem.u32(d.checked_add(CLIP_PROGRAM)?).ok()?, mem.u32(d.checked_add(CLIP_PROGRAM + 4)?).ok()?],
            mem.u32(d.checked_add(CLIP_FLAGS)?).ok()?,
        ))
    });
    let read = crate::native_world_glue::ReadMemory(mem);
    let pushed = device.depth_clip(&read, aux.rest_clip[1]).range;
    mem.u8(native_render::aux_depth::CACHED) == Ok(0)
        && (0..4).all(|r| bits(&device.constant(r)) == bits(&aux.rest_rows[r]))
        && stored.is_some_and(|(clip, flags)| clip == [aux.rest_clip[0].to_bits(), aux.rest_clip[1].to_bits()] && flags & 2 != 0)
        && bits(&pushed) == bits(&aux.rest_clip)
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
    let hold = palettes_hold(&models.palettes, &|s| device.stage_palette(&crate::native_world_glue::ReadMemory(mem), s));
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
    if !render_bridge::native_objects::on(&switches) {
        return;
    }
    let Some(gpu) = device.lock().ok().and_then(|d| d.backend.upload_device()) else { return };
    render_bridge::native_objects::set_host(Arc::new(Glue {
        gpu,
        device: Arc::clone(device),
        state: Arc::new(Mutex::new(None)),
    }));
}

/// Visit every object definition in the mounted tag index, including
/// definitions whose first instance will spawn after activation.
fn load_objects(
    name: &str,
    generation: u64,
    job: &Arc<()>,
    state: &Mutex<Option<MapState>>,
    palettes: StagePalettes,
    formats: Option<StreamFormats>,
    (device, queue): (wgpu::Device, wgpu::Queue),
) {
    if generation != native_map_work::generation() {
        return;
    }
    let retire = match native_map::retire_queue() {
        Ok(queue) => queue,
        Err(error) => {
            eprintln!("native-objects: {name}: could not start resource retirement ({error})");
            return;
        }
    };
    let mut opened = match open(name, palettes, formats) {
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
    let owns = |s: &MapState| s.owns(job, native_map_work::generation());
    if definitions.is_empty() {
        let current = state.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(owns);
        if current {
            native_map_work::map_stage_empty(generation, name, native_map_work::Stage::Objects);
        }
        return;
    }
    let mut index = HashMap::new();
    let mut coverage = HashMap::<TagId, Result<ModelCoverage, String>>::new();
    let mut lit = None;
    let mut refused = 0;
    for definition in definitions {
        if !state.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(owns) {
            return;
        }
        let result = (|| -> Result<TagId, String> {
            let model = opened.ctx.resolve_object(definition).map_err(|e| e.to_string())?
                .model.ok_or("the object has no model")?;
            let mode = model.render_model.as_ref().ok_or("the model has no render model")?.id;
            let cover = coverage.entry(mode).or_insert_with(|| {
                let (cpu, cover) = load_model(&mut opened, mode).map_err(|e| e.to_string())?;
                let lit = lit.get_or_insert_with(|| LitObjects::new(&device));
                if let Some(texture) = &cpu.normalization {
                    lit.set_normalization(&device, &queue, texture);
                }
                lit.add_model(&device, &queue, mode.0, cpu.lit);
                if !lit.has(mode.0) {
                    return Err("GPU refused the model's programs or textures".into());
                }
                Ok(cover)
            }).as_ref().map_err(Clone::clone)?;
            if model.node_count as usize != cover.nodes {
                return Err("model and render model disagree on the node count".into());
            }
            Ok(mode)
        })();
        match result {
            Ok(mode) => {
                index.insert(definition.0 as u16, mode.0);
            }
            Err(why) => {
                refused += 1;
                if refused == 1 || hooks::native_verbose() {
                    eprintln!("native-objects: {name}: definition {:#010x} left to D3D ({why})", definition.0);
                }
            }
        }
    }
    let loaded = index.len();
    if lit.is_some() {
        // Flush queued texture writes before offering the models for draws.
        queue.submit([]);
    }
    let models = Arc::new(Models {
        retire: Some(retire),
        lit: Mutex::new(lit),
        coverage: coverage.into_iter().filter_map(|(mode, result)| Some((mode, result.ok()?))).collect(),
        index,
        palettes: opened.decoder.used().clone(),
    });
    {
        let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(s) = state.as_mut().filter(|s| owns(s)) else { return };
        s.published = Some((timeline::frames(), models));
    }
    eprintln!("native-objects: {name}: {loaded} definitions ready, {refused} left to D3D");
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
    let ranges = lit.parts.iter().filter_map(|lit_part| {
        let (section, part) = (lit_part.section, lit_part.part);
        let section_data = geo.sections.get(section as usize)?;
        let p = section_data.geometry.parts.get(part as usize)?;
        Some(((section, part), PartRange {
            geometry_class: section_data.wire.geometry_classification.0,
            node_influences: section_data.wire.unknown_014,
            first: p.first_index,
            count: p.index_count,
            triangle_list: p.is_triangle_list(),
            first_subpart: p.first_subpart,
            subpart_count: p.subpart_count,
        }))
    }).collect();
    let cover = ModelCoverage { nodes: geo.nodes.len(), ranges };
    let cpu = CpuModel { lit, normalization: o.normalization.clone().flatten() };
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
            // A record with bit 6 may draw a choice of them
            // (`frame_scene::Subparts`).
            subparts: g
                .parts
                .iter()
                .enumerate()
                .filter(|(i, p)| *i < 16 && p.is_drawn() && p.kind != load::PART_TRANSPARENT)
                .filter_map(|(k, p)| Some((k as u8, lit_subparts(&g.indices, &g.subparts, p)?)))
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

    #[test]
    fn the_last_queued_run_retires_the_models_once() {
        use std::sync::mpsc;
        let (send, receive) = mpsc::channel();
        let mut models = Models::default();
        models.retire = Some(send);
        let models = Arc::new(models);
        let queued = Arc::clone(&models);
        drop(models);
        assert!(matches!(receive.try_recv(), Err(mpsc::TryRecvError::Empty)));
        drop(queued);
        let (stage, models) = receive.try_recv().unwrap();
        assert!(matches!(stage, timeline::setup::Stage::Objects));
        drop(models);
        assert!(matches!(receive.try_recv(), Err(mpsc::TryRecvError::Disconnected)));
    }

    #[test]
    fn a_completed_upload_is_visible_only_after_a_swap() {
        let mut state = MapState { name: "arena".into(), generation: 3, job: Arc::new(()), published: None };
        assert!(state.resident(10).is_none());
        let models = Arc::new(Models::default());
        state.published = Some((10, Arc::clone(&models)));
        assert!(state.resident(10).is_none());
        assert!(Arc::ptr_eq(&models, &state.resident(11).unwrap()));
    }

    #[test]
    fn a_worker_publishes_only_to_its_own_load() {
        let first = Arc::new(());
        let second = Arc::new(());
        let state = MapState { name: "arena".into(), generation: 3, job: Arc::clone(&second), published: None };
        assert!(!state.owns(&first, 3), "the same name, another load");
        assert!(!state.owns(&second, 4), "a snapshot reset");
        assert!(state.owns(&second, 3));
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

}
