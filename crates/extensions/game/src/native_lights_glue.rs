//! Native light draws over sorted guest geometry. Each target copy prepares
//! immutable draw resources alongside the exact resolved fallback.
mod host;
mod indexed;
mod materials;
pub mod models;
mod residency;
mod scene;
mod tiles;
pub mod walk;

use crate::native_gels_glue::{Binding, Checkpoint, Outputs};
use std::collections::{BTreeMap, HashMap, HashSet, btree_map::Entry};
use std::ops::Range;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use engine::memory::{GuestMemory, MemFault};
use frame_scene::{LayerOwner, LightEntry, Row};
use map::{Map, TagId};
use native_render::gpu::Image;
use native_render::light_gpu::{
    LayeredImage, LightDraw, LightGpu, LightSet, LightTarget, ModelGeometry, StaticLightTexture,
    TextureDim,
};
use native_render::world_layers::{Geometry, Owner as ResolvedOwner};
use native_render::light_look::{
    DIFFUSE_LAYER, LightInputs, LightProgram, LightStage, SPECULAR_LAYER,
};
use native_render::light_models::LightModels;
use native_render::lights::{Light, LightConstants};
use native_render::look;
use native_render::program::InheritedCull;
use native_render::program::ViewInputs;
use native_render::shadow_look::{CASTER_LAYER, OPACITY_LAYER, RECEIVER_LAYER};
use native_render::world::{ModelRows, Owner, World, WorldDraw};
use render_bridge::native_lights::{Alone, Dispatch, Host, ShadowAlone};
use tags::material::{EngineInputs, Material, ShaderLibrary, TextureSource};

use crate::native_world_glue::{
    ReadMemory, Ready, StagePalettes, State, current_ready, inherited_cull, light_image,
};

/// The hook's guest memory as the device's.
pub struct DeviceMemory<'a>(pub &'a mut dyn GuestMemory);

fn guest_error(f: MemFault) -> d3d::GuestError {
    d3d::GuestError {
        addr: f.addr,
        len: f.len,
    }
}

impl d3d::GuestMemory for DeviceMemory<'_> {
    fn read(&self, addr: u32, out: &mut [u8]) -> Result<(), d3d::GuestError> {
        self.0.read(addr, out).map_err(guest_error)
    }
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), d3d::GuestError> {
        self.0.write(addr, data).map_err(guest_error)
    }
}

struct Glue<B: d3d::backend::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
    state: State,
    gels: Arc<Outputs>,
    checkpoint: Arc<Mutex<Option<(u64, Checkpoint)>>>,
    /// What preparation returned, by reason ([`Tally`]).
    tally: Mutex<Tally>,
    order: Mutex<RunOrder>,
    admission: Arc<LightAdmission>,
    /// The map's model parts for object entries ([`models`]).
    models: models::Loader,
    /// A live edit's image refresh is running ([`Glue::refresh_edits`]);
    /// edits wait in the queue until it ends, so they apply in order.
    refreshing: Arc<std::sync::atomic::AtomicBool>,
    /// The held run of slot 4's lights ([`tiles`]).
    run: Mutex<tiles::Run>,
}

/// Why a native light plan declined a request.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Keep {
    /// The world (or its lights' part) is not loaded.
    NoWorld,
    /// The hook did not compute the light's constants.
    NoConstants,
    /// A part it meets has no material (its shader does not read).
    NoMaterial,
    /// The BSP worker has yet to publish this variant.
    MaterialPending,
    /// A pass it needs is refused (`light_look`, `compile_light`, the
    /// material), with why.
    Refused(Arc<str>),
    /// A texture a pass samples did not load: which.
    Unloaded(&'static str),
    Unsupported(&'static str),
    /// A scene-caster refusal category reported by `native_shadows`.
    Shadow(&'static str),
    /// The map's model parts for `what` are still loading.
    Loading(&'static str),
}

impl Keep {
    /// The world, a material variant, a texture or the model parts are
    /// still loading: the light waits for them ([`scene_drawn`]).
    fn loading(&self) -> bool {
        matches!(self, Keep::NoWorld | Keep::MaterialPending | Keep::Unloaded(_) | Keep::Loading(_))
    }
}

/// A scene light's or shadow's draw: the drawn value, `None` while what it
/// needs is loading (it draws once it has loaded), and a panic for a
/// refusal the native code does not handle (`lights: host: <why>`).
fn scene_drawn<T>(result: Result<T, Keep>) -> Option<T> {
    match result {
        Ok(v) => Some(v),
        Err(why) if why.loading() => None,
        Err(why) => unimplemented!("lights: host: {why}"),
    }
}

impl std::fmt::Display for Keep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Keep::NoWorld => f.write_str("the world's lights are not loaded"),
            Keep::NoConstants => f.write_str("no constants from the hook"),
            Keep::NoMaterial => f.write_str("meets a part without a material"),
            Keep::MaterialPending => f.write_str("light material preparation pending"),
            Keep::Refused(why) => write!(f, "pass refused: {why}"),
            Keep::Unloaded(what) => write!(f, "samples an unloaded {what}"),
            Keep::Unsupported(why) => f.write_str(why),
            Keep::Shadow(why) => write!(f, "scene caster refused: {why}"),
            Keep::Loading(what) => write!(f, "{what} loading"),
        }
    }
}

/// Host planning decisions between summaries.
pub const SUMMARY_EVERY: u64 = 1800;
const REFUSAL_DETAIL_LIMIT: usize = 32;

/// Whole-light, scene-group and resolved-layer preparation attempts. One
/// light can reach more than one path. Per-emission GPU results are separate.
#[derive(Debug, Default)]
pub struct Tally {
    pub asked: u64,
    pub planned: u64,
    pub shader_gels: u64,
    pub kept: BTreeMap<Keep, u64>,
    refusal_details: usize,
    other_refusals: u64,
}

impl Tally {
    /// Counts one answer; returns what to say on stderr: a reason the first
    /// time it keeps a light, and every [`SUMMARY_EVERY`] lights the
    /// counts.
    pub fn count(&mut self, answer: Result<(), Keep>) -> Vec<String> {
        self.asked += 1;
        let mut say = match answer {
            Ok(()) => {
                self.planned += 1;
                Vec::new()
            }
            Err(k) => self.count_kept(k),
        };
        if self.asked % SUMMARY_EVERY == 0 {
            say.push(self.summary());
        }
        say
    }

    /// Counts a reason a light keeps (some of) its D3D passes; the line to
    /// say the first time.
    pub fn count_kept(&mut self, k: Keep) -> Vec<String> {
        match self.kept.entry(k) {
            Entry::Occupied(mut entry) => {
                *entry.get_mut() += 1;
                Vec::new()
            }
            Entry::Vacant(entry) => {
                if matches!(entry.key(), Keep::Refused(_)) {
                    if self.refusal_details >= REFUSAL_DETAIL_LIMIT {
                        self.other_refusals += 1;
                        return Vec::new();
                    }
                    self.refusal_details += 1;
                }
                let line = format!("native-lights: light plan declined: {}", entry.key());
                entry.insert(1);
                vec![line]
            }
        }
    }

    /// One line: lights asked, planned, and kept by reason, most first.
    pub fn summary(&self) -> String {
        let mut kept: Vec<_> = self.kept.iter().collect();
        kept.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        let mut kept: Vec<String> = kept.iter().map(|(k, n)| format!("{n} {k}")).collect();
        if self.other_refusals != 0 {
            kept.push(format!("{} other shader refusals", self.other_refusals));
        }
        format!(
            "native-lights: {} of {} host planning requests accepted; declined: {}",
            self.planned,
            self.asked,
            if kept.is_empty() {
                "none".to_string()
            } else {
                kept.join(", ")
            }
        )
    }
}

