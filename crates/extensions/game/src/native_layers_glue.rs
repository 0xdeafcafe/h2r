//! The host of `frame-scene.layers` (docs/graphics/framescene.md, slice 4):
//! the scene's world layers 1, 5, 7, 8 and 20, and the first person's
//! layers 1, 7 and 8, drawn from the scene's plan
//! (`native_render::layer_pass`, `LitObjects::plan_layer_item`) over the
//! native world's loaded BSP and the object models layer 3 draws from,
//! not from the draws the guest resolves under the layer call.
//!
//! - **At load** ([`load`], `native_render::layers_load`, on the world's
//!   worker, with the fog's): each
//!   world material's passes of each layer compiled with their own
//!   programs (`fog_gpu::layer_passes` and `FogProgram`, the same
//!   translation the fog's layer 16 takes), all or nothing per material
//!   and layer, their stage images decoded; the parts that can draw them;
//!   and each material resolved once, with its instance's animated
//!   properties and input scales, for the plan. Object models compile
//!   their materials' passes of layers 1, 5, 7, 8 and 20 with their
//!   layer-3 passes, over the same buffers (`object_program_gpu::lit_model`).
//! - **Per layer call** ([`Glue::ask`], before the layer draw of a scene
//!   view or of the first person): the bridge extracts the layer's pass
//!   from guest memory (`render_bridge::native_layers::extract`); the plan
//!   takes each item or names why not. A structure item (a cluster's or
//!   a placement's, the scene's caller only) is drawn when its draw is
//!   static (the material as resolved at the load, no animation: what the
//!   compiled programs hold) and the layer's set draws its part with its
//!   shader. An object item is drawn when extraction names no cause
//!   (`frame_scene::LayerObjectRefusal`), the draw adds no PRT stream
//!   for it (layer 5, `frame_scene::PrtStream`: `object_prt`), every
//!   state its passes set and restore already holds
//!   (`layer_pass::skip_holds`: `object_state`; `state` for a structure
//!   item), its model is resident in the
//!   caller's set (the scene's, `native_objects_glue::layer_models`, or
//!   the first person's, `native_first_person_glue::layer_models`), and
//!   each of its material's passes in the layer plans
//!   (`LitObjects::plan_layer_item`) with its fog, sampling no texture
//!   accumulation target. The skip clears the layer's bit on the run of
//!   such items from the list's first (`layer_pass::clear_layer_prefix`);
//!   the rest, and every item after the first refused one, keep their D3D
//!   draws. One external pass records them, in the list's order.
//! - **The stencil** (`layer_pass::item_stencil`): in layer 1 the
//!   geometry draw's prologue sets stencil mode 0 when the stencil-off
//!   flags are set and mode 2 for a marked object's record, and its
//!   epilogue leaves mode 1. So with the stencil off the items draw with
//!   the stencil disabled, and with the stencil off or a marked object
//!   item taken the device must already hold mode 1; else the layer, or
//!   the marked items, stay on the D3D path. Layers 5, 7, 8 and 20 draw
//!   under the device's stencil.
//! - **The pass** ([`LayerViewPass`]): the device's view constants and W
//!   clip at the issue, then per run of the list `GpuWorld::layer_draws`
//!   or `LitObjects::draws` in the D3D pass open on the bound
//!   attachments.
//!
//! Each ask's outcomes are counted by layer and cause
//! (`render_bridge::native_layers::count_for`) for `render.scene`.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use frame_scene::{FrameScene, LayerItem, LayerOwner, LayerPass, PartFlags};
use native_render::layer_pass::{self, PrefixItem};
use native_render::object_program_gpu::{LitObjects, OBJECT_LAYERS, PlannedDraw};
use native_render::program::ViewInputs;
use native_render::world::Owner;
use render_bridge::native_layers::{Host, LayerCaller, count_for};

use crate::native_fog_glue::{device_fog_words, fog_of};
use crate::native_world_glue::{ReadMemory, Ready, State, current_ready, palettes_hold};

// The load moved to native-render, so the editor loads the same layers
// (docs/editor/editor.md).
pub use native_render::layers_load::{LayerData, LayersView, load, upload_layers};
// The plan's view of the load's shaders and the refusal names, shared with
// the editor's game-free plan.
use native_render::layers_load::{Resolved as Loaded, refusal_name as cause};

