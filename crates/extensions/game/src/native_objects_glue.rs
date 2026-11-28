//! Third-person object programs for the scene's layer 3.
//!
//! The activation worker reads the mounted tag index and uploads each
//! supported mode once. The sorted scene walk resolves every part of a
//! run, then pushes it as one frame stage (`FrameOp::Stage`).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use std::sync::atomic::{AtomicBool, Ordering};

use engine::memory::GuestMemory;
use render_bridge::native_map_work;
use render_bridge::native_objects::Host;
use map::TagId;
use frame_scene::{FrameScene, InstanceKey, PartFlags, View, ViewSlot};
use native_render::gpu::Image;
use native_render::light_gpu::StaticLightTexture;
use native_render::load;
use native_render::look;
use native_render::object_lighting::ShaderValues;
use native_render::object_nodes::{Compression, decompression_rows};
use native_render::object_program_gpu::{
    EngineStages, LitBuild, LitMaterial, LitModelInput, LitObjects, LitSection, OVERRIDE_LAYERS, OVERRIDE_MATERIAL, PlannedDraw, Texaccum,
    lit_listed, lit_model, lit_subparts, override_sections, uniform,
};
use native_render::object_state::EntryStates;
use native_render::object_streams::{self, StreamData, StreamFormats};
use native_render::object_walk::{Entry, Part as WalkPart};
use tags::model::RenderModelGeometry;
use timeline::native::{ObjectWalk, ReservationCause, Stage as NativeStage};
use native_render::program::ViewInputs;

use crate::native_map::{self, NativeMap};
use crate::native_world_glue::{Raw, StageDecoder, StageImages, StagePalettes, palettes_hold, scene_load_palettes, stage_palette};

mod display;
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
    fn matches(self, part: &WalkPart) -> bool {
        self.geometry_class == part.geometry_class
            && self.node_influences == part.node_influences
            && self.first == part.first_index
            && self.count == part.index_count
            && self.triangle_list == part.triangle_list
            && part.subparts.as_ref().is_none_or(|s| s.first == self.first_subpart && s.count == self.subpart_count)
    }
}

struct ModelCoverage {
    display_parts: HashMap<u16, Vec<(u8, u32)>>,
    node_maps: Vec<Vec<u8>>,
    nodes: usize,
    assets: Arc<palette::GpuAssets>,
    all_parts: bool,
    ranges: HashMap<(u16, u8), PartRange>,
}

#[derive(Default)]
struct Models {
    retire: Option<native_map::RetireQueue>,
    sun_models: Option<native_render::sun_casters::SharedModels>,
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

/// The draw state at the layer-3 call: the render states the items' draws
/// inherit ([`EntryStates`]) and whether its draws are W-buffered.
#[derive(Clone, Debug)]
struct LayerEntry {
    states: EntryStates,
    w_buffered: bool,
}

static STATE_TABLE: std::sync::LazyLock<gfx::xdk::RenderStateTable> =
    std::sync::LazyLock::new(gfx::xdk::RenderStateTable::default);

impl LayerEntry {
    fn read() -> Self {
        Self::read_with(&device_state::render_state)
    }

    /// The render states from `states`, by guest slot.
    fn read_with(states: &dyn Fn(u32) -> Option<u32>) -> Self {
        let table = &*STATE_TABLE;
        Self {
            states: EntryStates::read(|t| table.canonical_to_guest(t.index()).and_then(states)),
            w_buffered: device_state::w_buffered(),
        }
    }
}

/// The draw state the guest's D3D calls set, which the object, layer and
/// first-person plans check and inherit. No owner yet: each read is a
/// named gap.
pub(crate) mod device_state {
    /// The render state at guest slot `slot`.
    pub(crate) fn render_state(_slot: u32) -> Option<u32> {
        unimplemented!("objects: render state from the device")
    }

    /// Whether the draws are W-buffered (`SET_CONTROL0`'s z-perspective).
    pub(crate) fn w_buffered() -> bool {
        unimplemented!("objects: W-buffer control from the device")
    }

    /// The stencil the next draw takes.
    pub(crate) fn stencil() -> Option<gfx::state::StencilState> {
        unimplemented!("objects: stencil state from the device")
    }

    /// Vertex constant `c[register]`.
    pub(crate) fn constant(_register: usize) -> [f32; 4] {
        unimplemented!("objects: vertex constants from the device")
    }

    /// The polygon stipple switch (`SET_STIPPLE_CONTROL`).
    pub(crate) fn stipple_control() -> Option<u32> {
        unimplemented!("objects: stipple control from the device")
    }

    /// The polygon stipple pattern.
    pub(crate) fn stipple_pattern() -> [u32; 32] {
        unimplemented!("objects: stipple pattern from the device")
    }

    /// The W clip range pushed for a view whose far is `far`.
    pub(crate) fn depth_clip_range(_far: f32) -> [f32; 2] {
        unimplemented!("objects: depth clip from the device")
    }

    /// Texture stage `stage`'s state `state`.
    pub(crate) fn texture_state(_stage: u32, _state: u32) -> Option<u32> {
        unimplemented!("objects: texture stage states from the device")
    }

    /// The program clip words `[near, far]` and clip flags the game stores
    /// in the device.
    pub(crate) fn stored_clip() -> Option<([u32; 2], u32)> {
        unimplemented!("objects: stored clip words from the device")
    }

    /// The fog words the draws inherit.
    pub(crate) fn fog_words() -> [Option<u32>; 7] {
        unimplemented!("objects: fog words from the device")
    }
}

/// The host: the current map's models.
pub struct Glue {
    state: Arc<Mutex<Option<MapState>>>,
    gpu: (wgpu::Device, wgpu::Queue),
    /// A live edit's refresh is running ([`refresh_models`]); edits wait
    /// in the queue until it ends, so they apply in order.
    refreshing: Arc<AtomicBool>,
}

impl Glue {
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
        install_function_tables(mem);
        let palettes = scene_load_palettes(mem);
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
        if let Some(retired) = retired {
            native_map::retire_owned(timeline::setup::Stage::Objects, retired);
        }
        let launched = background::spawn("native-objects load", move || {
            load_objects(&name, generation, &job, &state, &palette, palettes, formats, gpu);
        });
        if let Err(e) = launched {
            eprintln!("native-objects: could not start map loader ({e})");
        }
        None
    }
}

impl Glue {
    /// Starts a live edit's refresh of the resident models
    /// (docs/editor/realtime-edits.md, "The native refresh") when edits
    /// are queued and no refresh is running: on a worker
    /// ([`refresh_models`]), with the stage palettes and stream formats
    /// bound now, as a load reads them.
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
        let palettes = scene_load_palettes(mem);
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
            // Sections are the world's. Another tag's field write changes
            // nothing this copy reads from metadata live; a shader's is
            // [`edited_shaders`].
            Edit::Section { .. } | Edit::Tag { .. } | Edit::Shader { .. } => {}
        }
    }
    (modes, images)
}

/// The shader, template, pass and vertex shader tags one drain of live
/// edits names (`Edit::Shader`).
fn edited_shaders(edits: &[render_bridge::native_edits::Edit]) -> Vec<TagId> {
    use render_bridge::native_edits::Edit;
    edits
        .iter()
        .filter_map(|e| match *e {
            Edit::Shader { tag } => Some(TagId(tag)),
            Edit::Section { .. } | Edit::Model { .. } | Edit::Image { .. } | Edit::Tag { .. } => None,
        })
        .collect()
}

/// The resident models (of `model_shaders`, `LitObjects::model_shaders`)
/// with a part whose shader `edited` says is built from an edited tag.
fn shader_models(model_shaders: &[(u32, Vec<u32>)], edited: &mut dyn FnMut(TagId) -> bool) -> Vec<TagId> {
    model_shaders
        .iter()
        .filter(|(_, shaders)| shaders.iter().any(|&s| edited(TagId(s))))
        .map(|&(model, _)| TagId(model))
        .collect()
}