impl<B: d3d::backend::Backend + Send + 'static> Glue<B> {
    fn gel_binding(
        &self,
        mem: &dyn GuestMemory,
        serial: u64,
        identity: Option<(u32, u32)>,
        c: &LightConstants,
    ) -> Result<Option<Binding>, Keep> {
        let Some((definition, shader)) = identity else {
            return if c.animated_gel {
                Err(Keep::Unsupported("missing gel invocation"))
            } else {
                Ok(None)
            };
        };
        use native_render::gels;
        if !uses_gel_target(mem, definition, shader, c.flags, c.definition_flags) {
            return Err(Keep::Unsupported(
                "engine texture 10 does not select this gel invocation",
            ));
        }
        // The light's entry checkpoint serves each of its layers.
        let held = *self.checkpoint.lock().unwrap_or_else(|e| e.into_inner());
        let before = held
            .filter(|(id, _)| *id == serial)
            .map(|(_, c)| c)
            .ok_or(Keep::Unsupported("missing gel entry checkpoint"))?;
        let target = gels::Target::read(&ReadMemory(mem))
            .ok_or(Keep::Unsupported("unavailable gel target 13"))?;
        self.gels
            .binding_after(before, definition, shader, target.surface)
            .filter(|b| b.guest_size == target.size)
            .map(Some)
            .ok_or(Keep::Unsupported(
                "no matching target 13 binding for this light",
            ))
    }

    fn ready(&self, mem: &dyn GuestMemory) -> Result<Arc<Ready>, Keep> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        Ok(Arc::clone(current_ready(&state, mem).ok_or(Keep::NoWorld)?))
    }

    /// Give the resolved draws the device state they take at this point.
    fn prepare_planned(
        &self,
        d: &d3d::Device<B>,
        mem: &dyn GuestMemory,
        ready: Arc<Ready>,
        constants: LightConstants,
        mut draws: Vec<Planned>,
        binding: Option<Binding>,
        colour_only: bool,
        typed_rect: Option<[u32; 4]>,
    ) -> Result<PreparedLight, Keep> {
        let world = ready.lights.get().ok_or(Keep::NoWorld)?;
        if draws.is_empty() {
            return Err(Keep::Unsupported(
                "no supported light draws in the active list",
            ));
        }
        let memory = ReadMemory(mem);
        // A pass the instance animates draws with its constants at the
        // render clock of this call.
        let time = draws
            .iter()
            .any(|p| p.compiled.program.animation.is_some())
            .then(|| native_render::layer_pass::render_time(&memory));
        for p in &mut draws {
            if let (Some(a), Some(time)) = (&p.compiled.program.animation, time) {
                p.sample = Some(a.sample(time).map_err(|_| {
                    Keep::Unsupported("a light pass's animation does not sample at the render clock")
                })?);
            }
        }
        let target = d
            .external_ready(&memory, colour_only)
            .map_err(|why| Keep::Unsupported(why.describe()))?;
        let clip = match typed_rect {
            Some(rect) => rect,
            None => {
                if !d.stencil_state(&memory).is_some_and(|s| !s.enable) {
                    return Err(Keep::Unsupported(
                        "enabled or unknown stencil state at light geometry",
                    ));
                }
                d.light_scissor(&memory).map_err(Keep::Unsupported)?
            }
        };
        let scissor = intersect_scissor(clip, target.rect);
        if d.depth_clip(&memory, target.z_scale).w_buffered != Some(true) {
            return Err(Keep::Unsupported(
                "light view does not have a known W-buffer",
            ));
        }
        // Each palettized stage image against the palette its load
        // expanded it through: the world's, or the models' for a model
        // draw.
        let mut verified = [[false; 4]; 2];
        for pass in &draws {
            let program = &pass.compiled.program;
            let (palettized, palettes, k) = match &pass.model {
                Some(m) => (&m.lights.palettized, &m.lights.palettes, 1),
                None => (&world.palettized, &world.palettes, 0),
            };
            for (n, stage) in program.stages.iter().enumerate() {
                if let LightStage::Bitmap { bitmap, image } = stage
                    && program.dims[n].is_some()
                    && !verified[k][n]
                    && palettized.contains(&(n, *bitmap, *image))
                {
                    if d.stage_palette(&memory, n) != palettes[n] {
                        return Err(Keep::Unsupported(
                            "stage palette changed since light texture upload",
                        ));
                    }
                    verified[k][n] = true;
                }
            }
        }
        let cull = d.cull_state(&memory).map(inherited_cull);
        if cull.is_none() && draws.iter().any(|p| p.compiled.program.inherits_cull) {
            return Err(Keep::Unsupported(
                "unreadable inherited CULLMODE or FRONTFACE",
            ));
        }
        let cull = InheritedCull {
            device: cull,
            clusters: true,
            placements: false,
        };
        let formats = if colour_only {
            d.external_pipeline_colour_target(&memory)
        } else {
            d.external_pipeline_target(&memory)
        };
        let (device, color, depth) =
            formats.ok_or(Keep::Unsupported("light pipeline target unavailable"))?;
        let mut pipelines_ready = true;
        for draw in &draws {
            let model = match &draw.model {
                Some(m) => Some(&m.buffer()?.layout),
                None => None,
            };
            if let Ok(mut pass) = draw.compiled.gpu.try_lock() {
                let inherited = draw.cull(&cull);
                let (_, compiled) = pass.pipeline(&device, color, depth, inherited, model);
                pipelines_ready &= compiled.previous_frame().is_some_and(Option::is_some);
            } else {
                pipelines_ready = false;
            }
        }
        if !pipelines_ready {
            return Err(Keep::Unsupported("light pipeline pending or unavailable"));
        }
        Ok(PreparedLight {
            ready,
            constants,
            projection: None,
            shadow_map: None,
            frozen: None,
            draws,
            cull,
            scissor,
            binding,
        })
    }

    /// Resolve all the entries of a diffuse or specular group. The typed
    /// light and the single-layer path use the same resident geometry and
    /// material variants, including each object's pose at its sorted slot.
    fn scene_light_draws(
        &self,
        d: &d3d::Device<B>,
        mem: &dyn GuestMemory,
        ready: &Ready,
        group: &render_bridge::native_lights::SceneGroup,
        c: &LightConstants,
        produced_gel: bool,
    ) -> Result<Vec<Planned>, Keep> {
        self.gathered_draws(
            d, mem, ready, usize::from(group.group.layer), c, produced_gel,
            group.entries.iter().enumerate().map(|(i, e)| {
                (e, group.rows.get(i).map_or(&[][..], Vec::as_slice))
            }),
        )
    }

    fn model_lights(
        &self,
        device: &d3d::Device<B>,
        mem: &dyn GuestMemory,
        ready: &Ready,
    ) -> Result<Arc<models::ModelLights>, Keep> {
        let name = crate::health::loaded_map_name(mem)
            .map_err(|_| Keep::Unsupported("the loaded map's name does not read"))?;
        self.models.get(
            &name,
            &ready.upload_device.as_ref().ok_or(Keep::NoWorld)?.0,
            || native_render::object_streams::StreamFormats::read(&ReadMemory(mem)),
            || crate::native_world_glue::scene_load_palettes(device, mem),
        ).ok_or(Keep::Loading("the map's model light parts"))
    }

    fn gathered_draws<'a>(
        &self,
        device: &d3d::Device<B>,
        mem: &dyn GuestMemory,
        ready: &Ready,
        layer: usize,
        c: &LightConstants,
        produced_gel: bool,
        entries: impl Iterator<Item = (&'a LightEntry, &'a [Row])>,
    ) -> Result<Vec<Planned>, Keep> {
        let world = ready.lights.get().ok_or(Keep::NoWorld)?;
        let mut model_lights = None;
        let mut held = None;
        // Runs of structure entries resolve together, as the sorted
        // list's do; each object entry is a draw of its own (its rows).
        let mut draws: Vec<Planned> = Vec::new();
        let mut run = Vec::new();
        // A flagged shader's entry before the frame's structure target
        // copy: a pass of the layer with flag bit 20 would make the copy
        // (`0x0003bea0`), which no native stage does.
        let mut uncopied = false;
        for (entry, rows) in entries {
            let binding = entry.binding;
            uncopied |= entry.state.shader_flag && entry.state.uncopied;
            let selection = Some((binding.variant, binding.selector));
            let aux = entry_state(device, mem, entry, &mut held)?;
            if structure_key(entry).is_some() {
                for (draw, flipped) in world.structure_entry(entry)? {
                    run.push((layer, usize::from(binding.implementation), draw, selection, flipped));
                }
                continue;
            }
            if !run.is_empty() {
                draws.extend(world.resolve_world_parts(c, run.drain(..), produced_gel)?);
            }
            let lights = model_lights
                .get_or_insert_with(|| self.model_lights(device, mem, ready))
                .as_ref()
                .map_err(Clone::clone)?;
            for mut planned in world.resolve_model(c, lights, layer, entry, rows, selection, produced_gel)? {
                if let Some(model) = &mut planned.model {
                    model.aux_depth = aux;
                }
                draws.push(planned);
            }
        }
        if !run.is_empty() {
            draws.extend(world.resolve_world_parts(c, run, produced_gel)?);
        }
        if uncopied && draws.iter().any(|p| p.compiled.program.copies_target) {
            return Err(Keep::Unsupported(
                "a flagged shader's pass with flag bit 20, before the frame's structure target copy",
            ));
        }
        Ok(draws)
    }

    /// A light layer drawn from its scene group alone
    /// ([`render_bridge::native_lights::draw_alone`]): the group's
    /// structure parts resolved as [`Self::reserve`] resolves the sorted
    /// list's, its object entries from the map's model parts
    /// ([`models`], [`LightsWorld::resolve_model`]) with their node rows,
    /// in the sorted order, issued now as one external pass, with no guest
    /// handler and so no resolved fallback. A part not held (or object
    /// entries before the models load), a shader gel (its target-13 slot
    /// is pinned through a draw's plan) or a state the device cannot give
    /// refuses, issuing nothing.
    fn alone(&self, mem: &mut dyn GuestMemory, a: Alone<'_>) -> Result<(), Keep> {
        if a.shader_gel.is_some() {
            return Err(Keep::Unsupported("a shader gel draws through its handler"));
        }
        let mut d = self
            .device
            .lock()
            .map_err(|_| Keep::Unsupported("device unavailable"))?;
        if !d
            .begin_external_boundary(&mut DeviceMemory(mem))
            .map_err(|_| Keep::Unsupported("could not drain light state"))?
        {
            return Err(Keep::Unsupported("light has an outstanding push callback"));
        }
        if d.bound_target() != a.view_target {
            return Err(Keep::Unsupported("light view target changed"));
        }
        let c = constants(&a.light)?;
        let ready = self.ready(mem)?;
        if u32::from(a.group.group.layer) != a.layer {
            return Err(Keep::Unsupported("light group names another layer"));
        }
        let layer = a.layer as usize;
        let draws = self.scene_light_draws(&d, mem, &ready, a.group, &c, false)?;
        let prepared =
            self.prepare_planned(&d, mem, Arc::clone(&ready), c, draws, None, false, None)?;
        use native_render::light_passes;
        let drawn = light_passes::read(&ReadMemory(&*mem), c.flags, c.definition_flags)
            .map_or(0, |i| light_passes::steps(&i).drawn());
        if drawn & 1 << a.layer == 0 || prepared.draws.iter().any(|p| p.layer != layer) {
            return Err(Keep::Unsupported(
                "active light layers changed during setup",
            ));
        }
        if mem.u32(engine::light_passes::global::PASS_FLAGS).ok() != Some(c.flags)
            || c.frame.iter().enumerate().any(|(i, word)| {
                mem.u32(engine::light_constants::global::FRAME + 4 * i as u32)
                    .ok()
                    != Some(*word)
            })
        {
            return Err(Keep::Unsupported(
                "light globals differ from the prepared frame or flags",
            ));
        }
        // This group has no resolved draw to replay. Reserve its packets on
        // every copy before allowing the hook to skip its guest handlers.
        let plan = d3d::ExternalPlan {
            stage: timeline::native::Stage::Lights,
            view: mem
                .u32(engine::render_loop::occlusion::guest::VIEW_INDEX)
                .unwrap_or(u32::MAX),
            ranges: prepared.draws.iter().map(|draw| draw.ranges.len()).sum(),
            slots: Vec::new(),
        };
        let pass = RunPass {
            retained: None,
            light: prepared,
            view: None,
            clip: None,
            gel_slot: None,
            fallback: None,
            colour_only: false,
            lighting_only: None,
            cone_shadow: None,
        };
        d.prepare_external_bound(&mut DeviceMemory(mem), plan, Box::new(pass), Vec::new())
            .map_err(|_| Keep::Unsupported("device error preparing the scene light pass"))?
            .ok_or(Keep::Unsupported("the scene light pass is not ready"))?
            .commit();
        Ok(())
    }

    /// A shadow group's resolved draws with the state they take: its
    /// constants (the dynamic light's, or the shadow's flags alone), its
    /// projection and, for a receiver that samples one, the frozen map
    /// `map` names. `typed_rect` is the target rectangle of a pass drawn
    /// on explicit attachments; `None` takes the device's scissor.
    #[allow(clippy::too_many_arguments)]
    fn shadow_prepared(
        &self,
        d: &d3d::Device<B>,
        mem: &dyn GuestMemory,
        view: u32,
        group: &render_bridge::native_lights::SceneGroup,
        light: Option<LightConstants>,
        map: ShadowMap,
        typed_rect: Option<[u32; 4]>,
        gel: Option<Binding>,
    ) -> Result<PreparedLight, Keep> {
        let layer = usize::from(group.group.layer);
        if !matches!(layer, CASTER_LAYER | RECEIVER_LAYER | OPACITY_LAYER) {
            return Err(Keep::Shadow("state"));
        }
        let shadow = group.group.pass.ok_or(Keep::Shadow("state"))?;
        let projection = group.group.projection;
        let colour_mask = layer == CASTER_LAYER && shadow.colour_mask();
        let ready = self.ready(mem)?;
        let world = ready.lights.get().ok_or(Keep::NoWorld)?;
        let objects = group.entries.iter().any(|e| structure_key(e).is_none());
        let model_lights = objects.then(|| self.model_lights(d, mem, &ready)).transpose()?;
        let held = Held {
            world,
            models: model_lights.as_deref().map(|m| &m.models),
        };
        native_render::shadow_scene::plan_entries(
            &group.group,
            &group.entries,
            &|e| group.rows_of(e),
            &held,
        )
        .map_err(|why| Keep::Shadow(why.name()))?;
        let mut draws: Vec<Planned> = Vec::new();
        let mut run: Vec<(usize, WorldDraw, Option<(u8, u8)>, bool)> = Vec::new();
        let mut state_held = None;
        // A flagged shader's entry before the frame's structure target
        // copy, as in `gathered_draws`.
        let mut uncopied = false;
        for e in &group.entries {
            let imp = usize::from(e.binding.implementation);
            uncopied |= e.state.shader_flag && e.state.uncopied;
            let aux = entry_state(d, mem, e, &mut state_held)?;
            if structure_key(e).is_some() {
                // The entry's level of detail and pass, as the handler
                // binds them (`selected_light_look`).
                let selection = Some((e.binding.variant, e.binding.selector));
                run.extend(
                    world
                        .structure_entry(e)?
                        .into_iter()
                        .map(|(draw, flipped)| (imp, draw, selection, flipped)),
                );
                continue;
            }
            if !run.is_empty() {
                if layer == CASTER_LAYER {
                    draws.extend(world.resolve_shadow_parts(std::mem::take(&mut run))?);
                } else {
                    draws.extend(world.resolve_receiver_parts(layer, std::mem::take(&mut run), gel.is_some())?);
                }
            }
            let lights = model_lights.as_ref().ok_or(Keep::Loading("the map's model shadow parts"))?;
            let mut planned = if layer == CASTER_LAYER {
                world.resolve_shadow_model(lights, e, group.rows_of(e))?
            } else {
                world.resolve_receiver_model(lights, e, group.rows_of(e), layer, gel.is_some())?
            };
            for p in &mut planned {
                if let Some(model) = &mut p.model {
                    model.aux_depth = aux;
                }
            }
            draws.extend(planned);
        }
        if !run.is_empty() {
            if layer == CASTER_LAYER {
                draws.extend(world.resolve_shadow_parts(run)?);
            } else {
                draws.extend(world.resolve_receiver_parts(layer, run, gel.is_some())?);
            }
        }
        if uncopied && draws.iter().any(|p| p.compiled.program.copies_target) {
            return Err(Keep::Unsupported(
                "a flagged shader's pass with flag bit 20, before the frame's structure target copy",
            ));
        }
        let constants = match light {
            // A scene light's gel is drawn into target 13 ahead of its
            // layers (`scene::Op::Gel`): its receivers sample it.
            Some(constants) if constants.animated_gel && gel.is_none() => {
                return Err(Keep::Unsupported(
                    "an animated gel keeps the receiver on D3D",
                ));
            }
            Some(constants) => constants,
            None => LightConstants {
                frame: [0; 36],
                flags: shadow.flags,
                colour_a: [0; 3],
                colour_b: [0; 3],
                mode: 0,
                definition_flags: 0,
                definition_8a: 0,
                definition_80: 0,
                falloff_0: 0,
                gel: None,
                animated_gel: false,
            },
        };
        let needs_map = layer == RECEIVER_LAYER
            && draws.iter().any(|draw| {
                draw.compiled
                    .program
                    .stages
                    .iter()
                    .zip(draw.compiled.program.dims)
                    .any(|(stage, dim)| {
                        matches!(stage, LightStage::DepthShadow | LightStage::ColourMask)
                            && dim.is_some()
                    })
            });
        let (shadow_map, frozen) = if needs_map {
            let projection = projection.ok_or(Keep::Shadow("state"))?;
            let projection = std::array::from_fn(|n| projection[n / 4][n % 4]);
            let kind = if shadow.colour_mask() {
                native_render::shadow_gpu::MapKind::ColourMask
            } else {
                native_render::shadow_gpu::MapKind::Depth
            };
            match map {
                ShadowMap::Latest => (
                    Some(
                        crate::native_shadows_glue::latest_receiver(view, projection, kind)
                            .ok_or(Keep::Shadow("target"))?,
                    ),
                    None,
                ),
                ShadowMap::Frozen(frozen) => (None, Some(frozen.ok_or(Keep::Shadow("target"))?)),
            }
        } else {
            (None, None)
        };
        let mut prepared = self.prepare_planned(
            d,
            mem,
            Arc::clone(&ready),
            constants,
            draws,
            gel,
            colour_mask,
            typed_rect,
        )?;
        prepared.projection =
            projection.map(|projection| std::array::from_fn(|n| projection[n / 4][n % 4]));
        prepared.shadow_map = shadow_map;
        prepared.frozen = frozen;
        if prepared.draws.iter().any(|p| p.layer != layer) {
            return Err(Keep::Unsupported(
                "another layer's pass is in the shadow run",
            ));
        }
        {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !current_ready(&state, mem).is_some_and(|current| Arc::ptr_eq(current, &ready)) {
                return Err(Keep::Unsupported("world changed during shadow preparation"));
            }
        }
        Ok(prepared)
    }

    /// A shadow group drawn from the scene with no geometry handler. Its
    /// own target, projection and frozen map are checked after the drain,
    /// all material and geometry resources are sealed, and only then may
    /// the guest loop be emptied. The guest's begin, clear and end still run.
    fn shadow_alone(&self, mem: &mut dyn GuestMemory, a: ShadowAlone<'_>) -> Result<(), Keep> {
        let group = a.group;
        let layer = usize::from(group.group.layer);
        if !matches!(layer, CASTER_LAYER | RECEIVER_LAYER | OPACITY_LAYER) {
            return Err(Keep::Shadow("state"));
        }
        let shadow = group.group.pass.ok_or(Keep::Shadow("state"))?;
        let colour_mask = layer == CASTER_LAYER && shadow.colour_mask();
        let expected = if layer == RECEIVER_LAYER {
            None
        } else {
            Some(
                crate::native_shadows_glue::expected_targets(mem, shadow.target)
                    .ok_or(Keep::Shadow("target"))?,
            )
        };
        let mut d = self.device.lock().map_err(|_| Keep::Shadow("device"))?;
        if !d
            .begin_external_boundary(&mut DeviceMemory(mem))
            .map_err(|_| Keep::Shadow("callback"))?
        {
            return Err(Keep::Shadow("callback"));
        }
        if !d.can_external_pass()
            || expected.is_some_and(|expected| d.bound_target()[..2] != expected)
        {
            return Err(Keep::Shadow("target"));
        }
        if render_bridge::scene_step::shadow_pass(&ReadMemory(&*mem), layer as u8) != Some(shadow) {
            return Err(Keep::Shadow("state"));
        }
        let prepared = self.shadow_prepared(
            &d,
            &*mem,
            a.view,
            group,
            a.light,
            ShadowMap::Latest,
            None,
            None,
        )?;
        let plan = d3d::ExternalPlan {
            stage: timeline::native::Stage::Shadows,
            view: a.view,
            ranges: prepared.draws.iter().map(|draw| draw.ranges.len()).sum(),
            slots: Vec::new(),
        };
        let pass: Box<dyn d3d::ExternalPass> = Box::new(RunPass {
            retained: None,
            light: prepared,
            view: None,
            clip: None,
            gel_slot: None,
            fallback: None,
            colour_only: colour_mask,
            lighting_only: None,
            cone_shadow: None,
        });
        // An object mask the shadow host offered a private chain draws into
        // that chain's pair, not target 10 (native-shadow-layers.md).
        let private = if colour_mask {
            crate::native_shadow_chain::take()
        } else {
            None
        };
        let pass: Box<dyn d3d::ExternalPass> = match private {
            Some(chain) => Box::new(crate::native_shadow_chain::PrivateCasters::new(pass, chain)),
            None => pass,
        };
        d.prepare_external_bound(&mut DeviceMemory(mem), plan, pass, Vec::new())
            .map_err(|_| Keep::Unsupported("device error preparing the scene shadow pass"))?
            .ok_or(Keep::Unsupported("the scene shadow pass is not ready"))?
            .commit();
        Ok(())
    }
}

/// Where a receiver's frozen shadow map comes from.
enum ShadowMap {
    /// The receiver boundary that just ran (`native_shadows_glue`).
    Latest,
    /// The caster of this light drawn from the scene, frozen on each copy
    /// in the same sequence ([`scene::Frozen`]); `None` when the light has
    /// no caster.
    Frozen(Option<scene::Frozen>),
}

/// The parts the light passes hold, for [`native_render::light_scene`]:
/// the world's structure parts in one stream (the world's buffers), each
/// part's range there, for its own shader only; and the map's model parts
/// ([`LightModels::part`]), a stream per model section.
struct Held<'a> {
    world: &'a LightsWorld,
    models: Option<&'a LightModels>,
}

impl native_render::light_scene::Resident for Held<'_> {
    /// The entry's part, or for visible subparts the span their native
    /// ranges cover (a range that winds the other way draws with the front
    /// face reversed, [`Planned::cull`]).
    fn part(&self, e: &LightEntry) -> Option<native_render::light_scene::ResidentPart> {
        if structure_key(e).is_none() {
            let (_, wound) = self.models?.part_wound(e)?;
            return Some(native_render::light_scene::ResidentPart {
                stream: LightModels::stream(e),
                indices: covering(wound.into_iter().map(|(r, _)| r))?,
            });
        }
        let draws = self.world.structure_entry(e).ok()?;
        let ranges = draws
            .iter()
            .map(|(d, _)| Some(d.first_index..d.first_index.checked_add(d.index_count)?))
            .collect::<Option<Vec<_>>>()?;
        Some(native_render::light_scene::ResidentPart {
            stream: 0,
            indices: covering(ranges.into_iter())?,
        })
    }
}

/// The span `ranges` cover, or `None` for none.
fn covering(ranges: impl Iterator<Item = Range<u32>>) -> Option<Range<u32>> {
    ranges.reduce(|a, b| a.start.min(b.start)..a.end.max(b.end))
}

