//! Device host for native first-person arms and weapon. Every item is
//! planned at the layer-3 call against the device's entry states before its
//! guest draw is surrendered. The two model calls remain separate external
//! passes in guest order; their light and transparent neighbours stay on D3D.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use native_render::object_program_gpu::PlannedDraw;
use native_render::object_state::EntryStates;
use native_render::object_streams::StreamFormats;
use native_render::program::ViewInputs;
use render_bridge::native_first_person::{Dropped, Host, Outcome};
use render_bridge::native_objects::NativeSet;

use crate::native_first_person_load::{self, MapState, Models};
use crate::native_world_glue::{ReadMemory, palettes_hold, stage_palettes};

/// Hook memory as the stream-format reader expects it.
struct Ro<'a>(&'a dyn GuestMemory);

impl native_render::camera::GuestRead for Ro<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

/// State every item's plan inherits, resolved before `Host::draw` returns.
#[derive(Clone, Debug)]
struct LayerEntry {
    states: EntryStates,
    fog_words: [Option<u32>; 7],
    specular: Option<u32>,
    w_buffered: bool,
    w_clip: ([f32; 2], bool),
    view: [[f32; 4]; d3d::VIEW_CONSTANTS],
    /// What the auxiliary state toggle sets for the first person's view,
    /// read at the call (`native_render::aux_depth::read`), and only when
    /// the device holds it off: an item toggling it draws with these, and
    /// skipping its draw leaves what its epilogue would
    /// (`native_objects_glue::aux_depth_at_rest`, layer 3's rule).
    aux: Option<frame_scene::AuxDepth>,
    /// The texture accumulation target engine texture 3 binds at the call,
    /// where the pass can be given its level
    /// (`native_objects_glue::sampled_texaccum`); an item sampling it
    /// refuses without one.
    texaccum: Option<native_render::world_layers::Texaccum>,
    /// The device's polygon stipple at the call: off
    /// (`native_objects_glue::stipple_off`), and holding what a faded
    /// record's epilogue leaves (`native_objects_glue::stipple_at_rest`).
    /// An item with a fade byte draws unmasked only with both.
    stipple_off: bool,
    stipple_at_rest: bool,
    /// What a flagged shader's prologue uploads from the active
    /// structure's parameter block, where the device and the engine
    /// already hold it (`native_objects_glue::structure_at_rest`); an item
    /// uploading it refuses without.
    structure: Option<frame_scene::StructureParameters>,
}

/// A live shader edit (`native_edits::Edit::Shader`;
/// docs/editor/realtime-edits.md, "The native refresh") drops the
/// viewmodels' map state: the next ask starts a new load of map `name`,
/// whose open reads the edited shaders through the metadata overlay, and
/// its parts draw on the D3D path until it publishes. Their programs are
/// compiled per model as the load reaches them, so a reload by name is
/// what rebuilds them.
fn reload_on_shader_edit(state: &Mutex<Option<MapState>>, name: &str) {
    use render_bridge::native_edits::{self, Consumer, Edit};
    if !native_edits::pending(Consumer::FirstPerson) {
        return;
    }
    let edits = native_edits::drain(Consumer::FirstPerson);
    // The raw model and image edits share this queue. Leave them for
    // `refresh_edits`, or for the load a shader edit starts below.
    for &edit in edits.iter().filter(|e| !matches!(e, Edit::Shader { .. })) {
        native_edits::push(edit);
    }
    if !edits.iter().any(|e| matches!(e, Edit::Shader { .. })) {
        return;
    }
    let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
    let retired = state.take();
    drop(state);
    if let Some(retired) = retired {
        retired.retire();
        eprintln!("native-first-person: {name}: a shader edit reloads the viewmodels");
    }
}

/// The installed host: current map generation and this call's entry state.
pub struct Glue<B: d3d::backend::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
    state: Arc<Mutex<Option<MapState>>>,
    gpu: (wgpu::Device, wgpu::Queue),
    generation: Mutex<u64>,
    entry: Mutex<Option<LayerEntry>>,
    /// A live edit's image refresh is running ([`Glue::refresh_edits`]);
    /// edits wait in the queue until it ends, so they apply in order.
    refreshing: Arc<std::sync::atomic::AtomicBool>,
}