/// Takes `mode` out of the draw set and the coverage: its parts keep
/// their guest draws, from guest memory, which the edit wrote.
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
/// Both say so. A model with a part whose shader is built from an edited
/// shader, template, pass or vertex shader (`Edit::Shader`) is loaded
/// again the same way: its programs are compiled from the edited tags.
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
    let (mut modes, images) = {
        let coverage = models.coverage.read().unwrap_or_else(|e| e.into_inner());
        edited_models_and_images(edits, &|tag| coverage.keys().find(|k| k.0 & 0xffff == tag & 0xffff).copied())
    };
    let shaders = edited_shaders(edits);
    if modes.is_empty() && images.is_empty() && shaders.is_empty() {
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
    if !shaders.is_empty() {
        let model_shaders = models.lit.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(LitObjects::model_shaders);
        let mut known = HashMap::new();
        let edited = shader_models(&model_shaders.unwrap_or_default(), &mut |s| {
            crate::native_world_glue::shader_edited(&mut opened.lib, &mut known, &shaders, s)
        });
        for mode in edited {
            if !modes.contains(&mode) {
                modes.push(mode);
            }
        }
    }
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
            eprintln!("native-objects: {name}: model {:#010x} leaves the native path after a live edit ({why})", mode.0);
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

impl Host for Glue {
    fn retire_map(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let retired = if state.as_ref().is_some_and(|s| s.generation != native_map_work::generation()) {
            state.take()
        } else {
            None
        };
        drop(state);
        if let Some(retired) = retired {
            native_map::retire_owned(timeline::setup::Stage::Objects, retired);
        }
    }

    fn preload(&self, mem: &dyn GuestMemory) {
        if let Some((models, generation)) = self.load(mem) {
            self.refresh_edits(mem, &models, generation);
        }
    }

    fn draw_display_objects(&self, mem: &mut dyn GuestMemory, view: &View, lod: render_bridge::native_scene_lists::LodDetail) {
        let _ = self.draw_display_models(mem, view, lod);
    }

    fn draw_walk(
        &self, mem: &mut dyn GuestMemory, view: Option<(&FrameScene, &View)>, slot: ViewSlot, first: usize,
        run: &[Entry], display_lod: Option<render_bridge::native_scene_lists::LodDetail>,
        consume: &mut dyn FnMut(&mut dyn GuestMemory) -> bool,
    ) -> Result<(), ObjectWalk> {
        self.draw_run(mem, view, slot, first, run, display_lod, consume, false)
    }

    fn draw_before(
        &self, mem: &mut dyn GuestMemory, view: Option<(&FrameScene, &View)>, slot: ViewSlot, first: usize,
        run: &[Entry], display_lod: Option<render_bridge::native_scene_lists::LodDetail>,
        consume: &mut dyn FnMut(&mut dyn GuestMemory) -> bool,
    ) -> Result<(), ObjectWalk> {
        self.draw_run(mem, view, slot, first, run, display_lod, consume, true)
    }

    fn entry_state(&self, mem: &dyn GuestMemory, predict: bool) -> Option<Vec<render_bridge::native_objects::before::Field>> {
        Some(entry_fields(mem, predict))
    }
}

impl Glue {
    /// A run of layer 3's objects: at its first geometry handler
    /// (`before` false, the guest's layer setup run), or before the layer
    /// draw from the Rust-built list (`before` true): then the layer
    /// begin's `ZENABLE` is applied first and the shader cache is the
    /// cleared one `0x00016b10` leaves (`render_bridge::native_objects::before`).
    #[allow(clippy::too_many_arguments)]
    fn draw_run(
        &self, mem: &mut dyn GuestMemory, view: Option<(&FrameScene, &View)>, slot: ViewSlot, first: usize,
        run: &[Entry], display_lod: Option<render_bridge::native_scene_lists::LodDetail>,
        consume: &mut dyn FnMut(&mut dyn GuestMemory) -> bool, before: bool,
    ) -> Result<(), ObjectWalk> {
        use timeline::native::{Metric, Stage, Timer};
        // The planning here is extraction: the guest reads and every check
        // that can refuse, which decide the run the guest skips. The
        // packets, pipelines and uploads are made on the render thread.
        let planning = Timer::start(Stage::Objects, Metric::ExtractNs);
        if !before && mem.u8(engine::render_loop::layer::DRAW_FLAG) != Ok(0) {
            // The guest's first shader begin initializes the whole layer's
            // state and clears the cached texture-stage bindings.
            return Err(ObjectWalk::State);
        }
        let (models, generation) = self.load(mem).ok_or(ObjectWalk::Model)?;
        self.refresh_edits(mem, &models, generation);
        if !model_palettes_hold(&models) {
            return Err(ObjectWalk::Inputs);
        }
        // The view needs a render camera that reads, as the layer's draws
        // do: the frame step opened it with one (render_bridge::scene_step).
        let (scene, scene_view) = view.ok_or(ObjectWalk::Inputs)?;
        // Before the call the cache is the one the layer setup clears.
        let cached = if before { Some(0) } else { None };
        let draws = plan_walk(&models, mem, scene, scene_view, first, run, cached, before)?;
        let texaccum = sampled_texaccum(mem, draws.iter().any(|(_, draw)| draw.texaccum))
            .map_err(|()| ObjectWalk::Inputs)?;
        // The display's detail substitution, when one plans.
        let draws = display_lod
            .and_then(|lod| display::plan_lod(&models, mem, scene, scene_view, run, &draws, lod))
            .unwrap_or(draws);
        let view = mem.u32(engine::render_loop::occlusion::guest::VIEW_INDEX).unwrap_or(u32::MAX);
        drop(planning);
        // The load the run was planned against, held to the push.
        let _state = match self.state.lock() {
            Ok(state)
                if state.as_ref().is_some_and(|s| {
                    s.generation == generation
                        && s.published.as_ref().is_some_and(|(_, current)| Arc::ptr_eq(current, &models))
                }) && generation == native_map_work::generation() =>
            {
                state
            }
            _ => return Err(refused(ReservationCause::Stale)),
        };
        if !consume(mem) {
            return Err(refused(ReservationCause::Consumed));
        }
        push_objects(view, ObjectsPass { slot, models: Arc::clone(&models), draws, texaccum });
        Ok(())
    }
}

/// Pushes an objects stage in view `view`'s opaque slot.
fn push_objects(view: u32, pass: ObjectsPass) {
    native_render::stage::frame_builder().push(native_render::stage::FrameOp::Stage {
        slot: frame_types::SlotAt { slot: frame_types::Slot::Opaque, view: view as u8 },
        stage: Box::new(pass),
    });
}

/// A refused reservation of the objects walk: the cause is counted under
/// `native_objects_walk_refused_reservation_<cause>`, and the returned
/// [`ObjectWalk::Reservation`] is counted (the sum) where the walk is
/// handed back to the guest.
fn refused(cause: ReservationCause) -> ObjectWalk {
    NativeStage::Objects.reservation_cause(cause);
    ObjectWalk::Reservation
}

/// The specular bit selects the colour term in its implicit final combiner.
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
/// Plans a run of the walk: the state checks against the draw state and
/// the guest's handler, and each part's draw from the frame step's view
/// (`scene`, `view`): its object's frame and its rows as extraction read
/// them. `first` is the run's first slot.
#[allow(clippy::too_many_arguments)]
fn plan_walk(
    models: &Models, mem: &dyn GuestMemory,
    scene: &FrameScene, view: &View, first: usize, run: &[Entry], cached: Option<u32>, before: bool,
) -> Result<Vec<(u32, PlannedDraw)>, ObjectWalk> {
    use gfx::state::RenderStateType as R;
    let mut entry = LayerEntry::read();
    // Before the layer draw, the draws run with the layer begin's
    // `ZENABLE` (`D3DZB_USEW`, which W-buffers).
    if before {
        entry.w_buffered = true;
        entry.states.0.retain(|(r, _)| *r != R::ZEnable);
        entry.states.0.push((R::ZEnable, LAYER_BEGIN_ZENABLE));
    }
    if !entry.w_buffered {
        return Err(ObjectWalk::State);
    }
    let read = crate::native_world_glue::ReadMemory(mem);
    let flags = mem.u32(native_render::skip::STENCIL_OFF_FLAGS).map_err(|_| ObjectWalk::Inputs)?;
    let stencil_during_draw = flags & 0x00ff_ff00 == 0;
    let in_mode_1 = || in_stencil_mode_1_with(mem, device_state::stencil());
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
    if aux && !view.aux_depth.is_some_and(|a| aux_depth_at_rest(mem, &a)) {
        return Err(ObjectWalk::State);
    }
    // A part with a fade byte (`frame_scene::PartFlags::FADE`): it draws
    // with its byte's pattern under the device's stipple ([`draw_stipple`]),
    // and its epilogue leaves the rest pattern and a cached fade of one.
    // Skipping its handler is state-neutral only if the device already
    // holds them.
    let fades = run.iter().enumerate().any(|(n, e)| {
        matches!(e, Entry::Object { .. })
            && scene.part_at(view, (first + n) as u32).is_some_and(|(_, p)| p.flags.contains(PartFlags::FADE))
    });
    if fades && !stipple_at_rest(mem) {
        return Err(ObjectWalk::State);
    }
    // A part whose shader's prologue uploads the structure's parameter
    // block (`frame_scene::PartFlags::STRUCTURE_PARAMETERS`) draws with its
    // view's upload. Skipping its handler is state-neutral only if the
    // device and the engine already hold every word of it.
    let uploads = run.iter().enumerate().any(|(n, e)| {
        matches!(e, Entry::Object { .. })
            && scene.part_at(view, (first + n) as u32).is_some_and(|(_, p)| p.flags.contains(PartFlags::STRUCTURE_PARAMETERS))
    });
    if uploads && !view.structure_parameters.is_some_and(|p| structure_at_rest(mem, &p)) {
        return Err(ObjectWalk::State);
    }
    let fog_words = device_state::fog_words();
    let table = &*STATE_TABLE;
    let device_state = |slot| {
        if before && Some(slot) == table.canonical_to_guest(R::ZEnable.index()) {
            return Some(LAYER_BEGIN_ZENABLE);
        }
        device_state::render_state(slot)
    };
    let raw = |state| table.canonical_to_guest(state).and_then(device_state);
    let specular = raw(SPECULAR_ENABLE);
    if raw(TWO_SIDED_LIGHTING) != Some(0)
        || raw(R::ShadeMode.index()) != Some(gfx::state::ShadeMode::Gouraud.xbox_value())
    {
        return Err(ObjectWalk::State);
    }
    let cached = match cached {
        Some(cached) => cached,
        None => mem.u32(CACHED_IMPLEMENTATION).map_err(|_| ObjectWalk::State)?,
    };
    let mut checked_transition = false;
    // An animated pass samples its constants at the render clock.
    let time = native_render::layer_pass::render_time(&read);
    let mut lit = models.lit.try_lock().map_err(|_| ObjectWalk::Model)?;
    let mut lit = lit.as_mut();
    let coverage = models.coverage.read().map_err(|_| ObjectWalk::Model)?;
    let mut draws = Vec::with_capacity(run.len());
    for (slot, item) in (first..).zip(run) {
        let (handle, part) = match item {
            Entry::Excluded => continue,
            Entry::Object { handle, part: Some(part) } => (*handle, part),
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
        let structure = view.structure_parameters.as_ref();
        let mut draw = lit.plan_part_of(model, scene, instance, scene_part, &entry.states, time, structure).map_err(|why| {
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
        // Under the stipple the draw masks with the pattern its record
        // draws with: a faded record's, from the rest state checked
        // above, else the one bound.
        draw.stipple = draw_stipple(mem, part.key.record, scene_part.flags.contains(PartFlags::FADE))
            .map_err(|()| ObjectWalk::State)?;
        draws.push((model, draw));
    }
    let lit = lit.ok_or(ObjectWalk::Model)?;
    let (gpu, color, depth) = crate::native_world_glue::pipeline_target();
    if !lit.prewarm_draws(&gpu, color, depth, &draws) {
        return Err(refused(ReservationCause::Declined));
    }
    Ok(draws)
}

/// Whether the draw state holds what the auxiliary state toggle leaves
/// off (`0x00012d50` with BL 0, `aux.rest_*`): the cached byte 0,
/// `c[0..3]` the rows off, and the program clip range off, owned by the
/// game and pushed. Every comparison is of the bits.
pub(crate) fn aux_depth_at_rest(mem: &dyn GuestMemory, aux: &frame_scene::AuxDepth) -> bool {
    let bits = |r: &[f32]| r.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    let stored = device_state::stored_clip();
    let pushed = device_state::depth_clip_range(aux.rest_clip[1]);
    mem.u8(native_render::aux_depth::CACHED) == Ok(0)
        && (0..4).all(|r| bits(&device_state::constant(r)) == bits(&aux.rest_rows[r]))
        && stored.is_some_and(|(clip, flags)| clip == [aux.rest_clip[0].to_bits(), aux.rest_clip[1].to_bits()] && flags & 2 != 0)
        && bits(&pushed) == bits(&aux.rest_clip)
}

/// The stipple switch: `SET_STIPPLE_CONTROL`, else `D3DRS_STIPPLEENABLE`.
fn stipple_switch() -> Option<u32> {
    device_state::stipple_control()
        .or_else(|| STATE_TABLE.canonical_to_guest(STIPPLE_ENABLE).and_then(device_state::render_state))
}

/// Whether the draws take the polygon stipple off: a record's fade
/// pattern then masks nothing.
pub(crate) fn stipple_off() -> bool {
    stipple_switch() == Some(0)
}

/// The polygon stipple as an object's draw would take it: `Ok(None)` off
/// (a record's pattern masks nothing), `Ok(Some(rows))` on with the bound
/// pattern; `Err` when its switch does not read.
pub(crate) fn device_stipple() -> Result<Option<[u32; 32]>, ()> {
    match stipple_switch() {
        Some(0) => Ok(None),
        None => Err(()),
        Some(_) => Ok(Some(device_state::stipple_pattern())),
    }
}

/// The rows record `record`'s draw masks with under `device`
/// ([`device_stipple`]), from the state a record's epilogue leaves
/// ([`stipple_at_rest`], which a faded record's caller checks): a faded
/// record's prologue pattern
/// (`engine::render_loop::opaque_material::faded_rows`), any other's the
/// device's; `Ok(None)` with the stipple off or every row set (it masks
/// nothing). `Err` when the device's stipple or the fade's pattern cannot
/// be known exactly.
pub(crate) fn record_stipple(
    mem: &dyn GuestMemory, device: Result<Option<[u32; 32]>, ()>, record: u16, faded: bool,
) -> Result<Option<[u32; 32]>, ()> {
    use engine::render_loop::opaque_material::{faded_rows, record_fade};
    // The guest's MXCSR: nearest, every exception masked.
    const MXCSR: u32 = 0x1f80;
    let Some(rest) = device? else { return Ok(None) };
    let rows = if faded {
        faded_rows(mem, record_fade(mem, record).ok_or(())?, &rest, MXCSR).ok_or(())?
    } else {
        rest
    };
    Ok((rows != [u32::MAX; 32]).then_some(rows))
}

/// [`record_stipple`] under the stipple now.
pub(crate) fn draw_stipple(mem: &dyn GuestMemory, record: u16, faded: bool) -> Result<Option<[u32; 32]>, ()> {
    record_stipple(mem, device_stipple(), record, faded)
}

/// Whether the device and the engine hold what a faded record's epilogue
/// (`0x0004d720`) leaves when its prologue (`0x0004d640`) started from
/// them: the engine's cached fade (`0x004b8494`) is its one
/// (`0x0045dbc0`), bit for bit, and the device's stipple pattern is the
/// table entry for one (`engine::render_loop::opaque_material::rest_stipple_table`).
/// From there the prologue sets the item's pattern and the epilogue
/// puts both back, so skipping the record's draws leaves what drawing
/// them would, however many there are.
pub(crate) fn stipple_at_rest(mem: &dyn GuestMemory) -> bool {
    use engine::render_loop::opaque_material::{CACHED_FADE, rest_stipple_table};
    // The guest's MXCSR: nearest, every exception masked.
    const MXCSR: u32 = 0x1f80;
    let one = mem.u32(engine::light_constants::constant::ONE);
    if one.is_err() || mem.u32(CACHED_FADE).ok() != one.ok() {
        return false;
    }
    let Some(table) = rest_stipple_table(mem, MXCSR) else {
        return false;
    };
    let pattern = device_state::stipple_pattern();
    (0..32u32).all(|row| mem.u32(table + 4 * row).ok() == Some(pattern[row as usize]))
}

/// Whether the device and the engine already hold everything a flagged
/// shader's prologue uploads from the active structure's parameter block
/// (`0x0003c650`, `0x0003d000`; `frame_scene::StructureParameters`): the
/// bump matrices of stages 1 to 3 (texture-stage states 22 to 25),
/// `c[21..25]`, the fog colour and the thirteen engine words at
/// `0x004c1b9c`, each bit for bit. From there the upload rewrites the same
/// words, so skipping the draws that would run it leaves what drawing them
/// would, however many there are; anything the host computed otherwise
/// than the guest declines.
pub(crate) fn structure_at_rest(mem: &dyn GuestMemory, p: &frame_scene::StructureParameters) -> bool {
    structure_at_rest_with(mem, p, device_state::fog_words()[6])
}

/// [`structure_at_rest`] with the fog colour `fog_colour` (a run's, as the
/// restore leaves it).
pub(crate) fn structure_at_rest_with(
    mem: &dyn GuestMemory, p: &frame_scene::StructureParameters, fog_colour: Option<u32>,
) -> bool {
    use engine::render_loop::structure_parameters::SCRATCH;
    const BUMPENVMAT00: u32 = 22;
    let stages = (1..4u32).all(|stage| {
        (0..4u32).all(|n| {
            device_state::texture_state(stage, BUMPENVMAT00 + n) == Some(p.bump_env[stage as usize - 1][n as usize])
        })
    });
    let first = frame_scene::StructureParameters::FIRST_CONSTANT;
    let constants = (0..5).all(|n| device_state::constant(first + n).map(f32::to_bits) == p.constants[n]);
    let fog = fog_colour == Some(p.fog_colour);
    let scratch = (0..13u32).all(|n| mem.u32(SCRATCH + 4 * n).ok() == Some(p.scratch[n as usize]));
    stages && constants && fog && scratch
}

/// What a layer-3 run's plan reads of the draw state and the engine
/// ([`plan_walk`], [`Glue::draw_run`]), by field: the entry render states,
/// the W buffer, the stencil words, the stipple, specular, two-sided
/// lighting and shade mode, the shader cache, the fog words and the
/// auxiliary toggle's byte. `predict`: as they will be at the layer's first
/// geometry handler, from the draw state before the layer draw: the layer
/// begin's `ZENABLE` (`D3DZB_USEW`, which W-buffers) and the cache
/// `0x00016b10` clears (`render_bridge::native_objects::before`).
fn entry_fields(mem: &dyn GuestMemory, predict: bool) -> Vec<render_bridge::native_objects::before::Field> {
    use gfx::state::RenderStateType as R;
    let entry = LayerEntry::read();
    let raw = |state| STATE_TABLE.canonical_to_guest(state).and_then(device_state::render_state);
    let mut out: Vec<render_bridge::native_objects::before::Field> = Vec::with_capacity(48);
    for t in native_render::object_state::FIELDS {
        let value = if predict && t == R::ZEnable { Some(LAYER_BEGIN_ZENABLE) } else { entry.states.get(t) };
        out.push((format!("{t:?}"), value));
    }
    let w = if predict { true } else { entry.w_buffered };
    out.push(("w_buffered".into(), Some(u32::from(w))));
    out.push(("stencil_off_flags".into(), mem.u32(native_render::skip::STENCIL_OFF_FLAGS).ok()));
    out.push(("stencil_mode".into(), mem.u16(SCENE_STENCIL_MODE).ok().map(u32::from)));
    out.push(("stencil_mode_1".into(), Some(u32::from(device_state::stencil() == Some(scene_stencil())))));
    out.push(("stipple".into(), stipple_switch()));
    out.push(("stipple_at_rest".into(), Some(u32::from(stipple_at_rest(mem)))));
    out.push(("specular".into(), raw(SPECULAR_ENABLE)));
    out.push(("two_sided_lighting".into(), raw(TWO_SIDED_LIGHTING)));
    out.push(("shade_mode".into(), raw(R::ShadeMode.index())));
    for (name, at) in [("cached_implementation", CACHED_IMPLEMENTATION), ("cached_descriptor", CACHED_PASS_DESCRIPTOR)] {
        out.push((name.into(), if predict { Some(0) } else { mem.u32(at).ok() }));
    }
    for (n, word) in device_state::fog_words().into_iter().enumerate() {
        out.push((format!("fog_{n}"), word));
    }
    out.push(("aux_cached".into(), mem.u8(native_render::aux_depth::CACHED).ok().map(u32::from)));
    out
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

/// Items the plan gave back to the guest, and the first of each reason
/// (logged once).
static REFUSED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static REASONS: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn count_refusal(why: &str) {
    REFUSED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut seen = REASONS.lock().unwrap_or_else(|e| e.into_inner());
    let seen = seen.get_or_insert_with(HashSet::new);
    if seen.len() < 64 && seen.insert(why.to_owned()) {
        eprintln!("native-objects: an item is left to the guest: {why} (each reason logged once)");
    }
}

static PALETTE_CHANGED: AtomicBool = AtomicBool::new(false);

fn model_palettes_hold(models: &Models) -> bool {
    let hold = palettes_hold(&models.palettes, &stage_palette);
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

/// Installs the host (beside the world's).
pub fn install() {
    let switches = hooks::Switches::from_env().unwrap_or_default();
    palette::clear();
    *LAYER_SOURCE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    if !render_bridge::native_objects::on(&switches) {
        return;
    }
    let gpu = crate::native_world_glue::upload_device();
    let state = Arc::new(Mutex::new(None));
    *LAYER_SOURCE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(&state));
    render_bridge::native_objects::set_host(Arc::new(Glue {
        gpu,
        state,
        refreshing: Arc::default(),
    }));
}

/// The objects host's map state, which the world layers' object items
/// draw from (`native_layers_glue`): set at install.
static LAYER_SOURCE: Mutex<Option<Arc<Mutex<Option<MapState>>>>> = Mutex::new(None);

impl crate::native_layers_glue::LayerModels for Models {
    fn lit(&self) -> &Mutex<Option<LitObjects>> {
        &self.lit
    }

    fn ready(&self, model: u32) -> bool {
        self.coverage.read().is_ok_and(|c| c.get(&TagId(model)).is_some_and(|cover| cover.assets.uploaded()))
    }
}

/// The current map's resident models for the object items of a scene
/// world layer (docs/graphics/framescene.md, slice 4): the ones layer 3
/// draws from, once the load has published them (the objects host's own
/// walk starts the load; this never does), and only while the stage
/// palettes they were expanded through hold.
pub(crate) fn layer_models(mem: &dyn GuestMemory) -> Option<Arc<dyn crate::native_layers_glue::LayerModels>> {
    let source = LAYER_SOURCE.lock().ok()?.clone()?;
    let generation = native_map_work::generation();
    let mut field = [0; 32];
    let name = crate::health::read_map_name(mem, &mut field).ok()?;
    let models = {
        let state = source.lock().ok()?;
        let s = state.as_ref().filter(|s| s.generation == generation && s.name == name)?;
        s.resident(timeline::frames())?
    };
    if !model_palettes_hold(&models) {
        return None;
    }
    Some(models)
}

/// Installs the engine's wave and shape tables from the running image
/// (`tags::effects::function::Tables::ENGINE_SLAB`) before a map's
/// materials compile, so a material animation on a wave the tables alone
/// give (3, 5, 7..=11; the hologram's scan noise is wave 9) samples as the
/// game's `0x0017c900` does. Once per process: the tables are static data.
pub(crate) fn install_function_tables(mem: &dyn GuestMemory) {
    use tags::effects::function::Tables;
    if Tables::engine_installed() {
        return;
    }
    let slab = Tables::ENGINE_SLAB;
    match mem.bytes(slab.start, slab.len()) {
        Ok(bytes) => {
            if let Err(e) = Tables::install_engine_slab(&bytes) {
                eprintln!("native-objects: the engine's function tables did not install ({e})");
            }
        }
        Err(e) => eprintln!("native-objects: the engine's function tables do not read ({e:?})"),
    }
}

/// The render states an object's draw inherits at a layer call
/// ([`EntryStates`]), and whether the draws are W-buffered.
pub(crate) fn entry_states(mem: &dyn GuestMemory) -> (native_render::object_program_gpu::ObjectLayerEntry, bool) {
    entry_states_with(mem, &device_state::render_state)
}

/// [`entry_states`] with the render states from `states`, by guest slot:
/// the state as a restore list leaves it
/// (`native_render::sorted_entries::after_restore`), for a draw in a
/// layer's sorted entries.
pub(crate) fn entry_states_with(
    mem: &dyn GuestMemory, states: &dyn Fn(u32) -> Option<u32>,
) -> (native_render::object_program_gpu::ObjectLayerEntry, bool) {
    use native_render::object_program_gpu::{MaterialConstant18, ObjectLayerEntry};
    use engine::render_loop::layer::SETUP_CONTEXT;
    let entry = LayerEntry::read_with(states);
    let bank = (|| {
        let mut row = [0; 4];
        for (lane, word) in row.iter_mut().enumerate() {
            *word = mem.u32(SETUP_CONTEXT + 0x1630 + lane as u32 * 4).ok()?;
        }
        Some(row)
    })();
    let dirty = mem.u32(SETUP_CONTEXT + 0x1734).ok().map(|mask| mask & 1 != 0);
    let c18 = MaterialConstant18::new(device_state::constant(18), bank, dirty);
    (ObjectLayerEntry { states: entry.states, c18 }, entry.w_buffered)
}

/// `D3DZB_USEW`: the `ZENABLE` a layer's first shader begin sets before any
/// of the layer's items draws (`0x0001bbf0`, at `0x0001bc11`: `push 2`,
/// `SetRenderState_ZEnable`).
pub(crate) const LAYER_BEGIN_ZENABLE: u32 = 2;

/// Whether a held `ZENABLE` (`None`: it does not read) is another than
/// the one the layer's draws run with ([`LAYER_BEGIN_ZENABLE`]).
pub(crate) fn needs_layer_begin_zenable(held: Option<u32>) -> bool {
    held != Some(LAYER_BEGIN_ZENABLE)
}

/// The specular bit, which a draw on the implicit final combiner takes
/// ([`object_fog`]).
pub(crate) fn device_specular() -> Option<u32> {
    specular_with(&device_state::render_state)
}

/// [`device_specular`] from `states`, by guest slot.
pub(crate) fn specular_with(states: &dyn Fn(u32) -> Option<u32>) -> Option<u32> {
    STATE_TABLE.canonical_to_guest(SPECULAR_ENABLE).and_then(states)
}

/// The stencil a draw would take from `states`, by guest slot, decoded
/// as the guest's render-state array holds it: the state as a restore
/// list leaves it (`native_render::sorted_entries::after_restore`).
/// `None` when a slot does not read or holds a value outside its enum.
pub(crate) fn stencil_with(states: &dyn Fn(u32) -> Option<u32>) -> Option<gfx::state::StencilState> {
    use gfx::state::RenderStateType as R;
    let mut rs = gfx::state::RenderState::default();
    for t in [
        R::StencilEnable, R::StencilFunc, R::StencilRef, R::StencilMask, R::StencilWriteMask,
        R::StencilFail, R::StencilZFail, R::StencilPass,
    ] {
        let guest = STATE_TABLE.canonical_to_guest(t.index())?;
        rs.set_raw(t, states(guest)?).ok()?;
    }
    Some(rs.stencil)
}

/// Whether the draws are W-buffered once `restore` is written: its
/// `ZENABLE`, whose setter redoes `SET_CONTROL0`'s z-perspective, is
/// `D3DZB_USEW`. `None` when it writes no `ZENABLE` (the held control
/// stands).
pub(crate) fn w_buffered_after(restore: &[(u32, u32)]) -> Option<bool> {
    let zenable = STATE_TABLE.canonical_to_guest(gfx::state::RenderStateType::ZEnable.index())?;
    restore.iter().rev().find(|&&(slot, _)| slot == zenable).map(|&(_, v)| v == LAYER_BEGIN_ZENABLE)
}

/// [`device_stipple`] once `restore` is written: its stipple switch, which
/// its setter pushes, stands for the held control.
pub(crate) fn device_stipple_after(restore: &[(u32, u32)]) -> Result<Option<[u32; 32]>, ()> {
    let slot = STATE_TABLE.canonical_to_guest(STIPPLE_ENABLE);
    match restore.iter().rev().find(|&&(s, _)| Some(s) == slot).map(|&(_, v)| v) {
        None => device_stipple(),
        Some(0) => Ok(None),
        Some(_) => Ok(Some(device_state::stipple_pattern())),
    }
}

/// Whether the scene is in stencil mode 1 with the stencil `stencil` (a run's, as
/// the restore leaves it: [`stencil_with`]).
pub(crate) fn in_stencil_mode_1_with(mem: &dyn GuestMemory, stencil: Option<gfx::state::StencilState>) -> bool {
    mem.u16(SCENE_STENCIL_MODE) == Ok(1) && stencil == Some(scene_stencil())
}

/// Puts the fog of `draw`'s pass in, as layer 3's walk does: the
/// implementation's own fog states over the inherited ones (`fog_words`), and
/// the specular bit for the implicit final combiner.
pub(crate) fn layer_draw_fog(
    lit: &LitObjects, model: u32, draw: &mut PlannedDraw, fog_words: &[Option<u32>; 7], specular: Option<u32>,
) -> Option<()> {
    let pass = lit.planned_pass(model, draw)?;
    let fog = native_render::fog_gpu::FogWords::from_states(&pass.implementation.render_states).resolve(fog_words)?;
    set_fog(draw, fog, specular)
}

/// Whether `draws` write depth or stencil.
pub(crate) fn draws_write_depth_stencil<'a>(draws: impl IntoIterator<Item = &'a PlannedDraw>) -> bool {
    draws.into_iter().any(|d| {
        use gfx::state::StencilOp;
        let stencil = d.state.stencil;
        d.state.depth_write
            || (stencil.enable
                && stencil.write_mask != 0
                && [stencil.fail, stencil.depth_fail, stencil.pass].iter().any(|&op| op != StencilOp::Keep))
    })
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
    let Some(_load) = native_map::begin_load(timeline::setup::Stage::Objects, || {
        generation == native_map_work::generation()
    }) else {
        return;
    };
    let _timer = timeline::setup::Stage::Objects.worker();
    let _scope = mem_watch::scope("native.objects.load");
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
                    eprintln!("native-objects: {name}: definition {:#010x} failed to preload ({why})", definition.0);
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
        sun_models: Some(Arc::new(casters.finish())),
    });
    {
        let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(s) = state.as_mut().filter(|s| owns(s)) else { return };
        native_render::sun_casters::set_models(generation, models.sun_models.as_ref());
        s.published = Some((timeline::frames(), Arc::clone(&models)));
    }
    // The palette counts a model resident once the walk can draw it.
    for cover in models.coverage.read().unwrap_or_else(|e| e.into_inner()).values() {
        cover.assets.finish(true);
    }
    palette.refresh();
    eprintln!("native-objects: {name}: {loaded} definitions indexed, {refused} failed to preload");
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
    /// Resolves shader tags against the same metadata as `ctx`.
    lib: tags::material::ShaderLibrary,
    /// The vector normalization cube, loaded as the lights load it at the
    /// first lit model (`Some(None)`: it does not load).
    normalization: Option<Option<Arc<StaticLightTexture>>>,
    /// The game's stream formats (`None`: lit materials are left out).
    formats: Option<StreamFormats>,
    /// What an object's draw samples for the lightmap (the globals'
    /// rasterizer bitmap at `+0x64`, `object_program::OBJECT_LIGHTMAP`),
    /// read at the first lit model: the image when uniform, else why not.
    lightmap: Option<Result<Image, String>>,
    /// The map's shaders with passes in the sorted handlers' layers
    /// ([`override_materials`]), each as the materials map of its own
    /// build, read at the first lit model.
    overrides: Option<Vec<HashMap<u16, LitMaterial>>>,
}

fn open(name: &str, palettes: StagePalettes, formats: Option<StreamFormats>) -> Result<Opened, load::Error> {
    let (mut map, files) = native_map::open(name)?;
    let ctx = tags::object::Context::read(&mut map)?;
    let lib = tags::material::ShaderLibrary::with_metadata(&map, ctx.meta.shader_view());
    Ok(Opened {
        map,
        ctx,
        raw: Raw(files),
        decoder: StageDecoder::new(palettes),
        images: StageImages::new(),
        lib,
        normalization: None,
        lightmap: None,
        overrides: None,
        formats,
    })
}

const SHAD: map::FourCC = map::FourCC(*b"shad");

/// The shaders the game may put on an object's record in place of its
/// part's material: every shader of the map with passes (at every level:
/// an item names its own) in the sorted handlers' layers
/// (`OVERRIDE_LAYERS`), the
/// camouflage's. The record producer takes the shader from the object's
/// render state at run time (`object_program_gpu::override_sections`), so
/// each model compiles their passes over all of its parts; a record names
/// the one it draws. Read once per map, each as the materials map
/// `{OVERRIDE_MATERIAL: shader}` of its own build, taken out of `o`
/// (put back by [`lit_parts`]).
fn override_materials(o: &mut Opened) -> Vec<HashMap<u16, LitMaterial>> {
    if let Some(found) = o.overrides.take() {
        return found;
    }
    let ids: Vec<TagId> = o.map.tags().iter().filter(|t| t.group == SHAD).map(|t| t.id).collect();
    let mut found = Vec::new();
    for id in ids {
        let Ok(mut m) = o.lib.material(id, None) else { continue };
        // The shader is substituted only by these sorted handlers. Its
        // other layers can still compile when it is a part's own material.
        m.layers.retain(|l| !l.passes.is_empty()
            && OVERRIDE_LAYERS.iter().any(|&x| usize::from(x) == l.layer));
        if m.layers.is_empty() { continue; }
        match lit_material(o, m) {
            Ok(lit) => found.push(HashMap::from([(OVERRIDE_MATERIAL, lit)])),
            Err(why) => eprintln!("native-objects: shader {:#010x}'s camouflage passes failed to preload: {why}", id.0),
        }
    }
    if hooks::native_verbose() {
        eprintln!("native-objects: {} shaders with camouflage passes compile over every model's parts", found.len());
    }
    found
}

/// Logs, once per shader and layer, why an override shader's passes
/// ([`override_materials`]) could not be preloaded. The indexed emitter
/// can submit the bound range when its handler runs.
fn log_override_refusal(shader: u32, layer: u8, why: &str) {
    static SEEN: Mutex<Option<HashSet<(u32, u8)>>> = Mutex::new(None);
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if seen.get_or_insert_with(HashSet::new).insert((shader, layer)) {
        eprintln!("native-objects: shader {shader:#010x}'s layer {layer} failed to preload: {why} (each shader and layer logged once)");
    }
}

/// A render model may have no scene-layer upload: kind 4's sorted draw
/// uses the live indexed range, and the game never submits kinds 0 or 5.
fn needs_object_program(part: &tags::geometry::Part) -> bool {
    part.is_drawn() && part.kind != load::PART_TRANSPARENT
}

/// Compile all sections, including runtime-selected damage variants,
/// permutations and LODs. The live render record selects the section.
fn load_model(o: &mut Opened, mode: TagId) -> Result<(CpuModel, ModelCoverage, RenderModelGeometry), load::Error> {
    let geo = RenderModelGeometry::read_in(&mut o.map, mode, &mut o.raw, Some(&o.ctx.meta))?;
    let mut materials = HashMap::new();
    let mut material_errors = Vec::new();
    for (index, m) in geo.materials.iter().enumerate() {
        let result = o.lib.material(m.shader.id, None).map_err(|e| e.to_string())
            .and_then(|material| lit_material(o, material));
        match result {
            Ok(lit) => { materials.insert(index as u16, lit); }
            Err(why) => material_errors.push(format!("material {index}: {why}")),
        }
    }
    let build = lit_parts(o, &geo, &materials);
    let parts = geo.sections.iter().flat_map(|s| &s.geometry.parts);
    let lit = model_draws(build, parts, material_errors)?;
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
    // A display-only record has a selected section but no sorted item list.
    // Require coverage of every drawn part before admitting that section.
    let mut display_parts = HashMap::new();
    for (i, s) in geo.sections.iter().enumerate() {
        let Ok(section) = u16::try_from(i) else { continue };
        let keys = lit.parts.iter().filter(|p| p.section == section)
            .map(|p| (p.part, p.shader_tag)).collect::<Vec<_>>();
        if !keys.is_empty() && s.geometry.parts.iter().all(|p| p.kind != load::PART_TRANSPARENT)
            && s.geometry.parts.iter().filter(|p| p.is_drawn()).count() == keys.len() {
            display_parts.insert(section, keys);
        }
    }
    let node_maps = geo.sections.iter().map(|section| section.node_map.clone()).collect();
    let cover = ModelCoverage { display_parts, node_maps, nodes: geo.nodes.len(), assets, all_parts, ranges };
    let cpu = CpuModel { lit, normalization: o.normalization.clone().flatten() };
    Ok((cpu, cover, geo))
}

/// Keep a model with any compiled layer, including a sorted override.
/// A transparent-only model reaches the indexed emitter through its
/// sorted handler without needing any of these cached scene programs.
fn model_draws<'a>(
    build: LitBuild, mut parts: impl Iterator<Item = &'a tags::geometry::Part>, mut errors: Vec<String>,
) -> Result<LitModelInput, load::Error> {
    if build.input.is_empty() && parts.any(needs_object_program) {
        errors.extend(build.refused.into_iter().map(|(m, why)| format!("material {m} layer 3: {why}")));
        errors.extend(build.layer_refused.into_iter().map(|(m, l, why)| format!("material {m} layer {l}: {why}")));
        if errors.is_empty() {
            errors.push("no indexed object part matches a material in the model".into());
        }
        return Err(errors.join("; ").into());
    }
    Ok(build.input)
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
fn lit_material(o: &Opened, material: tags::material::Material) -> Result<LitMaterial, String> {
    if o.formats.is_none() {
        return Err("the game's stream formats do not read".into());
    }
    let shader = material.shader;
    let address = o.map.tag(shader).ok().and_then(|t| t.location).map(|l| l.address).ok_or("its shader tag has no address")?;
    let values = o.lib.metadata().bytes(address + SHADER_VALUES, 8).map_err(|_| "its shader values do not read")?;
    let word = |k: usize| u32::from_le_bytes([values[k], values[k + 1], values[k + 2], values[k + 3]]);
    Ok(LitMaterial {
        shader: ShaderValues { scalar_21: word(0), vector_24: word(4) },
        properties: native_render::layer_pass::instance_properties(&o.lib, shader),
        material,
    })
}

/// The shader instance's values the object inputs scale by (`shad +0x44`
/// scalar 21, `+0x48`).
const SHADER_VALUES: u32 = 0x44;

/// Whether `layer` is a world layer besides 3 whose passes the lit build
/// compiles (`object_program_gpu::OBJECT_LAYERS`): their vertex programs
/// are read with layer 3's.
pub fn drawn_layer(layer: usize) -> bool {
    native_render::object_program_gpu::OBJECT_LAYERS.iter().any(|&l| usize::from(l) == layer)
}

/// The lit parts of `loaded`'s drawn sections for `materials`
/// ([`lit_model`]): its vertex programs per section classification read
/// from the shader tags, its stage bitmaps through the stage decoder.
fn lit_parts(o: &mut Opened, geo: &RenderModelGeometry, materials: &HashMap<u16, LitMaterial>) -> LitBuild {
    let overrides = override_materials(o);
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
                .filter(|(i, p)| *i < 16 && p.is_drawn())
                .map(|(k, p)| (k as u8, p.material, g.part_triangles(p)))
                .collect(),
            // A record with bit 6 may draw a choice of them
            // (`frame_scene::Subparts`).
            subparts: g
                .parts
                .iter()
                .enumerate()
                .filter(|(i, p)| *i < 16 && p.is_drawn())
                .filter_map(|(k, p)| Some((k as u8, lit_subparts(&g.indices, &g.subparts, p)?)))
                .collect(),
            // A record without bit 9 draws a strip part as a list.
            listed: g
                .parts
                .iter()
                .enumerate()
                .filter(|(i, p)| *i < 16 && p.is_drawn())
                .filter_map(|(k, p)| Some((k as u8, lit_listed(&g.indices, p)?)))
                .collect(),
        });
    }
    // The programs each section's classification takes, read first (the
    // stage decoder borrows the map after).
    let mut programs: HashMap<(TagId, u16, u8), Result<(Vec<u8>, Vec<u16>), String>> = HashMap::new();
    for lit in materials.values().chain(overrides.iter().flat_map(|m| m.values())) {
        let vertex_shaders = lit
            .material
            .layers
            .iter()
            .filter(|l| (l.lod == 0 && l.layer == look::LIGHTMAP_LAYER) || drawn_layer(l.layer))
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
        let cube = crate::native_lights_glue::light_textures(&mut o.map, o.lib.metadata(), &mut o.raw.0).0;
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
            Err(why) => eprintln!("native-objects: the objects' lightmap stage is left out: {why}"),
        }
        o.lightmap = Some(l);
    }
    let Some(formats) = o.formats.clone() else {
        o.overrides = Some(overrides);
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
        // The camouflage handler's layer copies the view
        // (`native_layers_glue`).
        view: true,
        // and makes the scene copy the camouflage and hologram layers
        // sample (engine texture 17).
        scene_copy: true,
        normalization: o.normalization.as_ref().is_some_and(Option::is_some),
        lightmap: o.lightmap.clone().and_then(Result::ok),
    };
    let mut build = lit_model(&sections, materials, &formats, &engine, &mut vertex, &mut image);
    // A record can override any drawn part, including one whose own
    // material has no base pass.
    if !overrides.is_empty() && sections.iter().any(|s| !s.parts.is_empty()) {
        let every_part = override_sections(&sections);
        for m in &overrides {
            let theirs = lit_model(&every_part, m, &formats, &engine, &mut vertex, &mut image);
            let shader = m[&OVERRIDE_MATERIAL].material.shader.0;
            for (_, layer, why) in &theirs.layer_refused {
                log_override_refusal(shader, *layer, why);
            }
            build.input.append(theirs.input);
        }
    }
    o.overrides = Some(overrides);
    build
}