/// The object models a layer's object items draw from: the scene's
/// (`native_objects_glue`) or the first person's
/// (`native_first_person_glue`), each the set its layer 3 draws from.
pub(crate) trait LayerModels: Send + Sync {
    fn lit(&self) -> &Mutex<Option<LitObjects>>;
    /// Whether model `model` (a render record's `+4`) is uploaded and
    /// current. Called without [`Self::lit`] held.
    fn ready(&self, model: u32) -> bool;
}

/// An object item's planned draws: its shader datum (which the skip
/// matches), whether its record marks the stencil, and per pass of its
/// material in the layer the draw with its model.
struct ObjectItem {
    shader: u32,
    marked: bool,
    draws: Vec<(u32, PlannedDraw)>,
}

struct Glue<B: d3d::backend::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
    state: State,
}

impl<B: d3d::backend::Backend + Send + 'static> Host for Glue<B> {
    fn ask(&self, mem: &mut dyn GuestMemory, layer: u32, caller: LayerCaller, scene: &FrameScene) -> Option<native_render::skip::Cleared> {
        let l = u8::try_from(layer).ok()?;
        let pass = scene.layer_passes.first()?;
        // The world's layer sets: the scene's caller only.
        let ready = match caller {
            LayerCaller::Scene => {
                let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                Some(Arc::clone(current_ready(&state, mem)?))
            }
            LayerCaller::FirstPerson => None,
        };
        let world = ready.as_deref().and_then(|r| r.layers.get());
        let data = world.and_then(|(view, _)| view.layers.get(&l));
        let mut outcomes: BTreeMap<&'static str, u64> = BTreeMap::new();
        let object_items: Vec<&LayerItem> =
            scene.items_of(pass).iter().filter(|i| matches!(i.owner, LayerOwner::Object(_))).collect();
        let is_object = |slot: u32| object_items.iter().any(|i| i.slot == slot);
        // The plan of the structure's items, from the scene alone.
        let mut taken: HashMap<u32, u32> = HashMap::new();
        let plan = world.map(|(view, _)| layer_pass::plan(scene, pass, &mut Loaded(view)));
        match &plan {
            Some(plan) => {
                for (slot, r) in &plan.refused {
                    if !is_object(*slot) {
                        *outcomes.entry(cause(r)).or_default() += 1;
                    }
                }
                for d in &plan.draws {
                    if layer_pass::world_owner(d.owner).is_none() {
                        continue; // an object's: planned below
                    }
                    if !layer_pass::is_static(d) {
                        *outcomes.entry("animated").or_default() += 1;
                    } else {
                        taken.insert(d.slot, d.shader);
                    }
                }
            }
            None => {
                let structure = scene.items_of(pass).iter().filter(|i| !matches!(i.owner, LayerOwner::Object(_))).count();
                let why = if caller == LayerCaller::FirstPerson { "first_person_structure" } else { "program" };
                *outcomes.entry(why).or_default() += structure as u64;
            }
        }
        let report = |outcomes: &BTreeMap<&'static str, u64>, drawn: u64| {
            for (why, &n) in outcomes {
                count_for(caller, layer, &format!("refused_{why}"), n);
            }
            count_for(caller, layer, "taken", drawn);
        };
        if data.is_none_or(|d| d.programs.is_empty()) {
            *outcomes.entry("program").or_default() += taken.len() as u64;
            taken.clear();
        }
        // Extraction's causes, and layer 5's PRT stream, before anything
        // is asked of the device.
        let mut objects: Vec<&LayerItem> = Vec::with_capacity(object_items.len());
        for &item in &object_items {
            if let Some(why) = item.refusal {
                *outcomes.entry(why.name()).or_default() += 1;
            } else if item.prt.is_some() {
                *outcomes.entry("object_prt").or_default() += 1;
            } else if !OBJECT_LAYERS.contains(&l) {
                *outcomes.entry("object_layer").or_default() += 1;
            } else {
                objects.push(item);
            }
        }
        if taken.is_empty() && objects.is_empty() {
            report(&outcomes, 0);
            return None;
        }
        let refuse_all = |outcomes: &mut BTreeMap<&'static str, u64>, why: &'static str, world: usize, objects: usize| {
            *outcomes.entry(why).or_default() += (world + objects) as u64;
        };
        let Ok(mut d) = self.device.lock() else { return None };
        if !d.can_external_pass() || !matches!(d.begin_external_boundary(&mut crate::native_lights_glue::DeviceMemory(mem)), Ok(true)) {
            refuse_all(&mut outcomes, "device", taken.len(), objects.len());
            report(&outcomes, 0);
            return None;
        }
        let read = ReadMemory(mem);
        // The device's stencil at the layer call, which the items draw
        // under (`layer_pass::item_stencil`); one that does not read
        // leaves the layer to the D3D path.
        let Some(device_stencil) = d.stencil_state(&read) else {
            refuse_all(&mut outcomes, "stencil", taken.len(), objects.len());
            report(&outcomes, 0);
            return None;
        };
        // The world's sets draw with the palettes the load expanded
        // through.
        if !taken.is_empty() && !world.is_some_and(|(_, palettes)| palettes_hold(palettes, &|n| d.stage_palette(&read, n))) {
            *outcomes.entry("palettes").or_default() += taken.len() as u64;
            taken.clear();
        }
        // Skipping an item's draws must leave the device as drawing them
        // would: every state its passes set and restore already holds
        // (`layer_pass::skip_holds`, as layer 3's walk requires).
        if let Some(plan) = &plan {
            let device_state = |slot: u32| d.render_state(&read, slot);
            for draw in &plan.draws {
                if taken.contains_key(&draw.slot)
                    && !draw.passes.iter().all(|p| draw.implementation(p).is_some_and(|imp| layer_pass::skip_holds(imp, &device_state)))
                {
                    taken.remove(&draw.slot);
                    *outcomes.entry("state").or_default() += 1;
                }
            }
        }
        let planned = if objects.is_empty() { None } else { plan_objects(&d, mem, l, caller, scene, pass, &objects, &mut outcomes) };
        let (models, mut object_draws) = match planned {
            Some((models, draws)) => (Some(models), draws),
            None => (None, HashMap::new()),
        };
        let marked = object_draws.values().any(|o| o.marked);
        let mut stencil = layer_pass::item_stencil(l, pass.stencil_off, device_stencil, marked);
        if stencil.needs_mode_1 && !crate::native_objects_glue::in_stencil_mode_1(&d, mem) {
            if pass.stencil_off {
                // Every item's draw leaves mode 1: skipping them is
                // state-neutral only from mode 1.
                refuse_all(&mut outcomes, "stencil", taken.len(), object_draws.len());
                report(&outcomes, 0);
                return None;
            }
            let before = object_draws.len();
            object_draws.retain(|_, o| !o.marked);
            *outcomes.entry("object_stencil").or_default() += (before - object_draws.len()) as u64;
            stencil = layer_pass::item_stencil(l, pass.stencil_off, device_stencil, false);
        }
        let stencil = stencil.stencil;
        // Pipelines warmed before any bit is cleared: a set not warm yet
        // (or that the bound depth cannot hold) leaves its items to D3D.
        if !taken.is_empty() {
            let warm = ready.as_ref().and_then(|ready| {
                ready.with_pipeline_target(&d, mem, |gpu, device, color, depth| {
                    if native_render::fog_gpu::layer_stencil(&stencil, depth).is_none() {
                        return Some(Err(()));
                    }
                    Some(Ok(gpu.prewarm_layer(l, device, color, depth, &stencil)))
                })
            });
            match warm {
                Some(Ok(true)) => {}
                Some(Err(())) => {
                    *outcomes.entry("stencil").or_default() += taken.len() as u64;
                    taken.clear();
                }
                _ => {
                    *outcomes.entry("warming").or_default() += taken.len() as u64;
                    taken.clear();
                }
            }
        }
        if let Some(models) = models.as_ref().filter(|_| !object_draws.is_empty()) {
            let flat: Vec<(u32, PlannedDraw)> = object_draws.values().flat_map(|o| o.draws.iter().cloned()).collect();
            let warm = d.external_pipeline_target(&ReadMemory(mem)).is_some_and(|(device, color, depth)| {
                models.lit().try_lock().ok().and_then(|mut lit| lit.as_mut().map(|lit| lit.prewarm_draws(&device, color, depth, &flat))) == Some(true)
            });
            if !warm {
                *outcomes.entry("object_warming").or_default() += object_draws.len() as u64;
                object_draws.clear();
            }
        }
        if taken.is_empty() && object_draws.is_empty() {
            report(&outcomes, 0);
            return None;
        }
        let device_words = device_fog_words(&d, mem);
        let mut fogs = HashMap::new();
        if let Some(data) = data {
            for (&material, words) in &data.words {
                if let Some(resolved) = words.resolve(&device_words) {
                    fogs.insert(material, fog_of(&resolved));
                }
            }
        }
        let read = ReadMemory(mem);
        let mut preview = native_render::skip::MaskPreview::new(&read)?;
        let candidates = taken.len() + object_draws.len();
        let (drawn, cleared) = layer_pass::clear_layer_prefix(&mut preview, layer, &|item| match item {
            PrefixItem::World(item) => data.is_some_and(|data| {
                taken.get(&item.slot) == Some(&item.shader)
                    && data.part_set.get(&(item.owner, item.part)) == Some(&item.shader)
                    && data.parts.iter().any(|&(k, (_, _, _, m))| k == (item.owner, item.part) && fogs.contains_key(&m))
            }),
            PrefixItem::Object { slot, shader } => object_draws.get(&slot).is_some_and(|o| o.shader == shader),
        })?;
        let masks = preview.finish()?;
        // The list's runs, in order, and the ranges they promise: one per
        // structure item, one per object pass draw.
        let mut runs: Vec<LayerRun> = Vec::new();
        let mut ranges = 0;
        for item in &drawn {
            match *item {
                PrefixItem::World(i) => {
                    ranges += 1;
                    match runs.last_mut() {
                        Some(LayerRun::World(parts)) => parts.push((i.owner, i.part)),
                        _ => runs.push(LayerRun::World(vec![(i.owner, i.part)])),
                    }
                }
                PrefixItem::Object { slot, .. } => {
                    let o = object_draws.remove(&slot)?;
                    ranges += o.draws.len();
                    match runs.last_mut() {
                        Some(LayerRun::Objects(draws)) => draws.extend(o.draws),
                        _ => runs.push(LayerRun::Objects(o.draws)),
                    }
                }
            }
        }
        // What the plan took that the skip did not: from the first item
        // the sets cannot draw (no compiled program, part or fog), in the
        // list's order.
        *outcomes.entry("order").or_default() += (candidates - drawn.len()) as u64;
        report(&outcomes, drawn.len() as u64);
        if ranges == 0 {
            return None;
        }
        let view_index = mem.u32(engine::render_loop::occlusion::guest::VIEW_INDEX).unwrap_or(u32::MAX);
        let stage = match caller {
            LayerCaller::Scene => timeline::native::Stage::World,
            LayerCaller::FirstPerson => timeline::native::Stage::FirstPerson,
        };
        let plan = d3d::ExternalPlan { stage, view: view_index, ranges, slots: Vec::new() };
        let pass = Box::new(LayerViewPass {
            ready: ready.clone(),
            objects: models,
            layer: l,
            runs,
            fogs,
            view: None,
            w_clip: None,
            writes_depth: data.is_some_and(|d| d.writes_depth),
            stencil,
            run: u64::from(d.swap_count()).wrapping_add(1),
        });
        let reservation = d
            .prepare_external_bound(&mut crate::native_lights_glue::DeviceMemory(mem), plan, pass, Vec::new())
            .ok()??;
        if let Some(ready) = &ready {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !current_ready(&state, mem).is_some_and(|current| Arc::ptr_eq(current, ready)) {
                return None;
            }
        }
        if !masks.apply(&mut render_bridge::native_fog::Mem(mem)) {
            return None;
        }
        reservation.commit();
        Some(cleared)
    }
}

