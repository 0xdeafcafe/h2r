//! Third-person object programs for the scene's layer 3.
//!
//! The activation worker reads the mounted tag index and uploads each
//! supported mode once. The sorted scene walk resolves every part of a
//! run before reserving it for every live target copy.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
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

mod palette;
pub(crate) use palette::{palette_counters, palette_ready};

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
    assets: Arc<palette::GpuAssets>,
    all_parts: bool,
    ranges: HashMap<(u16, u8), PartRange>,
}

#[derive(Default)]
struct Models {
    retire: Option<native_map::RetireQueue>,
    lit: Mutex<Option<LitObjects>>,
    /// Definition index to render-model datum, shared across definitions.
    index: HashMap<u16, u32>,
    /// Per render model, what the walk checks a part against; a live
    /// edit's refresh replaces a model's ([`refresh_models`]).
    coverage: RwLock<HashMap<TagId, ModelCoverage>>,
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
    /// Held only to keep the scenario palette alive: the game-view gate
    /// reaches it through a weak handle ([`palette::activate`]).
    _palette: Arc<palette::Palette>,
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
    /// A live edit's refresh is running ([`refresh_models`]); edits wait
    /// in the queue until it ends, so they apply in order.
    refreshing: Arc<AtomicBool>,
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
        let palette = Arc::new(palette::Palette::default());
        palette::activate(generation, &palette);
        let retired = state.replace(MapState {
            name: name.clone(),
            generation,
            job: Arc::clone(&job),
            _palette: Arc::clone(&palette),
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
            load_objects(&name, generation, &job, &state, &palette, palettes, formats, gpu);
        });
        if let Err(e) = launched {
            eprintln!("native-objects: could not start map loader ({e})");
        }
        None
    }
}

impl<B: d3d::backend::Backend + Send + 'static> Glue<B> {
    /// Starts a live edit's refresh of the resident models
    /// (docs/editor/realtime-edits.md, "The native refresh") when edits
    /// are queued and no refresh is running: on a worker
    /// ([`refresh_models`]), with the stage palettes and stream formats
    /// the device holds now, as a load reads them.
    fn refresh_edits(&self, mem: &dyn GuestMemory, models: &Arc<Models>, generation: u64) {
        use render_bridge::native_edits::{self, Consumer};
        if !native_edits::pending(Consumer::Objects) || self.refreshing.load(Ordering::Acquire) {
            return;
        }
        let name = self.state.lock().ok().and_then(|s| {
            s.as_ref().filter(|s| s.generation == generation).map(|s| s.name.clone())
        });
        let Some(name) = name else { return };
        let edits = native_edits::drain(Consumer::Objects);
        if edits.is_empty() {
            return;
        }
        let palettes = self.device.lock().ok().map(|d| scene_load_palettes(&d, mem)).unwrap_or_default();
        let formats = StreamFormats::read(&Ro(mem));
        self.refreshing.store(true, Ordering::Release);
        let refreshing = Arc::clone(&self.refreshing);
        let (models, gpu) = (Arc::clone(models), self.gpu.clone());
        let launched = background::spawn("native-objects edits", move || {
            refresh_models(&name, generation, &models, &edits, palettes, formats, gpu);
            refreshing.store(false, Ordering::Release);
        });
        if let Err(e) = launched {
            self.refreshing.store(false, Ordering::Release);
            eprintln!("native-objects: could not start the live edit refresh ({e}); edited models keep their loaded geometry until the map reloads");
        }
    }
}

/// The render models and images one drain of live edits names, the
/// models only where `resident` (the load's coverage) has them: another
/// model draws from guest memory already. Tags match by index, as
/// `native_map::tag_of` does.
fn edited_models_and_images(
    edits: &[render_bridge::native_edits::Edit], resident: &dyn Fn(u32) -> Option<TagId>,
) -> (Vec<TagId>, Vec<(TagId, u32)>) {
    use render_bridge::native_edits::Edit;
    let mut modes = Vec::new();
    let mut images = Vec::new();
    for &edit in edits {
        match edit {
            Edit::Model { tag } => {
                if let Some(mode) = resident(tag)
                    && !modes.contains(&mode)
                {
                    modes.push(mode);
                }
            }
            Edit::Image { bitmap, image } => images.push((TagId(bitmap), image)),
            Edit::Section { .. } => {}
        }
    }
    (modes, images)
}