/// What the device holds for an entry's state
/// (`frame_scene::EntryState`): the auxiliary state toggle's values off,
/// as layer 3's objects need them ([`crate::native_objects_glue::aux_depth_at_rest`]),
/// the stipple off and at rest, as a faded object's
/// ([`crate::native_objects_glue::stipple_off`], `stipple_at_rest`), and
/// the device and the engine holding the structure's parameter block
/// upload a flagged shader's prologue makes
/// ([`crate::native_objects_glue::structure_at_rest`]).
/// Skipping a handler then leaves what its prologue and epilogue would.
fn held_state<B: d3d::backend::Backend>(d: &d3d::Device<B>, mem: &dyn GuestMemory) -> frame_scene::EntryState {
    frame_scene::EntryState {
        aux_depth: held_aux(d, mem).is_some(),
        fade: crate::native_objects_glue::stipple_off(d, mem) && crate::native_objects_glue::stipple_at_rest(d, mem),
        // A flagged shader asks for residency, which sets no device state.
        shader_flag: true,
        structure: native_render::structure_parameters::read(&ReadMemory(mem))
            .is_some_and(|p| crate::native_objects_glue::structure_at_rest(d, mem, &p)),
        // Whether the frame's copy is made is the entry's own; a pass with
        // flag bit 20 is checked where the entry's draws resolve.
        uncopied: false,
    }
}

/// An entry's state (`frame_scene::EntryState`) as its draw takes it: the
/// toggle's values for an object entry toggling the auxiliary state, an
/// unmasked fade, and the structure's upload, each only while the device
/// holds what the handler's epilogue would leave. `held` is asked once per
/// layer. Every light and shadow layer draws through it.
fn entry_state<B: d3d::backend::Backend>(
    device: &d3d::Device<B>,
    mem: &dyn GuestMemory,
    entry: &LightEntry,
    held: &mut Option<(frame_scene::EntryState, Option<frame_scene::AuxDepth>)>,
) -> Result<Option<frame_scene::AuxDepth>, Keep> {
    if entry.state.plain() {
        return Ok(None);
    }
    let (now, values) = *held.get_or_insert_with(|| (held_state(device, mem), held_aux(device, mem)));
    if entry.state.fade && !now.fade {
        return Err(Keep::Unsupported("a faded entry, where the stipple is on or not at rest"));
    }
    if entry.state.structure && !now.structure {
        return Err(Keep::Unsupported(
            "a flagged entry whose structure parameter upload the device does not hold",
        ));
    }
    if !entry.state.aux_depth {
        return Ok(None);
    }
    if structure_key(entry).is_some() {
        return Err(Keep::Unsupported("a structure entry toggling the auxiliary state"));
    }
    values.map(Some).ok_or(Keep::Unsupported(
        "an entry toggling the auxiliary state, whose values the device does not hold off",
    ))
}

/// What the auxiliary state toggle sets for the current view, when the
/// device holds its values off.
fn held_aux<B: d3d::backend::Backend>(d: &d3d::Device<B>, mem: &dyn GuestMemory) -> Option<frame_scene::AuxDepth> {
    native_render::aux_depth::read(&ReadMemory(mem))
        .filter(|aux| crate::native_objects_glue::aux_depth_at_rest(d, mem, aux))
}

/// A structure entry's key in [`ClusterParts`]; `None` for an object's.
fn structure_key(e: &LightEntry) -> Option<(Owner, u8)> {
    match e.owner {
        LayerOwner::Cluster(c) => Some((Owner::Cluster(u32::from(c)), e.stream.part)),
        LayerOwner::Placement(p) => Some((Owner::Instance(u32::from(p)), e.stream.part)),
        LayerOwner::Object(_) => None,
    }
}

impl<B: d3d::backend::Backend + Send + 'static> Glue<B> {
    /// Applies the queued live edits to what the lights hold
    /// (docs/editor/realtime-edits.md, "The native refresh") once the
    /// world's lights are resident and no refresh is running: an edited
    /// `mode` makes the model parts load again from the map file, which
    /// the raw overlay serves edited; each edited image is decoded again
    /// at the stages the lit passes sample it, on a worker, and the
    /// built passes' textures and the gels are rewritten in place.
    fn refresh_edits(&self, mem: &dyn GuestMemory) {
        use render_bridge::native_edits::{self, Consumer, Edit};
        if !native_edits::pending(Consumer::Lights) || self.refreshing.load(Ordering::Acquire) {
            return;
        }
        let current = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            crate::native_world_glue::current_ready_keyed(&state, mem)
        };
        let Some((ready, key)) = current else { return };
        let Some(lights) = ready.lights.get().cloned() else {
            return;
        };
        let Some((device, queue)) = ready.upload_device.clone() else {
            return;
        };
        let edits = native_edits::drain(Consumer::Lights);
        if edits.iter().any(|e| matches!(e, Edit::Model { .. })) {
            self.models.invalidate();
        }
        let images: Vec<(TagId, u32)> = edits
            .iter()
            .filter_map(|e| match *e {
                Edit::Image { bitmap, image } => Some((TagId(bitmap), image)),
                // Sections and other tag notices are not routed here. A
                // shader the lights draw makes the world glue reload it.
                Edit::Section { .. }
                | Edit::Model { .. }
                | Edit::Tag { .. }
                | Edit::Shader { .. } => None,
            })
            .collect();
        if images.is_empty() {
            return;
        }
        self.refreshing.store(true, Ordering::Release);
        let refreshing = Arc::clone(&self.refreshing);
        let launched = background::spawn("native-lights edits", move || {
            match crate::native_map::open(key.map()) {
                Ok((mut map, mut files)) if key.current() => {
                    lights.refresh_images(&mut map, &mut files, &device, &queue, &images);
                }
                Ok(_) => {}
                Err(e) => eprintln!(
                    "native-lights: the map does not open for a live edit ({e}); the lights keep their textures"
                ),
            }
            refreshing.store(false, Ordering::Release);
        });
        if let Err(e) = launched {
            self.refreshing.store(false, Ordering::Release);
            eprintln!(
                "native-lights: could not start the live edit refresh ({e}); the lights keep their textures until the map reloads"
            );
        }
    }

    fn reserve(&self, mem: &mut dyn GuestMemory, dispatch: Dispatch) -> Result<(), Keep> {
        let Dispatch {
            light,
            serial,
            layer,
            shader_gel,
            view_target,
            sorted,
        } = dispatch;
        let constants = constants(&light)?;
        let ready = self.ready(mem)?;
        let mut entries = Vec::new();
        let read = ReadMemory(mem);
        sorted.read(&read, |item| {
            let (entry, rows, indices) = item.scene_entry(&read)?;
            entries.push((LightSlot::new(item.slot, indices, &entry), entry, rows));
            Ok(())
        }).map_err(Keep::Unsupported)?;
        let mut d = self
            .device
            .lock()
            .map_err(|_| Keep::Unsupported("device unavailable"))?;
        if !d
            .begin_external_boundary(&mut DeviceMemory(mem))
            .map_err(|_| Keep::Unsupported("could not drain light state"))?
        {
            return Err(Keep::Unsupported("light has an outstanding push callback"));
        }
        if d.bound_target() != view_target {
            return Err(Keep::Unsupported("light view target changed"));
        }
        let binding = self.gel_binding(mem, serial, shader_gel, &constants)?;
        let draws = self.gathered_draws(
            &d, mem, &ready, layer as usize, &constants, binding.is_some(),
            entries.iter().map(|(_, e, rows)| (e, rows.as_slice())),
        )?;
        let plan = self.prepare_planned(&d, mem, ready, constants, draws, binding, false, None)?;
        let c = &plan.constants;
        // The layer is one the light's steps draw, and every planned draw
        // is the layer's.
        use native_render::light_passes;
        let drawn = light_passes::read(&ReadMemory(&*mem), c.flags, c.definition_flags)
            .map_or(0, |i| light_passes::steps(&i).drawn());
        if drawn & 1 << layer == 0 || plan.draws.iter().any(|p| p.layer as u32 != layer) {
            return Err(Keep::Unsupported(
                "active light layers changed during setup",
            ));
        }
        if mem.u32(engine::light_passes::global::PASS_FLAGS).ok() != Some(c.flags)
            || c.frame.iter().enumerate().any(|(i, word)| {
                mem.u32(engine::light_constants::global::FRAME + 4 * i as u32)
                    .ok()
                    != Some(*word)
            })
        {
            return Err(Keep::Unsupported(
                "light globals differ from the prepared frame or flags",
            ));
        }
        let mut order = self.order.lock().unwrap_or_else(|e| e.into_inner());
        let targets = d.stats().target_changes;
        let isolated = plan.binding.is_some();
        let key = order.key(serial, targets, isolated);
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !current_ready(&state, mem).is_some_and(|current| Arc::ptr_eq(current, &plan.ready)) {
            return Err(Keep::Unsupported("world changed during light preparation"));
        }
        let slots = entries.into_iter().map(|(slot, _, _)| slot);
        let layer = LightLayer::new(view_target, plan, slots).ok_or(
            Keep::Unsupported("light ranges differ from its sorted entries"),
        )?;
        *self.admission.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(layer);
        order.previous = (!isolated).then_some((serial, targets, key));
        Ok(())
    }
}

impl<B: d3d::backend::Backend + Send + 'static> Host for Glue<B> {
    fn retire_map(&self) {
        self.clear_admission();
        self.models.invalidate();
        tiles::retire(&self.run);
    }

    fn hold_scene_light(&self, mem: &mut dyn GuestMemory, draw: &render_bridge::native_lights::SceneDraw<'_>) -> bool {
        self.hold_light(mem, draw)
    }

    fn flush_light_run(&self, mem: &mut dyn GuestMemory) -> usize {
        self.flush_run(mem)
    }

    fn submit(&self, mem: &mut dyn GuestMemory, dispatch: Dispatch) -> bool {
        self.refresh_edits(&*mem);
        let shader_gel = dispatch.shader_gel.is_some();
        let result = self.reserve(mem, dispatch);
        let ok = result.is_ok();
        let mut t = self.tally.lock().unwrap_or_else(|e| e.into_inner());
        if shader_gel && ok {
            t.shader_gels += 1;
        }
        for msg in t.count(result) {
            eprintln!("{msg}");
        }
        if t.asked == 1
            || (ok && t.planned.is_power_of_two())
            || (ok && shader_gel && t.shader_gels == 1)
            || t.asked % SUMMARY_EVERY == 0
        {
            // Admitted lights join the open D3D pass as packets: they add no
            // passes of their own (`native_lights_*_passes` in profile.counters).
            eprintln!(
                "native-lights: {} admitted, {} shader gels, {} worker round trips",
                t.planned,
                t.shader_gels,
                d3d::threaded::LIGHT_WORKER_WAITS.load(Ordering::Relaxed)
            );
        }
        ok
    }

    fn clear_admission(&self) {
        self.admission
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
    }

    fn checkpoint_gel(&self, serial: u64) {
        *self.checkpoint.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((serial, self.gels.checkpoint()));
    }

    fn target(&self) -> Option<[u32; 8]> {
        self.device.lock().ok().map(|d| d.bound_target())
    }

    fn draw_alone(&self, mem: &mut dyn GuestMemory, alone: Alone<'_>) -> bool {
        let result = self.alone(mem, alone);
        let ok = result.is_ok();
        let mut t = self.tally.lock().unwrap_or_else(|e| e.into_inner());
        for msg in t.count(result) {
            eprintln!("{msg}");
        }
        ok
    }

    /// The scene draws every layer of the cut light frame ([`scene`]),
    /// at any render scale and on every target copy, while the native
    /// shadows are on: each copy freezes its own caster maps. With them
    /// off the guest's frame runs.
    fn draws_scene(&self) -> bool {
        crate::native_shadows_glue::enabled()
    }

    /// An entry's state draws while the device holds what its handler's
    /// epilogue leaves ([`held_state`]).
    fn held_state(&self, mem: &dyn GuestMemory) -> frame_scene::EntryState {
        match self.device.lock() {
            Ok(d) => held_state(&*d, mem),
            Err(_) => frame_scene::EntryState::default(),
        }
    }

    fn draw_scene(
        &self,
        mem: &mut dyn GuestMemory,
        draw: render_bridge::native_lights::SceneDraw<'_>,
    ) -> usize {
        // One sequence per light: its layers are consecutive in draw order.
        let mut drawn = 0;
        let mut rest = draw.passes;
        while let Some(first) = rest.first() {
            let n = rest
                .iter()
                .take_while(|p| p.slot == first.slot && p.light.handle == first.light.handle)
                .count();
            let (light, after) = rest.split_at(n);
            rest = after;
            drawn += scene_drawn(self.scene_light_passes(mem, light, draw.view_target)).unwrap_or(0);
        }
        drawn
    }

    fn draw_scene_light(
        &self,
        mem: &mut dyn GuestMemory,
        draw: render_bridge::native_lights::SceneDraw<'_>,
    ) -> bool {
        // A light still loading is not drawn this frame and is not a
        // refusal: it draws once its parts have loaded.
        scene_drawn(self.scene_light_passes(mem, draw.passes, draw.view_target));
        true
    }

    /// Each object shadow of the open view is one sequence
    /// ([`scene::object_ops`]): the mask cleared and drawn on target 10,
    /// the filter, the mask frozen, its receivers on the view's target.
    fn draw_object_shadows(
        &self,
        mem: &mut dyn GuestMemory,
        draw: render_bridge::native_lights::object_shadow_scene::ObjectShadowDraw<'_>,
    ) -> usize {
        let mut drawn = 0;
        for pass in draw.passes {
            drawn += usize::from(scene_drawn(self.scene_object_shadow(mem, pass, draw.view_target)).is_some());
        }
        drawn
    }

    fn draw_host(
        &self,
        mem: &mut dyn GuestMemory,
        draw: render_bridge::native_lights::host::HostDraw<'_>,
    ) -> usize {
        self.host_lights(mem, draw)
    }

    fn draw_chosen(
        &self,
        mem: &mut dyn GuestMemory,
        draw: render_bridge::native_lights::chosen::ChosenDraw<'_>,
    ) -> usize {
        self.chosen_lights(mem, draw)
    }

    fn draw_first_person(
        &self,
        mem: &mut dyn GuestMemory,
        draw: render_bridge::native_lights::chosen::first_person::FirstPersonDraw<'_>,
    ) -> usize {
        self.first_person_lights(mem, draw)
    }

    fn draw_shadow_alone(
        &self,
        mem: &mut dyn GuestMemory,
        alone: ShadowAlone<'_>,
    ) -> Result<(), &'static str> {
        self.shadow_alone(mem, alone).map_err(|why| {
            let category = match &why {
                Keep::Shadow(category) => *category,
                Keep::NoWorld | Keep::NoConstants => "device",
                _ => "program",
            };
            eprintln!("native-shadows: a scene caster layer keeps its D3D handlers: {why}");
            category
        })
    }
}

#[derive(Default)]
struct LightAdmission(Mutex<Option<LightLayer>>);

struct LightLayer {
    targets: [u32; 8],
    light: PreparedLight,
    draws: HashMap<u32, (LightSlot, Planned)>,
}

impl LightLayer {
    fn new(
        targets: [u32; 8],
        mut light: PreparedLight,
        mut slots: impl ExactSizeIterator<Item = LightSlot>,
    ) -> Option<Self> {
        let mut draws = HashMap::with_capacity(slots.len());
        for pass in std::mem::take(&mut light.draws) {
            for range in pass.ranges {
                let slot = slots.next()?;
                draws.insert(
                    slot.geometry.sorted_slot,
                    (
                        slot,
                        Planned {
                            material: pass.material,
                            layer: pass.layer,
                            implementation: pass.implementation,
                            compiled: Arc::clone(&pass.compiled),
                            ranges: vec![range],
                            flipped: pass.flipped,
                            sample: pass.sample.clone(),
                            model: pass.model.clone(),
                            placement: pass.placement,
                        },
                    ),
                );
            }
        }
        if slots.next().is_some() {
            return None;
        }
        Some(Self {
            targets,
            light,
            draws,
        })
    }
}

