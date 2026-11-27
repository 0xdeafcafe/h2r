//! The host of `frame-scene.layers` (docs/graphics/framescene.md, slice 4):
//! the scene's world layers 1, 5, 7, 8, 20, 2 and 17, the alternate view's layers 1, 5 and 2, the first person's
//! layers 1, 7, 8, 20 and 2, and the sorted handlers' layers 18 and 19
//! (camouflage) and 21 (hologram) (docs/graphics/native-water-fog.md),
//! drawn from the scene's plan
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
//!   their materials' passes of layers 1, 2, 5, 7, 8, 17 and 20 with their
//!   layer-3 passes, over the same buffers (`object_program_gpu::lit_model`).
//! - **Per layer call** ([`Glue::ask`], before the layer draw of a scene
//!   view or of the first person): the bridge extracts the layer's pass
//!   from guest memory (`render_bridge::native_layers::extract`); the plan
//!   takes each item or names why not. A structure item (a cluster's or
//!   a placement's, the main and alternate scene callers) draws when the set holds its
//!   shader and part and the plan supplies its constants. Game-time
//!   animation updates the compiled passes' uniforms at the layer call.
//!   An object item is drawn when extraction names no cause
//!   (`frame_scene::LayerObjectRefusal`), a PRT stream the draw adds for
//!   it (layer 5, `frame_scene::PrtStream`) reads from guest memory in
//!   the format its pass's PRT variant was compiled for
//!   (`LitObjects::fill_prt`; else `object_prt`), every
//!   state its passes restore already holds and every state they set
//!   already holds or is restored (`layer_pass::skip_holds`:
//!   `object_state`; `state` for a structure item), its model is
//!   resident in the
//!   caller's set (the scene's, `native_objects_glue::layer_models`, or
//!   the first person's, `native_first_person_glue::layer_models`), and
//!   each of its material's passes in the layer plans
//!   (`LitObjects::plan_layer_item`) with its fog. A pass sampling the
//!   texture accumulation target (engine texture 3) binds its level as
//!   layer 3's objects do (`native_objects_glue::sampled_texaccum`), and
//!   not in layer 1, which draws into it (else `object_texaccum`). An
//!   item toggling the auxiliary state (record bit 11) draws with the
//!   rows and clip its view holds (`frame_scene::View::aux_depth`), and
//!   only when the device holds the toggle off
//!   (`native_objects_glue::aux_depth_at_rest`; else `object_aux_depth`).
//!   Under the device's polygon stipple every item draws masked by its
//!   record's pattern: a faded record's (`frame_scene::PartFlags::FADE`)
//!   its byte's, any other's the device's
//!   (`native_objects_glue::record_stipple`; else `object_fade`). An item
//!   with a fade byte draws only where the device holds the rest pattern
//!   and the engine a cached fade of one, which its epilogue leaves
//!   (`native_objects_glue::stipple_at_rest`; else `object_stipple`).
//!   An item whose shader's prologue uploads the structure's parameter
//!   block (`frame_scene::PartFlags::STRUCTURE_PARAMETERS`) draws with
//!   its view's upload, and only where the device and the engine already
//!   hold it (`native_objects_glue::structure_at_rest`; else
//!   `object_shader_flag`). A flagged shader's item before the frame's
//!   structure target copy (`frame_scene::PartFlags::TARGET_UNCOPIED`)
//!   draws unless a pass's program has flag bit 20, whose draw would make
//!   the copy (`object_program_gpu::makes_target_copy`; else
//!   `object_shader_flag_target`).
//!   The skip clears the layer's bit on each such item
//!   (`layer_pass::clear_layer_items`). The external pass draws before the
//!   guest's layer draw, so an item after one the guest keeps is taken only
//!   when both are order-free and every kept item before it restores what
//!   it sets (`layer_pass::order_free`, `skip_holds`); else it keeps its
//!   D3D draw (`order`). One external pass records them, in the list's
//!   order. On the scene's and the alternate view's layers 1, 2, 5, 7, 8,
//!   17 and 20 (`native_layers::SCENE_ONLY`) each item the pass does not
//!   draw keeps its guest draw, under the same order rule, and its cause
//!   goes to the bridge's `Left` (by slot, or for the whole layer), which
//!   counts it `scene_kept_<site><layer>_<kind>_<cause>`. Object items
//!   plan without the world: a BSP still loading leaves only the
//!   structure's items to the guest (`world_pending`).
//! - **The stencil** (`layer_pass::item_stencil`): in layer 1 the
//!   geometry draw's prologue sets stencil mode 0 when the stencil-off
//!   flags are set and mode 2 for a marked object's record, and its
//!   epilogue leaves mode 1. So with the stencil off the items draw with
//!   the stencil disabled, and with the stencil off or a marked object
//!   item taken the device must already hold mode 1; else the layer, or
//!   the marked items, stay on the D3D path. Layers 2, 5, 7, 8, 17 and 20
//!   draw under the device's stencil.
//! - **The pass** ([`LayerViewPass`]): the device's view constants and W
//!   clip at the issue, then per run of the list `GpuWorld::layer_draws`
//!   or `LitObjects::draws` in the D3D pass open on the bound
//!   attachments. A sorted handler's layer whose passes sample the view's
//!   own target (engine texture 5) is recorded as commands instead: the
//!   bound colour copied (`LitObjects::view_copy`), then a render pass of
//!   its own on the same attachments with the draws binding the copy
//!   (`LitObjects::draws_with_view`). A layer 18 or 21 pass that samples
//!   the scene copy (engine texture 17) first downsamples the bound colour
//!   to 320 by 240 times the render scale (`LitObjects::scene_copy`), once
//!   for the handler's call, and its draws bind that.
//! - **Sorted handlers** (`LayerCaller::Sorted`): their items are object
//!   records only, drawn from the scene's models; a record the game gives
//!   a shader in place of its part's material draws that shader's passes,
//!   which the models compile for every part
//!   (`native_objects_glue::load_model`, `object_program_gpu::override_sections`).
//!
//! Each ask's outcomes are counted by layer and cause
//! (`render_bridge::native_layers::count_for`) for `render.scene`.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use frame_scene::{FrameScene, LayerItem, LayerOwner, LayerPass, PartFlags};
use native_render::fog_gpu::{LayerSamples, sampled_constants};
use native_render::layer_pass::{self, ItemSkip, ListItem};
use native_render::object_program_gpu::{self, LitObjects, OBJECT_LAYERS, PlannedDraw};
use native_render::object_streams::StreamFormats;
use native_render::program::ViewInputs;
use native_render::world::Owner;
use render_bridge::late_sorted::{Run, RunAt};
use render_bridge::native_layers::{Host, LayerCaller, Left, count_for};

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
    fn ask(&self, mem: &mut dyn GuestMemory, layer: u32, caller: LayerCaller, scene: &FrameScene, left: &mut Left) -> Option<native_render::skip::Cleared> {
        self.ask_at(mem, layer, caller, scene, left, None, &mut Run::Refused)
    }

    fn ask_run(&self, mem: &mut dyn GuestMemory, layer: u32, caller: LayerCaller, scene: &FrameScene, at: &RunAt<'_>, left: &mut Left) -> Run {
        let mut run = Run::Refused;
        self.ask_at(mem, layer, caller, scene, left, Some(at), &mut run);
        run
    }
}

