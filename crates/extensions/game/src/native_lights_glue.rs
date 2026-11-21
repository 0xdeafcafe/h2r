//! Native light draws over sorted guest geometry. Each target copy prepares
//! immutable draw resources alongside the exact resolved fallback.
mod materials;
pub mod models;
pub mod walk;

use crate::native_gels_glue::{Binding, Checkpoint, Outputs};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;
use std::sync::{Arc, Mutex};
use std::sync::atomic::Ordering;

use engine::memory::{GuestMemory, MemFault};
use frame_scene::{LayerOwner, LightEntry, Row};
use render_bridge::native_lights::{Alone, Dispatch, Host};
use map::{Map, TagId};
use native_render::gpu::Image;
use native_render::light_gpu::{
    LayeredImage, LightDraw, LightGpu, LightSet, LightTarget, ModelGeometry, StaticLightTexture, TextureDim,
};
use native_render::light_look::{
    DIFFUSE_LAYER, LightInputs, LightProgram, LightStage, SPECULAR_LAYER,
};
use native_render::light_models::LightModels;
use native_render::light_list::{SortedItem, SortedLayer};
use native_render::lights::{Light, LightConstants};
use native_render::look;
use native_render::program::ViewInputs;
use native_render::world::{Owner, World, WorldDraw};
use native_render::program::InheritedCull;
use tags::material::{EngineInputs, Material, ShaderLibrary, TextureSource};

use crate::native_world_glue::{Ready, State, ReadMemory, StagePalettes, current_ready, inherited_cull, light_image};

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
    checkpoint: Mutex<Option<(u64, Checkpoint)>>,
    /// What preparation returned, by reason ([`Tally`]).
    tally: Mutex<Tally>,
    order: Mutex<RunOrder>,
    admission: Arc<LightAdmission>,
    /// The map's model parts for object entries ([`models`]).
    models: models::Loader,
}

/// Why a light keeps its D3D passes.
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
    Refused(String),
    /// A texture a pass samples did not load: which.
    Unloaded(&'static str),
    Unsupported(&'static str),
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
        }
    }
}

/// Lights considered between summaries.
pub const SUMMARY_EVERY: u64 = 1800;

/// The host's answers so far: lights asked, lights planned, and the lights
/// kept on D3D by reason.
#[derive(Debug, Default)]
pub struct Tally {
    pub asked: u64,
    pub planned: u64,
    pub shader_gels: u64,
    pub kept: BTreeMap<Keep, u64>,
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
        let n = self.kept.entry(k.clone()).or_insert(0);
        *n += 1;
        if *n == 1 { vec![format!("native-lights: a light keeps its D3D passes: {k}")] } else { Vec::new() }
    }

    /// One line: lights asked, planned, and kept by reason, most first.
    pub fn summary(&self) -> String {
        let mut kept: Vec<_> = self.kept.iter().collect();
        kept.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        let kept: Vec<String> = kept.iter().map(|(k, n)| format!("{n} {k}")).collect();
        format!(
            "native-lights: host planned {} of {} lights; kept on D3D: {}",
            self.planned,
            self.asked,
            if kept.is_empty() { "none".to_string() } else { kept.join(", ") }
        )
    }
}