impl d3d::DrawAdmission for LightAdmission {
    fn admit(
        &self,
        snapshot: &d3d::DrawSnapshot<'_>,
    ) -> Option<(d3d::ExternalPlan, Box<dyn d3d::ExternalPass>)> {
        if render_bridge::native_map_work::Stage::Lights.standing_down() {
            return None;
        }
        let context = crate::native_world_layers_glue::indexed_draw()?;
        let geometry = context.geometry?;
        let draw = snapshot.draw;
        let indices = draw.indices.as_deref()?;
        let index_count = context.index_count?;
        if context.layer.list != render_bridge::native_lights::LIGHT_LIST
            || !matches!(context.layer.number, 12 | 13)
            || draw.entry != "D3DDevice_DrawIndexedVertices"
            || index_count as usize != indices.len()
            || !draw.clip_rects.is_empty()
        {
            return None;
        }
        let mut pending = self.0.lock().ok()?;
        let run = pending.as_mut()?;
        let (entry, planned) = run.draws.get(&geometry.sorted_slot)?;
        if run.targets[..2] != [snapshot.color_header, snapshot.depth_header]
            || planned.layer != context.layer.number as usize
            || !entry.matches(
                geometry, context.light_selection, draw.primitive, context.index_address?, index_count,
            )
            || !run.light.matches(draw, planned)
            || !entry.matches_triangles(indices, planned.ranges.first()?.len())
        {
            return None;
        }
        let (_, planned) = run.draws.remove(&geometry.sorted_slot)?;
        let light = PreparedLight {
            draws: vec![planned],
            ..run.light.clone()
        };
        let mut plan = d3d::ExternalPlan {
            stage: timeline::native::Stage::Lights,
            view: context.layer.view,
            ranges: 1,
            slots: Vec::new(),
        };
        let gel_slot = light.binding.map(|binding| binding.append(&mut plan));
        let pass = RunPass {
            retained: None,
            light,
            view: None,
            clip: None,
            gel_slot,
            fallback: Some(Arc::new(draw.clone())),
            colour_only: false,
            lighting_only: None,
            cone_shadow: None,
        };
        Some((plan, Box::new(pass)))
    }
}

/// The complete part promised for one sorted handler. A subpart draw of
/// the same record and material can have a different index window.
struct LightSlot {
    geometry: Geometry,
    index_address: u32,
    selection: (u8, u16),
    primitive: gfx::vertex::PrimitiveType,
    index_count: u32,
}

impl LightSlot {
    fn new(slot: u32, index_address: u32, entry: &LightEntry) -> Self {
        let owner = match entry.owner {
            LayerOwner::Cluster(n) => ResolvedOwner::Cluster(u32::from(n)),
            LayerOwner::Placement(n) => ResolvedOwner::Placement(u32::from(n)),
            LayerOwner::Object(handle) => ResolvedOwner::Object {
                handle: handle.0,
                model: entry.model,
                section: u32::from(entry.stream.section),
            },
        };
        Self {
            geometry: Geometry {
                owner,
                record: entry.record,
                sorted_slot: slot,
                part: entry.stream.part,
                shader: entry.binding.shader,
                variant: entry.binding.variant,
                implementation: entry.binding.implementation,
            },
            index_address,
            selection: (entry.binding.selector, entry.stream.vertex_class),
            primitive: match entry.stream.topology {
                frame_scene::Topology::List => gfx::vertex::PrimitiveType::TriangleList,
                frame_scene::Topology::Strip => gfx::vertex::PrimitiveType::TriangleStrip,
            },
            index_count: entry.stream.index_count,
        }
    }

    fn matches(
        &self,
        geometry: Geometry,
        selection: Option<(u8, u16)>,
        primitive: gfx::vertex::PrimitiveType,
        indices: u32,
        index_count: u32,
    ) -> bool {
        self.geometry == geometry
            && Some(self.selection) == selection
            && self.primitive == primitive
            && self.index_address == indices
            && self.index_count == index_count
    }

    /// The resident triangle list omits degenerate triangles and an
    /// incomplete final triangle. The raw span was checked separately.
    fn matches_triangles(&self, indices: &[u32], expected: usize) -> bool {
        use gfx::vertex::PrimitiveType::{TriangleList, TriangleStrip};
        let drawn = |t: &&[u32]| t[0] != t[1] && t[0] != t[2] && t[1] != t[2];
        let triangles = match self.primitive {
            TriangleList => indices.chunks_exact(3).filter(drawn).count(),
            TriangleStrip => indices.windows(3).filter(drawn).count(),
            _ => return false,
        };
        triangles * 3 == expected
    }
}

fn uses_gel_target(
    mem: &dyn GuestMemory,
    definition: u32,
    shader: u32,
    flags: u32,
    definition_flags: u32,
) -> bool {
    use native_render::gels;
    shader != u32::MAX
        && definition != 0
        && flags & engine::light_passes::flag::CONTEXT_ZERO == 0
        && definition_flags & engine::light_passes::DEFINITION_TEXTURE_18 == 0
        && mem.u8(gels::BUILDING).ok() == Some(0)
        && mem.u32(gels::DEFINITION).ok() == Some(definition)
        && definition
            .checked_add(engine::light_passes::DEFINITION_GEL_SHADER)
            .and_then(|a| mem.u32(a).ok())
            == Some(shader)
}

/// The light's constants. A shadowed light's layers 12 and 13 are drawn
/// like any other's: no stencil links them to its layers 10, 11 and 14
/// (`light_passes`), and a pass sampling the shadow maps is refused by
/// `light_look` by name.
/// A scene light as the passes draw it, and its shader gel; `None` when
/// the scene holds no constants for it. The constants are the shadowed or
/// the unshadowed choice by the light's flag bit.
fn scene_light(light: &frame_scene::Light) -> Option<(Light, Option<(u32, u32)>)> {
    let c = light.candidate.as_ref()?;
    let variants = c.constants.as_ref()?;
    let shadow = light.shadowed();
    let constants = if shadow { variants.shadowed } else { variants.unshadowed };
    let gel = c.gel_bitmap.zip(c.gel_shader);
    Some((
        Light {
            handle: light.handle,
            definition: light.definition,
            position: c.position,
            forward: c.forward,
            up: c.up,
            colour_a: c.colour_a,
            colour_b: c.colour_b,
            intensity: c.intensity,
            bounds: c.bounds,
            shadow,
            weight: light.guest_weight.unwrap_or(c.weight),
            constants: Some(constants),
        },
        gel,
    ))
}

fn constants(light: &Light) -> Result<LightConstants, Keep> {
    light.constants.ok_or(Keep::NoConstants)
}

type ClusterParts = HashMap<(Owner, u8), (TagId, WorldDraw)>;

#[derive(Default)]
struct RunOrder {
    previous: Option<(u64, u64, u64)>,
}
impl RunOrder {
    fn key(&self, serial: u64, targets: u64, isolated: bool) -> u64 {
        match self.previous {
            Some((last, previous_targets, key))
                if !isolated
                    && targets == previous_targets
                    && last.checked_add(1) == Some(serial) =>
            {
                key
            }
            _ => serial,
        }
    }
}

/// The `matg` tag block of rasterizer data: its count at `+0x108`, the
/// elements' address at `+0x10c`. `0x00022590` reads the globals tag
/// (`[0x004e034c]`), tests the count (`[eax+0x108]`) and keeps the address
/// (`[eax+0x10c]`, the first element) at `[0x00485a80]`, zero when the
/// block is empty.
pub const RASTERIZER_DATA: u32 = 0x108;
/// The element's bitmap datums the light passes sample: engine texture 1
/// (`+0xc`, the normalization cube) and 9 (`+4`, the attenuation volume),
/// the datum words of the tag references at `+8` and `+0` (`0x00019581`
/// reads `[[0x00485a80]+0xc]`, `0x00019673` `[[0x00485a80]+4]`).
pub const NORMALIZATION_DATUM: u32 = 0xc;
pub const ATTENUATION_DATUM: u32 = 4;
/// The element's bitmap datum engine textures 14 and 15 bind (`+0x14`,
/// the datum word of the tag reference at `+0x10`: `0x000197e6` and
/// `0x00019801` read `[[0x00485a80]+0x14]`); 15 is its image 1.
pub const RASTERIZER_DATUM_14: u32 = 0x14;

/// The light textures' bitmaps named by the `matg` at `matg` (its payload
/// address), read through `read(address, len)`: `(normalization,
/// attenuation)`, each `None` when null or unreadable.
pub fn light_bitmaps(
    read: &dyn Fn(u32, usize) -> Option<Vec<u8>>,
    matg: u32,
) -> (Option<map::TagId>, Option<map::TagId>) {
    (
        rasterizer_bitmap(read, matg, NORMALIZATION_DATUM),
        rasterizer_bitmap(read, matg, ATTENUATION_DATUM),
    )
}

/// The bitmap datum at `offset` of the `matg`'s first rasterizer data
/// element (`[0x00485a80] + offset`), `None` when null or unreadable.
pub fn rasterizer_bitmap(
    read: &dyn Fn(u32, usize) -> Option<Vec<u8>>,
    matg: u32,
    offset: u32,
) -> Option<map::TagId> {
    let word = |a: u32| -> Option<u32> { Some(u32::from_le_bytes(read(a, 4)?.try_into().ok()?)) };
    let count = word(matg.wrapping_add(RASTERIZER_DATA))?;
    (count > 0).then_some(())?;
    let e = word(matg.wrapping_add(RASTERIZER_DATA + 4))?;
    word(e.wrapping_add(offset))
        .filter(|&d| d != u32::MAX)
        .map(map::TagId)
}

/// [`light_bitmaps`] over the map's ordinary metadata (`matg` the tag's
/// virtual address, `Tag::location`, which the element's address shares).
pub fn matg_light_bitmaps(
    meta: &tags::shader::Metadata,
    matg: u32,
) -> (Option<map::TagId>, Option<map::TagId>) {
    light_bitmaps(
        &|addr, len| meta.bytes(addr, len).ok().map(<[u8]>::to_vec),
        matg,
    )
}

/// [`rasterizer_bitmap`] over the map's ordinary metadata at `offset`.
pub fn matg_rasterizer_bitmap(
    meta: &tags::shader::Metadata,
    matg: u32,
    offset: u32,
) -> Option<map::TagId> {
    rasterizer_bitmap(
        &|addr, len| meta.bytes(addr, len).ok().map(<[u8]>::to_vec),
        matg,
        offset,
    )
}

/// A decoded bitmap's first image as a cube (six faces of level 0) or a
/// volume (level 0's slices), with the mip chain below it; `None` when it
/// is not that shape. `assets::decode_image` gives a cube's levels
/// face-major, each face's levels one slice deep, and a volume's as face 0
/// only, each level with all its slices. The texels are the decode's, as
/// the D3D path uploads them for a stage without `D3DTSS_COLORSIGN` (the
/// light passes' engine stages; one with signed channels is refused,
/// `native_render::program`).
pub fn layered(levels: Vec<assets::bitmap::Level>, dim: TextureDim) -> Option<LayeredImage> {
    let deepest = levels.iter().map(|l| l.level).max()?;
    let (top, rest): (Vec<_>, Vec<_>) = levels.into_iter().partition(|l| l.level == 0);
    let mut below: Vec<Vec<assets::bitmap::Level>> = (0..deepest).map(|_| Vec::new()).collect();
    for l in rest {
        below[l.level as usize - 1].push(l);
    }
    let image = match dim {
        TextureDim::Cube => {
            if top.iter().any(|l| l.depth != 1) {
                return None;
            }
            LayeredImage::cube(
                top.into_iter()
                    .map(|l| (l.face, l.width, l.height, l.rgba))
                    .collect(),
            )?
        }
        TextureDim::D3 => {
            // One level 0 of face 0: a cube's six faces are not a volume.
            let [l] = <[_; 1]>::try_from(top).ok()?;
            (l.face == 0).then_some(())?;
            LayeredImage::volume(l.width, l.height, l.depth, l.rgba)?
        }
        TextureDim::D2 => return None,
    };
    // Each level's layers in order (a cube's faces 0..6, a volume's one
    // face); a level that is not whole ends the chain (`with_mips`).
    let mips = below
        .into_iter()
        .map(|mut faces| {
            faces.sort_by_key(|l| l.face);
            let whole = faces.iter().map(|l| l.face).eq(0..faces.len() as u32);
            if whole {
                faces.into_iter().flat_map(|l| l.rgba).collect()
            } else {
                Vec::new()
            }
        })
        .collect();
    Some(image.with_mips(mips))
}

/// What a decoded light texture is, for the load's log line.
fn shape(levels: &[assets::bitmap::Level]) -> String {
    let faces = levels.iter().map(|l| l.face).max().map_or(0, |f| f + 1);
    let top: Vec<_> = levels
        .iter()
        .filter(|l| l.level == 0)
        .map(|l| (l.width, l.height, l.depth))
        .collect();
    format!("{faces} faces, level 0 {top:?}")
}

struct DecodedLightTexture {
    desc: gfx::texture::TextureDesc,
    bytes: Vec<u8>,
    texture: Arc<StaticLightTexture>,
}

fn cached_light_texture(
    slot: &mut Option<DecodedLightTexture>,
    desc: gfx::texture::TextureDesc,
    bytes: Vec<u8>,
    decode: impl FnOnce(&gfx::texture::TextureDesc, &[u8]) -> Result<LayeredImage, String>,
) -> Result<Arc<StaticLightTexture>, String> {
    if let Some(cached) = slot.as_ref().filter(|c| c.desc == desc && c.bytes == bytes) {
        return Ok(Arc::clone(&cached.texture));
    }
    let texture = Arc::new(StaticLightTexture::new(decode(&desc, &bytes)?));
    *slot = Some(DecodedLightTexture {
        desc,
        bytes,
        texture: Arc::clone(&texture),
    });
    Ok(texture)
}

/// [`light_bitmaps`] of the map's `matg`, decoded: `(normalization cube,
/// attenuation volume)`. Says on stderr what loaded and, for what does not,
/// why (the light passes that sample it stay on the D3D path).
pub fn light_textures<R: std::io::Read + std::io::Seek>(
    map: &mut map::Map<R>,
    meta: &tags::shader::Metadata,
    files: &mut assets::ResourceFiles,
) -> (
    Option<Arc<StaticLightTexture>>,
    Option<Arc<StaticLightTexture>>,
) {
    // One entry per engine texture. Match the descriptor and exact bytes so
    // different maps or mods can share unchanged images without aliasing.
    static TEXTURES: Mutex<[Option<DecodedLightTexture>; 2]> = Mutex::new([None, None]);
    let matg = map
        .tags()
        .iter()
        .find(|t| t.group.0 == *b"matg")
        .and_then(|t| t.location.as_ref().map(|l| l.address));
    let Some(matg) = matg else {
        eprintln!("native-lights: no matg; the light textures do not load");
        return (None, None);
    };
    let (n, a) = matg_light_bitmaps(meta, matg);
    let mut decode = |slot: usize, id: Option<map::TagId>, dim, what| {
        let image = (|| -> Result<Arc<StaticLightTexture>, String> {
            let id = id.ok_or_else(|| {
                format!("no datum in matg {matg:#x} +{RASTERIZER_DATA:#x}'s element")
            })?;
            let tag = tags::DecodedTag::read(map, id)
                .map_err(|e| format!("{id:?} does not read: {e}"))?;
            let tags::TagData::Bitmap(b) = tag.data else {
                return Err(format!("{id:?} is not a bitmap"));
            };
            let image = b
                .images
                .first()
                .ok_or_else(|| format!("{id:?} has no image"))?;
            let (desc, bytes) = assets::bitmap::image_bytes(image, files)
                .map_err(|e| format!("{id:?} does not read: {e}"))?;
            let mut cache = TEXTURES.lock().unwrap_or_else(|e| e.into_inner());
            cached_light_texture(&mut cache[slot], desc, bytes, |desc, bytes| {
                let levels = assets::bitmap::decode_bytes(desc, bytes)
                    .map_err(|e| format!("{id:?} does not decode: {e}"))?;
                let shape = shape(&levels);
                let image = layered(levels, dim)
                    .ok_or_else(|| format!("{id:?} is not a {dim:?} ({shape})"))?;
                if hooks::native_verbose() {
                    eprintln!(
                        "native-lights: the {what} loads ({}x{}, {} layers, {} levels)",
                        image.width,
                        image.height,
                        image.layers,
                        1 + image.mips.len()
                    );
                }
                Ok(image)
            })
        })();
        match image {
            Ok(i) => Some(i),
            Err(why) => {
                eprintln!("native-lights: the {what} does not load: {why}");
                None
            }
        }
    };
    (
        decode(0, n, TextureDim::Cube, "normalization cube"),
        decode(1, a, TextureDim::D3, "attenuation volume"),
    )
}