impl<B: d3d::backend::Backend + Send + 'static> Host for Glue<B> {
    fn retire_map(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let generation = render_bridge::native_map_work::generation();
        if state.as_ref().is_some_and(|s| s.map_generation != generation) {
            let retired = state.take().expect("a stale map exists");
            drop(state);
            retired.retire();
            *self.entry.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
    }

    fn native(&self, mem: &mut dyn GuestMemory, prepare_draw: bool) -> Option<Arc<NativeSet>> {
        *self.entry.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let mut map_name = [0; 32];
        let name = crate::health::read_map_name(mem, &mut map_name).ok()?;
        if name.is_empty() {
            return None;
        }
        let map_generation = render_bridge::native_map_work::generation();
        reload_on_shader_edit(&self.state, &name);
        let current = |state: &Option<MapState>| {
            state
                .as_ref()
                .is_some_and(|s| s.name == name.as_ref() && s.map_generation == map_generation)
        };
        let cold = !current(&self.state.lock().unwrap_or_else(|e| e.into_inner()));
        if (prepare_draw || cold)
            && !self.device.lock().ok()?
                .begin_external_boundary(&mut crate::native_lights_glue::DeviceMemory(mem))
                .ok()?
        {
            return None;
        }
        if cold {
            let _dispatch = timeline::setup::dispatch();
            let (palettes, formats) = {
                let device = self.device.lock().ok()?;
                (stage_palettes(&device, mem), StreamFormats::read(&Ro(mem)))
            };
            let mut generation = self.generation.lock().unwrap_or_else(|e| e.into_inner());
            *generation += 1;
            let next = *generation;
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if map_generation != render_bridge::native_map_work::generation() {
                return None;
            }
            if !current(&state) {
                let models = Arc::new(Models::default());
                let set = Arc::new(NativeSet {
                    map: name.to_string(),
                    generation: next,
                    ..NativeSet::default()
                });
                let worker = native_first_person_load::spawn_worker(
                    Arc::clone(&set),
                    Arc::downgrade(&models),
                    Arc::downgrade(&self.state),
                    palettes,
                    formats,
                    self.gpu.clone(),
                    map_generation,
                );
                let retired = state.replace(MapState {
                    name: name.into_owned(),
                    generation: next,
                    map_generation,
                    models,
                    set,
                    next: None,
                    tried: Default::default(),
                    requested: Default::default(),
                    worker,
                });
                drop(state);
                if let Some(retired) = retired {
                    retired.retire();
                }
            }
        }
        let (set, models) = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let state = state.as_mut().filter(|s| s.map_generation == map_generation)?;
            state.activate(timeline::frames());
            (Arc::clone(&state.set), Arc::clone(&state.models))
        };
        self.refresh_edits(&models);
        if !prepare_draw {
            return Some(set);
        }
        let used = models
            .palettes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if used.iter().any(Option::is_some) {
            let device = self.device.lock().ok()?;
            let now = stage_palettes(&device, mem);
            if !palettes_hold(&used, &|stage| now[stage].clone()) {
                return None;
            }
        }

        let entry = {
            let device = self.device.lock().ok()?;
            let viewport = device.viewport();
            let read = ReadMemory(mem);
            if viewport.min_z != 0.0 || viewport.max_z != 1.0 || !crate::native_layers_glue::viewport_clip(&device, &read) {
                return None;
            }
            let table = gfx::xdk::RenderStateTable::for_build(gfx::xdk::HALO2_BUILD);
            let clip = device.depth_clip(&read, native_render::camera::D24_SCALE);
            LayerEntry {
                states: EntryStates::read(|state| {
                    table
                        .canonical_to_guest(state.index())
                        .and_then(|guest| device.render_state(&read, guest))
                }),
                fog_words: crate::native_fog_glue::device_fog_words(&device, mem),
                specular: crate::native_objects_glue::device_specular(&device, mem),
                w_buffered: clip.w_buffered == Some(true),
                w_clip: (clip.range, clip.clamp),
                view: std::array::from_fn(|register| device.constant(register)),
                aux: native_render::aux_depth::read(&Ro(mem))
                    .filter(|aux| crate::native_objects_glue::aux_depth_at_rest(&device, mem, aux)),
                texaccum: crate::native_objects_glue::sampled_texaccum(&device, mem, true).ok().flatten(),
                stipple_off: crate::native_objects_glue::stipple_off(&device, mem),
                stipple_at_rest: crate::native_objects_glue::stipple_at_rest(&device, mem),
                structure: native_render::structure_parameters::read(&Ro(mem))
                    .filter(|p| crate::native_objects_glue::structure_at_rest(&device, mem, p)),
            }
        };
        if !entry.w_buffered {
            return None;
        }
        *self.entry.lock().unwrap_or_else(|e| e.into_inner()) = Some(entry);
        Some(set)
    }
    fn want(&self, model: u32) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(state) = state.as_mut() else {
            return;
        };
        state.requested.insert(model as u16, model);
        if state.tried.insert(model as u16)
            && let Some(worker) = &state.worker
        {
            let _ = worker.send(model);
        }
    }

    /// The first person's parts from the scene, drawn in slot 10
    /// (`render_bridge::native_first_person`) by one external pass, as
    /// the scene's objects draw theirs (`native_objects_glue::draw_walk`):
    /// the device's entry state read at the call ([`Host::native`] with
    /// `prepare_draw`), each part planned from its instance and rows as
    /// extraction copied them ([`plan_parts`]), the pipelines warmed, the
    /// pass reserved on the bound attachments and committed while the load
    /// it was planned against is current. A part the plan refuses is
    /// `unsupported`; a pass that cannot be reserved drops its parts as
    /// `reservation`. Nothing is drawn through D3D.
    fn draw(
        &self,
        mem: &mut dyn GuestMemory,
        scene: &frame_scene::FrameScene,
        _view: &frame_scene::View,
        _slot: frame_scene::ViewSlot,
        _mode: u32,
        parts: &[u32],
    ) -> Outcome {
        let reserved = || {
            let mut out = Outcome::default();
            out.drop(Dropped::Reservation, parts.len());
            out
        };
        if crate::h1_cinematics::reflection_active(mem) || self.native(mem, true).is_none() {
            return reserved();
        }
        let Some(entry) = self.entry.lock().unwrap_or_else(|e| e.into_inner()).take() else {
            return reserved();
        };
        let Some((gpu, generation)) = self.state.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|s| (Arc::clone(&s.models), s.generation)) else {
            return reserved();
        };
        // An animated pass samples its constants at the render clock.
        let time = native_render::layer_pass::render_time(&Ro(mem));
        let plan = plan_parts(&gpu, scene, parts, &entry, time);
        let committed = !plan.draws.is_empty() && self.commit(mem, &gpu, generation, &entry, &plan.draws);
        settle(plan.refused, plan.draws.len(), committed)
    }

    /// The held cut's check (`render_bridge::native_first_person::check`):
    /// the parts planned as [`Host::draw`] plans them, with their
    /// pipelines warm, reserving and drawing nothing.
    fn plan(
        &self,
        mem: &mut dyn GuestMemory,
        scene: &frame_scene::FrameScene,
        _view: &frame_scene::View,
        parts: &[u32],
    ) -> Outcome {
        let reserved = || settle(0, parts.len(), false);
        if self.native(mem, true).is_none() {
            return reserved();
        }
        let Some(entry) = self.entry.lock().unwrap_or_else(|e| e.into_inner()).take() else {
            return reserved();
        };
        let Some(gpu) = self.state.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|s| Arc::clone(&s.models)) else {
            return reserved();
        };
        let time = native_render::layer_pass::render_time(&Ro(mem));
        let plan = plan_parts(&gpu, scene, parts, &entry, time);
        let warm = !plan.draws.is_empty() && self.device.lock().ok().and_then(|device| {
            let (gpu_device, color, depth) = device.external_pipeline_target(&ReadMemory(mem))?;
            let mut lit = gpu.lit.try_lock().ok()?;
            Some(lit.as_mut()?.prewarm_draws(&gpu_device, color, depth, &plan.draws))
        }) == Some(true);
        settle(plan.refused, plan.draws.len(), warm)
    }
}