/// What an object's layer-3 draw samples where its pass reads the
/// lightmap (`native_render::object_program::OBJECT_LIGHTMAP`): the
/// globals' rasterizer bitmap at `+0x64`, decoded as stage 3 samples it,
/// when it is uniform (then the coordinates the draw adds do not matter).
fn object_lightmap(o: &mut Opened) -> Result<Image, String> {
    let lib = &o.lib;
    engine_lightmap(&mut o.map, lib, &mut o.raw.0, &mut o.decoder, &mut o.images, 3)
}

/// What a lightmap stage samples when the current page is -1 (an
/// object's record, `0x0004d2c4`; every decorator draw, `0x0004e277`):
/// the globals' rasterizer bitmap at `+0x64`
/// (`object_program::OBJECT_LIGHTMAP`), decoded as stage `stage` samples
/// it, taken only when uniform, since the coordinates the draw adds (an
/// object's: the BSP's stream `0x2e`) or leaves (a decorator's: none)
/// then do not matter.
pub(crate) fn engine_lightmap<R: std::io::Read + std::io::Seek>(
    map: &mut map::Map<R>, lib: &tags::material::ShaderLibrary, files: &mut assets::ResourceFiles,
    decoder: &mut StageDecoder, images: &mut StageImages, stage: usize,
) -> Result<Image, String> {
    let matg = map
        .tags()
        .iter()
        .find(|t| t.group.0 == *b"matg")
        .and_then(|t| t.location.as_ref().map(|l| l.address))
        .ok_or("no matg")?;
    let read = |a: u32, n: usize| lib.metadata().bytes(a, n).ok().map(<[u8]>::to_vec);
    let bitmap = crate::native_lights_glue::rasterizer_bitmap(&read, matg, native_render::object_program::OBJECT_LIGHTMAP)
        .ok_or("the globals' rasterizer data names no bitmap at +0x64")?;
    decoder.decode(map, files, images, bitmap, 0, stage, false)?;
    let image = images.get(&(bitmap, 0)).cloned().ok_or("its image does not decode")?;
    if !uniform(&image) {
        return Err(format!(
            "the globals' rasterizer bitmap {bitmap:?} ({}x{}) is not uniform, so the coordinates the draw adds or leaves would matter",
            image.width, image.height
        ));
    }
    Ok(image)
}