/// Plans a layer's object items (`items`, extraction naming no cause)
/// from `caller`'s models: per item its draws, or a cause counted in
/// `outcomes`. `None`, every item counted, when the models, the device's
/// viewport or its W mode leave them all to the D3D path.
#[allow(clippy::too_many_arguments)]
fn plan_objects<B: d3d::backend::Backend>(
    d: &d3d::Device<B>, mem: &dyn GuestMemory, layer: u8, caller: LayerCaller, scene: &FrameScene, pass: &LayerPass,
    items: &[&LayerItem], outcomes: &mut BTreeMap<&'static str, u64>,
) -> Option<(Arc<dyn LayerModels>, HashMap<u32, ObjectItem>)> {
    let mut all = |why: &'static str| {
        *outcomes.entry(why).or_default() += items.len() as u64;
        None
    };
    let models = match caller {
        LayerCaller::Scene => crate::native_objects_glue::layer_models(d, mem),
        LayerCaller::FirstPerson => crate::native_first_person_glue::layer_models(d, mem),
    };
    let Some(models) = models else { return all("object_models") };
    // The objects' programs draw over the whole target, as layer 3's.
    let read = ReadMemory(mem);
    let whole = d.external_ready(&read, false).ok().is_some_and(|request| {
        let (width, height) = request.surface_size;
        d.clip_area(&read).ok().flatten().is_some_and(|(clip, rects)| {
            rects.is_empty() && clip.is_whole(width, height) && request.rect == (0, 0, width, height)
        })
    });
    if !whole {
        return all("object_viewport");
    }
    let (entry, w_buffered) = crate::native_objects_glue::entry_states(d, mem);
    if !w_buffered {
        return all("object_w_buffer");
    }
    let fog_words = device_fog_words(d, mem);
    let specular = crate::native_objects_glue::device_specular(d, mem);
    let device_state = |slot: u32| d.render_state(&read, slot);
    let ready: Vec<bool> = items.iter().map(|i| models.ready(i.model)).collect();
    let Ok(guard) = models.lit().try_lock() else { return all("object_models") };
    let Some(lit) = guard.as_ref() else { return all("object_models") };
    let mut out = HashMap::with_capacity(items.len());
    for (item, ready) in items.iter().zip(ready) {
        if !ready || !lit.has(item.model) {
            *outcomes.entry("object_model").or_default() += 1;
            continue;
        }
        let draws = match lit.plan_layer_item(item.model, layer, scene, pass, item, &entry) {
            Ok(draws) => draws,
            Err(why) => {
                count_refusal(&why);
                *outcomes.entry("object_plan").or_default() += 1;
                continue;
            }
        };
        if draws.iter().any(|d| d.texaccum) {
            *outcomes.entry("object_texaccum").or_default() += 1;
            continue;
        }
        if !draws.iter().all(|draw| lit.planned_pass(item.model, draw).is_some_and(|p| layer_pass::skip_holds(&p.implementation, &device_state))) {
            *outcomes.entry("object_state").or_default() += 1;
            continue;
        }
        let passes = draws.len();
        let mut with_fog = Vec::with_capacity(passes);
        for mut draw in draws {
            if crate::native_objects_glue::layer_draw_fog(lit, item.model, &mut draw, &fog_words, specular).is_none() {
                break;
            }
            with_fog.push((item.model, draw));
        }
        if with_fog.len() != passes {
            *outcomes.entry("object_fog").or_default() += 1;
            continue;
        }
        let marked = item.flags.contains(PartFlags::STENCIL_MARK) && !pass.stencil_off;
        out.insert(item.slot, ObjectItem { shader: item.key.shader, marked, draws: with_fog });
    }
    drop(guard);
    Some((models, out))
}