/// What a call's draw comes to: the parts the plan refused are
/// `unsupported`; the planned ones are drawn when their pass committed,
/// else each is a `reservation` drop.
fn settle(refused: usize, planned: usize, committed: bool) -> Outcome {
    let mut out = Outcome::default();
    out.drop(Dropped::Unsupported, refused);
    if committed {
        out.drawn = planned;
    } else {
        out.drop(Dropped::Reservation, planned);
    }
    out
}

impl<B: d3d::backend::Backend + Send + 'static> Glue<B> {
    /// Reserves one first-person pass of `draws` on the bound attachments
    /// and commits it, while load `generation` (whose models are `gpu`) is
    /// the current map's: `false`, and nothing is drawn, when the
    /// pipelines are still warming or the pass cannot be reserved.
    fn commit(
        &self,
        mem: &mut dyn GuestMemory,
        gpu: &Arc<Models>,
        generation: u64,
        entry: &LayerEntry,
        draws: &[(u32, PlannedDraw)],
    ) -> bool {
        let Ok(mut device) = self.device.lock() else { return false };
        let Some((gpu_device, color, depth)) = device.external_pipeline_target(&ReadMemory(mem)) else {
            return false;
        };
        let warmed = gpu
            .lit
            .try_lock()
            .ok()
            .and_then(|mut lit| lit.as_mut().map(|lit| lit.prewarm_draws(&gpu_device, color, depth, draws)));
        if warmed != Some(true) {
            return false;
        }
        let texaccum = entry.texaccum.filter(|_| draws.iter().any(|(_, draw)| draw.texaccum));
        let pass = Box::new(FirstPersonPass {
            models: Arc::clone(gpu),
            draws: draws.to_vec(),
            texaccum,
            view: Some(entry.view),
            w_clip: entry.w_clip,
        });
        let view = mem
            .u32(engine::render_loop::occlusion::guest::VIEW_INDEX)
            .unwrap_or(u32::MAX);
        let promise = d3d::ExternalPlan {
            stage: timeline::native::Stage::FirstPerson,
            view,
            ranges: draws.len(),
            slots: Vec::new(),
        };
        let Ok(Some(ready)) = device.prepare_external_bound(
            &mut crate::native_lights_glue::DeviceMemory(mem),
            promise,
            pass,
            Vec::new(),
        ) else {
            return false;
        };
        // The load the pass was planned against, held to the commit.
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.as_ref().is_some_and(|state| {
            state.generation == generation
                && state.map_generation == render_bridge::native_map_work::generation()
                && Arc::ptr_eq(&state.models, gpu)
        }) {
            return false;
        }
        ready.commit();
        true
    }

    /// Applies the queued live edits to the first-person models
    /// (docs/editor/realtime-edits.md, "The native refresh"): an edited
    /// `mode` that was asked for is asked of the loader again, which
    /// reads it from the map file the raw overlay serves edited and
    /// replaces it in the draw set; each edited image is decoded again
    /// and its textures rewritten, on a worker
    /// ([`native_first_person_load::refresh_images`]).
    fn refresh_edits(&self, models: &Arc<Models>) {
        use render_bridge::native_edits::{self, Consumer, Edit};
        if !native_edits::pending(Consumer::FirstPerson)
            || self.refreshing.load(std::sync::atomic::Ordering::Acquire)
        {
            return;
        }
        let (name, map_generation) = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(state) = state
                .as_ref()
                .filter(|s| s.map_generation == render_bridge::native_map_work::generation())
            else {
                return;
            };
            (state.name.clone(), state.map_generation)
        };
        let edits = native_edits::drain(Consumer::FirstPerson);
        let mut images = Vec::new();
        {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(state) = state.as_ref() else { return };
            for edit in edits {
                match edit {
                    Edit::Model { tag } => {
                        if let Some(&asked) = state.requested.get(&(tag as u16))
                            && let Some(worker) = &state.worker
                        {
                            let _ = worker.send(asked);
                        }
                    }
                    Edit::Image { bitmap, image } => images.push((map::TagId(bitmap), image)),
                    Edit::Section { .. } | Edit::Tag { .. } | Edit::Shader { .. } => {}
                }
            }
        }
        if images.is_empty() {
            return;
        }
        self.refreshing
            .store(true, std::sync::atomic::Ordering::Release);
        let refreshing = Arc::clone(&self.refreshing);
        let (models, queue) = (Arc::clone(models), self.gpu.1.clone());
        let launched = background::spawn("native-first-person edits", move || {
            native_first_person_load::refresh_images(
                &name,
                map_generation,
                &models,
                &images,
                &queue,
            );
            refreshing.store(false, std::sync::atomic::Ordering::Release);
        });
        if let Err(e) = launched {
            self.refreshing
                .store(false, std::sync::atomic::Ordering::Release);
            eprintln!(
                "native-first-person: could not start the live edit refresh ({e}); the models keep their pixels until the map reloads"
            );
        }
    }
}