impl<B: d3d::backend::Backend + Send + 'static> Glue<B> {
    /// The layer call's ask (`at` `None`): plans the items, clears the bits
    /// of the leading ones the order rule lets the pass draw, and commits.
    /// Or one run of the sorted entries from `at` (`render_bridge::late_sorted`,
    /// docs/graphics/retire-d3d-r5.md, "Layer 2 from the plan"): the
    /// structure items the plan draws up to the first entry the guest
    /// keeps, drawn there and set in `run`. A run counts no outcome (the
    /// layer call's ask has), checks the skip against the device as the
    /// shader draw's cached restore leaves it, and leaves the objects to
    /// the guest (`object_sorted`: their checks read the device at the
    /// layer call).
    #[allow(clippy::too_many_arguments)]
    fn ask_at(
        &self, mem: &mut dyn GuestMemory, layer: u32, caller: LayerCaller, scene: &FrameScene, left: &mut Left, at: Option<&RunAt<'_>>,
        run: &mut Run,
    ) -> Option<native_render::skip::Cleared> {
        let running = at.is_some();
        let Ok(l) = u8::try_from(layer) else {
            left.all("layer");
            return None;
        };
        let Some(pass) = scene.layer_passes.first() else {
            left.all("unread");
            return None;
        };
        // The world's layer sets: the main and alternate scene callers. A
        // world not loaded yet leaves the structure's items to the guest,
        // and the objects still plan.
        let ready = match caller {
            LayerCaller::Scene | LayerCaller::AlternateView => {
                let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                current_ready(&state, mem).map(Arc::clone)
            }
            LayerCaller::FirstPerson | LayerCaller::Sorted(_) => None,
        };
        let world = ready.as_deref().and_then(|r| r.layers.get());
        let data = world.and_then(|(view, _)| view.layers.get(&l));
        let mut outcomes: BTreeMap<&'static str, u64> = BTreeMap::new();
        let object_items: Vec<&LayerItem> =
            scene.items_of(pass).iter().filter(|i| matches!(i.owner, LayerOwner::Object(_))).collect();
        let is_object = |slot: u32| object_items.iter().any(|i| i.slot == slot);
        // The plan of the structure's items, from the scene alone.
        let mut taken: HashMap<u32, u32> = HashMap::new();
        let mut samples = LayerSamples::new();
        let plan = world.map(|(view, _)| layer_pass::plan(scene, pass, &mut Loaded(view)));
        match &plan {
            Some(plan) => {
                for (slot, r) in &plan.refused {
                    if !is_object(*slot) {
                        *outcomes.entry(cause(r)).or_default() += 1;
                        left.item(*slot, cause(r));
                    }
                }
                for d in &plan.draws {
                    let Some(owner) = layer_pass::world_owner(d.owner) else { continue };
                    // Structure items of the same material share the layer clock,
                    // have no object inputs and draw the load's single implementation.
                    if let Some(&(material, shader)) = data.and_then(|l| l.part_set.get(&(owner, d.part)))
                        && shader == d.shader
                        && !samples.contains_key(&material)
                        && let Some(passes) = sampled_constants(d)
                    {
                        samples.insert(material, passes);
                    }
                    taken.insert(d.slot, d.shader);
                }
            }
            None => {
                let structure: Vec<u32> =
                    scene.items_of(pass).iter().filter(|i| !matches!(i.owner, LayerOwner::Object(_))).map(|i| i.slot).collect();
                let why = match caller {
                    LayerCaller::FirstPerson => "first_person_structure",
                    LayerCaller::Sorted(_) => "sorted_structure",
                    LayerCaller::Scene | LayerCaller::AlternateView if ready.is_none() => "world_pending",
                    LayerCaller::Scene | LayerCaller::AlternateView => "program",
                };
                *outcomes.entry(why).or_default() += structure.len() as u64;
                for slot in structure {
                    left.item(slot, why);
                }
            }
        }
        let report = |outcomes: &BTreeMap<&'static str, u64>, drawn: u64| {
            if running {
                return;
            }
            for (why, &n) in outcomes {
                count_for(caller, layer, &format!("refused_{why}"), n);
            }
            count_for(caller, layer, "taken", drawn);
        };
        if data.is_none_or(|d| d.programs.is_empty()) {
            *outcomes.entry("program").or_default() += taken.len() as u64;
            taken.drain().for_each(|(slot, _)| left.item(slot, "program"));
        }
        // Extraction's causes before anything is asked of the device.
        let mut objects: Vec<&LayerItem> = Vec::with_capacity(object_items.len());
        for &item in &object_items {
            if let Some(why) = item.refusal {
                *outcomes.entry(why.name()).or_default() += 1;
                left.item(item.slot, why.name());
            } else if running {
                left.item(item.slot, "object_sorted");
            } else if !OBJECT_LAYERS.contains(&l) {
                *outcomes.entry("object_layer").or_default() += 1;
                left.item(item.slot, "object_layer");
            } else {
                objects.push(item);
            }
        }
        if taken.is_empty() && objects.is_empty() {
            report(&outcomes, 0);
            return None;
        }
        let refuse_all = |outcomes: &mut BTreeMap<&'static str, u64>, left: &mut Left, why: &'static str, world: usize, objects: usize| {
            *outcomes.entry(why).or_default() += (world + objects) as u64;
            left.all(why);
        };
        let Ok(mut d) = self.device.lock() else {
            left.all("device");
            return None;
        };
        if !d.can_external_pass() {
            refuse_all(&mut outcomes, &mut *left, "device", taken.len(), objects.len());
            report(&outcomes, 0);
            return None;
        }
        // The layer's draws run W-buffered whatever the device holds at the
        // call: its first shader begin sets ZENABLE first
        // (`native_objects_glue::apply_layer_begin_zenable`). Set before
        // the drain, the objects' W check (`object_w_buffer`) and the
        // pass's view constants and clip read the state the draws run with.
        if !objects.is_empty() && crate::native_objects_glue::apply_layer_begin_zenable(&mut d, mem).is_err() {
            refuse_all(&mut outcomes, &mut *left, "object_layer_begin", taken.len(), objects.len());
            report(&outcomes, 0);
            return None;
        }
        if !matches!(d.begin_external_boundary(&mut crate::native_lights_glue::DeviceMemory(mem)), Ok(true)) {
            refuse_all(&mut outcomes, &mut *left, "device", taken.len(), objects.len());
            report(&outcomes, 0);
            return None;
        }
        let read = ReadMemory(mem);
        if !viewport_clip(&d, &read) {
            refuse_all(&mut outcomes, &mut *left, "viewport", taken.len(), objects.len());
            report(&outcomes, 0);
            return None;
        }
        // The device's stencil at the layer call, which the items draw
        // under (`layer_pass::item_stencil`); one that does not read
        // leaves the layer to the D3D path.
        let Some(device_stencil) = d.stencil_state(&read) else {
            refuse_all(&mut outcomes, &mut *left, "stencil", taken.len(), objects.len());
            report(&outcomes, 0);
            return None;
        };
        // The world's sets draw with the palettes the load expanded
        // through.
        if !taken.is_empty() && !world.is_some_and(|(_, palettes)| palettes_hold(palettes, &|n| d.stage_palette(&read, n))) {
            *outcomes.entry("palettes").or_default() += taken.len() as u64;
            taken.drain().for_each(|(slot, _)| left.item(slot, "palettes"));
        }
        // Skipping an item's draws must leave the device as drawing them
        // would: every state its passes restore already holds, and every
        // state they set already holds or is restored
        // (`layer_pass::skip_holds`, as layer 3's walk requires).
        // The items whose guest draws are order-free besides
        // (`layer_pass::order_free`, under the unmarked items' stencil),
        // by slot and shader: a kept one does not stop the skip of a later
        // one (`layer_pass::clear_layer_items`).
        let unmarked = layer_pass::item_stencil(l, pass.stencil_off, device_stencil, false).stencil;
        let mut free: HashMap<u32, u32> = HashMap::new();
        // A run's skip leaves the next kept entry the device the shader
        // draw's restore of its cached implementation leaves.
        let restore = if running {
            let Some(restore) = native_render::sorted_entries::cached_restore(&read) else {
                left.all("cached_state");
                return None;
            };
            restore
        } else {
            Vec::new()
        };
        if let Some(plan) = &plan {
            let device = |slot: u32| d.render_state(&read, slot);
            let device_state = native_render::sorted_entries::after_restore(&restore, &device);
            for draw in plan.draws.iter().filter(|d| layer_pass::world_owner(d.owner).is_some()) {
                let holds =
                    draw.passes.iter().all(|p| draw.implementation(p).is_some_and(|imp| layer_pass::skip_holds_with(imp, Some(&draw.inputs), &device_state)));
                if holds
                    && draw.passes.iter().all(|p| draw.implementation(p).is_some_and(|imp| layer_pass::order_free(imp, &device_state, &unmarked)))
                {
                    free.insert(draw.slot, draw.shader);
                }
                if taken.contains_key(&draw.slot) && !holds {
                    taken.remove(&draw.slot);
                    *outcomes.entry("state").or_default() += 1;
                    left.item(draw.slot, "state");
                }
            }
        }
        let planned = if objects.is_empty() {
            None
        } else {
            plan_objects(&d, mem, l, caller, scene, pass, &objects, &unmarked, &mut free, &mut outcomes, left)
        };
        let (models, mut object_draws, texaccum) = match planned {
            Some((models, draws, texaccum)) => (Some(models), draws, texaccum),
            None => (None, HashMap::new(), None),
        };
        let marked = object_draws.values().any(|o| o.marked);
        let mut stencil = layer_pass::item_stencil(l, pass.stencil_off, device_stencil, marked);
        if stencil.needs_mode_1 && !crate::native_objects_glue::in_stencil_mode_1(&d, mem) {
            if pass.stencil_off {
                // Every item's draw leaves mode 1: skipping them is
                // state-neutral only from mode 1.
                refuse_all(&mut outcomes, &mut *left, "stencil", taken.len(), object_draws.len());
                report(&outcomes, 0);
                return None;
            }
            let before = object_draws.len();
            object_draws.retain(|&slot, o| {
                if o.marked {
                    left.item(slot, "object_stencil");
                }
                !o.marked
            });
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
                    taken.drain().for_each(|(slot, _)| left.item(slot, "stencil"));
                }
                _ => {
                    *outcomes.entry("warming").or_default() += taken.len() as u64;
                    taken.drain().for_each(|(slot, _)| left.item(slot, "warming"));
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
                object_draws.drain().for_each(|(slot, _)| left.item(slot, "object_warming"));
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
        if let Some(at) = at {
            return self.commit_run(&mut d, mem, l, caller, &ready, data, &taken, &fogs, samples, stencil, at, left, run);
        }
        let read = ReadMemory(mem);
        let Some(mut preview) = native_render::skip::MaskPreview::new(&read) else {
            left.all("masks");
            return None;
        };
        let candidates = taken.len() + object_draws.len();
        let is_free = |slot: u32, shader: u32| free.get(&slot) == Some(&shader);
        // A planned item the sets cannot draw (no compiled part or fog).
        let unset = std::cell::RefCell::new(Vec::new());
        let take_world = |item: layer_pass::SkipItem| {
            let take = data.is_some_and(|data| {
                taken.get(&item.slot) == Some(&item.shader)
                    && data.part_set.get(&(item.owner, item.part))
                        .is_some_and(|(m, shader)| *shader == item.shader && fogs.contains_key(m))
            });
            if !take && taken.contains_key(&item.slot) {
                unset.borrow_mut().push(item.slot);
            }
            take
        };
        // The kept items keep their guest draws, so the order rule holds
        // on every layer: an item after a kept one it does not commute
        // with stays the guest's (`order`).
        let Some(skip) = layer_pass::clear_layer_items(&mut preview, layer, &|item| match item {
            ListItem::World(item) => ItemSkip { take: take_world(item), free: is_free(item.slot, item.shader) },
            ListItem::Object { slot, shader } => {
                let planned = object_draws.get(&slot);
                let take = planned.is_some_and(|o| o.shader == shader);
                if planned.is_some() && !take {
                    unset.borrow_mut().push(slot);
                }
                ItemSkip { take, free: is_free(slot, shader) }
            }
        }) else {
            left.all("list");
            return None;
        };
        let (drawn, cleared, order) = (skip.drawn, skip.cleared, skip.order);
        for slot in unset.into_inner() {
            left.item(slot, "set");
        }
        for &slot in taken.keys().chain(object_draws.keys()) {
            if !drawn.iter().any(|d| d.slot() == slot) {
                left.item(slot, "order");
            }
        }
        let Some(masks) = preview.finish() else {
            left.all("masks");
            return None;
        };
        // The list's runs, in order, and the ranges they promise: one per
        // structure item, one per object pass draw.
        let mut runs: Vec<LayerRun> = Vec::new();
        let mut ranges = 0;
        for item in &drawn {
            match *item {
                ListItem::World(i) => {
                    ranges += 1;
                    match runs.last_mut() {
                        Some(LayerRun::World(parts)) => parts.push((i.owner, i.part)),
                        _ => runs.push(LayerRun::World(vec![(i.owner, i.part)])),
                    }
                }
                ListItem::Object { slot, .. } => {
                    let o = object_draws.remove(&slot)?;
                    ranges += o.draws.len();
                    match runs.last_mut() {
                        Some(LayerRun::Objects(draws)) => draws.extend(o.draws),
                        _ => runs.push(LayerRun::Objects(o.draws)),
                    }
                }
            }
        }
        // What the plan took that the skip did not: an item after a kept
        // one it does not commute with (`order`), or one the sets cannot
        // draw (no compiled part or fog: `set`).
        *outcomes.entry("order").or_default() += order as u64;
        *outcomes.entry("set").or_default() += candidates.saturating_sub(drawn.len() + order) as u64;
        report(&outcomes, drawn.len() as u64);
        if ranges == 0 {
            return None;
        }
        let view_index = mem.u32(engine::render_loop::occlusion::guest::VIEW_INDEX).unwrap_or(u32::MAX);
        let stage = match caller {
            LayerCaller::Scene | LayerCaller::AlternateView => timeline::native::Stage::World,
            LayerCaller::FirstPerson => timeline::native::Stage::FirstPerson,
            LayerCaller::Sorted(_) => timeline::native::Stage::Objects,
        };
        let plan = d3d::ExternalPlan { stage, view: view_index, ranges, slots: Vec::new() };
        // Bound only when a surrendered draw samples it.
        let texaccum = texaccum.filter(|_| runs.iter().any(|r| matches!(r, LayerRun::Objects(draws) if draws.iter().any(|(_, draw)| draw.texaccum))));
        let pass = Box::new(LayerViewPass {
            ready: ready.clone(),
            objects: models,
            texaccum,
            layer: l,
            runs,
            samples,
            fogs,
            view: None,
            w_clip: None,
            writes_depth: data.is_some_and(|d| d.writes_depth),
            stencil,
            run: u64::from(d.swap_count()).wrapping_add(1),
        });
        let Some(reservation) = d.prepare_external_bound(&mut crate::native_lights_glue::DeviceMemory(mem), plan, pass, Vec::new()).ok().flatten() else {
            left.all("reservation");
            return None;
        };
        if let Some(ready) = &ready {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !current_ready(&state, mem).is_some_and(|current| Arc::ptr_eq(current, ready)) {
                left.all("world_changed");
                return None;
            }
        }
        if !masks.apply(&mut render_bridge::native_fog::Mem(mem)) {
            left.all("masks");
            return None;
        }
        reservation.commit();
        Some(cleared)
    }
}