/// Installs the host over the world's state (`native_world_glue::install`).
pub fn install<B: d3d::backend::Backend + Send + 'static>(
    device: &d3d::abi::SharedDevice<B>,
    state: State,
    gels: Arc<Outputs>,
) {
    let switches = hooks::Switches::from_env().unwrap_or_default();
    if !render_bridge::native_lights::on(&switches) {
        return;
    }
    let admission = Arc::new(LightAdmission::default());
    let checkpoint = Arc::new(Mutex::default());
    if let Ok(mut device) = device.lock() {
        device.add_draw_admission(admission.clone());
        device.add_draw_admission(Arc::new(indexed::Admission::new(
            Arc::clone(&gels),
            Arc::clone(&checkpoint),
        )));
    }
    render_bridge::native_lights::set_host(Arc::new(Glue {
        device: Arc::clone(device),
        state,
        gels,
        checkpoint,
        tally: Mutex::new(Tally::default()),
        order: Mutex::default(),
        admission,
        models: models::Loader::default(),
        refreshing: Arc::default(),
        run: Mutex::default(),
    }));
}

/// A world material as the light passes resolve it.
#[derive(Clone, Debug)]
pub struct LitMaterial {
    pub shader: TagId,
    /// `EngineInputs::for_shader`: its integer input 4 picks layer 12's
    /// implementation 0 and layer 13's.
    pub instance: EngineInputs,
}

/// A translated light pass and the resident shader module and stage textures.
pub struct Compiled {
    pub program: Arc<LightProgram>,
    gpu: Arc<Mutex<LightGpu>>,
}

/// One material's pass for one light: the program and the ranges.
#[derive(Clone)]
pub struct Planned {
    /// The world material; [`MODEL_MATERIAL`] for a model's part.
    material: u16,
    layer: usize,
    implementation: usize,
    pub compiled: Arc<Compiled>,
    /// Ranges of the world's indices, or of the model section's.
    pub ranges: Vec<Range<u32>>,
    /// The ranges' triangles wind the other way round than the buffer
    /// holds them (a visible subpart at an odd strip start): drawn with
    /// the front face reversed ([`Planned::cull`]).
    flipped: bool,
    /// The program's animation sampled at the light's draw clock
    /// ([`LightProgram::animation`]), set where the light is prepared.
    sample: Option<native_render::fog_gpu::LayerConstants>,
    /// Rows uploaded by the structure handler for a rigid placement.
    placement: Option<ModelRows>,
    /// A model's part: what it draws over.
    model: Option<ModelDraw>,
}

impl Planned {
    /// The cull this draw takes over the light's inherited one.
    fn cull(&self, inherited: &InheritedCull) -> Option<native_render::program::Cull> {
        let program = &self.compiled.program;
        let cull = inherited.of(program.inherits_cull, true);
        if self.flipped {
            native_render::program::reversed_winding(Some(&program.state), cull)
        } else {
            cull
        }
    }
}

/// `planned` over `wound`'s ranges: the forward ones, then the reversed
/// ones as a draw of their own with the front face reversed.
fn split_wound(wound: Vec<(Range<u32>, bool)>, planned: Planned) -> Vec<Planned> {
    let (reversed, forward): (Vec<_>, Vec<_>) = wound.into_iter().partition(|&(_, r)| r);
    let backward = Planned {
        ranges: reversed.into_iter().map(|(r, _)| r).collect(),
        flipped: true,
        ..planned.clone()
    };
    let forward = Planned { ranges: forward.into_iter().map(|(r, _)| r).collect(), ..planned };
    [forward, backward].into_iter().filter(|p| !p.ranges.is_empty()).collect()
}

/// [`Planned::material`] of a model's part.
const MODEL_MATERIAL: u16 = u16::MAX;

/// A model part's draw: the section's buffer its pass was compiled for,
/// and the entry's node rows.
#[derive(Clone)]
struct ModelDraw {
    lights: Arc<models::ModelLights>,
    model: u32,
    section: u16,
    buffer: usize,
    rows: Vec<Row>,
    /// What the auxiliary state toggle sets, for an entry toggling it
    /// (`frame_scene::EntryState::aux_depth`).
    aux_depth: Option<frame_scene::AuxDepth>,
}

impl std::fmt::Debug for ModelDraw {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelDraw")
            .field("model", &self.model)
            .field("section", &self.section)
            .field("buffer", &self.buffer)
            .field("rows", &self.rows.len())
            .finish()
    }
}

impl ModelDraw {
    fn section(&self) -> Option<&native_render::light_models::LightSection> {
        self.lights.models.models.get(&self.model)?.sections.get(&self.section)
    }

    fn buffer(&self) -> Result<&native_render::light_models::LightBuffer, Keep> {
        self.section()
            .and_then(|section| section.buffers.get(self.buffer))
            .ok_or(Keep::Unsupported("model light buffer unavailable"))
    }

    /// Its buffers on `device`, uploaded the first time.
    fn geometry(&self, device: &wgpu::Device) -> Option<ModelGeometry<'_>> {
        let section = self.section()?;
        let buffer = section.buffers.get(self.buffer)?;
        Some(ModelGeometry {
            vertices: buffer.vertices.get(device),
            indices: section.indices.get(device),
            layout: &buffer.layout,
            nodes: &self.rows,
            decompression: section.decompression,
            aux_depth: self.aux_depth.as_ref(),
        })
    }
}

/// The lights' part of a loaded world ([`load`]).
pub struct LightsWorld {
    resolver: materials::Resolver,
    materials: HashMap<u16, LitMaterial>,
    clusters: ClusterParts,
    placements: HashMap<u32, ModelRows>,
    structure_ranges: Arc<native_render::world::light_parts::Ranges>,
    /// Stage images by (stage, bitmap, image) since palettes are per stage.
    images: Arc<HashMap<(usize, TagId, u32), Image>>,
    palettized: HashSet<(usize, TagId, u32)>,
    palettes: StagePalettes,
    /// Gel bitmaps' image 0 by datum.
    gels: HashMap<u32, Image>,
    /// The gel shaders a scene light draws into target 13
    /// ([`crate::native_gels_scene`]).
    scene_gels: Arc<crate::native_gels_scene::Gels>,
    normalization: Option<Arc<StaticLightTexture>>,
    attenuation: Option<Arc<StaticLightTexture>>,
}

impl LightsWorld {
    /// A gathered structure entry's draws: its part's triangles in the
    /// record's topology, or each of its visible subparts' native ranges
    /// (docs/graphics/native-lights.md, "Visible subparts"), each with
    /// whether it winds the other way round than the buffer holds it
    /// (a strip span at an odd start).
    fn structure_entry(&self, e: &LightEntry) -> Result<Vec<(WorldDraw, bool)>, Keep> {
        let key = structure_key(e).ok_or(Keep::Unsupported("expected a gathered structure part"))?;
        let (shader, draw) = self.clusters.get(&key)
            .ok_or(Keep::Unsupported("structure part has no resident lit material or geometry"))?;
        if shader.0 != e.binding.shader {
            return Err(Keep::Unsupported("gathered shader datum differs from the structure material"));
        }
        if e.stream.vertex_class != residency::classification(draw.owner) {
            return Err(Keep::Unsupported("gathered structure vertex class differs from the loaded geometry"));
        }
        let ranges = self.structure_ranges.get(&key)
            .ok_or(Keep::Unsupported("structure source index range unavailable"))?;
        match &e.subparts {
            None => ranges.draw(&e.stream, *draw).map(|d| vec![(d, false)]),
            Some(spans) => ranges.subpart_draws_wound(&e.stream, spans, *draw),
        }
        .map_err(Keep::Unsupported)
    }

    fn placement(&self, owner: Owner) -> Result<Option<ModelRows>, Keep> {
        match owner {
            Owner::Cluster(_) => Ok(None),
            Owner::Instance(id) => self.placements.get(&id).copied().map(Some)
                .ok_or(Keep::Unsupported("light placement rows unavailable")),
        }
    }

    /// Whether the lights draw world material `material`'s passes: a
    /// live shader edit of it reloads the world (docs/editor/realtime-edits.md,
    /// "The native refresh").
    pub fn draws_material(&self, material: u16) -> bool {
        self.materials.contains_key(&material)
    }

    /// Whether the world's light set holds a gel made from `datum`'s
    /// bitmap.
    pub fn has_gel(&self, datum: u32) -> bool {
        self.gels.contains_key(&datum)
    }

    /// Decodes again each of `edited` (bitmap, image) at the stages the
    /// lit passes sample it, as the load decoded it, and rewrites the
    /// built passes' textures with it ([`materials::Resolver`]); the
    /// passes built later take it too. A stage whose image no longer
    /// decodes keeps its pixels. The gels are rewritten by the world's
    /// refresh, which holds the light set.
    pub fn refresh_images<R: std::io::Read + std::io::Seek>(
        &self,
        map: &mut Map<R>,
        files: &mut assets::ResourceFiles,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        edited: &[(TagId, u32)],
    ) {
        let mut decoded = HashMap::new();
        for &(stage, bitmap, image) in self
            .images
            .keys()
            .filter(|(_, b, i)| edited.contains(&(*b, *i)))
        {
            match light_image(
                map,
                files,
                bitmap,
                image as usize,
                stage,
                &self.palettes[stage],
            ) {
                Some((pixels, _)) => {
                    decoded.insert((stage, bitmap, image), pixels);
                }
                None => eprintln!(
                    "native-lights: bitmap {:#010x} image {image} no longer decodes at stage {stage}; the lights keep its pixels",
                    bitmap.0
                ),
            }
        }
        if !decoded.is_empty() {
            self.resolver.refresh_images(device, queue, &decoded);
        }
    }