/// Vertex shader `vs`'s program and input semantics for a section of
/// geometry classification `geometry` and influences `influences`, as
/// `0x0001cb20` picks its classification.
fn object_vertex_program(o: &mut Opened, vs: TagId, geometry: u16, influences: u8) -> Result<(Vec<u8>, Vec<u16>), String> {
    let address = o.map.tag(vs).ok().and_then(|t| t.location).map(|l| l.address).ok_or("the vertex shader tag has no address")?;
    let lib = &o.lib;
    let meta = lib.metadata();
    let count = look::classification_count(meta, address).ok_or("the vertex shader's classifications do not read")?;
    let class = object_streams::classification(geometry, influences, count).ok_or_else(|| format!("geometry classification {geometry}"))?;
    let code = look::vertex_program(meta, address, class).ok_or_else(|| format!("classification {class} has no program"))?;
    let semantics = look::vertex_semantics(meta, address, class).ok_or_else(|| format!("classification {class}'s inputs do not read"))?;
    Ok((code, semantics))
}

/// One sorted run's objects: the draws planned at the walk, in the list's
/// order. The stage owns everything it records from (`models` is an
/// `Arc`; `draws` and `texaccum` are owned values) and reads no guest
/// state after the walk.
struct ObjectsPass {
    /// The view slot the run draws in ([`render_bridge::native_objects::SLOT`]).
    slot: ViewSlot,
    models: Arc<Models>,
    /// The planned draws with their model index.
    draws: Vec<(u32, PlannedDraw)>,
    /// Guest descriptor captured at the same boundary as the draws.
    texaccum: Option<native_render::world_layers::Texaccum>,
}