/// Installs the host beside the other native render stages.
pub fn install<B: d3d::backend::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    *LAYER_SOURCE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let Some(gpu) = device.lock().ok().and_then(|d| d.backend.upload_device()) else {
        return;
    };
    let state = Arc::new(Mutex::new(None));
    *LAYER_SOURCE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(&state));
    render_bridge::native_first_person::set_host(Arc::new(Glue {
        gpu,
        device: Arc::clone(device),
        state,
        generation: Mutex::new(0),
        entry: Mutex::new(None),
        refreshing: Arc::default(),
    }));
}

/// The first-person host's map state, which the first person's layers
/// 1, 7, 8 and 20 draw their object items from (`native_layers_glue`): set at
/// install.
static LAYER_SOURCE: Mutex<Option<Arc<Mutex<Option<MapState>>>>> = Mutex::new(None);

impl crate::native_layers_glue::LayerModels for Models {
    fn lit(&self) -> &Mutex<Option<native_render::object_program_gpu::LitObjects>> {
        &self.lit
    }

    fn ready(&self, model: u32) -> bool {
        self.lit
            .try_lock()
            .is_ok_and(|lit| lit.as_ref().is_some_and(|lit| lit.has(model)))
    }
}

/// The current map's first-person models for the object items of the
/// first person's layers (docs/graphics/framescene.md, slice 4): the
/// ones its layer 3 draws from (its call loads them; this never does),
/// only while the stage palettes they were expanded through hold.
pub(crate) fn layer_models<B: d3d::backend::Backend>(
    device: &d3d::Device<B>,
    mem: &dyn GuestMemory,
) -> Option<Arc<dyn crate::native_layers_glue::LayerModels>> {
    let source = LAYER_SOURCE.lock().ok()?.clone()?;
    let mut map_name = [0; 32];
    let name = crate::health::read_map_name(mem, &mut map_name).ok()?;
    let map_generation = render_bridge::native_map_work::generation();
    let models = {
        let state = source.lock().unwrap_or_else(|e| e.into_inner());
        let s = state
            .as_ref()
            .filter(|s| s.name == name.as_ref() && s.map_generation == map_generation)?;
        Arc::clone(&s.models)
    };
    let used = models
        .palettes
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if used.iter().any(Option::is_some) {
        let now = stage_palettes(device, mem);
        if !palettes_hold(&used, &|stage| now[stage].clone()) {
            return None;
        }
    }
    Some(models)
}