/// Object items the plan gave back to the D3D path, the first of each
/// reason logged.
static REASONS: Mutex<Option<std::collections::HashSet<String>>> = Mutex::new(None);

fn count_refusal(why: &str) {
    let mut seen = REASONS.lock().unwrap_or_else(|e| e.into_inner());
    let seen = seen.get_or_insert_with(Default::default);
    if seen.len() < 64 && seen.insert(why.to_owned()) {
        eprintln!("native-layers: an object item keeps its D3D draw: {why} (each reason logged once)");
    }
}

/// Installs the host over the world's state (`native_world_glue::install`).
pub fn install<B: d3d::backend::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>, state: State) {
    render_bridge::native_layers::set_host(Arc::new(Glue { device: Arc::clone(device), state }));
}

/// A run of the list's items the masks surrendered.
enum LayerRun {
    /// Structure parts, drawn by the world's layer set.
    World(Vec<(Owner, u8)>),
    /// Object pass draws, each with its model.
    Objects(Vec<(u32, PlannedDraw)>),
}

/// One view's replacement of one layer.
struct LayerViewPass {
    /// The world (the scene's caller).
    ready: Option<Arc<Ready>>,
    /// The object models the object runs draw from.
    objects: Option<Arc<dyn LayerModels>>,
    layer: u8,
    /// The surrendered items' runs, in the list's order.
    runs: Vec<LayerRun>,
    /// Each material's fog unit and colour, the device's at the ask under
    /// the material's own words.
    fogs: HashMap<u16, (shaders::psh::fog::Fog, [f32; 4])>,
    view: Option<[[f32; 4]; d3d::VIEW_CONSTANTS]>,
    w_clip: Option<([f32; 2], bool)>,
    writes_depth: bool,
    /// The stencil the structure's draws take (`layer_pass::item_stencil`,
    /// `fog_gpu::layer_stencil`).
    stencil: gfx::state::StencilState,
    run: u64,
}