impl<B: d3d::backend::Backend + Send + 'static> Glue<B> {
    /// A run's draw: the structure items from `at` the plan took and the
    /// sets draw, up to the first entry the guest keeps
    /// (`layer_pass::sorted_layer_run`), reserved, committed and set in
    /// `run`. That entry is named `entry` when nothing else named it.
    #[allow(clippy::too_many_arguments)]
    fn commit_run(
        &self, d: &mut d3d::Device<B>, mem: &mut dyn GuestMemory, l: u8, caller: LayerCaller, ready: &Option<Arc<Ready>>, data: Option<&LayerData>,
        taken: &HashMap<u32, u32>, fogs: &HashMap<u16, (shaders::psh::fog::Fog, [f32; 4])>, samples: LayerSamples, stencil: gfx::state::StencilState,
        at: &RunAt<'_>, left: &mut Left, run: &mut Run,
    ) -> Option<native_render::skip::Cleared> {
        let take = |item: ListItem| match item {
            ListItem::World(item) => data.is_some_and(|data| {
                taken.get(&item.slot) == Some(&item.shader)
                    && data.part_set.get(&(item.owner, item.part)).is_some_and(|(m, shader)| *shader == item.shader && fogs.contains_key(m))
            }),
            ListItem::Object { .. } => false,
        };
        let Some(found) = layer_pass::sorted_layer_run(&ReadMemory(mem), u32::from(l), &at.sorted, at.from, &take) else {
            left.all("list");
            return None;
        };
        if let Some(stop) = found.stop {
            if taken.contains_key(&stop) && !left.items.contains_key(&stop) {
                left.item(stop, "set");
            } else if !left.items.contains_key(&stop) {
                left.item(stop, "entry");
            }
        }
        *run = Run::Stopped;
        if found.drawn.is_empty() {
            return None;
        }
        let mut runs: Vec<LayerRun> = Vec::new();
        for item in &found.drawn {
            let ListItem::World(i) = *item else { continue };
            match runs.last_mut() {
                Some(LayerRun::World(parts)) => parts.push((i.owner, i.part)),
                _ => runs.push(LayerRun::World(vec![(i.owner, i.part)])),
            }
        }
        let view_index = mem.u32(engine::render_loop::occlusion::guest::VIEW_INDEX).unwrap_or(u32::MAX);
        let stage = match caller {
            LayerCaller::Scene | LayerCaller::AlternateView => timeline::native::Stage::World,
            LayerCaller::FirstPerson => timeline::native::Stage::FirstPerson,
            LayerCaller::Sorted(_) => timeline::native::Stage::Objects,
        };
        let plan = d3d::ExternalPlan { stage, view: view_index, ranges: found.drawn.len(), slots: Vec::new() };
        let pass = Box::new(LayerViewPass {
            ready: ready.clone(),
            objects: None,
            texaccum: None,
            layer: l,
            runs,
            samples,
            fogs: fogs.clone(),
            view: None,
            w_clip: None,
            writes_depth: data.is_some_and(|d| d.writes_depth),
            stencil,
            run: u64::from(d.swap_count()).wrapping_add(1),
        });
        let Some(reservation) = d.prepare_external_bound(&mut crate::native_lights_glue::DeviceMemory(mem), plan, pass, Vec::new()).ok().flatten() else {
            left.all("reservation");
            *run = Run::Refused;
            return None;
        };
        if let Some(ready) = ready {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !current_ready(&state, mem).is_some_and(|current| Arc::ptr_eq(current, ready)) {
                left.all("world_changed");
                *run = Run::Refused;
                return None;
            }
        }
        reservation.commit();
        let slots = found.drawn.iter().map(|i| i.slot()).collect();
        *run = Run::Drawn { slots, entries: found.entries, parts: Vec::new() };
        None
    }
}