    /// Upload map textures and start a resolver tied to this device and BSP.
    pub fn upload_engine_textures(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> LightSet {
        let mut set = LightSet::new(
            device,
            queue,
            self.normalization.as_deref(),
            self.attenuation.as_deref(),
        );
        for (&datum, image) in &self.gels {
            set.upload_gel(device, queue, datum, image);
        }
        self.resolver.start(
            device,
            queue,
            set.uniforms_layout(),
            Arc::clone(&self.images),
        );
        set
    }

    fn resolve_parts(
        &self,
        c: &native_render::lights::LightConstants,
        parts: impl IntoIterator<Item = (usize, usize, WorldDraw)>,
        produced_gel: bool,
    ) -> Result<Vec<Planned>, Keep> {
        let parts = parts.into_iter().map(|(layer, imp, draw)| (layer, imp, draw, None, false));
        self.resolve_world_parts(c, parts, produced_gel)
    }

    fn resolve_world_parts(
        &self,
        c: &LightConstants,
        parts: impl IntoIterator<Item = (usize, usize, WorldDraw, Option<(u8, u8)>, bool)>,
        produced_gel: bool,
    ) -> Result<Vec<Planned>, Keep> {
        // Resolved without the colours, which each light's uniforms take
        // (`colour_uniforms`): a light whose colour animates keeps its key.
        let inputs = LightInputs::from_constants(c).without_colours();
        let mut out: Vec<Planned> = Vec::new();
        let mut pending = false;
        for (layer, imp, draw, selection, flipped) in parts {
            let m = draw.material;
            let end = draw
                .first_index
                .checked_add(draw.index_count)
                .ok_or(Keep::Unsupported("invalid cluster index range"))?;
            let range = draw.first_index..end;
            let lit = self.materials.get(&m).ok_or(Keep::NoMaterial)?;
            let class = u32::from(residency::classification(draw.owner));
            let placement = self.placement(draw.owner)?;
            let resolved = match self.resolver.world(lit, inputs, layer, imp, class, selection) {
                Err(Keep::MaterialPending) => {
                    // Request the remaining variants for this layer on the same frame.
                    pending = true;
                    continue;
                }
                other => other?,
            };
            let compiled = resolved.compiled;
            if compiled.program.inputs & !draw.inputs != 0 {
                return Err(Keep::Unsupported(
                    "cluster lacks a vertex stream read by its light program",
                ));
            }
            self.binds(&compiled.program, c.gel, produced_gel, &self.images)?;
            if let Some(previous) = out.last_mut()
                && placement.is_none()
                && previous.placement.is_none()
                && previous.material == m
                && previous.layer == layer
                && previous.flipped == flipped
                && Arc::ptr_eq(&previous.compiled, &compiled)
            {
                previous.ranges.push(range);
                continue;
            }
            out.push(Planned {
                compiled,
                material: m,
                layer,
                implementation: imp,
                ranges: vec![range],
                flipped,
                sample: None,
                placement,
                model: None,
            });
        }
        if pending {
            Err(Keep::MaterialPending)
        } else {
            Ok(out)
        }
    }

    /// A model part's draws for one light ([`models`]): `entry`'s part,
    /// its material resolved with the light's inputs, compiled for the
    /// section's buffer of the program the pass names at the entry's
    /// classification, with the entry's node rows. The visible subparts
    /// at an odd strip start are a draw of their own, with the front
    /// face reversed.
    fn resolve_model(
        &self,
        c: &LightConstants,
        lights: &Arc<models::ModelLights>,
        layer: usize,
        entry: &LightEntry,
        rows: &[Row],
        selection: Option<(u8, u8)>,
        produced_gel: bool,
    ) -> Result<Vec<Planned>, Keep> {
        let (section, wound) = lights.models.part_wound(entry).ok_or(Keep::Unsupported(
            "scene group entry has no native light stream",
        ))?;
        let (reversed, indices): (Vec<_>, Vec<_>) = wound.into_iter().partition(|&(_, reversed)| reversed);
        let indices: Vec<Range<u32>> = indices.into_iter().map(|(r, _)| r).collect();
        let reversed: Vec<Range<u32>> = reversed.into_iter().map(|(r, _)| r).collect();
        let (shader, instance) = lights
            .instances
            .get(&entry.binding.shader)
            .ok_or(Keep::NoMaterial)?;
        let inputs = LightInputs::from_constants(c).without_colours();
        let imp = usize::from(entry.binding.implementation);
        let resolved = self
            .resolver
            .model(*shader, instance, inputs, layer, imp, lights, entry, selection)?;
        let buffer = resolved
            .buffer
            .ok_or(Keep::Unsupported("model light buffer unavailable"))?;
        let b = section
            .buffers
            .get(buffer)
            .ok_or(Keep::Unsupported("model light buffer unavailable"))?;
        let compiled = resolved.compiled;
        if compiled.program.inputs & !b.provided() != 0 {
            return Err(Keep::Unsupported(
                "model section lacks a vertex stream read by its light program",
            ));
        }
        if rows.is_empty() || rows.len() > 3 * native_render::object_program::MAX_PALETTE {
            return Err(Keep::Unsupported(
                "object entry's node rows do not fit the palette",
            ));
        }
        self.binds(&compiled.program, c.gel, produced_gel, &lights.images)?;
        let forward = Planned {
            material: MODEL_MATERIAL,
            layer,
            implementation: imp,
            compiled,
            ranges: indices,
            flipped: false,
            sample: None,
            placement: None,
            model: Some(ModelDraw {
                lights: Arc::clone(lights),
                model: entry.model,
                section: entry.stream.section,
                buffer,
                rows: rows.to_vec(),
                aux_depth: None,
            }),
        };
        if reversed.is_empty() {
            return Ok(vec![forward]);
        }
        let backward = Planned { ranges: reversed, flipped: true, ..forward.clone() };
        Ok(if forward.ranges.is_empty() { vec![backward] } else { vec![forward, backward] })
    }

    /// Structure caster parts resolved with their instance inputs alone.
    fn resolve_shadow_parts(
        &self,
        parts: Vec<(usize, WorldDraw, Option<(u8, u8)>, bool)>,
    ) -> Result<Vec<Planned>, Keep> {
        let mut out: Vec<Planned> = Vec::new();
        let mut pending = false;
        for (imp, draw, selection, flipped) in parts {
            let m = draw.material;
            let end = draw
                .first_index
                .checked_add(draw.index_count)
                .ok_or(Keep::Unsupported("invalid shadow index range"))?;
            let range = draw.first_index..end;
            let lit = self.materials.get(&m).ok_or(Keep::NoMaterial)?;
            let placement = self.placement(draw.owner)?;
            let class = u32::from(residency::classification(draw.owner));
            let resolved = match self.resolver.world_shadow(lit, imp, class, selection) {
                Err(Keep::MaterialPending) => {
                    pending = true;
                    continue;
                }
                other => other?,
            };
            let compiled = resolved.compiled;
            if compiled.program.inputs & !draw.inputs != 0 {
                return Err(Keep::Unsupported(
                    "cluster lacks a vertex stream read by its shadow program",
                ));
            }
            self.binds(&compiled.program, None, false, &self.images)?;
            if let Some(previous) = out.last_mut()
                && previous.placement.is_none() && placement.is_none()
                && previous.material == m
                && previous.flipped == flipped
                && Arc::ptr_eq(&previous.compiled, &compiled)
            {
                previous.ranges.push(range);
                continue;
            }
            out.push(Planned {
                compiled,
                material: m,
                layer: CASTER_LAYER,
                implementation: imp,
                ranges: vec![range],
                flipped,
                sample: None,
                placement,
                model: None,
            });
        }
        if pending {
            Err(Keep::MaterialPending)
        } else {
            Ok(out)
        }
    }

    /// A model caster part over its resident section's vertex buffer: one
    /// draw, and one more with the front face reversed for visible
    /// subparts at an odd strip start ([`split_wound`]).
    fn resolve_shadow_model(
        &self,
        lights: &Arc<models::ModelLights>,
        entry: &LightEntry,
        rows: &[Row],
    ) -> Result<Vec<Planned>, Keep> {
        let (section, wound) = lights
            .models
            .part_wound(entry)
            .ok_or(Keep::Unsupported("scene shadow entry has no native stream"))?;
        let (shader, instance) = lights
            .instances
            .get(&entry.binding.shader)
            .ok_or(Keep::NoMaterial)?;
        let resolved = self
            .resolver
            .model_shadow(*shader, instance, lights, entry)?;
        let buffer = resolved
            .buffer
            .ok_or(Keep::Unsupported("model shadow buffer unavailable"))?;
        let b = section
            .buffers
            .get(buffer)
            .ok_or(Keep::Unsupported("model shadow buffer unavailable"))?;
        let compiled = resolved.compiled;
        if compiled.program.inputs & !b.provided() != 0 {
            return Err(Keep::Unsupported(
                "model section lacks a vertex stream read by its shadow program",
            ));
        }
        if rows.is_empty() || rows.len() > 3 * native_render::object_program::MAX_PALETTE {
            return Err(Keep::Unsupported(
                "shadow entry's node rows do not fit the palette",
            ));
        }
        self.binds(&compiled.program, None, false, &lights.images)?;
        Ok(split_wound(wound, Planned {
            material: MODEL_MATERIAL,
            layer: CASTER_LAYER,
            implementation: usize::from(entry.binding.implementation),
            compiled,
            ranges: Vec::new(),
            flipped: false,
            sample: None,
            placement: None,
            model: Some(ModelDraw {
                lights: Arc::clone(lights),
                model: entry.model,
                section: entry.stream.section,
                buffer,
                rows: rows.to_vec(),
                aux_depth: None,
            }),
        }))
    }

    /// Structure receiver parts resolved with their instance inputs alone.
    fn resolve_receiver_parts(
        &self,
        layer: usize,
        parts: Vec<(usize, WorldDraw, Option<(u8, u8)>, bool)>,
        produced_gel: bool,
    ) -> Result<Vec<Planned>, Keep> {
        let mut out: Vec<Planned> = Vec::new();
        let mut pending = false;
        for (imp, draw, selection, flipped) in parts {
            let m = draw.material;
            let end = draw
                .first_index
                .checked_add(draw.index_count)
                .ok_or(Keep::Unsupported("invalid receiver index range"))?;
            let range = draw.first_index..end;
            let lit = self.materials.get(&m).ok_or(Keep::NoMaterial)?;
            let placement = self.placement(draw.owner)?;
            let class = u32::from(residency::classification(draw.owner));
            let resolved = match self.resolver.world_receiver(lit, layer, imp, class, selection) {
                Err(Keep::MaterialPending) => {
                    pending = true;
                    continue;
                }
                other => other?,
            };
            let compiled = resolved.compiled;
            if compiled.program.inputs & !draw.inputs != 0 {
                return Err(Keep::Unsupported(
                    "cluster lacks a vertex stream read by its receiver program",
                ));
            }
            self.binds(&compiled.program, None, produced_gel, &self.images)?;
            if let Some(previous) = out.last_mut()
                && previous.placement.is_none() && placement.is_none()
                && previous.material == m
                && previous.flipped == flipped
                && Arc::ptr_eq(&previous.compiled, &compiled)
            {
                previous.ranges.push(range);
                continue;
            }
            out.push(Planned {
                compiled,
                material: m,
                layer,
                implementation: imp,
                ranges: vec![range],
                flipped,
                sample: None,
                placement,
                model: None,
            });
        }
        if pending {
            Err(Keep::MaterialPending)
        } else {
            Ok(out)
        }
    }

    /// A model receiver part over its resident section's vertex buffer,
    /// split by winding as [`Self::resolve_shadow_model`].
    fn resolve_receiver_model(
        &self,
        lights: &Arc<models::ModelLights>,
        entry: &LightEntry,
        rows: &[Row],
        layer: usize,
        produced_gel: bool,
    ) -> Result<Vec<Planned>, Keep> {
        let (section, wound) = lights.models.part_wound(entry).ok_or(Keep::Unsupported(
            "scene receiver entry has no native stream",
        ))?;
        let (shader, instance) = lights
            .instances
            .get(&entry.binding.shader)
            .ok_or(Keep::NoMaterial)?;
        let resolved = self
            .resolver
            .model_receiver(*shader, instance, lights, entry, layer)?;
        let buffer = resolved
            .buffer
            .ok_or(Keep::Unsupported("model receiver buffer unavailable"))?;
        let b = section
            .buffers
            .get(buffer)
            .ok_or(Keep::Unsupported("model receiver buffer unavailable"))?;
        let compiled = resolved.compiled;
        if compiled.program.inputs & !b.provided() != 0 {
            return Err(Keep::Unsupported(
                "model section lacks a vertex stream read by its receiver program",
            ));
        }
        if rows.is_empty() || rows.len() > 3 * native_render::object_program::MAX_PALETTE {
            return Err(Keep::Unsupported(
                "receiver entry's node rows do not fit the palette",
            ));
        }
        self.binds(&compiled.program, None, produced_gel, &lights.images)?;
        Ok(split_wound(wound, Planned {
            material: MODEL_MATERIAL,
            layer,
            implementation: usize::from(entry.binding.implementation),
            compiled,
            ranges: Vec::new(),
            flipped: false,
            sample: None,
            placement: None,
            model: Some(ModelDraw {
                lights: Arc::clone(lights),
                model: entry.model,
                section: entry.stream.section,
                buffer,
                rows: rows.to_vec(),
                aux_depth: None,
            }),
        }))
    }

    /// Whether every texture `program` samples is loaded (its stage
    /// bitmaps in `images`), with `gel` the light's; the first that is
    /// not, when one is not.
    fn binds(
        &self,
        program: &LightProgram,
        gel: Option<u32>,
        produced_gel: bool,
        images: &HashMap<(usize, TagId, u32), Image>,
    ) -> Result<(), Keep> {
        for (stage, (s, d)) in program.stages.iter().zip(&program.dims).enumerate() {
            if d.is_none() {
                continue;
            }
            let (loaded, what) = match *s {
                LightStage::Empty => (false, "empty stage"),
                LightStage::Bitmap { bitmap, image } => {
                    (images.contains_key(&(stage, bitmap, image)), "stage bitmap")
                }
                LightStage::Normalization => (self.normalization.is_some(), "normalization cube"),
                LightStage::Attenuation => (self.attenuation.is_some(), "attenuation volume"),
                LightStage::Gel => (
                    produced_gel || gel.is_some_and(|g| self.gels.contains_key(&g)),
                    "gel",
                ),
                LightStage::DepthShadow => (true, "depth shadow"),
                LightStage::ColourMask => (true, "colour mask"),
            };
            if !loaded {
                return Err(Keep::Unloaded(what));
            }
        }
        Ok(())
    }
}

/// Diffuse and specular dispatch select their pass from the sorted entry.
/// The shadow pass resolvers use the first level of detail.
fn light_layer(lod: usize, layer: usize) -> bool {
    matches!(layer, DIFFUSE_LAYER | SPECULAR_LAYER)
        || lod == 0 && matches!(layer, CASTER_LAYER | RECEIVER_LAYER | OPACITY_LAYER)
}

fn lit_layers(material: &Material) -> bool {
    material.layers.iter().any(|l| light_layer(l.lod, l.layer) && !l.passes.is_empty())
}

/// Load light programs and their geometry from the complete BSP.
/// `shaders` gives the full shader datum of each material. `None` when the shader tags do not
/// read. Says on stderr what loaded.
pub fn load<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    files: &mut assets::ResourceFiles,
    shaders: &[TagId],
    world: &World,
    palettes: &StagePalettes,
) -> Option<LightsWorld> {
    let mut lib = match ShaderLibrary::read(map) {
        Ok(l) => l,
        Err(e) => {
            eprintln!(
                "native-lights: the shader tags do not read ({e}); every light keeps its D3D passes"
            );
            return None;
        }
    };
    let mut materials = HashMap::new();
    let mut microcode = HashMap::new();
    let mut images = HashMap::new();
    let mut palettized = HashSet::new();
    // Each material once, with default inputs: what any light's pass may
    // need that does not depend on the light (programs, bitmaps).
    let mut lit_shaders = HashMap::new();
    let mut ids: Vec<u16> = world.materials();
    ids.extend(world.transparent_materials());
    ids.sort_unstable();
    ids.dedup();
    for m in ids {
        let Some(&shader) = shaders.get(usize::from(m)) else {
            continue;
        };
        let Ok(instance) = lib.shader(shader).map(|s| EngineInputs::for_shader(&s)) else {
            continue;
        };
        let Ok(material) = lib.material(shader, None) else {
            continue;
        };
        let lit = lit_layers(&material);
        materials.insert(m, LitMaterial { shader, instance });
        if !lit {
            continue;
        }
        lit_shaders.insert(m, shader);
        for imp in material
            .layers
            .iter()
            .filter(|l| light_layer(l.lod, l.layer))
            .flat_map(|l| &l.passes)
            .flat_map(|p| &p.implementations)
        {
            if let Some(vs) = imp.vertex_shader
                && let Some(address) = map.tag(vs).ok().and_then(|t| t.location).map(|l| l.address)
            {
                for class in [look::WORLD_CLASSIFICATION, look::RIGID_CLASSIFICATION] {
                    if !microcode.contains_key(&(vs, class))
                        && let Some(code) = look::vertex_program(lib.metadata(), address, class)
                    {
                        microcode.insert((vs, class), code);
                    }
                }
            }
            for (stage, t) in imp.textures.iter().enumerate().take(4) {
                let Some(TextureSource::Bitmap { bitmap, image, .. }) = t.map(|t| t.source) else {
                    continue;
                };
                let key = (stage, bitmap, image);
                if !images.contains_key(&key)
                    && let Some((pixels, expanded)) =
                        light_image(map, files, bitmap, image as usize, stage, &palettes[stage])
                {
                    images.insert(key, pixels);
                    if expanded {
                        palettized.insert(key);
                    }
                }
            }
        }
    }
    let (clusters, unread) = residency::structure_parts(world, &lit_shaders, &materials);
    let (normalization, attenuation) = light_textures(map, lib.metadata(), files);
    let gels = gels(map, &lib, files);
    let scene_gels = Arc::new(crate::native_gels_scene::load(map, &mut lib, files));
    eprintln!(
        "native-lights: {} resident lit structure parts, {unread} structure parts with unread materials, {} vertex programs, {} stage images, {} gels",
        clusters.len(),
        microcode.len(),
        images.len(),
        gels.len()
    );
    Some(LightsWorld {
        resolver: materials::Resolver::new(lib, microcode),
        materials,
        clusters,
        placements: world.placements.iter().copied().collect(),
        structure_ranges: world.light_parts.clone(),
        images: Arc::new(images),
        palettized,
        palettes: palettes.clone(),
        gels,
        scene_gels,
        normalization,
        attenuation,
    })
}

/// The gels of the map's light definitions (`ligh +0x7c`), image 0 each.
fn gels<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    lib: &ShaderLibrary,
    files: &mut assets::ResourceFiles,
) -> HashMap<u32, Image> {
    let datums: Vec<u32> = map
        .tags()
        .iter()
        .filter(|t| t.group.0 == *b"ligh")
        .filter_map(|t| t.location.as_ref().map(|l| l.address))
        .filter_map(|a| {
            let b = lib
                .metadata()
                .bytes(a.wrapping_add(engine::light_passes::DEFINITION_GEL), 4)
                .ok()?;
            Some(u32::from_le_bytes(b.try_into().ok()?))
        })
        .filter(|&d| d != u32::MAX)
        .collect();
    let mut out = HashMap::new();
    for d in datums {
        if !out.contains_key(&d)
            && let Some((i, _)) = light_image(map, files, TagId(d), 0, 0, &None)
        {
            out.insert(d, i);
        }
    }
    out
}

/// Owned at admission, including the world whose index buffer the ranges
/// address. Issuing or replaying a run never reads the list or resolves a
/// material a second time.
#[derive(Clone)]
struct PreparedLight {
    ready: Arc<Ready>,
    constants: native_render::lights::LightConstants,
    /// A scene caster group's projection; ordinary lit draws leave it out.
    projection: Option<[u32; native_render::shadow_look::PROJECTION_DWORDS]>,
    /// A receiver group's frozen map; lit and caster draws leave it out.
    shadow_map: Option<native_render::shadow_gpu::SampledMap>,
    /// A scene light's receiver: the map its caster froze on the copy
    /// being recorded, in place of [`Self::shadow_map`].
    frozen: Option<scene::Frozen>,
    draws: Vec<Planned>,
    cull: InheritedCull,
    scissor: [u32; 4],
    binding: Option<Binding>,
}

impl PreparedLight {
    fn matches(&self, draw: &d3d::ResolvedDraw, planned: &Planned) -> bool {
        use gfx::state::{CompareFunc, DepthBufferMode, FillMode};
        let r = &draw.state.render;
        let program = &planned.compiled.program;
        if draw.hw.zpass_count_enable
            || !draw.state.bad.is_empty()
            || !matches!(&draw.shader.vertex, d3d::shader::VertexProcessing::Program { microcode, .. }
                if &microcode[..] == program.words.as_slice()) {
            return false;
        }
        let rows = planned.model.as_ref().map(|m| m.rows.as_slice())
            .or_else(|| planned.placement.as_ref().map(|rows| rows.as_slice()));
        if let Some(rows) = rows {
            let decompression = match &planned.model {
                Some(model) => match model.section() {
                    Some(section) => Some(&section.decompression),
                    None => return false,
                },
                None => None,
            };
            if !pose_matches(&draw.shader.constants[..], rows, decompression)
                || draw.retail_constants.as_ref().is_some_and(|(constants, _)|
                    !pose_matches(&constants[..], rows, decompression)) {
                return false;
            }
        }
        let (primitive, (writes_depth, depth_compare)) = native_render::program::pipeline_state(
            Some(&program.state),
            planned.cull(&self.cull),
        );
        let live_primitive = gfx::translate::primitive_state(
            r,
            wgpu::PrimitiveTopology::TriangleList,
            gfx::translate::Capabilities::from_features(wgpu::Features::empty()),
        );
        let depth = r.depth_mode != DepthBufferMode::Disabled;
        let compare = if depth {
            gfx::translate::compare_function(r.depth_func)
        } else {
            wgpu::CompareFunction::Always
        };
        let alpha = (r.alpha_test_enable && r.alpha_func != CompareFunc::Always)
            .then(|| (gfx::translate::compare_function(r.alpha_func), r.alpha_ref));
        draw.clip
            .is_some_and(|r| [r.x, r.y, r.width, r.height] == self.scissor)
            && primitive == live_primitive
            && writes_depth == (depth && r.depth_write)
            && depth_compare == compare
            && program.state.blend == gfx::translate::blend_state(r)
            && (program.state.blend.is_none()
                || program.state.blend_constant == gfx::translate::blend_constant(r))
            && program.state.color_writes == gfx::translate::color_writes(r.color_write)
            && program.state.alpha_test == alpha
            && !r.fog_enable
            && program.state.stencil_matches_off_layer(&r.stencil)
            && r.fill_mode == FillMode::Solid
            && r.back_fill_mode == FillMode::Solid
            && (!r.solid_offset_enable || (r.depth_bias == 0.0 && r.depth_bias_slope_scale == 0.0))
    }