#[derive(Default)]
struct ViewPlan {
    /// Parts the plan refused, each counted with its reason.
    refused: usize,
    /// Exact planned draws, in the view's order.
    draws: Vec<(u32, PlannedDraw)>,
}

/// Plans every offered part (`parts`, indices into `scene.parts`) of the
/// first person's view from its instance and rows as extraction copied
/// them (`LitObjects::plan_part_of`, the scene objects' planner). Only a
/// fully resolved plan is a draw: a missing model, program, constant,
/// entry field or W mode is a refusal here, before `Host::draw` returns,
/// never a silent encode-time omission.
fn plan_parts(
    models: &Models,
    scene: &frame_scene::FrameScene,
    parts: &[u32],
    entry: &LayerEntry,
    time: Option<f64>,
) -> ViewPlan {
    let mut out = ViewPlan::default();
    let Ok(lit) = models.lit.try_lock() else {
        out.refused = parts.len();
        count_refusal("the first-person models are being replaced");
        return out;
    };
    for &index in parts {
        let planned = (|| -> Result<(u32, PlannedDraw), String> {
            let part = scene.parts.get(index as usize).ok_or("the part is not in the scene")?;
            let instance = scene.instances.get(part.instance as usize).ok_or("the part's instance is not in the scene")?;
            let model = instance.model;
            if !entry.w_buffered {
                return Err("the device's draws are not W-buffered".into());
            }
            // A record's fade byte: its prologue sets the stipple pattern
            // the byte picks and its epilogue the solid one
            // (`frame_scene::PartFlags::FADE`).
            if part.flags.contains(frame_scene::PartFlags::FADE) {
                if !entry.stipple_off {
                    return Err("the record's fade byte, drawn with the polygon stipple on, which the first-person draw does not apply".into());
                }
                if !entry.stipple_at_rest {
                    return Err("the record's fade byte, where the device does not hold the solid stipple and a cached fade of one, which its epilogue leaves".into());
                }
            }
            // A shader whose prologue uploads the structure's parameter
            // block (`frame_scene::PartFlags::STRUCTURE_PARAMETERS`).
            if part.flags.contains(frame_scene::PartFlags::STRUCTURE_PARAMETERS) && entry.structure.is_none() {
                return Err("the structure's parameter block, whose upload the device and the engine do not hold".into());
            }
            let lit = lit.as_ref().ok_or("no first-person model is uploaded")?;
            let mut draw = lit.plan_part_of(model, scene, instance, part, &entry.states, time, entry.structure.as_ref())?;
            // A record toggling the auxiliary state draws with what the
            // toggle sets for the first person's view.
            if part.flags.contains(frame_scene::PartFlags::AUX_DEPTH) {
                draw.aux_depth = Some(
                    entry.aux.ok_or("the auxiliary depth state, which the device does not hold off")?,
                );
            }
            // A flagged shader's item before the frame's structure target
            // copy (`frame_scene::PartFlags::TARGET_UNCOPIED`): drawn unless
            // its program has flag bit 20, whose draw would make the copy.
            if native_render::object_program_gpu::makes_target_copy(part.flags, [lit.planned_pass(model, &draw)]) {
                return Err("the shader's flag bit 0 before the frame's structure target copy, with a program of flag bit 20, whose draw would make the copy".into());
            }
            if draw.texaccum && entry.texaccum.is_none() {
                return Err(
                    "it samples the texture accumulation target, which is not bound or not given to this pass"
                        .into(),
                );
            }
            crate::native_objects_glue::layer_draw_fog(
                lit, model, &mut draw, &entry.fog_words, entry.specular,
            ).ok_or("the first-person fog or final combiner state is unavailable")?;
            Ok((model, draw))
        })();
        match planned {
            Ok(draw) => out.draws.push(draw),
            Err(why) => {
                out.refused += 1;
                count_refusal(&why);
            }
        }
    }
    out
}