/// Plans a layer's object items (`items`, extraction naming no cause)
/// from `caller`'s models: per item its draws, or a cause counted in
/// `outcomes`. `None`, every item counted, when the models, the device's
/// viewport or its W mode leave them all to the D3D path.
#[allow(clippy::too_many_arguments)]
fn plan_objects<B: d3d::backend::Backend>(
    d: &d3d::Device<B>, mem: &dyn GuestMemory, layer: u8, caller: LayerCaller, scene: &FrameScene, pass: &LayerPass,
    items: &[&LayerItem], stencil: &gfx::state::StencilState, free: &mut HashMap<u32, u32>, outcomes: &mut BTreeMap<&'static str, u64>, left: &mut Left,
) -> Option<(Arc<dyn LayerModels>, HashMap<u32, ObjectItem>, Option<native_render::world_layers::Texaccum>)> {
    let mut all = |why: &'static str| {
        *outcomes.entry(why).or_default() += items.len() as u64;
        for item in items {
            left.item(item.slot, why);
        }
        None
    };
    let models = match caller {
        LayerCaller::Scene | LayerCaller::AlternateView | LayerCaller::Sorted(_) => crate::native_objects_glue::layer_models(d, mem),
        LayerCaller::FirstPerson => crate::native_first_person_glue::layer_models(d, mem),
    };
    let Some(models) = models else { return all("object_models") };
    let read = ReadMemory(mem);
    let (entry, w_buffered) = crate::native_objects_glue::entry_states(d, mem);
    if !w_buffered {
        return all("object_w_buffer");
    }
    let fog_words = device_fog_words(d, mem);
    let specular = crate::native_objects_glue::device_specular(d, mem);
    let device_state = |slot: u32| d.render_state(&read, slot);
    let mut prt_formats: Option<Option<StreamFormats>> = None;
    // Whether the device holds the auxiliary state toggle off: the view's
    // values are one per layer call, so asked once.
    let mut aux_at_rest: Option<bool> = None;
    // The texture accumulation target, read once when an item samples it.
    let mut texaccum: Option<Option<native_render::world_layers::Texaccum>> = None;
    // The device's polygon stipple (`native_objects_glue::device_stipple`),
    // and whether it holds what a faded record's epilogue leaves, each
    // read once.
    let mut stipple: Option<Result<Option<[u32; 32]>, ()>> = None;
    let mut stipple_rest: Option<bool> = None;
    // Whether the device and the engine hold the structure parameters a
    // flagged shader's prologue uploads, read once when an item uploads
    // them.
    let mut structure_held: Option<bool> = None;
    let read_bytes = |at: u32, len: usize| -> Option<Vec<u8>> {
        let mut bytes = vec![0; len];
        mem.read(at, &mut bytes).ok()?;
        Some(bytes)
    };
    let ready: Vec<bool> = items.iter().map(|i| models.ready(i.model)).collect();
    let Ok(guard) = models.lit().try_lock() else { return all("object_models") };
    let Some(lit) = guard.as_ref() else { return all("object_models") };
    let mut out = HashMap::with_capacity(items.len());
    for (item, ready) in items.iter().zip(ready) {
        if !ready || !lit.has(item.model) {
            *outcomes.entry("object_model").or_default() += 1;
            left.item(item.slot, "object_model");
            continue;
        }
        let draws = match lit.plan_layer_item(item.model, layer, scene, pass, item, &entry) {
            Ok(draws) => draws,
            Err(why) => {
                count_refusal(&why.to_string());
                *outcomes.entry(why.name()).or_default() += 1;
                left.item(item.slot, why.name());
                if let Some(detail) = why.detail() {
                    *outcomes.entry(detail).or_default() += 1;
                }
                continue;
            }
        };
        // A flagged shader's item before the frame's structure target
        // copy: its draw makes the copy for a program with flag bit 20
        // (`0x0003bea0`), which no native stage does. Such an item keeps
        // its draw, ahead of `order_free`, so the items after it see one
        // of unknown effect.
        if object_program_gpu::makes_target_copy(item.flags, draws.iter().map(|draw| lit.planned_pass(item.model, draw))) {
            *outcomes.entry("object_shader_flag_target").or_default() += 1;
            left.item(item.slot, "object_shader_flag_target");
            continue;
        }
        // Engine texture 3, the texture accumulation target, as layer 3's
        // objects bind it (`native_objects_glue::sampled_texaccum`): not in
        // layer 1, which draws into it.
        if draws.iter().any(|draw| draw.texaccum)
            && (layer == 1 || texaccum.get_or_insert_with(|| crate::native_objects_glue::sampled_texaccum(d, mem, true).ok().flatten()).is_none())
        {
            *outcomes.entry("object_texaccum").or_default() += 1;
            left.item(item.slot, "object_texaccum");
            continue;
        }
        // An item toggling the auxiliary state: its epilogue leaves the
        // toggle off, so skipping it is state-neutral only when the device
        // already holds it off, as layer 3's walk requires.
        if let Some(aux) = draws.iter().find_map(|draw| draw.aux_depth)
            && !*aux_at_rest.get_or_insert_with(|| crate::native_objects_glue::aux_depth_at_rest(d, mem, &aux))
        {
            *outcomes.entry("object_aux_depth").or_default() += 1;
            left.item(item.slot, "object_aux_depth");
            continue;
        }
        // An item whose shader's prologue uploads the structure's
        // parameter block draws with its view's upload, and is skipped
        // only where the device and the engine already hold it.
        if item.flags.contains(PartFlags::STRUCTURE_PARAMETERS)
            && !*structure_held.get_or_insert_with(|| {
                scene
                    .view_of_pass(pass)
                    .and_then(|v| v.structure_parameters)
                    .is_some_and(|p| crate::native_objects_glue::structure_at_rest(d, mem, &p))
            })
        {
            *outcomes.entry("object_shader_flag").or_default() += 1;
            left.item(item.slot, "object_shader_flag");
            continue;
        }
        // An item with a fade byte: its prologue sets the stipple pattern
        // the byte picks and its epilogue the rest one, so it is skipped
        // only where the device and the engine already hold what the
        // epilogue leaves. Under the device's stipple every item draws
        // masked by its record's pattern
        // (`native_objects_glue::record_stipple`).
        let faded = item.flags.contains(PartFlags::FADE);
        if faded && !*stipple_rest.get_or_insert_with(|| crate::native_objects_glue::stipple_at_rest(d, mem)) {
            *outcomes.entry("object_stipple").or_default() += 1;
            left.item(item.slot, "object_stipple");
            continue;
        }
        let device_stipple = *stipple.get_or_insert_with(|| crate::native_objects_glue::device_stipple(d, mem));
        let Ok(rows) = crate::native_objects_glue::record_stipple(mem, device_stipple, item.key.record, faded) else {
            *outcomes.entry("object_fade").or_default() += 1;
            left.item(item.slot, "object_fade");
            continue;
        };
        let mut draws = draws;
        for draw in &mut draws {
            draw.stipple = rows;
        }
        if !draws.iter().all(|draw| lit.planned_pass(item.model, draw).is_some_and(|p| layer_pass::skip_holds(&p.implementation, &device_state))) {
            *outcomes.entry("object_state").or_default() += 1;
            left.item(item.slot, "object_state");
            continue;
        }
        // Order-free (`layer_pass::order_free`): an unmarked record whose
        // passes draw opaque and depth-writing under the items' stencil.
        let marked = item.flags.contains(PartFlags::STENCIL_MARK) && !pass.stencil_off;
        if !marked
            && draws.iter().all(|draw| lit.planned_pass(item.model, draw).is_some_and(|p| layer_pass::order_free(&p.implementation, &device_state, stencil)))
        {
            free.insert(item.slot, item.key.shader);
        }
        // Layer 5's PRT stream, gathered from guest memory now
        // (`LitObjects::fill_prt`).
        let mut draws = draws;
        if item.prt.is_some() {
            let formats = prt_formats.get_or_insert_with(|| StreamFormats::read(&Words(mem)));
            let filled = match formats {
                Some(formats) => draws.iter_mut().try_for_each(|draw| lit.fill_prt(item.model, draw, formats, &read_bytes)),
                None => Err("the game's stream formats do not read".to_string()),
            };
            if let Err(why) = filled {
                count_refusal(&why);
                *outcomes.entry("object_prt").or_default() += 1;
                left.item(item.slot, "object_prt");
                continue;
            }
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
            left.item(item.slot, "object_fog");
            continue;
        }
        out.insert(item.slot, ObjectItem { shader: item.key.shader, marked, draws: with_fog });
    }
    drop(guard);
    let texaccum = texaccum.flatten().filter(|_| out.values().any(|o| o.draws.iter().any(|(_, draw)| draw.texaccum)));
    Some((models, out, texaccum))
}