    fn draw<'a>(&'a self, p: &'a Planned) -> LightDraw<'a> {
        let c = &self.constants;
        let inputs = LightInputs::from_constants(c);
        LightDraw {
            program: &p.compiled.program,
            gpu: &p.compiled.gpu,
            cull: p.cull(&self.cull),
            sample: p.sample.as_ref(),
            colours: [inputs.colour_a, inputs.colour_b],
            frame: &c.frame,
            projection: self.projection.as_ref(),
            gel: c.gel,
            gel_view: None,
            gel_scale: self.binding.map(Binding::tex_scale),
            shadow_map: self.shadow_map.as_ref(),
            ranges: &p.ranges,
            placement: p.placement.as_ref(),
            model: None,
        }
    }
}

/// The handler may re-pose a model between planning and indexed emission.
fn pose_matches(constants: &[Row], nodes: &[Row], decompression: Option<&[Row; 3]>) -> bool {
    use native_render::program::{MODEL_ROWS, POSITION_DECOMPRESSION};
    let matches = |start: usize, rows: &[Row]| {
        constants.get(start..start + rows.len()).is_some_and(|live|
            live.iter().zip(rows).all(|(a, b)| a.map(f32::to_bits) == b.map(f32::to_bits)))
    };
    matches(MODEL_ROWS, nodes)
        && decompression.is_none_or(|rows| matches(POSITION_DECOMPRESSION, rows))
}

/// Intersect in guest pixels, after the guest projected and rounded its
/// light rectangle. Use half-open edges, including offset viewports.
fn intersect_scissor([x, y, w, h]: [u32; 4], (vx, vy, vw, vh): (u32, u32, u32, u32)) -> [u32; 4] {
    let (left, top) = (x.max(vx), y.max(vy));
    [
        left,
        top,
        x.saturating_add(w)
            .min(vx.saturating_add(vw))
            .saturating_sub(left),
        y.saturating_add(h)
            .min(vy.saturating_add(vh))
            .saturating_sub(top),
    ]
}

fn scale_scissor(rect: [u32; 4], guest: (u32, u32), target: (u32, u32)) -> Option<[u32; 4]> {
    let scale = d3d::scaled::RenderScale::between(guest, target)?;
    let [x, y, w, h] = rect;
    if w == 0 || h == 0 {
        return Some([0; 4]);
    }
    if x.checked_add(w)? > guest.0 || y.checked_add(h)? > guest.1 {
        return None;
    }
    Some(scale.rect(x, y, w, h))
}

/// Resources captured from this BSP and its material variants at admission.
struct RetainedLight {
    set: LightSet,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    materials: Vec<Arc<Mutex<LightGpu>>>,
}

/// One admitted light draw, appended as packets to the D3D pass open on
/// its attachments ([`d3d::ExternalPass::record_draws`]) with the light
/// passes' cached pipelines and texture groups and resident uniforms.
struct RunPass {
    /// Only the whole typed path snapshots its materials, gel and geometry.
    retained: Option<RetainedLight>,
    light: PreparedLight,
    view: Option<[[f32; 4]; d3d::VIEW_CONSTANTS]>,
    clip: Option<d3d::DepthClip>,
    gel_slot: Option<usize>,
    /// The handler's resolved draw it replaces; none for a layer drawn
    /// from the scene alone.
    fallback: Option<Arc<d3d::ResolvedDraw>>,
    /// A colour-mask caster has no depth attachment; the backend supplies
    /// its scratch depth for the shared packet path.
    colour_only: bool,
    /// E3's chosen lights draw only in an E3 frame
    /// (`render_bridge::native_lights::chosen`).
    lighting_only: Option<d3d::Lighting>,
    /// A chosen light drawn with its shadowed implementation: its handle,
    /// whose cone shadow the frame graph writes into the destination
    /// alpha before the pass (`d3d::ExternalPass::cone_shadow`).
    cone_shadow: Option<u32>,
}

impl RetainedLight {
    /// `light`'s materials, gel and the world's geometry held for the
    /// scene to record its pass again (`d3d::ExternalPass::replays_scene`).
    fn of(ready: &Ready, light: &PreparedLight) -> Result<Self, Keep> {
        let upload = &ready.upload_device.as_ref().ok_or(Keep::NoWorld)?.0;
        let (set, vertices, indices) = ready
            .gpu()
            .as_mut()
            .and_then(|gpu| gpu.retain_light(upload, light.constants.gel))
            .ok_or(Keep::NoWorld)?;
        let materials = light
            .draws
            .iter()
            .map(|draw| {
                let mut gpu = draw
                    .compiled
                    .gpu
                    .try_lock()
                    .map_err(|_| Keep::Unsupported("light material is busy"))?;
                Ok(gpu.retain())
            })
            .collect::<Result<Vec<_>, Keep>>()?;
        Ok(Self {
            set,
            vertices,
            indices,
            materials,
        })
    }
}

impl d3d::ExternalPass for RunPass {
    fn replays_scene(&self) -> bool {
        self.retained.is_some()
    }

    fn lighting_only(&self) -> Option<d3d::Lighting> {
        self.lighting_only
    }

    fn cone_shadow(&self) -> Option<u32> {
        self.cone_shadow
    }

    fn uses_validated_resources(&self) -> bool {
        true
    }

    fn colour_only(&self) -> bool {
        self.colour_only
    }

    fn writes(&self) -> d3d::ExternalWrites {
        d3d::ExternalWrites {
            color: self
                .light
                .draws
                .iter()
                .any(|d| !d.compiled.program.state.color_writes.is_empty()),
            depth_stencil: self
                .light
                .draws
                .iter()
                .any(|d| d.compiled.program.state.depth_write),
        }
    }

    fn resolved_fallback(&self) -> Option<Arc<d3d::ResolvedDraw>> {
        self.fallback.clone()
    }

    fn record_draws(
        &mut self,
        t: &d3d::backend::ExternalAttachments,
        request: d3d::ExternalPassRequest,
    ) -> Option<Result<d3d::backend::ExternalDraws, &'static str>> {
        Some(self.draws(t, request))
    }

    fn view_constants(&mut self, constants: &[[f32; 4]; d3d::VIEW_CONSTANTS]) {
        self.view = Some(*constants);
    }

    fn depth_clip(&mut self, clip: d3d::DepthClip) {
        self.clip = Some(clip);
    }
}

/// Target 13's sample view on this copy: the plan's pinned slot, when its
/// guest size is the binding's (`Binding::sample` over pinned slots).
fn gel_sample(
    binding: Binding,
    slots: &[Option<d3d::ExternalColor>],
    slot: usize,
) -> Option<&wgpu::TextureView> {
    let surface = slots.get(slot)?.as_ref()?;
    (surface.guest_size == binding.guest_size).then_some(&surface.sample)
}