/// Takes `mode` out of the draw set and the coverage: its parts keep
/// their D3D draws, from guest memory, which the edit wrote.
fn leave_native(models: &Models, mode: TagId) {
    let mut lit = models.lit.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(lit) = lit.as_mut() {
        lit.remove_model(mode.0);
    }
    models.coverage.write().unwrap_or_else(|e| e.into_inner()).remove(&mode);
}

/// Applies one drain of live edits to the resident `models` of map `name`
/// (docs/editor/realtime-edits.md, "The native refresh"). Each edited
/// render model is loaded again from the map file, which the raw overlay
/// serves edited, as the map's load does ([`load_model`]), uploaded into
/// a staging set, then moved into the draw set under its lock with its
/// coverage (the load's residency kept). Each edited image is decoded
/// again at the stages that sample it and its textures rewritten. A model
/// that no longer loads natively leaves the native path, so its parts
/// draw from guest memory rather than as they loaded; an image whose
/// size, levels or format changed keeps its pixels until the map reloads.
/// Both say so.
fn refresh_models(
    name: &str,
    generation: u64,
    models: &Models,
    edits: &[render_bridge::native_edits::Edit],
    palettes: StagePalettes,
    formats: Option<StreamFormats>,
    (device, queue): (wgpu::Device, wgpu::Queue),
) {
    let current = || native_map_work::generation() == generation;
    let (modes, images) = {
        let coverage = models.coverage.read().unwrap_or_else(|e| e.into_inner());
        edited_models_and_images(edits, &|tag| coverage.keys().find(|k| k.0 & 0xffff == tag & 0xffff).copied())
    };
    if modes.is_empty() && images.is_empty() {
        return;
    }
    let mut opened = match open(name, palettes, formats) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("native-objects: {name}: a live edit's map does not open ({e}); the edited models leave the native path");
            for &mode in &modes {
                leave_native(models, mode);
            }
            return;
        }
    };
    for mode in modes {
        if !current() {
            return;
        }
        let staged = (|| -> Result<(LitObjects, ModelCoverage), String> {
            let (cpu, cover, _) = load_model(&mut opened, mode).map_err(|e| e.to_string())?;
            let mut staging = models
                .lit
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .map(LitObjects::empty_with_layout)
                .ok_or("the map has no draw set")?;
            if let Some(texture) = &cpu.normalization {
                staging.set_normalization(&device, &queue, texture);
            }
            if !staging.add_model(&device, &queue, mode.0, cpu.lit) {
                return Err("GPU refused the model's programs or textures".into());
            }
            Ok((staging, cover))
        })();
        let why = match staged {
            Ok((mut staging, mut cover)) => {
                // The upload's writes land before the first draw of it.
                queue.submit([]);
                let mut lit = models.lit.lock().unwrap_or_else(|e| e.into_inner());
                let mut coverage = models.coverage.write().unwrap_or_else(|e| e.into_inner());
                let old = coverage.get(&mode).map(|old| (old.nodes, Arc::clone(&old.assets)));
                match (lit.as_mut(), old) {
                    (Some(_), Some((nodes, _))) if nodes != cover.nodes => Some("its node count changed".to_owned()),
                    (Some(lit), Some((_, assets))) => {
                        cover.assets = assets;
                        if staging.move_model_into(mode.0, lit) {
                            coverage.insert(mode, cover);
                            None
                        } else {
                            Some("its upload does not fit the draw set".to_owned())
                        }
                    }
                    _ => None,
                }
            }
            Err(why) => Some(why),
        };
        if let Some(why) = why {
            eprintln!("native-objects: {name}: model {:#010x} leaves the native path after a live edit ({why}); D3D draws it", mode.0);
            leave_native(models, mode);
        }
    }
    if images.is_empty() || !current() {
        return;
    }
    // Which stages sample each image, read under the lock; the decodes
    // run without it.
    let stages: Vec<((TagId, u32), Vec<usize>)> = {
        let lit = models.lit.lock().unwrap_or_else(|e| e.into_inner());
        let Some(lit) = lit.as_ref() else { return };
        images.iter().map(|&(bitmap, image)| ((bitmap, image), lit.image_stages(bitmap, image))).collect()
    };
    let palettes = opened.decoder.palettes().clone();
    let decoded: Vec<_> = stages
        .into_iter()
        .map(|((bitmap, image), at)| {
            let pixels = native_render::stage_images::decode_stages(&mut opened.map, &mut opened.raw.0, &palettes, bitmap, image, &at);
            ((bitmap, image), pixels)
        })
        .collect();
    // The lock is released before the submit.
    {
        let lit = models.lit.lock().unwrap_or_else(|e| e.into_inner());
        let Some(lit) = lit.as_ref() else { return };
        for ((bitmap, image), pixels) in &decoded {
            if let Err(why) = lit.rewrite_image(&queue, *bitmap, *image, pixels) {
                eprintln!(
                    "native-objects: {name}: bitmap {:#010x} image {image} keeps its pixels until the map reloads ({why:?})",
                    bitmap.0
                );
            }
        }
    }
    queue.submit([]);
}