static REFUSED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static REASONS: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn count_refusal(why: &str) {
    REFUSED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut reasons = REASONS.lock().unwrap_or_else(|e| e.into_inner());
    let reasons = reasons.get_or_insert_with(HashSet::new);
    if reasons.len() < 64 && reasons.insert(why.to_owned()) {
        eprintln!(
            "native-first-person: a part is not drawn: {why} (each reason logged once)"
        );
    }
}

/// One first-person call's preplanned draws.
struct FirstPersonPass {
    models: Arc<Models>,
    draws: Vec<(u32, PlannedDraw)>,
    /// The texture accumulation target, when a draw samples it.
    texaccum: Option<native_render::world_layers::Texaccum>,
    view: Option<[[f32; 4]; d3d::VIEW_CONSTANTS]>,
    w_clip: ([f32; 2], bool),
}

fn weapon_view_inputs(
    constants: [[f32; 4]; d3d::VIEW_CONSTANTS],
    request: d3d::ExternalPassRequest,
    copy: d3d::TargetCopy,
    size: (u32, u32),
    w_clip: ([f32; 2], bool),
) -> ViewInputs {
    ViewInputs {
        constants: request.view_constants(copy, constants),
        guest_size: request.surface_size,
        size,
        z_scale: request.z_scale,
        w_clip: Some(w_clip),
    }
}