impl native_render::stage::Stage for ObjectsPass {
    fn name(&self) -> &'static str { "objects" }

    fn record(
        &mut self, t: &gfx::external::ExternalAttachments, request: &native_render::program::ViewInputs,
    ) -> native_render::stage::StageWork {
        debug_assert_eq!(self.slot, ViewSlot::OpaqueBase);
        match self.packets(t, request) {
            Ok(draws) => native_render::stage::StageWork::Draws(draws),
            Err(why) => unimplemented!("objects: {why}"),
        }
    }
}

impl ObjectsPass {
    fn packets(
        &self, t: &gfx::external::ExternalAttachments, request: &native_render::program::ViewInputs,
    ) -> Result<gfx::external::ExternalDraws, &'static str> {
        if self.texaccum.is_some() {
            unimplemented!("objects: accumulation level for the stage");
        }
        let mut lit = self.models.lit.lock().unwrap_or_else(|e| e.into_inner());
        let lit = lit.as_mut().ok_or("planned object geometry unavailable")?;
        if self.draws.iter().any(|(model, draw)| !lit.has_draw_assets(*model, draw)) {
            return Err("planned object geometry or material unavailable");
        }
        if self.draws.iter().any(|(_, d)| d.indices.is_empty()) {
            return Err("planned object indices empty");
        }
        let view = ViewInputs { size: t.size, ..*request };
        let draws = lit.draws(&t.device, &t.queue, &t.uploads, t.color_format, t.depth_format, t.size, &view,
            crate::native_world_glue::projection_factor(), &self.draws, None)?;
        Ok(draws)
    }
}