/// Guest memory, read only, as the stream formats read it.
struct Words<'a>(&'a dyn GuestMemory);

impl native_render::camera::GuestRead for Words<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
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

/// The guest programs apply the viewport transform in their constants. A
/// single clip equal to the viewport can be reproduced on either target copy.
pub(crate) fn viewport_clip<B: d3d::backend::Backend>(device: &d3d::Device<B>, read: &ReadMemory<'_>) -> bool {
    let viewport = device.viewport();
    device.clip_area(read).ok().flatten().is_some_and(|(clip, rects)| {
        rects.is_empty() && (viewport.x, viewport.y, viewport.width, viewport.height) == (clip.x, clip.y, clip.width, clip.height)
    })
}

pub(crate) fn clip_packets(
    mut draws: d3d::backend::ExternalDraws, request: d3d::ExternalPassRequest, size: (u32, u32),
) -> Result<d3d::backend::ExternalDraws, &'static str> {
    if request.rect != (0, 0, request.surface_size.0, request.surface_size.1) {
        let scissor = viewport_scissor(request.rect, request.surface_size, size).ok_or("empty layer viewport")?;
        draws.packets = draws.packets.into_iter().map(|packet| packet.with_scissor(scissor, size)).collect();
    }
    Ok(draws)
}

fn viewport_scissor(
    (x, y, width, height): (u32, u32, u32, u32), guest: (u32, u32), size: (u32, u32),
) -> Option<gfx::ScissorRect> {
    let scale = d3d::scaled::RenderScale::between(guest, size)?;
    let [x, y, width, height] = scale.rect(x, y, width, height);
    let rect = gfx::ScissorRect { x, y, width, height }.intersect(gfx::ScissorRect::full(size.0, size.1));
    (!rect.is_empty()).then_some(rect)
}