impl FirstPersonPass {
    /// The call's packets for the D3D render pass open on these attachments.
    fn packets(
        &self,
        target: &d3d::backend::ExternalAttachments,
        request: d3d::ExternalPassRequest,
    ) -> Result<d3d::backend::ExternalDraws, &'static str> {
        let constants = self.view.ok_or("first-person view constants unavailable")?;
        let mut guard = self.models.lit.lock().unwrap_or_else(|e| e.into_inner());
        let lit = guard
            .as_mut()
            .ok_or("planned first-person geometry unavailable")?;
        if !self
            .draws
            .iter()
            .all(|(model, draw)| lit.has(*model) && !draw.indices.is_empty())
        {
            return Err("planned first-person geometry unavailable");
        }
        let view = weapon_view_inputs(constants, request, target.copy, target.size, self.w_clip);
        let texaccum = crate::native_objects_glue::texaccum_level(target, self.texaccum)?;
        let draws = lit.draws(
            &target.device,
            &target.queue,
            &target.uploads,
            target.color_format,
            target.depth_format,
            target.size,
            &view,
            request.projection_factor(target.copy),
            &self.draws,
            texaccum,
        )?;
        crate::native_layers_glue::clip_packets(draws, request, target.size)
    }
}

impl d3d::ExternalPass for FirstPersonPass {
    fn view_constants(&mut self, constants: &[[f32; 4]; d3d::VIEW_CONSTANTS]) {
        self.view = Some(*constants);
    }

    fn depth_clip(&mut self, clip: d3d::DepthClip) {
        self.w_clip = (clip.range, clip.clamp);
    }

    fn sampled(&self) -> Vec<u32> {
        self.texaccum.iter().map(|target| target.texture).collect()
    }

    fn writes(&self) -> d3d::ExternalWrites {
        d3d::ExternalWrites {
            color: self
                .draws
                .iter()
                .any(|(_, draw)| !draw.state.color_writes.is_empty()),
            depth_stencil: self.draws.iter().any(|(_, draw)| draw.state.depth_write),
        }
    }

    /// The draws join the D3D render pass already open on the call's
    /// attachments: no pass or command buffer of their own.
    fn record_draws(
        &mut self,
        target: &d3d::backend::ExternalAttachments,
        request: d3d::ExternalPassRequest,
    ) -> Option<Result<d3d::backend::ExternalDraws, &'static str>> {
        Some(self.packets(target, request))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_committed_pass_draws_every_planned_part_and_names_each_drop() {
        let out = settle(2, 5, true);
        assert_eq!(out.drawn, 5);
        assert_eq!(out.dropped, vec![(Dropped::Unsupported, 2)]);
        let out = settle(1, 4, false);
        assert_eq!(out.drawn, 0);
        assert_eq!(out.dropped, vec![(Dropped::Unsupported, 1), (Dropped::Reservation, 4)]);
        assert_eq!(out.dropped_total(), 5);
        assert_eq!(settle(0, 3, true), Outcome { drawn: 3, dropped: vec![] });
    }

    #[test]
    fn weapon_view_preserves_retail_size_in_pixels_at_the_same_height() {
        let mut constants = [[0.0; 4]; d3d::VIEW_CONSTANTS];
        constants[..4].copy_from_slice(&[
            [1.5, 0.0, 0.25, 0.0], [0.0, 2.0, -0.125, 0.0],
            [0.0, 0.0, 1.0, -0.25], [0.0, 0.0, 1.0, 0.0],
        ]);
        let request = d3d::ExternalPassRequest {
            rect: (0, 0, 640, 480), surface_size: (640, 480),
            z_scale: 1.0, display_projection_factor: 0.75,
        };
        let clip = ([0.125, 100.0], true);
        let retail = weapon_view_inputs(constants, request, d3d::TargetCopy::Reference, (640, 480), clip);
        let wide = weapon_view_inputs(constants, request, d3d::TargetCopy::Display, (5120, 2880), clip);
        assert_eq!(retail.constants, constants);
        assert_eq!(wide.constants[1..], retail.constants[1..]);
        assert_eq!(wide.w_clip, retail.w_clip);
        // For any point on the gun, its projected distance from screen centre
        // in units of screen height is identical. Includes the lens shift.
        for x in [-0.5, 0.0, 0.25] {
            let offset = |row: [f32; 4], width: f32, height: f32| {
                (row[0] * x + row[2]) * 0.5 * width / height
            };
            let a = offset(retail.constants[0], 640.0, 480.0);
            let b = offset(wide.constants[0], 5120.0, 2880.0);
            assert!((a - b).abs() < 1e-6);
        }
    }
}