impl LayerViewPass {
    fn world_runs(&self) -> impl Iterator<Item = &[(Owner, u8)]> {
        self.runs.iter().filter_map(|r| match r {
            LayerRun::World(parts) => Some(parts.as_slice()),
            LayerRun::Objects(_) => None,
        })
    }

    fn object_draws(&self) -> impl Iterator<Item = &PlannedDraw> {
        self.runs.iter().flat_map(|r| match r {
            LayerRun::Objects(draws) => draws.as_slice(),
            LayerRun::World(_) => &[][..],
        }).map(|(_, d)| d)
    }
}

impl d3d::ExternalPass for LayerViewPass {
    fn batch_key(&self) -> Option<u64> {
        Some(self.run)
    }

    fn view_constants(&mut self, constants: &[[f32; 4]; d3d::VIEW_CONSTANTS]) {
        self.view = Some(*constants);
    }

    fn depth_clip(&mut self, clip: d3d::DepthClip) {
        self.w_clip = (clip.w_buffered == Some(true)).then_some((clip.range, clip.clamp));
    }

    fn prepare_copy(&mut self, _t: &d3d::ExternalTarget<'_>) -> Result<(), &'static str> {
        if self.view.is_none() || self.w_clip.is_none() {
            return Err("layer view constants or W-buffer clip unavailable");
        }
        if self.world_runs().next().is_some() {
            let ready = self.ready.as_ref().ok_or("world geometry unavailable")?;
            let mut guard = ready.gpu();
            let gpu = guard.as_mut().ok_or("world geometry unavailable")?;
            let set = gpu.layer(self.layer).ok_or("layer materials unavailable")?;
            if self.world_runs().flatten().any(|&(owner, part)| !set.draws_part(owner, part)) {
                return Err("a selected layer part did not bind");
            }
        }
        Ok(())
    }

    fn writes(&self) -> d3d::ExternalWrites {
        let world = self.world_runs().next().is_some()
            && (self.writes_depth || native_render::fog_gpu::stencil_writes(&self.stencil));
        d3d::ExternalWrites {
            color: true,
            depth_stencil: world || crate::native_objects_glue::draws_write_depth_stencil(self.object_draws()),
        }
    }

    /// The layer's draws join the D3D render pass already open on these
    /// attachments, run by run in the list's order. A refusal here leaves
    /// the D3D draws.
    fn record_draws(
        &mut self, t: &d3d::backend::ExternalAttachments, request: d3d::ExternalPassRequest,
    ) -> Option<Result<d3d::backend::ExternalDraws, &'static str>> {
        Some(self.record(t, request))
    }
}