/// The texture accumulation level a pass sampling `target` (the guest's
/// engine texture 3, read when the pass was planned:
/// `world_layers::Texaccum::read`) binds on this target copy: the first
/// sampled level the targets give (the target's texture header), at the
/// target's guest size. `None` without a target;
/// an error when the level is not given or is one of the attachments
/// the pass draws into (the layer that accumulates into it).
pub(crate) fn texaccum_level(
    t: &gfx::external::ExternalAttachments, target: Option<native_render::world_layers::Texaccum>,
) -> Result<Option<Texaccum<'_>>, &'static str> {
    let Some(target) = target else { return Ok(None) };
    let level = t.sampled.first().and_then(Option::as_ref).ok_or("object texture accumulation unavailable")?;
    if level.view.texture() == t.color.texture() || level.view.texture() == t.depth.texture() {
        return Err("object texture accumulation is an attachment");
    }
    Ok(Some(Texaccum { view: &level.view, guest_size: target.size }))
}

/// The texture accumulation target a pass whose draws sample it binds,
/// read at the hook as layer 3's objects read it: the guest binds one
/// only while `0x0001dcc0` gives it (`world_layers::Texaccum::read`).
/// `Ok(None)` when no draw samples it; `Err` when one does and it cannot
/// be bound.
pub(crate) fn sampled_texaccum(
    mem: &dyn GuestMemory, samples: bool,
) -> Result<Option<native_render::world_layers::Texaccum>, ()> {
    if !samples {
        return Ok(None);
    }
    native_render::world_layers::Texaccum::read(&Ro(mem)).map(Some).ok_or(())
}