impl<B: d3d::backend::Backend + Send + 'static> Glue<B> {
    fn gel_binding(&self, mem: &dyn GuestMemory, serial: u64, identity: Option<(u32, u32)>,
        c: &LightConstants) -> Result<Option<Binding>, Keep>
    {
        let Some((definition, shader)) = identity else {
            return if c.animated_gel { Err(Keep::Unsupported("missing gel invocation")) } else { Ok(None) };
        };
        use native_render::gels;
        if !uses_gel_target(mem, definition, shader, c.flags, c.definition_flags) {
            return Err(Keep::Unsupported("engine texture 10 does not select this gel invocation"));
        }
        // The light's entry checkpoint serves each of its layers.
        let held = *self.checkpoint.lock().unwrap_or_else(|e| e.into_inner());
        let before = held.filter(|(id, _)| *id == serial).map(|(_, c)| c)
            .ok_or(Keep::Unsupported("missing gel entry checkpoint"))?;
        let target = gels::Target::read(&ReadMemory(mem)).ok_or(Keep::Unsupported("unavailable gel target 13"))?;
        self.gels.binding_after(before, definition, shader, target.surface)
            .filter(|b| b.guest_size == target.size)
            .map(Some).ok_or(Keep::Unsupported("no matching target 13 binding for this light"))
    }

    fn prepare_light(&self, d: &d3d::Device<B>, mem: &dyn GuestMemory, light: &Light, sorted: &SortedLayer, binding: Option<Binding>)
        -> Result<(PreparedLight, usize), Keep>
    {
        let ready = self.ready(mem)?;
        let world = ready.lights.get().ok_or(Keep::NoWorld)?;
        let parts = select_parts(&world.clusters, &sorted.entries)?;
        let consumed = parts.len();
        Ok((self.prepare_parts(d, mem, ready, light, parts, binding)?, consumed))
    }

    fn ready(&self, mem: &dyn GuestMemory) -> Result<Arc<Ready>, Keep> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        Ok(Arc::clone(current_ready(&state, mem).ok_or(Keep::NoWorld)?))
    }

    /// The light's draws of `parts` (layer, implementation, world part,
    /// in draw order) with the device state they take at this point.
    fn prepare_parts(&self, d: &d3d::Device<B>, mem: &dyn GuestMemory, ready: Arc<Ready>, light: &Light,
        parts: Vec<(usize, usize, WorldDraw)>, binding: Option<Binding>) -> Result<PreparedLight, Keep>
    {
        let constants = constants(light)?;
        let world = ready.lights.get().ok_or(Keep::NoWorld)?;
        let draws = world.resolve_parts(&constants, parts, binding.is_some())?;
        self.prepare_planned(d, mem, ready, constants, draws, binding)
    }

    /// [`Self::prepare_parts`] for draws already resolved.
    fn prepare_planned(&self, d: &d3d::Device<B>, mem: &dyn GuestMemory, ready: Arc<Ready>, constants: LightConstants,
        draws: Vec<Planned>, binding: Option<Binding>) -> Result<PreparedLight, Keep>
    {
        let world = ready.lights.get().ok_or(Keep::NoWorld)?;
        if draws.is_empty() {
            return Err(Keep::Unsupported("no supported light draws in the active list"));
        }
        let memory = ReadMemory(mem);
        let target = d.external_ready(&memory, false).map_err(|why| Keep::Unsupported(why.describe()))?;
        if !d.stencil_state(&memory).is_some_and(|s| !s.enable) {
            return Err(Keep::Unsupported("enabled or unknown stencil state at light geometry"));
        }
        let clip = d.light_scissor(&memory).map_err(Keep::Unsupported)?;
        let scissor = intersect_scissor(clip, target.rect);
        if d.depth_clip(&memory, target.z_scale).w_buffered != Some(true) {
            return Err(Keep::Unsupported("light view does not have a known W-buffer"));
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
                    && program.dims[n].is_some() && !verified[k][n]
                    && palettized.contains(&(n, *bitmap, *image))
                {
                    if d.stage_palette(&memory, n) != palettes[n] {
                        return Err(Keep::Unsupported("stage palette changed since light texture upload"));
                    }
                    verified[k][n] = true;
                }
            }
        }
        let cull = d.cull_state(&memory).map(inherited_cull);
        if cull.is_none() && draws.iter().any(|p| p.compiled.program.inherits_cull) {
            return Err(Keep::Unsupported("unreadable inherited CULLMODE or FRONTFACE"));
        }
        let cull = InheritedCull { device: cull, clusters: true, placements: false };
        let (device, color, depth) = d.external_pipeline_target(&memory)
            .ok_or(Keep::Unsupported("light pipeline target unavailable"))?;
        let mut pipelines_ready = true;
        for draw in &draws {
            let model = match &draw.model {
                Some(m) => Some(&m.buffer()?.layout),
                None => None,
            };
            if let Ok(mut pass) = draw.compiled.gpu.try_lock() {
                let inherited = cull.of(draw.compiled.program.inherits_cull, true);
                let (_, compiled) = pass.pipeline(&device, color, depth, inherited, model);
                pipelines_ready &= compiled.previous_frame().is_some_and(Option::is_some);
            } else {
                pipelines_ready = false;
            }
        }
        if !pipelines_ready {
            return Err(Keep::Unsupported("light pipeline pending or unavailable"));
        }
        Ok(PreparedLight { ready, constants, draws, cull, scissor, binding })
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
        use native_render::light_scene::{Refusal, plan_entries};
        if a.shader_gel.is_some() {
            return Err(Keep::Unsupported("a shader gel draws through its handler"));
        }
        let mut d = self.device.lock().map_err(|_| Keep::Unsupported("device unavailable"))?;
        if !d.begin_external_boundary(&mut DeviceMemory(mem)).map_err(|_| Keep::Unsupported("could not drain light state"))? {
            return Err(Keep::Unsupported("light has an outstanding push callback"));
        }
        if d.bound_target() != a.view_target {
            return Err(Keep::Unsupported("light view target changed"));
        }
        let c = constants(&a.light)?;
        let ready = self.ready(mem)?;
        let world = ready.lights.get().ok_or(Keep::NoWorld)?;
        let group = a.group;
        let objects = group.entries.iter().any(|e| structure_key(e).is_none());
        let model_lights = if objects {
            let name = crate::health::loaded_map_name(mem).map_err(|_| Keep::Unsupported("the loaded map's name does not read"))?;
            let read = ReadMemory(&*mem);
            Some(self.models.get(&name, &ready.upload_device.as_ref().ok_or(Keep::NoWorld)?.0, || native_render::object_streams::StreamFormats::read(&read),
                || crate::native_world_glue::scene_load_palettes(&*d, &*mem))
                .ok_or(Keep::Unsupported("the map's model light parts are loading"))?)
        } else {
            None
        };
        let held = Held { clusters: &world.clusters, models: model_lights.as_deref().map(|m| &m.models) };
        plan_entries(&group.entries, &|e| group.rows_of(e), &held).map_err(|r| match r {
            Refusal::Remainder(_) => Keep::Unsupported("scene group has a remainder"),
            Refusal::Stream => Keep::Unsupported("scene group entry has no native light stream"),
            Refusal::Pose => Keep::Unsupported("scene group entry has no pose"),
        })?;
        let layer = a.layer as usize;
        // Runs of structure entries resolve together, as the sorted
        // list's do; each object entry is a draw of its own (its rows).
        let mut draws: Vec<Planned> = Vec::new();
        let mut run: Vec<(usize, usize, WorldDraw)> = Vec::new();
        for e in &group.entries {
            let imp = usize::from(e.binding.implementation);
            if let Some(k) = structure_key(e) {
                let (_, draw) = world.clusters.get(&k).ok_or(Keep::Unsupported("scene group entry has no native light stream"))?;
                run.push((layer, imp, *draw));
                continue;
            }
            if !run.is_empty() {
                draws.extend(world.resolve_parts(&c, std::mem::take(&mut run), false)?);
            }
            let lights = model_lights.as_ref().ok_or(Keep::Unsupported("the map's model light parts are loading"))?;
            draws.push(world.resolve_model(&c, lights, layer, e, group.rows_of(e))?);
        }
        if !run.is_empty() {
            draws.extend(world.resolve_parts(&c, run, false)?);
        }
        let prepared = self.prepare_planned(&d, mem, Arc::clone(&ready), c, draws, None)?;
        use native_render::light_passes;
        let drawn = light_passes::read(&ReadMemory(&*mem), c.flags, c.definition_flags)
            .map_or(0, |i| light_passes::steps(&i).drawn());
        if drawn & 1 << a.layer == 0 || prepared.draws.iter().any(|p| p.layer != layer) {
            return Err(Keep::Unsupported("active light layers changed during setup"));
        }
        if mem.u32(engine::light_passes::global::PASS_FLAGS).ok() != Some(c.flags) ||
            c.frame.iter().enumerate().any(|(i, word)|
                mem.u32(engine::light_constants::global::FRAME + 4 * i as u32).ok() != Some(*word)) {
            return Err(Keep::Unsupported("light globals differ from the prepared frame or flags"));
        }
        // This group has no resolved draw to replay. Reserve its packets on
        // every copy before allowing the hook to skip its guest handlers.
        let plan = d3d::ExternalPlan {
            stage: timeline::native::Stage::Lights,
            view: mem.u32(engine::render_loop::occlusion::guest::VIEW_INDEX).unwrap_or(u32::MAX),
            ranges: prepared.draws.iter().map(|draw| draw.ranges.len()).sum(),
            slots: Vec::new(),
        };
        let pass = RunPass { light: prepared, view: None, clip: None, gel_slot: None, fallback: None };
        d.prepare_external_bound(&mut DeviceMemory(mem), plan, Box::new(pass), Vec::new())
            .map_err(|_| Keep::Unsupported("device error preparing the scene light pass"))?
            .ok_or(Keep::Unsupported("the scene light pass is not ready"))?
            .commit();
        Ok(())
    }
}

/// The parts the light passes hold, for [`native_render::light_scene`]:
/// the world's structure parts in one stream (the world's buffers), each
/// part's range there, for its own shader only; and the map's model parts
/// ([`LightModels::part`]), a stream per model section.
struct Held<'a> {
    clusters: &'a ClusterParts,
    models: Option<&'a LightModels>,
}

impl native_render::light_scene::Resident for Held<'_> {
    fn part(&self, e: &LightEntry) -> Option<native_render::light_scene::ResidentPart> {
        let Some(key) = structure_key(e) else {
            let (_, part) = self.models?.part(e)?;
            return Some(native_render::light_scene::ResidentPart { stream: LightModels::stream(e), indices: part.indices.clone() });
        };
        let (shader, d) = self.clusters.get(&key)?;
        if shader.0 != e.binding.shader {
            return None;
        }
        Some(native_render::light_scene::ResidentPart { stream: 0, indices: d.first_index..d.first_index.checked_add(d.index_count)? })
    }
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
    fn reserve(&self, mem: &mut dyn GuestMemory, dispatch: Dispatch<'_>) -> Result<(), Keep> {
        let Dispatch { light, serial, layer, shader_gel, view_target, sorted } = dispatch;
        let mut d = self.device.lock().map_err(|_| Keep::Unsupported("device unavailable"))?;
        if !d.begin_external_boundary(&mut DeviceMemory(mem)).map_err(|_| Keep::Unsupported("could not drain light state"))? {
            return Err(Keep::Unsupported("light has an outstanding push callback"));
        }
        if d.bound_target() != view_target {
            return Err(Keep::Unsupported("light view target changed"));
        }
        let binding = self.gel_binding(mem, serial, shader_gel, &constants(&light)?)?;
        let (plan, count) = self.prepare_light(&d, mem, &light, sorted, binding)?;
        let c = &plan.constants;
        // The layer is one the light's steps draw, and every planned draw
        // is the layer's.
        use native_render::light_passes;
        let drawn = light_passes::read(&ReadMemory(&*mem), c.flags, c.definition_flags)
            .map_or(0, |i| light_passes::steps(&i).drawn());
        if drawn & 1 << layer == 0 || plan.draws.iter().any(|p| p.layer as u32 != layer) {
            return Err(Keep::Unsupported("active light layers changed during setup"));
        }
        if mem.u32(engine::light_passes::global::PASS_FLAGS).ok() != Some(c.flags) ||
            c.frame.iter().enumerate().any(|(i, word)|
                mem.u32(engine::light_constants::global::FRAME + 4 * i as u32).ok() != Some(*word)) {
            return Err(Keep::Unsupported("light globals differ from the prepared frame or flags"));
        }
        let mut order = self.order.lock().unwrap_or_else(|e| e.into_inner());
        let targets = d.stats().target_changes;
        let isolated = plan.binding.is_some();
        let key = order.key(serial, targets, isolated);
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !current_ready(&state, mem).is_some_and(|current| Arc::ptr_eq(current, &plan.ready)) {
            return Err(Keep::Unsupported("world changed during light preparation"));
        }
        let layer = LightLayer::new(view_target, plan, &sorted.entries[..count])
            .ok_or(Keep::Unsupported("light ranges differ from its sorted entries"))?;
        *self.admission.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(layer);
        order.previous = (!isolated).then_some((serial, targets, key));
        Ok(())
    }
}