/// One view's replacement of one layer.
struct LayerViewPass {
    /// The world (the scene's caller).
    ready: Option<Arc<Ready>>,
    /// The object models the object runs draw from.
    objects: Option<Arc<dyn LayerModels>>,
    /// The texture accumulation target, when an object draw samples it
    /// (engine texture 3, bound as layer 3's objects bind it).
    texaccum: Option<native_render::world_layers::Texaccum>,
    layer: u8,
    /// The surrendered items' runs, in the list's order.
    runs: Vec<LayerRun>,
    /// Sampled world constants in each material's compiled pass order.
    samples: LayerSamples,
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

    /// Whether a draw samples the view's own target or the scene copy: the
    /// pass is then recorded as commands with a copy of the bound colour
    /// ([`LayerViewPass::encoded`]).
    fn samples_view(&self) -> bool {
        self.object_draws().any(|d| d.view || d.scene_copy)
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

    fn sampled(&self) -> Vec<u32> {
        self.texaccum.iter().map(|target| target.texture).collect()
    }

    fn prepare_copy(&mut self, t: &d3d::ExternalTarget<'_>) -> Result<(), &'static str> {
        if self.view.is_none() || self.w_clip.is_none() {
            return Err("layer view constants or W-buffer clip unavailable");
        }
        if self.samples_view() {
            if self.world_runs().next().is_some() {
                return Err("structure items in a layer whose objects sample the view");
            }
            if t.color.texture().format() != t.color_format || !LitObjects::can_copy_view(t.color, t.size) {
                return Err("the bound colour cannot be copied for the view stages");
            }
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
    /// the D3D draws. A layer that samples the view is recorded by
    /// [`d3d::ExternalPass::record_encoded`] instead.
    fn record_draws(
        &mut self, t: &d3d::backend::ExternalAttachments, request: d3d::ExternalPassRequest,
    ) -> Option<Result<d3d::backend::ExternalDraws, &'static str>> {
        if self.samples_view() {
            return None;
        }
        Some(self.record(t, request))
    }

    /// The copy of the bound colour and the pass that samples it, as
    /// commands in the frame's open encoder ([`LayerViewPass::encoded`]).
    fn record_encoded(
        &mut self, t: &d3d::backend::ExternalAttachments, request: d3d::ExternalPassRequest,
    ) -> Option<Result<d3d::backend::ExternalEncoded, &'static str>> {
        if !self.samples_view() {
            return None;
        }
        Some(self.encoded(t, request))
    }
}

impl LayerViewPass {
    /// The view inputs of this recording.
    fn view_inputs(&self, t: &d3d::backend::ExternalAttachments, request: d3d::ExternalPassRequest) -> Result<ViewInputs, &'static str> {
        let constants = self.view.ok_or("layer view constants unavailable")?;
        if self.w_clip.is_none() {
            return Err("layer W-buffer clip unavailable");
        }
        Ok(ViewInputs {
            constants: request.view_constants(t.copy, constants),
            guest_size: request.surface_size,
            size: t.size,
            z_scale: request.z_scale,
            w_clip: self.w_clip,
        })
    }

    /// The object runs as commands: their uniforms' writes, the bound
    /// colour copied into this target copy's scratch texture, then one
    /// render pass on the bound attachments (loaded and stored) with the
    /// draws in the list's order, each view stage binding the copy, its
    /// coordinates texels of the target's guest size. The copy holds the
    /// target as the draws before the layer left it, as the D3D path's
    /// first item reads it.
    fn encoded(
        &self, t: &d3d::backend::ExternalAttachments, request: d3d::ExternalPassRequest,
    ) -> Result<d3d::backend::ExternalEncoded, &'static str> {
        if self.world_runs().next().is_some() {
            return Err("structure items in a layer whose objects sample the view");
        }
        if t.color.texture().format() != t.color_format || !LitObjects::can_copy_view(&t.color, t.size) {
            return Err("the bound colour cannot be copied for the view stages");
        }
        let view = self.view_inputs(t, request)?;
        let draws: Vec<(u32, PlannedDraw)> = self
            .runs
            .iter()
            .flat_map(|r| match r {
                LayerRun::Objects(draws) => draws.as_slice(),
                LayerRun::World(_) => &[][..],
            })
            .cloned()
            .collect();
        let models = self.objects.as_ref().ok_or("layer object geometry unavailable")?;
        let mut lit = models.lit().lock().unwrap_or_else(|e| e.into_inner());
        let lit = lit.as_mut().ok_or("layer object geometry unavailable")?;
        if draws.iter().any(|(model, d)| !lit.has_draw_assets(*model, d) || d.indices.is_empty()) {
            return Err("layer object geometry or material unavailable");
        }
        let texaccum = crate::native_objects_glue::texaccum_level(t, self.texaccum)?;
        let needs_view = draws.iter().any(|(_, d)| d.view);
        let needs_scene = draws.iter().any(|(_, d)| d.scene_copy);
        if needs_scene && !t.color.texture().usage().contains(wgpu::TextureUsages::TEXTURE_BINDING) {
            return Err("the bound colour cannot be sampled for the scene copy");
        }
        let copy = needs_view.then(|| lit.view_copy(&t.device, &t.color, t.copy));
        // Once for the handler's call: its layers 18 and 21 draw from one
        // downsample, made before they draw (the guest's `0x000352e0`).
        let scene = needs_scene.then(|| lit.scene_copy(&t.device, &t.color, request.surface_size, t.copy));
        let sampled = copy
            .as_ref()
            .map(|c| native_render::object_program_gpu::Texaccum { view: &c.view, guest_size: request.surface_size });
        let scene_sampled = scene.as_ref().map(|c| native_render::object_program_gpu::Texaccum {
            view: &c.view,
            guest_size: native_render::object_program::SCENE_COPY_SIZE,
        });
        let out = lit.draws_with_view(
            &t.device, &t.queue, &t.uploads, t.color_format, t.depth_format, t.size, &view,
            request.projection_factor(t.copy), &draws, texaccum, sampled, scene_sampled,
        )?;
        let out = clip_packets(out, request, t.size)?;
        let writes: Vec<(wgpu::Buffer, u64, wgpu::Buffer, u64, u64)> = out
            .writes
            .into_iter()
            .map(|(dst, offset, bytes)| {
                let (src, at) = t.uploads.stage(&t.device, &t.queue, &bytes);
                (src, at, dst, offset, bytes.len() as u64)
            })
            .collect();
        let packets = out.packets;
        let (ranges, count) = (out.ranges, packets.len());
        let (color, depth) = (t.color.clone(), t.depth.clone());
        let has_stencil = t.depth_format.has_stencil_aspect();
        let copy = copy.map(|c| (c.from, c.to, c.extent));
        let device = t.device.clone();
        let commands = move |encoder: &mut wgpu::CommandEncoder| {
            for (src, at, dst, offset, len) in &writes {
                encoder.copy_buffer_to_buffer(src, *at, dst, *offset, *len);
            }
            if let Some(scene) = &scene {
                scene.record(&device, encoder);
            }
            if let Some((from, to, extent)) = &copy {
                encoder.copy_texture_to_texture(from.as_image_copy(), to.as_image_copy(), *extent);
            }
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("native layer with view copy"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                    stencil_ops: has_stencil
                        .then_some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            // Checked when the packets were made: every pipeline built, no
            // counted draw.
            let _ = gfx::renderer::encode_in_pass(&mut pass, &packets);
        };
        let blit = usize::from(needs_scene);
        Ok(d3d::backend::ExternalEncoded { ranges, draws: count + blit, passes: 1 + blit, commands: Box::new(commands) })
    }

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
                    gpu.layer_draws(
                        self.layer, &t.device, t.color_format, t.depth_format, t.size, selected, &view,
                        &self.fogs, &self.stencil, &self.samples,
                    )?
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
                        request.projection_factor(t.copy), draws, crate::native_objects_glue::texaccum_level(t, self.texaccum)?,
                    )?
                }
            };
            out.ranges += draws.ranges;
            out.packets.extend(draws.packets);
            out.discards |= draws.discards;
            out.writes.extend(draws.writes);
        }
        clip_packets(out, request, t.size)
    }
}

#[cfg(test)]
mod viewport_tests {
    use super::*;

    #[test]
    fn adjacent_views_share_the_same_edge_on_each_copy() {
        for (size, boundary) in [((10, 8), 3), ((15, 13), 5)] {
            let top = viewport_scissor((0, 0, 10, 3), (10, 8), size).unwrap();
            let bottom = viewport_scissor((0, 3, 10, 5), (10, 8), size).unwrap();
            assert_eq!(top.y, 0);
            assert_eq!(top.height, boundary);
            assert_eq!(bottom.y, boundary);
            assert_eq!(bottom.y + bottom.height, size.1);
            assert_eq!(bottom.width, size.0);
        }
    }
}