/// The texture accumulation target layer 1 draws into, read at the hook
/// (`world_layers::Texaccum::read`): its passes that sample it bind a copy
/// of the bound colour made before each such draw
/// (`native_layers_glue::layer_segments`), so the levels need not
/// be sampled in place. `None` when the guest gives none.
pub(crate) fn accumulation_target(mem: &dyn GuestMemory) -> Option<native_render::world_layers::Texaccum> {
    native_render::world_layers::Texaccum::read(&Ro(mem))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A layer's draws run with the `ZENABLE` its first shader begin sets,
    /// `D3DZB_USEW` (`0x0001bbf0`): any other held value, or one that does
    /// not read, needs it.
    #[test]
    fn the_layer_begin_zenable_is_set_over_anything_but_usew() {
        assert_eq!(LAYER_BEGIN_ZENABLE, 2, "D3DZB_USEW");
        assert!(needs_layer_begin_zenable(Some(0)), "the screen passes before layer 1 draw unbuffered");
        assert!(needs_layer_begin_zenable(Some(1)), "Z-buffered");
        assert!(needs_layer_begin_zenable(None), "does not read");
        assert!(!needs_layer_begin_zenable(Some(2)));
    }

    /// A live edit's drain names the native models it edited, each once
    /// and by the load's own tag (matched by index), and every image;
    /// a model the load left out and a structure section are not the
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

    /// A shader edit reloads the models with a part built from it, and
    /// only those.
    #[test]
    fn a_shader_edit_picks_the_models_whose_parts_use_it() {
        use render_bridge::native_edits::Edit;
        assert_eq!(
            edited_shaders(&[Edit::Shader { tag: 0xe000_0300 }, Edit::Model { tag: 1 }]),
            vec![TagId(0xe000_0300)]
        );
        let model_shaders = [
            (0xe1c0_0040, vec![0xe000_0010, 0xe000_0011]),
            (0xe1c0_0041, vec![0xe000_0012]),
            (0xe1c0_0042, vec![0xe000_0011]),
        ];
        let picked = shader_models(&model_shaders, &mut |s| s == TagId(0xe000_0011));
        assert_eq!(picked, vec![TagId(0xe1c0_0040), TagId(0xe1c0_0042)]);
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

    #[test]
    fn transparent_and_non_submitted_parts_need_no_cached_object_pass() {
        let part = |kind| tags::geometry::Part {
            kind, flags: 0, material: 0, first_index: 0, index_count: 3,
            first_subpart: 0, subpart_count: 0, raw: Vec::new(),
        };
        let sorted = [part(load::PART_TRANSPARENT), part(0), part(5)];
        assert!(model_draws(LitBuild::default(), sorted.iter(), vec![]).unwrap().is_empty());

        let mixed = [part(load::PART_TRANSPARENT), part(2)];
        let build = LitBuild {
            layer_refused: vec![(0, 17, "missing vertex stream".into())],
            ..LitBuild::default()
        };
        let error = model_draws(build, mixed.iter(), vec![]).err().expect("missing opaque pass");
        assert!(error.to_string().contains("material 0 layer 17: missing vertex stream"));
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
            listed: None,
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