impl<B: d3d::backend::Backend + Send + 'static> Host for Glue<B> {
    fn submit(&self, mem: &mut dyn GuestMemory, dispatch: Dispatch<'_>) -> bool {
        let shader_gel = dispatch.shader_gel.is_some();
        let result = self.reserve(mem, dispatch);
        let ok = result.is_ok();
        let mut t = self.tally.lock().unwrap_or_else(|e| e.into_inner());
        if shader_gel && ok { t.shader_gels += 1; }
        for msg in t.count(result) { eprintln!("{msg}"); }
        if t.asked == 1 || (ok && t.planned.is_power_of_two()) || (ok && shader_gel && t.shader_gels == 1) || t.asked % SUMMARY_EVERY == 0 {
            // Admitted lights join the open D3D pass as packets: they add no
            // passes of their own (`native_lights_*_passes` in profile.counters).
            eprintln!("native-lights: {} admitted, {} shader gels, {} worker round trips",
                t.planned, t.shader_gels,
                d3d::threaded::LIGHT_WORKER_WAITS.load(Ordering::Relaxed));
        }
        ok
    }

    fn clear_admission(&self) {
        self.admission.0.lock().unwrap_or_else(|e| e.into_inner()).take();
    }

    fn checkpoint_gel(&self, serial: u64) {
        *self.checkpoint.lock().unwrap_or_else(|e| e.into_inner()) = Some((serial, self.gels.checkpoint()));
    }

    fn target(&self) -> Option<[u32; 8]> {
        self.device.lock().ok().map(|d| d.bound_target())
    }

    fn draw_alone(&self, mem: &mut dyn GuestMemory, alone: Alone<'_>) -> bool {
        let result = self.alone(mem, alone);
        let ok = result.is_ok();
        let mut t = self.tally.lock().unwrap_or_else(|e| e.into_inner());
        for msg in t.count(result) { eprintln!("{msg}"); }
        ok
    }
}

#[derive(Default)]
struct LightAdmission(Mutex<Option<LightLayer>>);

struct LightLayer {
    targets: [u32; 8],
    light: PreparedLight,
    draws: HashMap<u32, (SortedItem, Planned)>,
}

impl LightLayer {
    fn new(targets: [u32; 8], mut light: PreparedLight, entries: &[SortedItem]) -> Option<Self> {
        let mut draws = HashMap::with_capacity(entries.len());
        let mut entries = entries.iter();
        for pass in std::mem::take(&mut light.draws) {
            for range in pass.ranges {
                let entry = *entries.next()?;
                draws.insert(entry.slot, (entry, Planned {
                    material: pass.material, layer: pass.layer, implementation: pass.implementation,
                    compiled: Arc::clone(&pass.compiled), ranges: vec![range], model: pass.model.clone(),
                }));
            }
        }
        if entries.next().is_some() {
            return None;
        }
        Some(Self { targets, light, draws })
    }
}

impl d3d::DrawAdmission for LightAdmission {
    fn admit(&self, snapshot: &d3d::DrawSnapshot<'_>) -> Option<(d3d::ExternalPlan, Box<dyn d3d::ExternalPass>)> {
        if render_bridge::native_map_work::Stage::Lights.standing_down() {
            return None;
        }
        let context = crate::native_world_layers_glue::indexed_draw()?;
        let geometry = context.geometry?;
        let draw = snapshot.draw;
        if context.layer.list != render_bridge::native_lights::LIGHT_LIST
            || !matches!(context.layer.number, 12 | 13)
            || draw.entry != "D3DDevice_DrawIndexedVertices"
            || context.index_count? as usize != draw.indices.as_ref()?.len()
            || !draw.clip_rects.is_empty()
        {
            return None;
        }
        let mut pending = self.0.lock().ok()?;
        let run = pending.as_mut()?;
        let (entry, planned) = run.draws.get(&geometry.sorted_slot)?;
        if run.targets[..2] != [snapshot.color_header, snapshot.depth_header]
            || planned.layer != context.layer.number as usize
            || !matches_light_entry(*entry, geometry)
            || !run.light.matches(draw, planned)
            || !crate::native_world_layers_glue::matches_triangle_range(draw, planned.ranges.first()?.len())
        {
            return None;
        }
        let (_, planned) = run.draws.remove(&geometry.sorted_slot)?;
        let light = PreparedLight { draws: vec![planned], ..run.light.clone() };
        let mut plan = d3d::ExternalPlan {
            stage: timeline::native::Stage::Lights, view: context.layer.view, ranges: 1,
            slots: Vec::new(),
        };
        let gel_slot = light.binding.map(|binding| binding.append(&mut plan));
        let pass = RunPass {
            light, view: None, clip: None, gel_slot,
            fallback: Some(Arc::new(draw.clone())),
        };
        Some((plan, Box::new(pass)))
    }
}

fn matches_light_entry(entry: SortedItem, geometry: native_render::world_layers::Geometry) -> bool {
    use native_render::world_layers::Owner as ResolvedOwner;
    let Some(item) = entry.geometry else { return false };
    let owner = match geometry.owner {
        ResolvedOwner::Cluster(section) => Owner::Cluster(section),
        ResolvedOwner::Placement(index) => Owner::Instance(index),
        ResolvedOwner::Object { .. } => return false,
    };
    geometry.variant == 0 && geometry.sorted_slot == entry.slot && geometry.implementation == entry.implementation
        && item.part == (owner, geometry.part) && item.shader == geometry.shader
}

fn uses_gel_target(mem: &dyn GuestMemory, definition: u32, shader: u32, flags: u32, definition_flags: u32) -> bool {
    use native_render::gels;
    shader != u32::MAX && definition != 0
        && flags & engine::light_passes::flag::CONTEXT_ZERO == 0
        && definition_flags & engine::light_passes::DEFINITION_TEXTURE_18 == 0
        && mem.u8(gels::BUILDING).ok() == Some(0)
        && mem.u32(gels::DEFINITION).ok() == Some(definition)
        && definition.checked_add(engine::light_passes::DEFINITION_GEL_SHADER)
            .and_then(|a| mem.u32(a).ok()) == Some(shader)
}

/// The light's constants. A shadowed light's layers 12 and 13 are drawn
/// like any other's: no stencil links them to its layers 10, 11 and 14
/// (`light_passes`), and a pass sampling the shadow maps is refused by
/// `light_look` by name.
fn constants(light: &Light) -> Result<LightConstants, Keep> {
    light.constants.ok_or(Keep::NoConstants)
}