impl<B: d3d::backend::Backend + Send + 'static> Host for Glue<B> {
    fn preload(&self, mem: &dyn GuestMemory) {
        if let Some((models, generation)) = self.load(mem) {
            self.refresh_edits(mem, &models, generation);
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
        let (models, generation) = self.load(mem).ok_or(ObjectWalk::Model)?;
        self.refresh_edits(mem, &models, generation);
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
        let texaccum = if draws.iter().any(|(_, draw)| draw.texaccum) {
            if !device.samples_levels_in_place() {
                return Err(ObjectWalk::Inputs);
            }
            Some(native_render::world_layers::Texaccum::read(&Ro(mem)).ok_or(ObjectWalk::Inputs)?)
        } else {
            None
        };
        let ranges = draws.len();
        let view = mem.u32(engine::render_loop::occlusion::guest::VIEW_INDEX).unwrap_or(u32::MAX);
        let promise = d3d::ExternalPlan { stage: Stage::Objects, view, ranges, slots: Vec::new() };
        let batch = u64::from(device.swap_count()).wrapping_add(1);
        let pass = |draws: Vec<(u32, PlannedDraw)>| {
            Box::new(ObjectsPass { models: Arc::clone(&models), draws, texaccum, view: None, w_clip: None, run: batch })
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
    let mut lit = lit.as_mut();
    let coverage = models.coverage.read().map_err(|_| ObjectWalk::Model)?;
    let mut draws = Vec::with_capacity(run.len());
    for (slot, &item) in (first..).zip(run) {
        let (handle, part) = match item {
            Entry::Excluded => continue,
            Entry::Object { handle, part: Some(part) } => (handle, part),
            _ => return Err(ObjectWalk::Unsupported),
        };
        let instance = scene.instance(view, InstanceKey(handle)).ok_or(ObjectWalk::Inputs)?;
        let model = *models.index.get(&(instance.definition as u16)).ok_or(ObjectWalk::Model)?;
        let cover = coverage.get(&TagId(model)).ok_or(ObjectWalk::Model)?;
        if model != part.mode || instance.nodes.len() != cover.nodes {
            return Err(ObjectWalk::Model);
        }
        if !cover.ranges.get(&(part.key.section, part.key.part)).is_some_and(|range| range.matches(part)) {
            return Err(ObjectWalk::Unsupported);
        }
        let (_, scene_part) = scene.part_at(view, slot as u32)
            .filter(|(owner, p)| owner.key == instance.key && p.key == part.key)
            .ok_or(ObjectWalk::Inputs)?;
        // The model and part agree with the CPU loader's coverage. Only a
        // failed GPU lookup for that same model is a residency fallback.
        if !cover.assets.uploaded() { return Err(ObjectWalk::Unresident); }
        let lit = lit.as_deref_mut().filter(|assets| assets.has(model)).ok_or(ObjectWalk::Unresident)?;
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
        draws.push((model, draw));
    }
    let lit = lit.ok_or(ObjectWalk::Model)?;
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
    palette::clear();
    if !render_bridge::native_objects::on(&switches) {
        return;
    }
    let Some(gpu) = device.lock().ok().and_then(|d| d.backend.upload_device()) else { return };
    render_bridge::native_objects::set_host(Arc::new(Glue {
        gpu,
        device: Arc::clone(device),
        state: Arc::new(Mutex::new(None)),
        refreshing: Arc::default(),
    }));
}

/// Visit every object definition in the mounted tag index, including
/// definitions whose first instance will spawn after activation.
fn load_objects(
    name: &str,
    generation: u64,
    job: &Arc<()>,
    state: &Mutex<Option<MapState>>,
    palette: &palette::Palette,
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
    // The scenario's palettes load first, including entries without
    // placements; the rest of the tag index follows.
    let manifest = (|| -> Result<Vec<TagId>, load::Error> {
        let root = opened.map.scenario().and_then(|tag| tag.location).ok_or("scenario has no local root")?;
        let bytes = opened.ctx.meta.slice(root.address, u64::from(root.size))?;
        Ok(palette::manifest(&opened.ctx.meta, bytes, |id| opened.map.tag(id).ok().map(|tag| tag.group))?)
    })();
    let mut definitions = match manifest {
        Ok(ids) => {
            palette.start(&ids);
            ids
        }
        Err(why) => {
            eprintln!("native-objects: {name}: scenario palette unavailable ({why})");
            Vec::new()
        }
    };
    let prioritized: HashSet<_> = definitions.iter().copied().collect();
    definitions.extend(opened.map.tags().iter()
        .filter(|tag| tags::object::is_object_group(tag.group) && !prioritized.contains(&tag.id))
        .map(|tag| tag.id));
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
    let mut casters = CasterSet::default();
    for definition in definitions {
        if !state.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(owns) {
            return;
        }
        let result = (|| -> Result<Option<(TagId, Arc<palette::GpuAssets>, bool)>, String> {
            let Some(model) = opened.ctx.resolve_object(definition).map_err(|e| e.to_string())?.model else {
                return Ok(None);
            };
            if model.tag.layers.first().and_then(|r| r.tag_ref("render_model"))
                .is_some_and(|reference| reference.id == TagId::NULL) {
                return Ok(None);
            }
            let mode = model.render_model.as_ref().ok_or("the model has no valid render model")?.id;
            // The geometry read for the first definition of a render model.
            let mut fresh = None;
            let cover = coverage.entry(mode).or_insert_with(|| {
                let (cpu, cover, geo) = load_model(&mut opened, mode).map_err(|e| e.to_string())?;
                fresh = Some(geo);
                let lit = lit.get_or_insert_with(|| LitObjects::new(&device));
                if let Some(texture) = &cpu.normalization {
                    lit.set_normalization(&device, &queue, texture);
                }
                // Model-local geometry and material ranges are validated
                // before the model counts as resident.
                if !lit.add_model(&device, &queue, mode.0, cpu.lit) || !lit.has(mode.0) {
                    return Err("GPU refused the model's programs or textures".into());
                }
                Ok(cover)
            }).as_ref().map_err(Clone::clone)?;
            if model.node_count as usize != cover.nodes {
                return Err("model and render model disagree on the node count".into());
            }
            casters.add(definition, mode, &model, fresh.take(), &mut opened);
            Ok(Some((mode, Arc::clone(&cover.assets), cover.all_parts)))
        })();
        match result {
            Ok(None) => {
                palette.resolve(definition, palette::Definition::NoModel);
            }
            Ok(Some((mode, gpu, all_parts))) => {
                palette.resolve(definition, palette::Definition::Model { gpu, all_parts });
                index.insert(definition.0 as u16, mode.0);
            }
            Err(why) => {
                palette.resolve(definition, palette::Definition::Failed);
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
        coverage: RwLock::new(coverage.into_iter().filter_map(|(mode, result)| Some((mode, result.ok()?))).collect()),
        index,
        palettes: opened.decoder.used().clone(),
    });
    {
        let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(s) = state.as_mut().filter(|s| owns(s)) else { return };
        s.published = Some((timeline::frames(), Arc::clone(&models)));
    }
    // E3's sun tiles read these on the render thread; retail never does.
    native_render::sun_casters::set_models(generation, Some(Arc::new(casters.finish())));
    // The palette counts a model resident once the walk can draw it.
    for cover in models.coverage.read().unwrap_or_else(|e| e.into_inner()).values() {
        cover.assets.finish(true);
    }
    palette.refresh();
    eprintln!("native-objects: {name}: {loaded} definitions ready, {refused} left to D3D");
}

/// Each definition's model for E3's sun tiles
/// (docs/graphics/e3-lighting.md, "Sun shadows from objects"): its render
/// model with its default variant (`model_load::default_model`), the CPU
/// mesh the sun's casters are skinned from. Definitions of one render
/// model that choose the same regions share one model. A definition whose
/// model is refused has none, and its shadow keeps retail's mask
/// (`CasterRefusal::NoModel`).
#[derive(Default)]
struct CasterSet {
    by_definition: HashMap<u32, Arc<native_render::model::Model>>,
    /// Per render model, each variant's regions seen and its model.
    built: HashMap<TagId, Vec<(Vec<tags::object::RegionChoice>, Option<Arc<native_render::model::Model>>)>>,
}

impl CasterSet {
    /// `definition`'s model, from `geo` when the caller has just read
    /// `mode`'s geometry, else read again for a variant not seen yet.
    fn add(
        &mut self, definition: TagId, mode: TagId, resolved: &tags::object::ResolvedModel,
        geo: Option<RenderModelGeometry>, o: &mut Opened,
    ) {
        let seen = self.built.entry(mode).or_default();
        let shared = match seen.iter().find(|(regions, _)| *regions == resolved.regions) {
            Some((_, shared)) => shared.clone(),
            None => {
                let geo = match geo {
                    Some(g) => Ok(g),
                    None => RenderModelGeometry::read_in(&mut o.map, mode, &mut o.raw, Some(&o.ctx.meta)),
                };
                let built = geo.ok().and_then(|g| native_render::model_load::default_model(&g, resolved).ok());
                let shared = built.map(|d| Arc::new(d.model));
                seen.push((resolved.regions.clone(), shared.clone()));
                shared
            }
        };
        if let Some(m) = shared {
            self.by_definition.insert(definition.0, m);
        }
    }

    fn finish(self) -> SunModels {
        SunModels(self.by_definition)
    }
}

/// The models [`CasterSet`] built, by object definition datum.
struct SunModels(HashMap<u32, Arc<native_render::model::Model>>);

impl native_render::sun_casters::CasterModels for SunModels {
    fn model(&self, definition: u32) -> Option<&native_render::model::Model> {
        self.0.get(&definition).map(|m| &**m)
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
fn load_model(o: &mut Opened, mode: TagId) -> Result<(CpuModel, ModelCoverage, RenderModelGeometry), load::Error> {
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
    // A part without index data is not resident.
    let built = implementation_masks(&lit);
    let ranges = built.keys().filter_map(|&(section, part)| {
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
    // A model with even one carried part is only partially covered. Check
    // every drawable part against the implementation indices actually
    // declared by its material, including sections the current view omits.
    let required_masks: HashMap<_, _> = materials.iter().filter_map(|(&index, material)| {
        let material = &material.material;
        let base = native_render::object_program::object_layer3(material).ok()?;
        // The native uploader compiles the base LOD. Another material LOD
        // is covered only when it resolves to the same pass and inputs.
        let same_passes = material.layers.iter().filter(|l| l.layer == look::LIGHTMAP_LAYER)
            .all(|layer| layer.passes.as_slice() == std::slice::from_ref(base));
        if !same_passes { return None; }
        Some((index, palette::implementation_mask(base.implementations.iter().map(|imp| imp.index))?))
    }).collect();
    let all_parts = geo.sections.iter().enumerate().all(|(section, s)| {
        s.geometry.parts.iter().enumerate().filter(|(_, p)| p.is_drawn()).all(|(index, p)| {
            let have = u16::try_from(section).ok().zip(u8::try_from(index).ok())
                .and_then(|key| built.get(&key));
            have.zip(required_masks.get(&p.material)).is_some_and(|(&have, &want)| have & want == want)
        })
    });
    let assets = Arc::new(palette::GpuAssets::default());
    let cover = ModelCoverage { nodes: geo.nodes.len(), assets, all_parts, ranges };
    let cpu = CpuModel { lit, normalization: o.normalization.clone().flatten() };
    Ok((cpu, cover, geo))
}

/// Each part with index data and the native implementations it carries.
fn implementation_masks(lit: &LitModelInput) -> HashMap<(u16, u8), u8> {
    lit.parts
        .iter()
        .filter(|p| !p.indices.is_empty())
        .map(|p| {
            let mask = p.implementations.iter().enumerate()
                .filter(|(_, slot)| slot.is_some())
                .fold(0u8, |mask, (n, _)| mask | 1 << n);
            ((p.section, p.part), mask)
        })
        .collect()
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
                programs
                    .entry(key)
                    .or_insert_with(|| object_vertex_program(o, vs, s.geometry, s.influences));
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
    /// Guest descriptor captured at the same boundary as the draws.
    texaccum: Option<native_render::world_layers::Texaccum>,
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
        self.texaccum.iter().map(|target| target.texture).collect()
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
        let texaccum = match self.texaccum {
            Some(target) => {
                let level = t.sampled.first().and_then(Option::as_ref).ok_or("object texture accumulation unavailable")?;
                Some(Texaccum { view: &level.view, guest_size: target.size })
            }
            None => None,
        };
        if let Some(accum) = &texaccum
            && (accum.view.texture() == t.color.texture() || accum.view.texture() == t.depth.texture())
        {
            return Err("object texture accumulation is an attachment");
        }
        let mut lit = self.models.lit.lock().unwrap_or_else(|e| e.into_inner());
        let lit = lit.as_mut().ok_or_else(|| {
            for (_, draw) in &self.draws {
                d3d::scene_stage::residency::missing_object(draw.instance);
            }
            "planned object geometry unavailable"
        })?;
        let mut missing = self.draws.iter().filter(|(model, draw)| !lit.has_draw_assets(*model, draw)).peekable();
        if missing.peek().is_some() {
            for (_, draw) in missing {
                d3d::scene_stage::residency::missing_object(draw.instance);
            }
            return Err("planned object geometry or material unavailable");
        }
        if self.draws.iter().any(|(_, d)| d.indices.is_empty()) {
            return Err("planned object indices empty");
        }
        let view = ViewInputs {
            constants: request.view_constants(t.copy, constants),
            guest_size: request.surface_size,
            size: t.size,
            z_scale: request.z_scale,
            w_clip: self.w_clip,
        };
        lit.draws(&t.device, &t.queue, &t.uploads, t.color_format, t.depth_format, t.size, &view, request.projection_factor(t.copy), &self.draws, texaccum)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A live edit's drain names the native models it edited, each once
    /// and by the load's own tag (matched by index), and every image;
    /// a model the load left to D3D and a structure section are not the
    /// objects' to refresh.
    #[test]
    fn a_drain_names_the_resident_models_once_and_every_image() {
        use render_bridge::native_edits::{Edit, KIND_CLUSTER};
        let resident = |tag: u32| (tag & 0xffff == 0x0042).then_some(TagId(0xe1c0_0042));
        let (modes, images) = edited_models_and_images(&[
            Edit::Model { tag: 0x0000_0042 },
            Edit::Model { tag: 0xe1c0_0042 },
            Edit::Model { tag: 0xe1c0_0099 },
            Edit::Image { bitmap: 0xe000_0007, image: 2 },
            Edit::Section { bsp: 1, kind: KIND_CLUSTER, index: 0, grown: false },
        ], &resident);
        assert_eq!(modes, vec![TagId(0xe1c0_0042)]);
        assert_eq!(images, vec![(TagId(0xe000_0007), 2)]);
    }

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
            texaccum: None,
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
        let mut state = MapState {
            name: "arena".into(), generation: 3, job: Arc::new(()), _palette: Arc::default(), published: None,
        };
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
        let state = MapState {
            name: "arena".into(), generation: 3, job: Arc::clone(&second), _palette: Arc::default(), published: None,
        };
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

    /// Coverage is keyed by what a record names, (section, part), with the
    /// implementations the part carries natively.
    #[test]
    fn a_models_lit_parts_cover_their_implementations_by_section_and_part() {
        use native_render::object_program_gpu::LitPart;
        let part = |section, part, implementations| LitPart {
            section,
            part,
            decompression: [[0.0; 4]; 3],
            shader_tag: 0xe5da_0465,
            shader: Default::default(),
            indices: 0..3,
            implementations,
            subparts: None,
        };
        let mut empty = part(2, 5, [Some((0, 0)), None, None, None, None]);
        empty.indices = 0..0;
        let lit = LitModelInput {
            parts: vec![part(2, 0, [Some((0, 0)), None, Some((1, 0)), None, None]), part(2, 3, [None, Some((2, 0)), None, None, None]), empty],
            ..LitModelInput::default()
        };
        let parts = implementation_masks(&lit);
        assert_eq!(parts[&(2, 0)], 0b101);
        assert_eq!(parts[&(2, 3)], 0b010);
        assert!(!parts.contains_key(&(2, 5)), "a part without index data is not resident");
        assert_eq!(parts.len(), 2);
    }

}