impl RunPass {
    /// Each copy records on its own: the scissor scaled to its texels, the
    /// gel sampled from its own pinned slot.
    fn draws(
        &self,
        t: &d3d::backend::ExternalAttachments,
        request: d3d::ExternalPassRequest,
    ) -> Result<d3d::backend::ExternalDraws, &'static str> {
        let guest_size = request.surface_size;
        let [x, y, width, height] = self.light.scissor;
        let clip = d3d::PixelRect {
            x,
            y,
            width,
            height,
        }
        .projected_x(request.rect, request.projection_factor(t.copy));
        let scissor = scale_scissor(
            [clip.x, clip.y, clip.width, clip.height],
            guest_size,
            t.size,
        )
        .ok_or("unsupported scissor scale")?;
        let constants = self.view.ok_or("missing view constants")?;
        let clip = self
            .clip
            .filter(|c| c.w_buffered == Some(true))
            .ok_or("unknown W-buffer at issue")?;
        let view = ViewInputs {
            constants: request.view_constants(t.copy, constants),
            guest_size,
            size: t.size,
            z_scale: request.z_scale,
            w_clip: Some((clip.range, clip.clamp)),
        };
        // Resolve the pinned slot on this copy before replacing the guest draw.
        let sample = match self.light.binding {
            Some(binding) => Some(
                gel_sample(binding, &t.slots, self.gel_slot.ok_or("missing gel slot")?)
                    .ok_or("target 13 has no matching sample view on this copy")?,
            ),
            None => None,
        };
        // A scene light's receivers sample the map frozen on this copy.
        let frozen = match &self.light.frozen {
            Some(frozen) => Some(frozen.on(t.copy).ok_or("no caster map frozen on this copy")?),
            None => None,
        };
        let draws: Vec<_> = self
            .light
            .draws
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let mut draw = self.light.draw(p);
                if let Some(map) = &frozen {
                    draw.shadow_map = Some(map);
                }
                if let Some(held) = &self.retained {
                    draw.gpu = &held.materials[i];
                }
                draw.gel_view = sample;
                if let Some(m) = &p.model {
                    draw.model = Some(
                        m.geometry(&t.device)
                            .ok_or("model light part unavailable")?,
                    );
                }
                Ok(draw)
            })
            .collect::<Result<_, &'static str>>()?;
        let target = LightTarget {
            device: &t.device,
            queue: &t.queue,
            uploads: &t.uploads,
            color: &t.color,
            color_format: t.color_format,
            depth: &t.depth,
            depth_format: t.depth_format,
            size: t.size,
        };
        if let Some(held) = &self.retained {
            return held.set.draws(&target, &held.vertices, &held.indices, &view, scissor, &draws);
        }
        let mut gpu = self.light.ready.gpu();
        gpu.as_mut().ok_or("world upload failed")?.prepare_lights(&target, scissor, &view, &draws)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::FlatMemory;

    #[test]
    fn a_scene_light_draws_with_the_constants_of_its_shadow_choice() {
        let consts = |flags| LightConstants {
            frame: [0; 36],
            flags,
            colour_a: [0; 3],
            colour_b: [0; 3],
            mode: 0,
            definition_flags: 0,
            definition_8a: 0,
            definition_80: 0,
            falloff_0: 0,
            gel: None,
            animated_gel: false,
        };
        let mut light = frame_scene::Light::host_light(0, [1.0, 2.0, 3.0], [1.0; 3], 4.0);
        assert!(scene_light(&light).is_none(), "no constants, no draw");
        light.candidate.as_mut().unwrap().constants = Some(frame_scene::LightVariants {
            unshadowed: consts(1),
            shadowed: consts(5),
            projection: None,
        });
        let (l, gel) = scene_light(&light).unwrap();
        assert_eq!((l.constants.unwrap().flags, l.position, gel), (1, [1.0, 2.0, 3.0], None));
        light.flags = 4;
        assert_eq!(scene_light(&light).unwrap().0.constants.unwrap().flags, 5);
    }

    #[test]
    fn gathered_models_match_the_live_instance_pass_class_and_complete_index_window() {
        use frame_scene::{InstanceKey, LightBinding, LightPose, LightStream, Topology};
        use gfx::vertex::PrimitiveType::{TriangleList, TriangleStrip};
        let entry = LightEntry {
            slot: 2,
            record: 5,
            owner: LayerOwner::Object(InstanceKey(101)),
            model: 80,
            stream: LightStream {
                section: 7, part: 3, vertex_class: 6, first_index: 12,
                index_count: 9, topology: Topology::Strip,
            },
            pose: LightPose::Rows(0..6),
            binding: LightBinding { shader: 64, selector: 2, variant: 1, implementation: 4 },
            state: Default::default(),
            subparts: None,
        };
        let slot = LightSlot::new(8, 72, &entry);
        let owner = |handle, model, section| ResolvedOwner::Object { handle, model, section };
        let live = Geometry {
            shader: 64, variant: 1, implementation: 4, sorted_slot: 8,
            record: 5, part: 3, owner: owner(101, 80, 7),
        };
        let selection = Some((2, 6));
        assert!(slot.matches(live, selection, TriangleStrip, 72, 9));
        for changed in [
            Geometry { sorted_slot: 32, ..live },
            Geometry { record: 6, ..live },
            Geometry { part: 2, ..live },
            Geometry { shader: 64 + (1 << 16), ..live },
            Geometry { variant: 0, ..live },
            Geometry { implementation: 2, ..live },
            Geometry { owner: owner(102, 80, 7), ..live },
            Geometry { owner: owner(101, 81, 7), ..live },
            Geometry { owner: owner(101, 80, 6), ..live },
            Geometry { owner: ResolvedOwner::Cluster(7), ..live },
        ] {
            assert!(!slot.matches(changed, selection, TriangleStrip, 72, 9));
        }
        for changed in [None, Some((1, 6)), Some((2, 5))] {
            assert!(!slot.matches(live, changed, TriangleStrip, 72, 9));
        }
        assert!(!slot.matches(live, selection, TriangleList, 72, 9));
        assert!(!slot.matches(live, selection, TriangleStrip, 72, 6));
        assert!(!slot.matches(live, selection, TriangleStrip, 74, 9));
        let indices = [1, 2, 3, 4, 4, 5, 6];
        assert!(slot.matches_triangles(&indices, 9));
        let list = LightSlot { primitive: TriangleList, ..slot };
        assert!(list.matches_triangles(&indices, 3));
        assert!(!list.matches_triangles(&indices, indices.len()));
    }

    #[test]
    fn moved_nodes_and_replaced_decompression_refuse_the_resident_model() {
        use native_render::program::{MODEL_ROWS, POSITION_DECOMPRESSION};
        let nodes = [[0.5; 4]; 6];
        let compression = [[2.; 4]; 3];
        let mut live = vec![[0.; 4]; POSITION_DECOMPRESSION + 3];
        live[MODEL_ROWS..MODEL_ROWS + nodes.len()].copy_from_slice(&nodes);
        live[POSITION_DECOMPRESSION..].copy_from_slice(&compression);
        assert!(pose_matches(&live, &nodes, Some(&compression)));
        live[MODEL_ROWS + 4][1] = 0.;
        assert!(!pose_matches(&live, &nodes, Some(&compression)));
        live[MODEL_ROWS + 4] = nodes[4];
        live[POSITION_DECOMPRESSION][3] = 0.;
        assert!(!pose_matches(&live, &nodes, Some(&compression)));
        assert!(pose_matches(&live, &nodes, None));
    }

    fn level(face: u32, level: u32, depth: u32) -> assets::bitmap::Level {
        assets::bitmap::Level {
            face,
            level,
            width: 2,
            height: 2,
            depth,
            rgba: vec![face as u8; (16 * depth) as usize],
        }
    }

    /// A synthetic metadata region at virtual `base`, zeroes but `words`.
    fn metadata(base: u32, len: usize, words: &[(u32, u32)]) -> tags::shader::Metadata {
        let mut b = vec![0u8; len];
        for &(a, v) in words {
            let o = (a - base) as usize;
            b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        }
        tags::shader::Metadata::new(base, b)
    }

    #[test]
    fn static_light_textures_reuse_the_decode_until_the_source_changes() {
        use gfx::{
            D3DFormat,
            texture::{TextureDesc, TextureKind},
        };
        let desc = TextureDesc::new(D3DFormat::Al8, TextureKind::Volume, 2, 2, 2, 1);
        let image = |_: &TextureDesc, bytes: &[u8]| {
            LayeredImage::volume(2, 2, 2, vec![bytes[0]; 32])
                .ok_or_else(|| "wrong volume shape".into())
        };
        let mut slot = None;
        let a = cached_light_texture(&mut slot, desc.clone(), vec![42], image).unwrap();
        let b = cached_light_texture(&mut slot, desc.clone(), vec![42], |_, _| {
            panic!("decoded twice")
        })
        .unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        let c = cached_light_texture(&mut slot, desc, vec![43], image).unwrap();
        assert!(!Arc::ptr_eq(&a, &c));
        assert_eq!((a.image().rgba[0], c.image().rgba[0]), (42, 43));
    }

    #[test]
    fn the_light_bitmaps_come_from_matgs_rasterizer_block_through_the_metadata() {
        // `0x00022590`: count at matg +0x108, the first element's address
        // at +0x10c; `0x00019530` reads datums at the element's +0xc
        // (texture 1) and +4 (texture 9), each a tag reference's datum.
        assert_eq!(
            (RASTERIZER_DATA, NORMALIZATION_DATUM, ATTENUATION_DATUM),
            (0x108, 0xc, 4)
        );
        let base = 0x8000_0000;
        let matg = base + 0x200;
        let element = base + 0x800;
        let bitm = map::FourCC(*b"bitm").as_u32();
        let mut words = vec![
            (matg + 0x108, 1),
            (matg + 0x10c, element),
            // The next field: reading the block one word late took this
            // for the elements' address (and the address for the count).
            (matg + 0x110, 0x40),
            (element, bitm),
            (element + 4, 0xe000_0009),
            (element + 8, bitm),
            (element + 0xc, 0xe000_0001),
        ];
        let meta = metadata(base, 0x1000, &words);
        assert_eq!(
            matg_light_bitmaps(&meta, matg),
            (Some(TagId(0xe000_0001)), Some(TagId(0xe000_0009)))
        );
        // An object's lightmap stage: the element's +0x64 (0x00018d1f).
        let read = |a: u32, n: usize| meta.bytes(a, n).ok().map(<[u8]>::to_vec);
        assert_eq!(
            rasterizer_bitmap(&read, matg, 0x64),
            Some(TagId(0)),
            "an unwritten word reads as datum 0, as the game reads it"
        );
        // A null reference: datum -1.
        words.push((element + 0x60, bitm));
        words.push((element + 0x64, u32::MAX));
        let meta = metadata(base, 0x1000, &words);
        let read = |a: u32, n: usize| meta.bytes(a, n).ok().map(<[u8]>::to_vec);
        assert_eq!(rasterizer_bitmap(&read, matg, 0x64), None, "a null datum");
        words.pop();
        words.push((element + 0x64, 0xe000_0064));
        let meta = metadata(base, 0x1000, &words);
        let read = |a: u32, n: usize| meta.bytes(a, n).ok().map(<[u8]>::to_vec);
        assert_eq!(
            rasterizer_bitmap(&read, matg, 0x64),
            Some(TagId(0xe000_0064))
        );
        // The tag's file offset is not a metadata address.
        assert_eq!(matg_light_bitmaps(&meta, 0x200), (None, None));
        // A null reference is no bitmap.
        words[6].1 = u32::MAX;
        assert_eq!(
            matg_light_bitmaps(&metadata(base, 0x1000, &words), matg),
            (None, Some(TagId(0xe000_0009)))
        );
        // An empty block names nothing, whatever its address says.
        words[0].1 = 0;
        assert_eq!(
            matg_light_bitmaps(&metadata(base, 0x1000, &words), matg),
            (None, None)
        );
        // An element outside the metadata does not read.
        words[0].1 = 1;
        words[1].1 = base + 0x2000;
        assert_eq!(
            matg_light_bitmaps(&metadata(base, 0x1000, &words), matg),
            (None, None)
        );
    }

    #[test]
    fn the_light_textures_take_the_shapes_the_bitmap_decode_gives() {
        use gfx::D3DFormat;
        use gfx::texture::{TextureDesc, TextureKind};
        // A swizzled X8R8G8B8 cube (an Xrgb8888 cube bitmap's descriptor),
        // 4x4 with three levels, each face's level 0 filled with its number.
        let desc = TextureDesc::new(D3DFormat::X8R8G8B8, TextureKind::Cube, 4, 4, 1, 3);
        let layout = desc.layout().unwrap();
        let mut data = vec![0u8; layout.total_size];
        for f in 0..6 {
            let at = f * layout.face_stride;
            data[at..at + layout.levels[0].size].fill(f as u8 + 1);
        }
        let levels = assets::bitmap::decode_bytes(&desc, &data).unwrap();
        assert_eq!(levels.len(), 18, "six faces of three levels, face-major");
        assert!(levels.iter().all(|l| l.depth == 1));
        let cube_levels = levels.clone();
        let c = layered(levels, TextureDim::Cube).expect("a cube");
        assert_eq!(
            (c.dim, c.width, c.height, c.layers, c.rgba.len()),
            (TextureDim::Cube, 4, 4, 6, 6 * 64)
        );
        for f in 0..6 {
            assert_eq!(c.rgba[f * 64], f as u8 + 1, "face {f} in its layer");
        }
        assert!(
            layered(cube_levels, TextureDim::D3).is_none(),
            "a cube is not a volume"
        );

        // A swizzled AL8 volume (an Ay8 volume bitmap's), 4x4x4, three levels.
        let desc = TextureDesc::new(D3DFormat::Al8, TextureKind::Volume, 4, 4, 4, 3);
        let data = vec![0x80u8; desc.layout().unwrap().total_size];
        let levels = assets::bitmap::decode_bytes(&desc, &data).unwrap();
        let shape: Vec<_> = levels
            .iter()
            .map(|l| (l.face, l.level, l.width, l.depth))
            .collect();
        assert_eq!(
            shape,
            vec![(0, 0, 4, 4), (0, 1, 2, 2), (0, 2, 1, 1)],
            "face 0 only, each level all its slices"
        );
        let volume_levels = levels.clone();
        let v = layered(levels, TextureDim::D3).expect("a volume");
        assert_eq!(
            (v.dim, v.width, v.height, v.layers, v.rgba.len()),
            (TextureDim::D3, 4, 4, 4, 4 * 64)
        );
        assert!(
            v.rgba.iter().all(|&b| b == 0x80),
            "AL8 decodes to (v, v, v, v)"
        );
        assert!(
            layered(volume_levels, TextureDim::Cube).is_none(),
            "a volume is not a cube"
        );
    }

    /// A decoded texel as the sampler returns it (R, G, B, A), whichever
    /// host format the decode chose.
    fn sampled(host: gfx::HostFormat, px: &[u8]) -> [u8; 4] {
        match host {
            gfx::HostFormat::Bgra8Unorm => [px[2], px[1], px[0], px[3]],
            gfx::HostFormat::Rgba8Unorm => [px[0], px[1], px[2], px[3]],
            h => panic!("{h:?}"),
        }
    }

    #[test]
    fn the_native_light_textures_hold_the_texels_the_d3d_path_uploads() {
        use gfx::D3DFormat;
        use gfx::texture::{TextureDesc, TextureKind};
        // The engine stages sample without D3DTSS_COLORSIGN (the combiner
        // expands with _bx2), so the D3D path decodes with no sign bits.
        let cases = [
            (
                TextureDesc::new(D3DFormat::X8R8G8B8, TextureKind::Cube, 4, 4, 1, 3),
                TextureDim::Cube,
            ),
            (
                TextureDesc::new(D3DFormat::Al8, TextureKind::Volume, 4, 4, 4, 3),
                TextureDim::D3,
            ),
        ];
        for (desc, dim) in cases {
            let len = desc.layout().unwrap().total_size;
            // Synthetic texels, all different nearby.
            let data: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            let d3d = gfx::decode_texture(
                &desc,
                &data,
                None,
                gfx::DecodeOptions::default().with_color_sign(0),
            )
            .unwrap();
            let native = layered(assets::bitmap::decode_bytes(&desc, &data).unwrap(), dim)
                .expect("the shape");
            assert_eq!(
                1 + native.mips.len() as u32,
                desc.levels,
                "{dim:?}: the whole chain"
            );
            for l in &d3d.levels {
                let bytes = if l.level == 0 {
                    &native.rgba
                } else {
                    &native.mips[l.level as usize - 1]
                };
                assert_eq!(
                    native.level_size(l.level),
                    (
                        l.width,
                        l.height,
                        if dim == TextureDim::Cube { 6 } else { l.depth }
                    )
                );
                let layer = (l.width * l.height * l.depth * 4) as usize;
                let at = l.face as usize * layer;
                for (i, px) in l.data.chunks_exact(4).enumerate() {
                    let ours = &bytes[at + 4 * i..at + 4 * i + 4];
                    assert_eq!(
                        sampled(d3d.host, px),
                        [ours[0], ours[1], ours[2], ours[3]],
                        "{dim:?} face {} level {} texel {i}",
                        l.face,
                        l.level
                    );
                }
            }
        }
    }

    #[test]
    fn light_clip_intersects_guest_viewport_before_scaling() {
        let clip = intersect_scissor([7, 4, 8, 10], (9, 2, 8, 9));
        assert_eq!(clip, [9, 4, 6, 7]);
        assert_eq!(
            scale_scissor(clip, (20, 15), (60, 45)),
            Some([27, 12, 18, 21])
        );
        assert_eq!(
            scale_scissor(clip, (20, 15), (60, 40)),
            Some([27, 11, 18, 18])
        );
        let empty = intersect_scissor([17, 0, 2, 9], (9, 2, 8, 9));
        assert_eq!(scale_scissor(empty, (20, 15), (40, 30)), Some([0; 4]));
    }

    #[test]
    fn a_run_requires_adjacent_whole_lights_with_the_same_target_generation() {
        let order = RunOrder {
            previous: Some((5, 9, 3)),
        };
        assert_eq!(order.key(6, 9, false), 3);
        assert_eq!(order.key(6, 11, false), 6); // away and back
        assert_eq!(order.key(7, 9, false), 7); // refused light or view boundary
        assert_eq!(order.key(6, 9, true), 6); // animated gel
        assert_eq!(RunOrder::default().key(7, 9, false), 7); // no preceding run
    }

    #[test]
    fn the_tally_says_each_reason_once_and_sums_up() {
        let mut t = Tally::default();
        assert_eq!(
            t.count(Err(Keep::Unloaded("normalization cube"))),
            vec!["native-lights: light plan declined: samples an unloaded normalization cube".to_string()]
        );
        assert!(
            t.count(Err(Keep::Unloaded("normalization cube")))
                .is_empty(),
            "once per reason"
        );
        assert_eq!(
            t.count(Err(Keep::Unsupported("missing pose"))),
            vec!["native-lights: light plan declined: missing pose".to_string()]
        );
        assert!(t.count(Ok(())).is_empty());
        assert_eq!(
            t.summary(),
            "native-lights: 1 of 4 host planning requests accepted; declined: 2 samples an unloaded normalization cube, 1 missing pose"
        );
        for _ in 5..SUMMARY_EVERY {
            assert!(t.count(Ok(())).is_empty());
        }
        let said = t.count(Ok(()));
        assert_eq!(t.asked, SUMMARY_EVERY);
        assert_eq!(
            said,
            vec![t.summary()],
            "the counts every SUMMARY_EVERY lights"
        );
        assert_eq!(
            Tally::default().summary(),
            "native-lights: 0 of 0 host planning requests accepted; declined: none"
        );
    }

    #[test]
    fn the_tally_bounds_unique_shader_diagnostics_without_losing_counts() {
        let mut tally = Tally::default();
        for i in 0..4 * REFUSAL_DETAIL_LIMIT {
            tally.count(Err(Keep::Refused(format!("shader {i}").into())));
        }
        assert_eq!(tally.kept.len(), REFUSAL_DETAIL_LIMIT);
        assert_eq!(tally.other_refusals, (3 * REFUSAL_DETAIL_LIMIT) as u64);
        assert!(tally.count(Err(Keep::Refused("shader 0".into()))).is_empty());
        assert_eq!(tally.kept[&Keep::Refused("shader 0".into())], 2);
        assert_eq!(tally.count(Err(Keep::NoMaterial)).len(), 1);
        assert!(tally.summary().contains("other shader refusals"));
        assert_eq!(
            tally.kept.values().sum::<u64>() + tally.other_refusals,
            tally.asked
        );
    }

    #[test]
    fn cube_and_volume_shapes_from_decoded_levels() {
        let faces: Vec<_> = (0..6)
            .flat_map(|f| [level(f, 0, 1), level(f, 1, 1)])
            .collect();
        let c = layered(faces, TextureDim::Cube).expect("six faces at level 0");
        assert_eq!((c.layers, c.rgba.len()), (6, 96));
        assert!(layered(vec![level(0, 0, 4)], TextureDim::Cube).is_none());
        let v = layered(vec![level(0, 0, 4), level(0, 1, 2)], TextureDim::D3)
            .expect("level 0's slices");
        assert_eq!((v.layers, v.rgba.len()), (4, 64));
        assert!(
            layered(vec![level(0, 1, 4)], TextureDim::D3).is_none(),
            "no level 0"
        );
    }

    #[test]
    fn target_13_requires_the_current_light_and_engine_texture_10_selector() {
        use native_render::gels;
        let mut mem = FlatMemory::new(gels::DEFINITION, 0x1000);
        let definition = gels::DEFINITION + 0x400;
        let shader = 0xabcd_0005;
        mem.set_u32(gels::DEFINITION, definition).unwrap();
        mem.set_u32(
            definition + engine::light_passes::DEFINITION_GEL_SHADER,
            shader,
        )
        .unwrap();
        assert!(uses_gel_target(&mem, definition, shader, 0, 0));
        assert!(!uses_gel_target(
            &mem,
            definition,
            shader,
            engine::light_passes::flag::CONTEXT_ZERO,
            0
        ));
        assert!(!uses_gel_target(
            &mem,
            definition,
            shader,
            0,
            engine::light_passes::DEFINITION_TEXTURE_18
        ));
        assert!(!uses_gel_target(&mem, definition, shader + 1, 0, 0));
        assert!(!uses_gel_target(&mem, definition + 4, shader, 0, 0));
        mem.write(gels::BUILDING, &[1]).unwrap();
        assert!(!uses_gel_target(&mem, definition, shader, 0, 0));
    }

    #[test]
    fn lights_with_constants_are_drawable_shadowed_or_not() {
        use native_render::lights::LightConstants;
        let c = LightConstants {
            frame: [0; 36],
            flags: 0,
            colour_a: [0; 3],
            colour_b: [0; 3],
            mode: 0,
            definition_flags: 0,
            definition_8a: 0,
            definition_80: 0,
            falloff_0: 0,
            gel: None,
            animated_gel: false,
        };
        let light = |constants| Light {
            handle: 1,
            definition: 2,
            position: [0.0; 3],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            colour_a: [1.0; 3],
            colour_b: [0.0; 3],
            intensity: 1.0,
            bounds: ([0.0; 3], 1.0),
            shadow: false,
            weight: 1.0,
            constants,
        };
        assert!(constants(&light(Some(c))).is_ok());
        assert!(
            constants(&light(Some(LightConstants {
                flags: 4 | 0x20,
                ..c
            })))
            .is_ok(),
            "shadowed"
        );
        assert!(
            constants(&light(Some(LightConstants {
                animated_gel: true,
                ..c
            })))
            .unwrap()
            .animated_gel
        );
        assert!(matches!(constants(&light(None)), Err(Keep::NoConstants)));
    }

    #[test]
    fn the_device_sees_the_hooks_memory_and_its_faults() {
        let mut m = FlatMemory::new(0x1000, 0x100);
        let mut dm = DeviceMemory(&mut m);
        d3d::GuestMemory::write_u32(&mut dm, 0x1010, 0xdead_beef).unwrap();
        assert_eq!(
            d3d::GuestMemory::read_u32(&dm, 0x1010).unwrap(),
            0xdead_beef
        );
        let e = d3d::GuestMemory::read_u32(&dm, 0x10fe).unwrap_err();
        assert_eq!((e.addr, e.len), (0x10fe, 4));
        assert!(d3d::GuestMemory::write(&mut dm, 0x0ffc, &[0; 4]).is_err());
        assert_eq!(m.u32(0x1010).unwrap(), 0xdead_beef);
    }
}