type ClusterParts = HashMap<(Owner, u8), (TagId, WorldDraw)>;

/// Only a prefix can precede the untouched D3D suffix. A later native
/// match must never move ahead of a fallback draw (even the same shader).
fn select_parts(clusters: &ClusterParts, items: &[SortedItem])
    -> Result<Vec<(usize, usize, WorldDraw)>, Keep> {
    let mut selected = Vec::new();
    for entry in items {
        let part = entry.geometry.ok_or("gathered model or unsupported geometry handler").and_then(|item| {
            match clusters.get(&item.part) {
                Some((shader, d)) if shader.0 == item.shader =>
                    Ok((item.layers.trailing_zeros() as usize, usize::from(entry.implementation), *d)),
                Some(_) => Err("gathered shader datum differs from the cluster material"),
                None => Err("gathered part has no native cluster draw"),
            }
        });
        match part {
            Ok(p) => selected.push(p),
            Err(why) => return Err(Keep::Unsupported(why)),
        }
    }
    Ok(selected)
}

#[derive(Default)]
struct RunOrder {
    previous: Option<(u64, u64, u64)>,
}
impl RunOrder {
    fn key(&self, serial: u64, targets: u64, isolated: bool) -> u64 {
        match self.previous {
            Some((last, previous_targets, key))
                if !isolated && targets == previous_targets && last.checked_add(1) == Some(serial) => key,
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

/// The light textures' bitmaps named by the `matg` at `matg` (its payload
/// address), read through `read(address, len)`: `(normalization,
/// attenuation)`, each `None` when null or unreadable.
pub fn light_bitmaps(read: &dyn Fn(u32, usize) -> Option<Vec<u8>>, matg: u32) -> (Option<map::TagId>, Option<map::TagId>) {
    (rasterizer_bitmap(read, matg, NORMALIZATION_DATUM), rasterizer_bitmap(read, matg, ATTENUATION_DATUM))
}

/// The bitmap datum at `offset` of the `matg`'s first rasterizer data
/// element (`[0x00485a80] + offset`), `None` when null or unreadable.
pub fn rasterizer_bitmap(read: &dyn Fn(u32, usize) -> Option<Vec<u8>>, matg: u32, offset: u32) -> Option<map::TagId> {
    let word = |a: u32| -> Option<u32> { Some(u32::from_le_bytes(read(a, 4)?.try_into().ok()?)) };
    let count = word(matg.wrapping_add(RASTERIZER_DATA))?;
    (count > 0).then_some(())?;
    let e = word(matg.wrapping_add(RASTERIZER_DATA + 4))?;
    word(e.wrapping_add(offset)).filter(|&d| d != u32::MAX).map(map::TagId)
}

/// [`light_bitmaps`] over the map's ordinary metadata (`matg` the tag's
/// virtual address, `Tag::location`, which the element's address shares).
pub fn matg_light_bitmaps(meta: &tags::shader::Metadata, matg: u32) -> (Option<map::TagId>, Option<map::TagId>) {
    light_bitmaps(&|addr, len| meta.bytes(addr, len).ok().map(<[u8]>::to_vec), matg)
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
            LayeredImage::cube(top.into_iter().map(|l| (l.face, l.width, l.height, l.rgba)).collect())?
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
            if whole { faces.into_iter().flat_map(|l| l.rgba).collect() } else { Vec::new() }
        })
        .collect();
    Some(image.with_mips(mips))
}

/// What a decoded light texture is, for the load's log line.
fn shape(levels: &[assets::bitmap::Level]) -> String {
    let faces = levels.iter().map(|l| l.face).max().map_or(0, |f| f + 1);
    let top: Vec<_> = levels.iter().filter(|l| l.level == 0).map(|l| (l.width, l.height, l.depth)).collect();
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
    *slot = Some(DecodedLightTexture { desc, bytes, texture: Arc::clone(&texture) });
    Ok(texture)
}

/// [`light_bitmaps`] of the map's `matg`, decoded: `(normalization cube,
/// attenuation volume)`. Says on stderr what loaded and, for what does not,
/// why (the light passes that sample it stay on the D3D path).
pub fn light_textures<R: std::io::Read + std::io::Seek>(
    map: &mut map::Map<R>,
    meta: &tags::shader::Metadata,
    files: &mut assets::ResourceFiles,
) -> (Option<Arc<StaticLightTexture>>, Option<Arc<StaticLightTexture>>) {
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
            let id = id.ok_or_else(|| format!("no datum in matg {matg:#x} +{RASTERIZER_DATA:#x}'s element"))?;
            let tag = tags::DecodedTag::read(map, id).map_err(|e| format!("{id:?} does not read: {e}"))?;
            let tags::TagData::Bitmap(b) = tag.data else {
                return Err(format!("{id:?} is not a bitmap"));
            };
            let image = b.images.first().ok_or_else(|| format!("{id:?} has no image"))?;
            let (desc, bytes) = assets::bitmap::image_bytes(image, files).map_err(|e| format!("{id:?} does not read: {e}"))?;
            let mut cache = TEXTURES.lock().unwrap_or_else(|e| e.into_inner());
            cached_light_texture(&mut cache[slot], desc, bytes, |desc, bytes| {
                let levels = assets::bitmap::decode_bytes(desc, bytes).map_err(|e| format!("{id:?} does not decode: {e}"))?;
                let shape = shape(&levels);
                let image = layered(levels, dim).ok_or_else(|| format!("{id:?} is not a {dim:?} ({shape})"))?;
                if hooks::native_verbose() {
                    eprintln!("native-lights: the {what} loads ({}x{}, {} layers, {} levels)",
                        image.width, image.height, image.layers, 1 + image.mips.len());
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
    (decode(0, n, TextureDim::Cube, "normalization cube"), decode(1, a, TextureDim::D3, "attenuation volume"))
}

/// Installs the host over the world's state (`native_world_glue::install`).
pub fn install<B: d3d::backend::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>, state: State, gels: Arc<Outputs>) {
    let switches = hooks::Switches::from_env().unwrap_or_default();
    if !render_bridge::native_lights::on(&switches) {
        return;
    }
    let admission = Arc::new(LightAdmission::default());
    if let Ok(mut device) = device.lock() {
        device.add_draw_admission(admission.clone());
    }
    render_bridge::native_lights::set_host(Arc::new(Glue {
        device: Arc::clone(device),
        state, gels, checkpoint: Mutex::default(),
        tally: Mutex::new(Tally::default()),
        order: Mutex::default(),
        admission,
        models: models::Loader::default(),
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
    /// A model's part: what it draws over.
    model: Option<ModelDraw>,
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
    fn buffer(&self) -> Result<&native_render::light_models::LightBuffer, Keep> {
        self.lights.models.models.get(&self.model)
            .and_then(|model| model.sections.get(&self.section))
            .and_then(|section| section.buffers.get(self.buffer))
            .ok_or(Keep::Unsupported("model light buffer unavailable"))
    }

    /// Its buffers on `device`, uploaded the first time.
    fn geometry(&self, device: &wgpu::Device) -> Option<ModelGeometry<'_>> {
        let section = self.lights.models.models.get(&self.model)?.sections.get(&self.section)?;
        let buffer = section.buffers.get(self.buffer)?;
        Some(ModelGeometry {
            vertices: buffer.vertices.get(device),
            indices: section.indices.get(device),
            layout: &buffer.layout,
            nodes: &self.rows,
            decompression: section.decompression,
        })
    }
}

/// The lights' part of a loaded world ([`load`]).
pub struct LightsWorld {
    resolver: materials::Resolver,
    materials: HashMap<u16, LitMaterial>,
    clusters: ClusterParts,
    /// Stage images by (stage, bitmap, image) since palettes are per stage.
    images: Arc<HashMap<(usize, TagId, u32), Image>>,
    palettized: HashSet<(usize, TagId, u32)>,
    palettes: StagePalettes,
    /// Gel bitmaps' image 0 by datum.
    gels: HashMap<u32, Image>,
    normalization: Option<Arc<StaticLightTexture>>,
    attenuation: Option<Arc<StaticLightTexture>>,
}

impl LightsWorld {
    /// Upload map textures and start a resolver tied to this device and BSP.
    pub fn upload_engine_textures(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> LightSet {
        let mut set = LightSet::new(device, queue, self.normalization.as_deref(), self.attenuation.as_deref());
        for (&datum, image) in &self.gels {
            set.upload_gel(device, queue, datum, image);
        }
        self.resolver.start(device, queue, set.uniforms_layout(), Arc::clone(&self.images));
        set
    }


    fn resolve_parts(&self, c: &native_render::lights::LightConstants, parts: Vec<(usize, usize, WorldDraw)>, produced_gel: bool)
        -> Result<Vec<Planned>, Keep>
    {
        // Resolved without the colours, which each light's uniforms take
        // (`colour_uniforms`): a light whose colour animates keeps its key.
        let inputs = LightInputs::from_constants(c).without_colours();
        let mut out: Vec<Planned> = Vec::new();
        let mut pending = false;
        for (layer, imp, draw) in parts {
            let m = draw.material;
            let end = draw.first_index.checked_add(draw.index_count).ok_or(Keep::Unsupported("invalid cluster index range"))?;
            let range = draw.first_index..end;
            let lit = self.materials.get(&m).ok_or(Keep::NoMaterial)?;
            let resolved = match self.resolver.world(lit, inputs, layer, imp) {
                Err(Keep::MaterialPending) => {
                    // Request the remaining variants for this layer on the same frame.
                    pending = true;
                    continue;
                }
                other => other?,
            };
            let compiled = resolved.compiled;
            if compiled.program.inputs & !draw.inputs != 0 {
                return Err(Keep::Unsupported("cluster lacks a vertex stream read by its light program"));
            }
            self.binds(&compiled.program, c.gel, produced_gel, &self.images)?;
            if let Some(previous) = out.last_mut()
                && previous.material == m && previous.layer == layer && Arc::ptr_eq(&previous.compiled, &compiled)
            {
                previous.ranges.push(range);
                continue;
            }
            out.push(Planned {
                compiled,
                material: m, layer, implementation: imp, ranges: vec![range], model: None,
            });
        }
        if pending { Err(Keep::MaterialPending) } else { Ok(out) }
    }

    /// A model part's draw for one light ([`models`]): `entry`'s part,
    /// its material resolved with the light's inputs, compiled for the
    /// section's buffer of the program the pass names at the entry's
    /// classification, with the entry's node rows.
    fn resolve_model(&self, c: &LightConstants, lights: &Arc<models::ModelLights>, layer: usize, entry: &LightEntry, rows: &[Row])
        -> Result<Planned, Keep>
    {
        let (section, part) = lights.models.part(entry).ok_or(Keep::Unsupported("scene group entry has no native light stream"))?;
        let (shader, instance) = lights.instances.get(&entry.binding.shader).ok_or(Keep::NoMaterial)?;
        let inputs = LightInputs::from_constants(c).without_colours();
        let imp = usize::from(entry.binding.implementation);
        let resolved = self.resolver.model(*shader, instance, inputs, layer, imp, lights, entry)?;
        let buffer = resolved.buffer.ok_or(Keep::Unsupported("model light buffer unavailable"))?;
        let b = section.buffers.get(buffer).ok_or(Keep::Unsupported("model light buffer unavailable"))?;
        let compiled = resolved.compiled;
        if compiled.program.inputs & !b.provided() != 0 {
            return Err(Keep::Unsupported("model section lacks a vertex stream read by its light program"));
        }
        if rows.is_empty() || rows.len() > 3 * native_render::object_program::MAX_PALETTE {
            return Err(Keep::Unsupported("object entry's node rows do not fit the palette"));
        }
        self.binds(&compiled.program, c.gel, false, &lights.images)?;
        Ok(Planned {
            material: MODEL_MATERIAL,
            layer,
            implementation: imp,
            compiled,
            ranges: vec![part.indices.clone()],
            model: Some(ModelDraw {
                lights: Arc::clone(lights),
                model: entry.model,
                section: entry.stream.section,
                buffer,
                rows: rows.to_vec(),
            }),
        })
    }

    /// Whether every texture `program` samples is loaded (its stage
    /// bitmaps in `images`), with `gel` the light's; the first that is
    /// not, when one is not.
    fn binds(&self, program: &LightProgram, gel: Option<u32>, produced_gel: bool,
        images: &HashMap<(usize, TagId, u32), Image>) -> Result<(), Keep>
    {
        for (stage, (s, d)) in program.stages.iter().zip(&program.dims).enumerate() {
            if d.is_none() {
                continue;
            }
            let (loaded, what) = match *s {
                LightStage::Empty => (false, "empty stage"),
                LightStage::Bitmap { bitmap, image } => (images.contains_key(&(stage, bitmap, image)), "stage bitmap"),
                LightStage::Normalization => (self.normalization.is_some(), "normalization cube"),
                LightStage::Attenuation => (self.attenuation.is_some(), "attenuation volume"),
                LightStage::Gel => (produced_gel || gel.is_some_and(|g| self.gels.contains_key(&g)), "gel"),
            };
            if !loaded {
                return Err(Keep::Unloaded(what));
            }
        }
        Ok(())
    }
}

/// Whether `material` draws anything in layer 12 or 13 (first level of
/// detail).
fn lit_layers(material: &Material) -> bool {
    material
        .layers
        .iter()
        .any(|l| l.lod == 0 && (l.layer == DIFFUSE_LAYER || l.layer == SPECULAR_LAYER) && !l.passes.is_empty())
}

/// At load, before the world drops the materials left to D3D: the lights'
/// part of `world` (`shaders` the BSP's shader per material, `native` the
/// materials the native world draws). `None` when the shader tags do not
/// read. Says on stderr what loaded.
pub fn load<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    files: &mut assets::ResourceFiles,
    shaders: &[TagId],
    world: &World,
    native: &dyn Fn(&WorldDraw) -> bool,
    palettes: &StagePalettes,
) -> Option<LightsWorld> {
    let mut lib = match ShaderLibrary::read(map) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("native-lights: the shader tags do not read ({e}); every light keeps its D3D passes");
            return None;
        }
    };
    let mut materials = HashMap::new();
    let mut microcode = HashMap::new();
    let mut images = HashMap::new();
    let mut palettized = HashSet::new();
    // Each material once, with default inputs: what any light's pass may
    // need that does not depend on the light (programs, bitmaps).
    let native_materials: HashSet<_> = world.draws.iter().filter(|d| native(d)).map(|d| d.material).collect();
    let mut lit_of: HashMap<u16, Option<bool>> = HashMap::new();
    let mut ids: Vec<u16> = world.materials();
    ids.extend(world.transparent_materials());
    ids.sort_unstable();
    ids.dedup();
    for m in ids {
        let Some(&shader) = shaders.get(usize::from(m)) else {
            lit_of.insert(m, None);
            continue;
        };
        let Ok(instance) = lib.shader(shader).map(|s| EngineInputs::for_shader(&s)) else {
            lit_of.insert(m, None);
            continue;
        };
        let Ok(material) = lib.material(shader, None) else {
            lit_of.insert(m, None);
            continue;
        };
        let lit = lit_layers(&material);
        lit_of.insert(m, Some(lit));
        materials.insert(m, LitMaterial { shader, instance });
        if !lit || !native_materials.contains(&m) {
            continue;
        }
        for imp in material
            .layers
            .iter()
            .filter(|l| l.lod == 0 && (l.layer == DIFFUSE_LAYER || l.layer == SPECULAR_LAYER))
            .flat_map(|l| &l.passes)
            .flat_map(|p| &p.implementations)
        {
            if let Some(vs) = imp.vertex_shader
                && !microcode.contains_key(&vs)
                && let Some(code) = look::world_program(map, lib.metadata(), vs)
            {
                microcode.insert(vs, code);
            }
            for (stage, t) in imp.textures.iter().enumerate().take(4) {
                let Some(TextureSource::Bitmap { bitmap, image, .. }) = t.map(|t| t.source) else { continue };
                let key = (stage, bitmap, image);
                if !images.contains_key(&key)
                    && let Some((pixels, expanded)) = light_image(map, files, bitmap, image as usize, stage, &palettes[stage])
                {
                    images.insert(key, pixels);
                    if expanded {
                        palettized.insert(key);
                    }
                }
            }
        }
    }
    // Opaque parts of native materials are drawn; any other surface that
    // has (or may have) lit layers keeps the lights meeting it on D3D.
    let mut clusters = HashMap::new();
    // Why each surface is left to D3D: opaque and drawn by D3D (the world
    // left its material there), opaque with a shader that does not read,
    // transparent with lit layers, transparent with a shader that does not
    // read.
    let mut why = [0usize; 4];
    for d in &world.draws {
        match lit_of.get(&d.material).copied().flatten() {
            Some(true) if native(d) && matches!(d.owner, Owner::Cluster(_)) => {
                clusters.insert((d.owner, d.part), (shaders[usize::from(d.material)], *d));
            }
            Some(false) => {}
            Some(true) => {
                why[0] += 1;
            }
            None => {
                why[1] += 1;
            }
        }
    }
    for t in &world.transparents {
        match lit_of.get(&t.material).copied().flatten() {
            Some(false) => {}
            Some(true) => {
                why[2] += 1;
            }
            None => {
                why[3] += 1;
            }
        }
    }
    let (normalization, attenuation) = light_textures(map, lib.metadata(), files);
    let gels = gels(map, &lib, files);
    eprintln!(
        "native-lights: {} lit parts, {} surfaces left to D3D ({} opaque D3D draws, {} opaque unread shaders, {} transparent lit, {} transparent unread; a whole light containing one stays on D3D), {} vertex programs, {} stage images, {} gels",
        clusters.len(),
        why.iter().sum::<usize>(),
        why[0],
        why[1],
        why[2],
        why[3],
        microcode.len(),
        images.len(),
        gels.len()
    );
    if clusters.is_empty() {
        return None;
    }
    Some(LightsWorld {
        resolver: materials::Resolver::new(lib, microcode),
        materials,
        clusters,
        images: Arc::new(images),
        palettized,
        palettes: palettes.clone(),
        gels,
        normalization,
        attenuation,
    })
}

/// The gels of the map's light definitions (`ligh +0x7c`), image 0 each.
fn gels<R: std::io::Read + std::io::Seek>(map: &mut Map<R>, lib: &ShaderLibrary, files: &mut assets::ResourceFiles) -> HashMap<u32, Image> {
    let datums: Vec<u32> = map
        .tags()
        .iter()
        .filter(|t| t.group.0 == *b"ligh")
        .filter_map(|t| t.location.as_ref().map(|l| l.address))
        .filter_map(|a| {
            let b = lib.metadata().bytes(a.wrapping_add(engine::light_passes::DEFINITION_GEL), 4).ok()?;
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
        let (primitive, (writes_depth, depth_compare)) = native_render::program::pipeline_state(
            Some(&program.state), self.cull.of(program.inherits_cull, true),
        );
        let live_primitive = gfx::translate::primitive_state(r, wgpu::PrimitiveTopology::TriangleList,
            gfx::translate::Capabilities::from_features(wgpu::Features::empty()));
        let depth = r.depth_mode != DepthBufferMode::Disabled;
        let compare = if depth { gfx::translate::compare_function(r.depth_func) } else { wgpu::CompareFunction::Always };
        let alpha = (r.alpha_test_enable && r.alpha_func != CompareFunc::Always)
            .then(|| (gfx::translate::compare_function(r.alpha_func), r.alpha_ref));
        draw.clip.is_some_and(|r| [r.x, r.y, r.width, r.height] == self.scissor)
            && primitive == live_primitive
            && writes_depth == (depth && r.depth_write) && depth_compare == compare
            && program.state.blend == gfx::translate::blend_state(r)
            && (program.state.blend.is_none() || program.state.blend_constant == gfx::translate::blend_constant(r))
            && program.state.color_writes == gfx::translate::color_writes(r.color_write)
            && program.state.alpha_test == alpha
            && !r.fog_enable && !r.stencil.enable && r.fill_mode == FillMode::Solid && r.back_fill_mode == FillMode::Solid
            && (!r.solid_offset_enable || (r.depth_bias == 0.0 && r.depth_bias_slope_scale == 0.0))
    }

    fn draw<'a>(&'a self, p: &'a Planned) -> LightDraw<'a> {
        let c = &self.constants;
        let inputs = LightInputs::from_constants(c);
        LightDraw {
            program: &p.compiled.program,
            gpu: &p.compiled.gpu,
            cull: self.cull.of(p.compiled.program.inherits_cull, true),
            colours: [inputs.colour_a, inputs.colour_b],
            frame: &c.frame,
            gel: c.gel,
            gel_view: None,
            gel_scale: self.binding.map(Binding::tex_scale),
            ranges: &p.ranges,
            model: None,
        }
    }
}

/// Intersect in guest pixels, after the guest projected and rounded its
/// light rectangle. Use half-open edges, including offset viewports.
fn intersect_scissor([x,y,w,h]: [u32;4], (vx,vy,vw,vh): (u32,u32,u32,u32)) -> [u32;4] {
    let (left, top) = (x.max(vx), y.max(vy));
    [left, top, x.saturating_add(w).min(vx.saturating_add(vw)).saturating_sub(left),
        y.saturating_add(h).min(vy.saturating_add(vh)).saturating_sub(top)]
}

fn scale_scissor(rect: [u32;4], guest: (u32,u32), target: (u32,u32)) -> Option<[u32;4]> {
    let scale = d3d::scaled::RenderScale::between(guest, target)?;
    let [x,y,w,h] = rect;
    if w == 0 || h == 0 { return Some([0;4]); }
    if x.checked_add(w)? > guest.0 || y.checked_add(h)? > guest.1 { return None; }
    Some(scale.rect(x, y, w, h))
}

/// One admitted light draw, appended as packets to the D3D pass open on
/// its attachments ([`d3d::ExternalPass::record_draws`]) with the light
/// passes' cached pipelines and texture groups and resident uniforms.
struct RunPass {
    light: PreparedLight,
    view: Option<[[f32; 4]; d3d::VIEW_CONSTANTS]>,
    clip: Option<d3d::DepthClip>,
    gel_slot: Option<usize>,
    /// The handler's resolved draw it replaces; none for a layer drawn
    /// from the scene alone.
    fallback: Option<Arc<d3d::ResolvedDraw>>,
}

impl d3d::ExternalPass for RunPass {
    fn uses_validated_resources(&self) -> bool { true }

    fn writes(&self) -> d3d::ExternalWrites {
        d3d::ExternalWrites {
            color: self.light.draws.iter().any(|d| !d.compiled.program.state.color_writes.is_empty()),
            depth_stencil: self.light.draws.iter().any(|d| d.compiled.program.state.depth_write),
        }
    }

    fn resolved_fallback(&self) -> Option<Arc<d3d::ResolvedDraw>> {
        self.fallback.clone()
    }

    fn record_draws(
        &mut self, t: &d3d::backend::ExternalAttachments, request: d3d::ExternalPassRequest,
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
fn gel_sample(binding: Binding, slots: &[Option<d3d::ExternalColor>], slot: usize) -> Option<&wgpu::TextureView> {
    let surface = slots.get(slot)?.as_ref()?;
    (surface.guest_size == binding.guest_size).then_some(&surface.sample)
}

impl RunPass {
    /// Each copy records on its own: the scissor scaled to its texels, the
    /// gel sampled from its own pinned slot.
    fn draws(
        &self, t: &d3d::backend::ExternalAttachments, request: d3d::ExternalPassRequest,
    ) -> Result<d3d::backend::ExternalDraws, &'static str> {
        let guest_size = request.surface_size;
        let [x, y, width, height] = self.light.scissor;
        let clip = d3d::PixelRect { x, y, width, height }.projected_x(
            request.rect, request.projection_factor(t.copy),
        );
        let scissor = scale_scissor([clip.x, clip.y, clip.width, clip.height], guest_size, t.size)
            .ok_or("unsupported scissor scale")?;
        let constants = self.view.ok_or("missing view constants")?;
        let clip = self.clip.filter(|c| c.w_buffered == Some(true)).ok_or("unknown W-buffer at issue")?;
        let view = ViewInputs { constants: request.view_constants(t.copy, constants), guest_size, size: t.size, z_scale: request.z_scale, w_clip: Some((clip.range, clip.clamp)) };
        let ready = &self.light.ready;
        let mut gpu = ready.gpu();
        let gpu = gpu.as_mut().ok_or("world upload failed")?;
        // Resolve the pinned slot on this copy before replacing the guest draw.
        let sample = match self.light.binding {
            Some(binding) => Some(gel_sample(binding, &t.slots, self.gel_slot.ok_or("missing gel slot")?)
                .ok_or("target 13 has no matching sample view on this copy")?),
            None => None,
        };
        let draws: Vec<_> = self.light.draws.iter().map(|p| {
            let mut draw = self.light.draw(p);
            draw.gel_view = sample;
            if let Some(m) = &p.model {
                draw.model = Some(m.geometry(&t.device).ok_or("model light part unavailable")?);
            }
            Ok(draw)
        }).collect::<Result<_, &'static str>>()?;
        let target = LightTarget {
            device: &t.device, queue: &t.queue, uploads: &t.uploads,
            color: &t.color, color_format: t.color_format,
            depth: &t.depth, depth_format: t.depth_format, size: t.size,
        };
        gpu.prepare_lights(&target, scissor, &view, &draws)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::FlatMemory;

    #[test]
    fn a_light_range_requires_its_original_sorted_slot_and_geometry() {
        use native_render::world_layers::{Geometry, Owner as ResolvedOwner};
        let entry = SortedItem {
            slot: 8,
            handler_item: 16,
            implementation: 2,
            geometry: Some(native_render::light_list::Item {
                part: (Owner::Cluster(4), 3), shader: 0xe000_0001, layers: 1 << 12,
            }),
        };
        let draw = Geometry {
            shader: 0xe000_0001, variant: 0, implementation: 2, sorted_slot: 8,
            record: 5, part: 3, owner: ResolvedOwner::Cluster(4),
        };
        assert!(matches_light_entry(entry, draw));
        for changed in [
            Geometry { sorted_slot: 32, ..draw },
            Geometry { implementation: 1, ..draw },
            Geometry { shader: 0xe000_0002, ..draw },
            Geometry { owner: ResolvedOwner::Placement(4), ..draw },
        ] {
            assert!(!matches_light_entry(entry, changed));
        }
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
        use gfx::{D3DFormat, texture::{TextureDesc, TextureKind}};
        let desc = TextureDesc::new(D3DFormat::Al8, TextureKind::Volume, 2, 2, 2, 1);
        let image = |_: &TextureDesc, bytes: &[u8]| {
            LayeredImage::volume(2, 2, 2, vec![bytes[0]; 32]).ok_or_else(|| "wrong volume shape".into())
        };
        let mut slot = None;
        let a = cached_light_texture(&mut slot, desc.clone(), vec![42], image).unwrap();
        let b = cached_light_texture(&mut slot, desc.clone(), vec![42], |_, _| panic!("decoded twice")).unwrap();
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
        assert_eq!((RASTERIZER_DATA, NORMALIZATION_DATUM, ATTENUATION_DATUM), (0x108, 0xc, 4));
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
        assert_eq!(matg_light_bitmaps(&meta, matg), (Some(TagId(0xe000_0001)), Some(TagId(0xe000_0009))));
        // An object's lightmap stage: the element's +0x64 (0x00018d1f).
        let read = |a: u32, n: usize| meta.bytes(a, n).ok().map(<[u8]>::to_vec);
        assert_eq!(rasterizer_bitmap(&read, matg, 0x64), Some(TagId(0)), "an unwritten word reads as datum 0, as the game reads it");
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
        assert_eq!(rasterizer_bitmap(&read, matg, 0x64), Some(TagId(0xe000_0064)));
        // The tag's file offset is not a metadata address.
        assert_eq!(matg_light_bitmaps(&meta, 0x200), (None, None));
        // A null reference is no bitmap.
        words[6].1 = u32::MAX;
        assert_eq!(matg_light_bitmaps(&metadata(base, 0x1000, &words), matg), (None, Some(TagId(0xe000_0009))));
        // An empty block names nothing, whatever its address says.
        words[0].1 = 0;
        assert_eq!(matg_light_bitmaps(&metadata(base, 0x1000, &words), matg), (None, None));
        // An element outside the metadata does not read.
        words[0].1 = 1;
        words[1].1 = base + 0x2000;
        assert_eq!(matg_light_bitmaps(&metadata(base, 0x1000, &words), matg), (None, None));
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
        assert_eq!((c.dim, c.width, c.height, c.layers, c.rgba.len()), (TextureDim::Cube, 4, 4, 6, 6 * 64));
        for f in 0..6 {
            assert_eq!(c.rgba[f * 64], f as u8 + 1, "face {f} in its layer");
        }
        assert!(layered(cube_levels, TextureDim::D3).is_none(), "a cube is not a volume");

        // A swizzled AL8 volume (an Ay8 volume bitmap's), 4x4x4, three levels.
        let desc = TextureDesc::new(D3DFormat::Al8, TextureKind::Volume, 4, 4, 4, 3);
        let data = vec![0x80u8; desc.layout().unwrap().total_size];
        let levels = assets::bitmap::decode_bytes(&desc, &data).unwrap();
        let shape: Vec<_> = levels.iter().map(|l| (l.face, l.level, l.width, l.depth)).collect();
        assert_eq!(shape, vec![(0, 0, 4, 4), (0, 1, 2, 2), (0, 2, 1, 1)], "face 0 only, each level all its slices");
        let volume_levels = levels.clone();
        let v = layered(levels, TextureDim::D3).expect("a volume");
        assert_eq!((v.dim, v.width, v.height, v.layers, v.rgba.len()), (TextureDim::D3, 4, 4, 4, 4 * 64));
        assert!(v.rgba.iter().all(|&b| b == 0x80), "AL8 decodes to (v, v, v, v)");
        assert!(layered(volume_levels, TextureDim::Cube).is_none(), "a volume is not a cube");
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
            (TextureDesc::new(D3DFormat::X8R8G8B8, TextureKind::Cube, 4, 4, 1, 3), TextureDim::Cube),
            (TextureDesc::new(D3DFormat::Al8, TextureKind::Volume, 4, 4, 4, 3), TextureDim::D3),
        ];
        for (desc, dim) in cases {
            let len = desc.layout().unwrap().total_size;
            // Synthetic texels, all different nearby.
            let data: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            let d3d = gfx::decode_texture(&desc, &data, None, gfx::DecodeOptions::default().with_color_sign(0)).unwrap();
            let native = layered(assets::bitmap::decode_bytes(&desc, &data).unwrap(), dim).expect("the shape");
            assert_eq!(1 + native.mips.len() as u32, desc.levels, "{dim:?}: the whole chain");
            for l in &d3d.levels {
                let bytes = if l.level == 0 { &native.rgba } else { &native.mips[l.level as usize - 1] };
                assert_eq!(native.level_size(l.level), (l.width, l.height, if dim == TextureDim::Cube { 6 } else { l.depth }));
                let layer = (l.width * l.height * l.depth * 4) as usize;
                let at = l.face as usize * layer;
                for (i, px) in l.data.chunks_exact(4).enumerate() {
                    let ours = &bytes[at + 4 * i..at + 4 * i + 4];
                    assert_eq!(sampled(d3d.host, px), [ours[0], ours[1], ours[2], ours[3]], "{dim:?} face {} level {} texel {i}", l.face, l.level);
                }
            }
        }
    }

    #[test]
    fn gathered_parts_match_owner_part_and_full_shader_in_guest_layer_order() {
        let shader = TagId(0x1234_0017);
        let part = |owner, first_index| WorldDraw {
            owner: Owner::Cluster(owner), part: 1, material: 3, lightmap: None,
            first_index, index_count: 3, bounds: native_render::world::Aabb::EMPTY,
            batch: 0, inputs: 1,
        };
        let (a, b) = (part(7, 30), part(8, 6));
        let clusters = ClusterParts::from([((a.owner, 1), (shader, a)), ((b.owner, 1), (shader, b))]);
        let entry = |slot, d: WorldDraw, implementation| SortedItem {
            slot, handler_item: slot + 80, implementation,
            geometry: Some(native_render::light_list::Item { part: (d.owner, d.part), shader: shader.0, layers: 1 << 12 }),
        };
        let mut items = [entry(1, b, 2), entry(2, b, 3), entry(3, a, 1), entry(4, b, 1)];
        assert_eq!(select_parts(&clusters, &items).unwrap(),
            [(DIFFUSE_LAYER, 2, b), (DIFFUSE_LAYER, 3, b), (DIFFUSE_LAYER, 1, a), (DIFFUSE_LAYER, 1, b)]);
        items[2].geometry.as_mut().unwrap().shader ^= 1 << 24;
        assert_eq!(select_parts(&clusters, &items).unwrap_err(),
            Keep::Unsupported("gathered shader datum differs from the cluster material"));
        items[2].geometry.as_mut().unwrap().shader ^= 1 << 24;
        items[0].geometry = None;
        assert!(select_parts(&clusters, &items).is_err(), "a gathered model keeps the whole light on D3D");
    }

    #[test]
    fn light_clip_intersects_guest_viewport_before_scaling() {
        let clip = intersect_scissor([7, 4, 8, 10], (9, 2, 8, 9));
        assert_eq!(clip, [9, 4, 6, 7]);
        assert_eq!(scale_scissor(clip, (20, 15), (60, 45)), Some([27, 12, 18, 21]));
        assert_eq!(scale_scissor(clip, (20, 15), (60, 40)), Some([27, 11, 18, 18]));
        let empty = intersect_scissor([17, 0, 2, 9], (9, 2, 8, 9));
        assert_eq!(scale_scissor(empty, (20, 15), (40, 30)), Some([0; 4]));
    }

    #[test]
    fn a_run_requires_adjacent_whole_lights_with_the_same_target_generation() {
        let order = RunOrder { previous: Some((5, 9, 3)) };
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
            vec!["native-lights: a light keeps its D3D passes: samples an unloaded normalization cube".to_string()]
        );
        assert!(t.count(Err(Keep::Unloaded("normalization cube"))).is_empty(), "once per reason");
        assert_eq!(
            t.count(Err(Keep::Unsupported("gathered model"))),
            vec!["native-lights: a light keeps its D3D passes: gathered model".to_string()]
        );
        assert!(t.count(Ok(())).is_empty());
        assert_eq!(
            t.summary(),
            "native-lights: host planned 1 of 4 lights; kept on D3D: 2 samples an unloaded normalization cube, 1 gathered model"
        );
        for _ in 5..SUMMARY_EVERY {
            assert!(t.count(Ok(())).is_empty());
        }
        let said = t.count(Ok(()));
        assert_eq!(t.asked, SUMMARY_EVERY);
        assert_eq!(said, vec![t.summary()], "the counts every SUMMARY_EVERY lights");
        assert_eq!(Tally::default().summary(), "native-lights: host planned 0 of 0 lights; kept on D3D: none");
    }

    #[test]
    fn cube_and_volume_shapes_from_decoded_levels() {
        let faces: Vec<_> = (0..6).flat_map(|f| [level(f, 0, 1), level(f, 1, 1)]).collect();
        let c = layered(faces, TextureDim::Cube).expect("six faces at level 0");
        assert_eq!((c.layers, c.rgba.len()), (6, 96));
        assert!(layered(vec![level(0, 0, 4)], TextureDim::Cube).is_none());
        let v = layered(vec![level(0, 0, 4), level(0, 1, 2)], TextureDim::D3).expect("level 0's slices");
        assert_eq!((v.layers, v.rgba.len()), (4, 64));
        assert!(layered(vec![level(0, 1, 4)], TextureDim::D3).is_none(), "no level 0");
    }

    #[test]
    fn target_13_requires_the_current_light_and_engine_texture_10_selector() {
        use native_render::gels;
        let mut mem = FlatMemory::new(gels::DEFINITION, 0x1000);
        let definition = gels::DEFINITION + 0x400;
        let shader = 0xabcd_0005;
        mem.set_u32(gels::DEFINITION, definition).unwrap();
        mem.set_u32(definition + engine::light_passes::DEFINITION_GEL_SHADER, shader).unwrap();
        assert!(uses_gel_target(&mem, definition, shader, 0, 0));
        assert!(!uses_gel_target(&mem, definition, shader, engine::light_passes::flag::CONTEXT_ZERO, 0));
        assert!(!uses_gel_target(&mem, definition, shader, 0, engine::light_passes::DEFINITION_TEXTURE_18));
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
        assert!(constants(&light(Some(LightConstants { flags: 4 | 0x20, ..c }))).is_ok(), "shadowed");
        assert!(constants(&light(Some(LightConstants { animated_gel: true, ..c }))).unwrap().animated_gel);
        assert!(matches!(constants(&light(None)), Err(Keep::NoConstants)));
    }

    #[test]
    fn the_device_sees_the_hooks_memory_and_its_faults() {
        let mut m = FlatMemory::new(0x1000, 0x100);
        let mut dm = DeviceMemory(&mut m);
        d3d::GuestMemory::write_u32(&mut dm, 0x1010, 0xdead_beef).unwrap();
        assert_eq!(d3d::GuestMemory::read_u32(&dm, 0x1010).unwrap(), 0xdead_beef);
        let e = d3d::GuestMemory::read_u32(&dm, 0x10fe).unwrap_err();
        assert_eq!((e.addr, e.len), (0x10fe, 4));
        assert!(d3d::GuestMemory::write(&mut dm, 0x0ffc, &[0; 4]).is_err());
        assert_eq!(m.u32(0x1010).unwrap(), 0xdead_beef);
    }
}