impl LayerViewPass {
    fn record(
        &self, t: &d3d::backend::ExternalAttachments, request: d3d::ExternalPassRequest,
    ) -> Result<d3d::backend::ExternalDraws, &'static str> {
        let constants = self.view.ok_or("layer view constants unavailable")?;
        if self.w_clip.is_none() {
            return Err("layer W-buffer clip unavailable");
        }
        let view = ViewInputs {
            constants: request.view_constants(t.copy, constants),
            guest_size: request.surface_size,
            size: t.size,
            z_scale: request.z_scale,
            w_clip: self.w_clip,
        };
        let mut out = d3d::backend::ExternalDraws { ranges: 0, packets: Vec::new(), discards: false, writes: Vec::new() };
        for run in &self.runs {
            let draws = match run {
                LayerRun::World(selected) => {
                    let ready = self.ready.as_ref().ok_or("world geometry unavailable")?;
                    let mut guard = ready.gpu();
                    let gpu = guard.as_mut().ok_or("world geometry unavailable")?;
                    gpu.layer_draws(self.layer, &t.device, t.color_format, t.depth_format, t.size, selected, &view, &self.fogs, &self.stencil)?
                }
                LayerRun::Objects(draws) => {
                    let models = self.objects.as_ref().ok_or("layer object geometry unavailable")?;
                    let mut lit = models.lit().lock().unwrap_or_else(|e| e.into_inner());
                    let lit = lit.as_mut().ok_or("layer object geometry unavailable")?;
                    if draws.iter().any(|(model, d)| !lit.has_draw_assets(*model, d) || d.indices.is_empty()) {
                        return Err("layer object geometry or material unavailable");
                    }
                    lit.draws(
                        &t.device, &t.queue, &t.uploads, t.color_format, t.depth_format, t.size, &view,
                        request.projection_factor(t.copy), draws, None,
                    )?
                }
            };
            out.ranges += draws.ranges;
            out.packets.extend(draws.packets);
            out.discards |= draws.discards;
            out.writes.extend(draws.writes);
        }
        Ok(out)
    }
}
