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
//!   layer 3's objects do (`native_objects_glue::sampled_texaccum`; else
//!   `object_texaccum`); in layer 1, which draws into it, each such draw
//!   binds a copy of the bound colour made just before it
//!   (`layer_segments`). An
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
//!   draws; where a pass's program has flag bit 20
//!   (`object_program_gpu::makes_target_copy`) only ahead of every kept
//!   item, the commit setting the frame's copy flag its call would
//!   ([`TARGET_COPIED`]) and, with a structure parameter block, the pass
//!   copying the colour into the refraction target before its draws
//!   ([`STRUCTURE_COPY_TARGET`]; kept `object_shader_flag_target` where
//!   the target does not read).
//!   The skip clears the layer's bit on each such item
//!   (`layer_pass::clear_layer_items`). The external pass draws before the
//!   guest's layer draw, so an item after one the guest keeps is taken only
//!   when both are order-free and every kept item before it restores what
//!   it sets (`layer_pass::order_free`, `skip_holds`); else it is not
//!   taken (`order`). One stage records them, in the list's order. On the scene's and the alternate view's layers 1, 2, 5, 7, 8,
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
//!   the marked items, are not taken. Layers 2, 5, 7, 8, 17 and 20
//!   draw under the device's stencil.
//! - **The pass** ([`LayerViewPass`], a stage pushed at its caller's
//!   slot): the frame graph's view inputs, then per run of the list
//!   `GpuWorld::layer_draws` or `LitObjects::draws` in the pass open on
//!   the bound attachments. A scene view's structure runs are culled on the GPU
//!   instead (`LayerViewPass::encoded`, `GpuWorld::culled_layer_draws`;
//!   docs/graphics/native-world.md, "Cull"): recorded as commands, the
//!   layer cull, then the passes of its segments ([`layer_segments`]). A sorted handler's layer whose passes sample the view's
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
use native_render::camera::RenderCamera;
use native_render::gpu::PassViewport;
use native_render::program::ViewInputs;
use native_render::stage::{FrameOp, Stage, StageWork, frame_builder};
use frame_types::{Slot, SlotAt};
use gfx::external::{ExternalAttachments, ExternalDraws, ExternalEncoded, TargetCopy};
use native_render::world::Owner;
use render_bridge::late_sorted::{Run, RunAt};
use render_bridge::native_layers::{Host, LayerCaller, Left, count_for};

use crate::native_fog_glue::fog_of;
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
    /// Its guest draw would be the frame's first call of `0x0003bea0`
    /// ([`TARGET_COPIED`]).
    sets_copied: bool,
    /// That call would also copy the bound colour into the refraction
    /// target (a structure parameter block: [`STRUCTURE_COPY_TARGET`]).
    copies: bool,
    draws: Vec<(u32, PlannedDraw)>,
}

/// An object item's skip: taken when planned for `shader`. An item whose
/// draw would set the frame's copy flag ([`TARGET_COPIED`]) is never
/// order-free, so the skip takes it only ahead of every kept item
/// (`layer_pass::clear_layer_items`), where the cache is cleared.
fn object_skip(planned: Option<&ObjectItem>, shader: u32, free: bool) -> ItemSkip {
    let take = planned.is_some_and(|o| o.shader == shader);
    ItemSkip { take, free: free && !planned.is_some_and(|o| o.sets_copied) }
}

/// The frame's structure target copy flag (`0x00509430`; `0x0002c0f5`
/// clears it each frame). The first call of `0x0003bea0` a frame (a
/// flagged shader's draw whose program has flag bit 20) sets the draw byte
/// `0x004670bc`, clears the shader cache (`0x00016b10`), with a structure
/// parameter block copies and binds the structure's target
/// (`0x000352e0`, `0x00014bc0`), sets this flag and clears the cache
/// again. An item drawn ahead of every kept item of its layer finds the
/// cache as the body's own `0x00016b10` left it, cleared, so its call
/// leaves this flag, which the layer's commit sets, and with a block the
/// copy, which the pass makes before the item's draws
/// ([`STRUCTURE_COPY_TARGET`]).
pub(crate) const TARGET_COPIED: u32 = 0x0050_9430;

/// The target `0x0003bea0` copies the bound colour into with a structure
/// parameter block (`0x000352e0(0x16, 1)`, then `0x00014bc0` binds the
/// view's target `[0x004858b8]` again): engine texture 20, the
/// refraction target the water samples (`native_render::screen_look::engine_texture`).
/// The layer's pass writes it as a slot of its plan, a snapshot of the
/// colour as the draws before the item left it
/// (`LitObjects::structure_copy`).
pub(crate) const STRUCTURE_COPY_TARGET: u32 = 0x16;

/// A run's planned objects (`Glue::commit_run`): the models they draw
/// from, their draws by slot, and the accumulation target they sample.
struct RunObjects {
    models: Option<Arc<dyn LayerModels>>,
    draws: HashMap<u32, ObjectItem>,
    texaccum: Option<native_render::world_layers::Texaccum>,
}


/// The object draw, counted over the drawn object items in order (each
/// whether it copies and its draw count), before which the structure
/// target copy is made: the first of the first item that copies.
fn copy_point(items: impl IntoIterator<Item = (bool, usize)>) -> Option<usize> {
    let mut at = 0;
    for (copies, draws) in items {
        if copies {
            return Some(at);
        }
        at += draws;
    }
    None
}


struct Glue {
    state: State,
}

impl Host for Glue {
    fn ask(&self, mem: &mut dyn GuestMemory, layer: u32, caller: LayerCaller, scene: &FrameScene, left: &mut Left) -> Option<native_render::skip::Cleared> {
        self.ask_at(mem, layer, caller, scene, left, None, &mut Run::Refused)
    }

    fn ask_run(&self, mem: &mut dyn GuestMemory, layer: u32, caller: LayerCaller, scene: &FrameScene, at: &RunAt<'_>, left: &mut Left) -> Run {
        let mut run = Run::Refused;
        self.ask_at(mem, layer, caller, scene, left, Some(at), &mut run);
        run
    }

    fn restore(&self, mem: &mut dyn GuestMemory, writes: &[render_bridge::open_shader::Write]) -> Result<(), &'static str> {
        restore_list(mem, writes)
    }
}

/// The open shader's restore list (`render_bridge::open_shader`), as
/// `0x00018900`'s calls leave guest memory: a simple state and a deferred
/// one into the state shadow (a deferred one into the dirty word too).
/// There is no device state behind them. A write through any other setter
/// is a named gap.
pub(crate) fn restore_list(mem: &mut dyn GuestMemory, writes: &[render_bridge::open_shader::Write]) -> Result<(), &'static str> {
    use engine::render_loop::states::{DIRTY, SHADOW};
    use render_bridge::open_shader::Write;
    if writes.iter().any(|w| matches!(w, Write::Setter { .. })) {
        unimplemented!("layers: restore list write through a render state setter");
    }
    for w in writes {
        let done = match *w {
            Write::Simple { state, value, .. } => mem.set_u32(SHADOW + 4 * u32::from(state), value).ok(),
            Write::Deferred { state, mask, value } => mem
                .u32(DIRTY)
                .and_then(|dirty| mem.set_u32(DIRTY, dirty | mask))
                .and_then(|()| mem.set_u32(SHADOW + 4 * u32::from(state), value))
                .ok(),
            Write::Setter { .. } => None,
        };
        done.ok_or("write")?;
    }
    Ok(())
}

/// A skipped item's `0x0003bea0` when it is the frame's first call
/// ([`TARGET_COPIED`]) and the shader cache may hold an implementation: a
/// sorted run's item (`Glue::commit_run`) or layer 15's object entry
/// (`TransparentAnswer::draw_object`). `0x00016b10` writes the cached
/// implementation's restore list (`0x00018900`, [`restore_list`]) and then
/// zeroes the cache (`frame_step::layer::clear_cache`); the call sets the
/// draw byte and the frame's copy flag. Read before anything is written
/// ([`FirstCopy::read`]), the list written before the pass is prepared
/// ([`FirstCopy::restore`]), the bytes and the clear with the commit
/// ([`FirstCopy::finish`]).
struct FirstCopy(Option<render_bridge::open_shader::Restore>);

impl FirstCopy {
    /// The cache's open shader, if any; `cached_state` when it does not read.
    fn read(mem: &dyn GuestMemory) -> Result<Self, &'static str> {
        render_bridge::open_shader::read(mem).map(Self).map_err(|_| "cached_state")
    }

    /// The cache's restore list, as `0x00018900` writes it ([`restore_list`]).
    fn restore(&self, mem: &mut dyn GuestMemory) -> Result<(), &'static str> {
        self.0.as_ref().map_or(Ok(()), |open| restore_list(mem, &open.writes))
    }

    /// The copy flag, the draw byte and the cleared cache.
    fn finish(mem: &mut dyn GuestMemory) -> Result<(), engine::MemFault> {
        mem.set_u8(TARGET_COPIED, 1)?;
        mem.set_u8(engine::render_loop::layer::DRAW_FLAG, 1)?;
        render_bridge::frame_step::layer::clear_cache(mem)
    }
}

impl Glue {
    /// The layer call's ask (`at` `None`): plans the items, clears the bits
    /// of the leading ones the order rule lets the pass draw, and commits.
    /// Or one run of the sorted entries from `at` (`render_bridge::late_sorted`):
    /// the structure items the plan draws up to the first entry the guest
    /// keeps, drawn there and set in `run`. A run counts no outcome (the
    /// layer call's ask has), sets no device state but the frame's copy
    /// flag its items' calls would, and plans its items against the device
    /// as the shader draw's cached restore leaves it
    /// (`sorted_entries::after_restore`): the stencil, the objects' entry
    /// states, fog, W buffer, stipple and skip.
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
        let read = ReadMemory(mem);
        // A run's items draw at the next kept entry, with the device as
        // the shader draw's restore of its cached implementation leaves it
        // (`sorted_entries::after_restore`): every check of the run reads
        // the device through it.
        let restore = if running {
            let Some(restore) = native_render::sorted_entries::cached_restore(&read) else {
                left.all("cached_state");
                return None;
            };
            restore
        } else {
            Vec::new()
        };
        let device = |slot: u32| crate::native_objects_glue::device_state::render_state(slot);
        let device_state = native_render::sorted_entries::after_restore(&restore, &device);
        // The device's stencil at the layer call (in a run, after the
        // restore), which the items draw under (`layer_pass::item_stencil`);
        // one that does not read takes nothing.
        let Some(device_stencil) = crate::native_objects_glue::stencil_with(&device_state) else {
            refuse_all(&mut outcomes, &mut *left, "stencil", taken.len(), objects.len());
            report(&outcomes, 0);
            return None;
        };
        // The world's sets draw with the palettes the load expanded
        // through.
        if !taken.is_empty() && !world.is_some_and(|(_, palettes)| palettes_hold(palettes, &crate::native_world_glue::stage_palette)) {
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
        if let Some(plan) = &plan {
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
            plan_objects(mem, l, caller, scene, pass, &objects, &unmarked, running.then_some(restore.as_slice()), &mut free, &mut outcomes, left)
        };
        let (models, mut object_draws, texaccum) = match planned {
            Some((models, draws, texaccum)) => (Some(models), draws, texaccum),
            None => (None, HashMap::new(), None),
        };
        let marked = object_draws.values().any(|o| o.marked);
        let mut stencil = layer_pass::item_stencil(l, pass.stencil_off, device_stencil, marked);
        if stencil.needs_mode_1 && !crate::native_objects_glue::in_stencil_mode_1_with(mem, Some(device_stencil)) {
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
        // leaves its items untaken.
        if !taken.is_empty() {
            let warm = ready.as_ref().and_then(|ready| {
                ready.with_pipeline_target(|gpu, device, color, depth| {
                    Some(gpu.prewarm_layer(l, device, color, depth, &stencil))
                })
            });
            match warm {
                Some(true) => {}
                _ => {
                    *outcomes.entry("warming").or_default() += taken.len() as u64;
                    taken.drain().for_each(|(slot, _)| left.item(slot, "warming"));
                }
            }
        }
        if let Some(models) = models.as_ref().filter(|_| !object_draws.is_empty()) {
            let flat: Vec<(u32, PlannedDraw)> = object_draws.values().flat_map(|o| o.draws.iter().cloned()).collect();
            let (device, color, depth) = crate::native_world_glue::pipeline_target();
            let warm = models.lit().try_lock().ok().and_then(|mut lit| lit.as_mut().map(|lit| lit.prewarm_draws(&device, color, depth, &flat))) == Some(true);
            if !warm {
                *outcomes.entry("object_warming").or_default() += object_draws.len() as u64;
                object_draws.drain().for_each(|(slot, _)| left.item(slot, "object_warming"));
            }
        }
        if taken.is_empty() && object_draws.is_empty() {
            report(&outcomes, 0);
            return None;
        }
        drop(device_state);
        let device_words = crate::native_objects_glue::device_state::fog_words();
        let mut fogs = HashMap::new();
        if let Some(data) = data {
            for (&material, words) in &data.words {
                if let Some(resolved) = words.resolve(&device_words) {
                    fogs.insert(material, fog_of(&resolved));
                }
            }
        }
        if let Some(at) = at {
            let objects = RunObjects { models, draws: object_draws, texaccum };
            return self.commit_run(mem, l, caller, &ready, data, &taken, objects, &fogs, samples, stencil, at, left, run);
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
                let skip = object_skip(planned, shader, is_free(slot, shader));
                if planned.is_some() && !skip.take {
                    unset.borrow_mut().push(slot);
                }
                skip
            }
        }) else {
            left.all("list");
            return None;
        };
        let (drawn, cleared, order) = (skip.drawn, skip.cleared, skip.order);
        let sets_copied = drawn.iter().any(|d| matches!(*d, ListItem::Object { slot, .. } if object_draws.get(&slot).is_some_and(|o| o.sets_copied)));
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
        let copy_at = copy_point(drawn.iter().filter_map(|d| match *d {
            ListItem::Object { slot, .. } => object_draws.get(&slot).map(|o| (o.copies, o.draws.len())),
            ListItem::World(_) => None,
        }));
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
        let camera = scene.views.first().map(|v| v.camera);
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
            stencil,
            copy_at,
            camera,
            token: crate::native_world_glue::view_token(crate::presents::presents() + 1, view_index),
        });
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
        // What the skipped item's `0x0003bea0` leaves: the frame's copy
        // flag ([`TARGET_COPIED`]); the copy itself is the pass's.
        if sets_copied && mem.set_u8(TARGET_COPIED, 1).is_err() {
            left.all("masks");
            return None;
        }
        push(caller, view_index, pass);
        Some(cleared)
    }
}

impl Glue {
    /// A run's draw: the structure items from `at` the plan took and the
    /// sets draw, up to the first entry the guest keeps
    /// (`layer_pass::sorted_layer_run`), reserved, committed and set in
    /// `run`. That entry is named `entry` when nothing else named it.
    #[allow(clippy::too_many_arguments)]
    fn commit_run(
        &self, mem: &mut dyn GuestMemory, l: u8, caller: LayerCaller, ready: &Option<Arc<Ready>>, data: Option<&LayerData>,
        taken: &HashMap<u32, u32>, objects: RunObjects, fogs: &HashMap<u16, (shaders::psh::fog::Fog, [f32; 4])>, samples: LayerSamples,
        stencil: gfx::state::StencilState, at: &RunAt<'_>, left: &mut Left, run: &mut Run,
    ) -> Option<native_render::skip::Cleared> {
        let RunObjects { models, draws: mut object_draws, texaccum } = objects;
        let take = |item: ListItem| match item {
            ListItem::World(item) => data.is_some_and(|data| {
                taken.get(&item.slot) == Some(&item.shader)
                    && data.part_set.get(&(item.owner, item.part)).is_some_and(|(m, shader)| *shader == item.shader && fogs.contains_key(m))
            }),
            ListItem::Object { slot, shader } => object_draws.get(&slot).is_some_and(|o| o.shader == shader),
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
        // A drawn item's `0x0003bea0` as layer 15's object entry leaves it
        // (`TransparentAnswer::draw_object`): the shader cache's restore
        // list written before the pass, the cache cleared with the frame's
        // copy flag ([`FirstCopy`]), and with a structure parameter block
        // the copy into the refraction target before its first draw.
        let drawn_objects = || found.drawn.iter().filter_map(|i| match *i {
            ListItem::Object { slot, .. } => object_draws.get(&slot),
            ListItem::World(_) => None,
        });
        let sets_copied = drawn_objects().any(|o| o.sets_copied);
        let copy_at = copy_point(drawn_objects().map(|o| (o.copies, o.draws.len())));
        let first = if sets_copied {
            match FirstCopy::read(&*mem) {
                Ok(first) => Some(first),
                Err(why) => {
                    left.all(why);
                    *run = Run::Refused;
                    return None;
                }
            }
        } else {
            None
        };
        // One range per structure item, one per object pass draw, as the
        // layer call's.
        let mut runs: Vec<LayerRun> = Vec::new();
        let mut ranges = 0;
        for item in &found.drawn {
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
        if ranges == 0 {
            return None;
        }
        // Bound only when a drawn object samples it.
        let texaccum = texaccum.filter(|_| runs.iter().any(|r| matches!(r, LayerRun::Objects(draws) if draws.iter().any(|(_, draw)| draw.texaccum))));
        let view_index = mem.u32(engine::render_loop::occlusion::guest::VIEW_INDEX).unwrap_or(u32::MAX);
        let pass = Box::new(LayerViewPass {
            ready: ready.clone(),
            objects: models,
            texaccum,
            layer: l,
            runs,
            samples,
            fogs: fogs.clone(),
            stencil,
            copy_at,
            // Layer 2's sorted runs are not the cull's: drawn unculled.
            camera: None,
            token: 0,
        });
        // The cache's restore list, as `0x00018900` writes it.
        if let Some(first) = &first
            && let Err(why) = first.restore(mem)
        {
            left.all(why);
            *run = Run::Refused;
            return None;
        }
        if let Some(ready) = ready {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !current_ready(&state, mem).is_some_and(|current| Arc::ptr_eq(current, ready)) {
                left.all("world_changed");
                *run = Run::Refused;
                return None;
            }
        }
        if first.is_some() && FirstCopy::finish(mem).is_err() {
            left.all("object_target");
            *run = Run::Refused;
            return None;
        }
        push(caller, view_index, pass);
        let slots = found.drawn.iter().map(|i| i.slot()).collect();
        *run = Run::Drawn { slots, entries: found.entries, parts: Vec::new() };
        None
    }
}

/// Plans a layer's object items (`items`, extraction naming no cause)
/// from `caller`'s models: per item its draws, or a cause counted in
/// `outcomes`. `None`, every item counted, when the models or the W mode
/// leave them all untaken.
#[allow(clippy::too_many_arguments)]
fn plan_objects(
    mem: &dyn GuestMemory, layer: u8, caller: LayerCaller, scene: &FrameScene, pass: &LayerPass,
    items: &[&LayerItem], stencil: &gfx::state::StencilState, restore: Option<&[(u32, u32)]>, free: &mut HashMap<u32, u32>,
    outcomes: &mut BTreeMap<&'static str, u64>, left: &mut Left,
) -> Option<(Arc<dyn LayerModels>, HashMap<u32, ObjectItem>, Option<native_render::world_layers::Texaccum>)> {
    let mut all = |why: &'static str| {
        *outcomes.entry(why).or_default() += items.len() as u64;
        for item in items {
            left.item(item.slot, why);
        }
        None
    };
    let models = match caller {
        LayerCaller::Scene | LayerCaller::AlternateView | LayerCaller::Sorted(_) => crate::native_objects_glue::layer_models(mem),
        LayerCaller::FirstPerson => crate::native_first_person_glue::layer_models(mem),
    };
    let Some(models) = models else { return all("object_models") };
    let read = ReadMemory(mem);
    // The render states the items' draws start from: the device's at the
    // layer call; in a run (`restore`) as the shader draw's restore of its
    // cached implementation leaves them before the next entry. What the
    // checks read outside the state array follows the restore too: the W
    // buffer its `ZENABLE` sets, the stipple switch it pushes, the fog
    // colour the structure's upload is compared with.
    let restore = restore.unwrap_or(&[]);
    let device = |slot: u32| crate::native_objects_glue::device_state::render_state(slot);
    let device_state = native_render::sorted_entries::after_restore(restore, &device);
    let (entry, w_buffered) = crate::native_objects_glue::entry_states_with(mem, &device_state);
    let w_buffered = crate::native_objects_glue::w_buffered_after(restore).unwrap_or(w_buffered);
    if !w_buffered {
        return all("object_w_buffer");
    }
    let fog_words = crate::native_fog_glue::fog_words_with(&device_state);
    let specular = crate::native_objects_glue::specular_with(&device_state);
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
        // copy: its draw calls `0x0003bea0` for a program with flag bit
        // 20, which with a structure parameter block copies the colour
        // into the refraction target. The pass makes that copy into the
        // target's own colour; a target that does not read keeps the
        // item's draw.
        let sets_copied = object_program_gpu::makes_target_copy(item.flags, draws.iter().map(|draw| lit.planned_pass(item.model, draw)));
        let copies = sets_copied && item.flags.contains(PartFlags::STRUCTURE_PARAMETERS);
        if copies && crate::native_shadows_glue::target_header(mem, STRUCTURE_COPY_TARGET).is_none() {
            *outcomes.entry("object_shader_flag_target").or_default() += 1;
            left.item(item.slot, "object_shader_flag_target");
            continue;
        }
        // Engine texture 3, the texture accumulation target, as layer 3's
        // objects bind it (`native_objects_glue::sampled_texaccum`); layer
        // 1, which draws into it, binds a copy of the bound colour made
        // before each draw that samples it (`layer_segments`).
        if draws.iter().any(|draw| draw.texaccum)
            && texaccum
                .get_or_insert_with(|| match layer {
                    1 => crate::native_objects_glue::accumulation_target(mem),
                    _ => crate::native_objects_glue::sampled_texaccum(mem, true).ok().flatten(),
                })
                .is_none()
        {
            *outcomes.entry("object_texaccum").or_default() += 1;
            left.item(item.slot, "object_texaccum");
            continue;
        }
        // An item toggling the auxiliary state: its epilogue leaves the
        // toggle off, so skipping it is state-neutral only when the device
        // already holds it off, as layer 3's walk requires.
        if let Some(aux) = draws.iter().find_map(|draw| draw.aux_depth)
            && !*aux_at_rest.get_or_insert_with(|| crate::native_objects_glue::aux_depth_at_rest(mem, &aux))
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
                    .is_some_and(|p| crate::native_objects_glue::structure_at_rest_with(mem, &p, fog_words[6]))
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
        if faded && !*stipple_rest.get_or_insert_with(|| crate::native_objects_glue::stipple_at_rest(mem)) {
            *outcomes.entry("object_stipple").or_default() += 1;
            left.item(item.slot, "object_stipple");
            continue;
        }
        let device_stipple = *stipple.get_or_insert_with(|| crate::native_objects_glue::device_stipple_after(restore));
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
        out.insert(item.slot, ObjectItem { shader: item.key.shader, marked, sets_copied, copies, draws: with_fog });
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

/// Object items the plan did not take, the first of each reason logged.
static REASONS: Mutex<Option<std::collections::HashSet<String>>> = Mutex::new(None);

fn count_refusal(why: &str) {
    let mut seen = REASONS.lock().unwrap_or_else(|e| e.into_inner());
    let seen = seen.get_or_insert_with(Default::default);
    if seen.len() < 64 && seen.insert(why.to_owned()) {
        eprintln!("native-layers: an object item is not taken: {why} (each reason logged once)");
    }
}

/// Installs the host over the world's state (`native_world_glue::install`).
pub fn install(state: State) {
    render_bridge::effects_shadow::geometry::set_answer_host(Some(Arc::new(TransparentAnswer { state: Arc::clone(&state) })));
    render_bridge::native_layers::set_host(Arc::new(Glue { state }));
}

/// Pushes a layer's pass at its caller's slot: the world's and the first
/// person's with the opaque scene, a sorted handler's with the
/// transparents.
fn push(caller: LayerCaller, view: u32, pass: Box<LayerViewPass>) {
    let slot = match caller {
        LayerCaller::Scene | LayerCaller::AlternateView | LayerCaller::FirstPerson => Slot::Opaque,
        LayerCaller::Sorted(_) => Slot::Transparents,
    };
    frame_builder().push(FrameOp::Stage { slot: SlotAt { slot, view: view as u8 }, stage: pass });
}

/// The display's projection factor: the lens the layer culls and the
/// objects draw by.
fn display_factor() -> f32 {
    crate::native_world_glue::projection_factor()
}

/// R2's geometry answer (`render_bridge::effects_shadow::geometry`): one
/// BSP cluster entry of the sorted transparents drawn from the world's
/// layer-15 set, as a run of one item (like [`Glue::commit_run`]'s), in
/// place of the guest's handler `0x00041020`. The set's passes were
/// compiled at the load from the same tags.
struct TransparentAnswer {
    state: State,
}

impl render_bridge::effects_shadow::geometry::AnswerHost for TransparentAnswer {
    fn draw(&self, mem: &mut dyn GuestMemory, r: &render_bridge::effects_shadow::geometry::Request) -> Result<(), &'static str> {
        use native_render::layers_load::TRANSPARENT_LAYER as L;
        if let render_bridge::effects_shadow::geometry::Owner::Object(handle) = r.owner {
            return self.draw_object(mem, r, handle);
        }
        let ready = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            current_ready(&state, mem).map(Arc::clone)
        }
        .ok_or("world_pending")?;
        let (view, palettes) = ready.layers.get().ok_or("no_layers")?;
        let data = view.layers.get(&L).ok_or("no_layer")?;
        let owner = match r.owner {
            render_bridge::effects_shadow::geometry::Owner::Cluster(c) => Owner::Cluster(c),
            render_bridge::effects_shadow::geometry::Owner::Placement(p) => Owner::Instance(p),
            render_bridge::effects_shadow::geometry::Owner::Object(_) => return Err("set"),
        };
        let part = (owner, r.part);
        let &(material, shader) = data.part_set.get(&part).ok_or("set")?;
        if shader & 0xffff != r.shader & 0xffff {
            return Err("set");
        }
        // Each pass's implementation 0 (the build model's, which the arm
        // checked), with nothing that reads a per-draw input or animates:
        // the compiled passes hold the load's values.
        let tags_material = view
            .materials
            .iter()
            .find(|(k, _)| **k & 0xffff == r.shader & 0xffff)
            .map(|(_, m)| Arc::clone(m))
            .ok_or("material")?;
        let layer = tags_material.layers.iter().find(|l| l.lod == 0 && l.layer == usize::from(L)).ok_or("layer")?;
        let imps: Vec<_> = layer.passes.iter().filter_map(|p| p.implementations.iter().find(|i| i.index == 0)).collect();
        if imps.len() != layer.passes.len() || imps.len() != r.passes as usize {
            return Err("passes");
        }
        if imps.iter().any(|i| i.per_draw_inputs || !i.animated.is_empty()) {
            return Err("per_draw");
        }
        let (stencil, fog) = {
            let read = ReadMemory(mem);
            let device_stencil = crate::native_objects_glue::device_state::stencil().ok_or("stencil")?;
            if !palettes_hold(palettes, &crate::native_world_glue::stage_palette) {
                return Err("palettes");
            }
            // Skipping the handler must leave the device as its draws would:
            // what the cached implementation restores, then what each pass
            // sets already holds or is restored (`layer_pass::skip_holds`).
            let restore = native_render::sorted_entries::cached_restore(&read).ok_or("cached_state")?;
            let device_words = |slot: u32| crate::native_objects_glue::device_state::render_state(slot);
            let device_state = native_render::sorted_entries::after_restore(&restore, &device_words);
            if !imps.iter().all(|i| layer_pass::skip_holds(i, &device_state)) {
                return Err("state");
            }
            let words = crate::native_objects_glue::device_state::fog_words();
            let fog = data.words.get(&material).and_then(|w| w.resolve(&words)).map(|w| fog_of(&w)).ok_or("fog")?;
            (layer_pass::item_stencil(L, false, device_stencil, false).stencil, fog)
        };
        let warm = ready.with_pipeline_target(|gpu, device, color, depth| {
            Some(gpu.prewarm_layer(L, device, color, depth, &stencil))
        });
        if warm != Some(true) {
            return Err("warming");
        }
        let view_index = mem.u32(engine::render_loop::occlusion::guest::VIEW_INDEX).unwrap_or(u32::MAX);
        let pass = Box::new(LayerViewPass {
            ready: Some(Arc::clone(&ready)),
            objects: None,
            texaccum: None,
            layer: L,
            runs: vec![LayerRun::World(vec![part])],
            samples: LayerSamples::new(),
            fogs: HashMap::from([(material, fog)]),
            stencil,
            copy_at: None,
            camera: None,
            token: 0,
        });
        {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !current_ready(&state, mem).is_some_and(|current| Arc::ptr_eq(current, &ready)) {
                return Err("world_changed");
            }
        }
        frame_builder().push(FrameOp::Stage { slot: SlotAt { slot: Slot::Transparents, view: view_index as u8 }, stage: pass });
        Ok(())
    }
}

impl TransparentAnswer {
    /// An object entry: its layer-15 item from the scene the list's masks
    /// give (the entry's record, part and shader name it), planned like a
    /// sorted run's object (`plan_objects` over the cached restore), drawn
    /// as a run of its passes from its model's set. An item that marks the
    /// stencil, copies a target or samples the view or the accumulation
    /// target keeps the guest's handler.
    fn draw_object(&self, mem: &mut dyn GuestMemory, r: &render_bridge::effects_shadow::geometry::Request, handle: u32) -> Result<(), &'static str> {
        use native_render::layers_load::TRANSPARENT_LAYER as L;
        // Only the model set and the scene's view kind depend on the
        // caller: this is a sorted handler's, as the camouflage's.
        let caller = LayerCaller::Sorted(native_render::world_layers::SortedHandler::Camouflage);
        let scene = render_bridge::native_layers::extract(&*mem, L, caller).ok_or("object_scene")?;
        let pass = scene.layer_passes.first().ok_or("object_scene")?;
        let item = scene
            .items_of(pass)
            .iter()
            .find(|i| {
                matches!(i.owner, LayerOwner::Object(k) if k.0 == handle)
                    && i.key.record == r.record
                    && i.key.part == r.part
                    && i.key.shader & 0xffff == r.shader & 0xffff
            })
            .ok_or("object_item")?;
        if let Some(why) = item.refusal {
            return Err(why.name());
        }
        let (device_stencil, restore) = {
            let read = ReadMemory(&*mem);
            let restore = native_render::sorted_entries::cached_restore(&read).ok_or("cached_state")?;
            // The stencil as the restore leaves it, as `plan_objects` reads
            // the rest.
            let device = |slot: u32| crate::native_objects_glue::device_state::render_state(slot);
            let after = native_render::sorted_entries::after_restore(&restore, &device);
            let device_stencil = crate::native_objects_glue::stencil_with(&after);
            drop(after);
            (device_stencil.ok_or("stencil")?, restore)
        };
        let unmarked = layer_pass::item_stencil(L, pass.stencil_off, device_stencil, false).stencil;
        let (mut free, mut outcomes, mut left) = (HashMap::new(), BTreeMap::new(), Left::default());
        let Some((models, mut planned, texaccum)) =
            plan_objects(&*mem, L, caller, &scene, pass, &[item], &unmarked, Some(&restore), &mut free, &mut outcomes, &mut left)
        else {
            return Err(left.layer.unwrap_or("object_models"));
        };
        let Some(planned) = planned.remove(&item.slot) else {
            return Err(left.items.get(&item.slot).copied().unwrap_or("object_plan"));
        };
        // A stencil-marking item draws under the mode-1 stencil, which the
        // device must hold when the item's own draw would not set it
        // (`layer_pass::item_stencil`).
        let stencil = if planned.marked {
            let marked = layer_pass::item_stencil(L, pass.stencil_off, device_stencil, true);
            if marked.needs_mode_1 && !crate::native_objects_glue::in_stencil_mode_1_with(&*mem, Some(device_stencil)) {
                return Err("object_stencil");
            }
            marked.stencil
        } else {
            unmarked
        };
        // The item's `0x0003bea0` the first time the frame calls it
        // (`TARGET_COPIED`): skipping it writes the cache's restore list
        // before the pass and clears the cache with the flag
        // ([`FirstCopy`]). With a structure parameter block the call also
        // copies the bound colour into the refraction target, which the
        // pass does before its first draw
        // (`snapshot_gpu::Snapshots::into_target`).
        let first = if planned.sets_copied { Some(FirstCopy::read(&*mem)?) } else { None };
        let copy_at = planned.copies.then_some(0);
        let draws = planned.draws;
        // Bound only when a surrendered draw samples it.
        let texaccum = texaccum.filter(|_| draws.iter().any(|(_, draw)| draw.texaccum));
        let (device, color, depth) = crate::native_world_glue::pipeline_target();
        let warm = models.lit().try_lock().ok().and_then(|mut lit| lit.as_mut().map(|lit| lit.prewarm_draws(&device, color, depth, &draws))) == Some(true);
        if !warm {
            return Err("warming");
        }
        let view_index = mem.u32(engine::render_loop::occlusion::guest::VIEW_INDEX).unwrap_or(u32::MAX);
        let pass = Box::new(LayerViewPass {
            ready: None,
            objects: Some(models),
            texaccum,
            layer: L,
            runs: vec![LayerRun::Objects(draws)],
            samples: LayerSamples::new(),
            fogs: HashMap::new(),
            stencil,
            copy_at,
            camera: None,
            token: 0,
        });
        // The cache's restore list, as `0x00018900` writes it.
        if let Some(first) = &first {
            first.restore(mem)?;
        }
        if first.is_some() && FirstCopy::finish(mem).is_err() {
            return Err("object_target");
        }
        frame_builder().push(FrameOp::Stage { slot: SlotAt { slot: Slot::Transparents, view: view_index as u8 }, stage: pass });
        Ok(())
    }
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
    /// The stencil the structure's draws take (`layer_pass::item_stencil`,
    /// `fog_gpu::layer_stencil`).
    stencil: gfx::state::StencilState,
    /// The object draw before which the bound colour is copied into the
    /// refraction target, the plan's slot 0 ([`STRUCTURE_COPY_TARGET`]).
    copy_at: Option<usize>,
    /// The scene view's camera: its display lens culls the structure on
    /// the GPU. Without one the structure runs draw unculled.
    camera: Option<RenderCamera>,
    /// This view in this frame (`native_world_glue::view_token`): the
    /// pyramid its layer 3 built, which the structure's cull may read.
    token: u64,
}

impl LayerViewPass {
    /// Whether the structure runs are culled on the GPU
    /// ([`LayerViewPass::encoded`]), whatever else the layer copies.
    fn culls(&self) -> bool {
        self.world_runs().next().is_some() && self.camera.is_some()
    }

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

    /// Whether a draw samples the view's own target, the scene copy or
    /// layer 1's accumulation target, or the pass makes the frame's
    /// structure target copy: it is then recorded as commands
    /// ([`LayerViewPass::encoded`]).
    fn samples_view(&self) -> bool {
        self.accumulates() || self.copy_at.is_some() || self.object_draws().any(|d| d.view || d.scene_copy)
    }

    /// Whether this is layer 1 and a draw samples the accumulation target
    /// it draws into ([`layer_segments`]).
    fn accumulates(&self) -> bool {
        self.layer == 1 && self.object_draws().any(|d| d.texaccum)
    }
}

impl Stage for LayerViewPass {
    fn name(&self) -> &'static str {
        "layers"
    }

    /// The layer's draws appended to the pass open on these attachments,
    /// run by run in the list's order; a layer that samples the view or
    /// culls on the GPU as commands ([`LayerViewPass::encoded`]).
    fn record(&mut self, t: &ExternalAttachments, view: &ViewInputs) -> StageWork {
        if self.world_runs().next().is_some() {
            if let Err(why) = self.parts_bind() {
                panic!("layers: {why}");
            }
        }
        let work = if self.culls() || self.samples_view() {
            self.encoded(t, view).map(StageWork::Encoded)
        } else {
            self.draws(t, view).map(StageWork::Draws)
        };
        work.unwrap_or_else(|why| panic!("layers: {why}"))
    }
}

/// One render pass of a layer recorded as commands ([`layer_segments`]):
/// whether the bound colour is copied before it, whether the frame's
/// structure target copy is made before it, and its draws.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Segment {
    pub copies_colour: bool,
    pub copies_target: bool,
    pub units: std::ops::Range<usize>,
}

/// A layer's draws (structure parts and object pass draws, in the list's
/// order) split where a draw reads the bound colour as the draws before
/// it left it: each draw sampling the view's own target or layer 1's
/// accumulation target starts a segment the bound colour is copied
/// before, as the game's draw reads the attachment it samples, and
/// the draw the frame's structure target copy is made before starts one.
/// `units` is each draw's: whether it samples the colour, whether the copy
/// is made before it.
pub(crate) fn layer_segments(units: impl IntoIterator<Item = (bool, bool)>) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    for (n, (samples, copy)) in units.into_iter().enumerate() {
        match out.last_mut() {
            Some(s) if !samples && !copy => s.units.end = n + 1,
            _ => out.push(Segment { copies_colour: samples, copies_target: copy, units: n..n + 1 }),
        }
    }
    out
}

/// Records `encoder`'s render pass on the bound attachments, loaded and
/// stored.
fn load_pass<'a>(
    encoder: &'a mut wgpu::CommandEncoder, color: &'a wgpu::TextureView, depth: &'a wgpu::TextureView, stencil: bool,
) -> wgpu::RenderPass<'a> {
    fn keep<V>() -> wgpu::Operations<V> {
        wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }
    }
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("native layer segment"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: color,
            depth_slice: None,
            resolve_target: None,
            ops: keep(),
        })],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: depth,
            depth_ops: Some(keep()),
            stencil_ops: stencil.then(keep),
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

impl LayerViewPass {
    /// Whether every selected structure part binds in the world's layer set.
    fn parts_bind(&self) -> Result<(), &'static str> {
        let ready = self.ready.as_ref().ok_or("world geometry unavailable")?;
        let mut guard = ready.gpu();
        let gpu = guard.as_mut().ok_or("world geometry unavailable")?;
        let set = gpu.layer(self.layer).ok_or("layer materials unavailable")?;
        if self.world_runs().flatten().any(|&(owner, part)| !set.draws_part(owner, part)) {
            return Err("a selected layer part did not bind");
        }
        Ok(())
    }

    /// The packets of `runs`, in order: the structure's from the world's
    /// layer set, the objects' with the accumulation target's level.
    fn draws(&self, t: &ExternalAttachments, view: &ViewInputs) -> Result<ExternalDraws, &'static str> {
        let texaccum = crate::native_objects_glue::texaccum_level(t, self.texaccum)?;
        let mut out = ExternalDraws { ranges: 0, packets: Vec::new(), discards: false, writes: Vec::new() };
        for run in &self.runs {
            let draws = match run {
                LayerRun::World(selected) => self.world_run(t, view, selected)?,
                LayerRun::Objects(draws) => self.object_run(t, view, draws, texaccum, None, None)?,
            };
            out.ranges += draws.ranges;
            out.packets.extend(draws.packets);
            out.discards |= draws.discards;
            out.writes.extend(draws.writes);
        }
        Ok(out)
    }

    /// One run of structure parts, from the world's layer set, unculled.
    fn world_run(
        &self, t: &ExternalAttachments, view: &ViewInputs, selected: &[(Owner, u8)],
    ) -> Result<ExternalDraws, &'static str> {
        let ready = self.ready.as_ref().ok_or("world geometry unavailable")?;
        let mut guard = ready.gpu();
        let gpu = guard.as_mut().ok_or("world geometry unavailable")?;
        gpu.layer_draws(
            self.layer, &t.device, t.color_format, t.depth_format, t.size, selected, view,
            &self.fogs, &self.stencil, &self.samples,
        )
    }

    /// One run of object pass draws, from the models, with the
    /// accumulation target, the view copy and the scene copy each binds.
    #[allow(clippy::too_many_arguments)]
    fn object_run(
        &self, t: &ExternalAttachments, view: &ViewInputs,
        draws: &[(u32, PlannedDraw)],
        texaccum: Option<native_render::object_program_gpu::Texaccum<'_>>,
        view_copy: Option<native_render::object_program_gpu::Texaccum<'_>>,
        scene_copy: Option<native_render::object_program_gpu::Texaccum<'_>>,
    ) -> Result<ExternalDraws, &'static str> {
        let models = self.objects.as_ref().ok_or("layer object geometry unavailable")?;
        let mut lit = models.lit().lock().unwrap_or_else(|e| e.into_inner());
        let lit = lit.as_mut().ok_or("layer object geometry unavailable")?;
        if draws.iter().any(|(model, d)| !lit.has_draw_assets(*model, d) || d.indices.is_empty()) {
            return Err("layer object geometry or material unavailable");
        }
        lit.draws_with_view(
            &t.device, &t.queue, &t.uploads, t.color_format, t.depth_format, t.size, view,
            display_factor(), draws, texaccum, view_copy, scene_copy,
        )
    }

    /// The layer as commands: their uniforms' writes; where the structure
    /// is culled on the GPU ([`LayerViewPass::culls`]), the layer cull by
    /// the camera's display lens (and, where this view's layer 3 built one on this
    /// target, its pyramid, `GpuWorld::culled_layer_draws`); the scene
    /// copy once (a sorted handler's call, the guest's `0x000352e0` at its
    /// entry); then per segment ([`layer_segments`]) the bound colour
    /// copied when it starts with a draw sampling the view's own target or
    /// layer 1's accumulation target, the frame's structure target copy
    /// downsampled into the refraction target (the plan's slot 0) when it
    /// starts at [`LayerViewPass::copy_at`], and a render pass on the bound
    /// attachments with the segment's structure and object draws in the
    /// list's order. A segment starts only at an object draw, so each
    /// structure run lies in one.
    fn encoded(&self, t: &ExternalAttachments, view: &ViewInputs) -> Result<ExternalEncoded, &'static str> {
        use native_render::object_program_gpu::Texaccum;
        if t.color.texture().format() != t.color_format {
            return Err("the bound colour's format is not the pass's");
        }
        // Per draw (each structure part, each object pass draw): whether it
        // reads the bound colour, whether the structure copy is made
        // before it.
        let mut units: Vec<(bool, bool)> = Vec::new();
        let mut object = 0usize;
        for run in &self.runs {
            match run {
                LayerRun::World(parts) => units.extend(parts.iter().map(|_| (false, false))),
                LayerRun::Objects(draws) => {
                    for (_, d) in draws {
                        units.push((d.view || (self.layer == 1 && d.texaccum), self.copy_at == Some(object)));
                        object += 1;
                    }
                }
            }
        }
        let segments = layer_segments(units.iter().copied());
        let copies_colour = segments.iter().any(|s| s.copies_colour);
        let needs_scene = self.object_draws().any(|d| d.scene_copy);
        if copies_colour && !LitObjects::can_copy_view(&t.color, t.size) {
            return Err("the bound colour cannot be copied for the view stages");
        }
        if (needs_scene || self.copy_at.is_some()) && !t.color.texture().usage().contains(wgpu::TextureUsages::TEXTURE_BINDING) {
            return Err("the bound colour cannot be sampled for a snapshot");
        }
        let target: Option<&gfx::external::ExternalColor> = match self.copy_at {
            Some(_) => unimplemented!("layers: refraction target from the frame graph"),
            None => None,
        };
        let (mut copy, mut scene, mut structure) = (None, None, None);
        if copies_colour || needs_scene || target.is_some() {
            let models = self.objects.as_ref().ok_or("layer object geometry unavailable")?;
            let mut lit = models.lit().lock().unwrap_or_else(|e| e.into_inner());
            let lit = lit.as_mut().ok_or("layer object geometry unavailable")?;
            if copies_colour {
                copy = Some(lit.view_copy(&t.device, &t.color, TargetCopy::Display));
            }
            // Once for the handler's call: its layers 18 and 21 draw from
            // one downsample, made before they draw.
            if needs_scene {
                scene = Some(lit.scene_copy(&t.device, &t.color, view.guest_size, TargetCopy::Display));
            }
            if let Some(target) = target {
                structure = Some(lit.structure_copy(&t.color, &target.attachment, target.format));
            }
        }
        let texaccum = match (&copy, self.accumulates()) {
            (Some(c), true) => {
                let target = self.texaccum.ok_or("texture accumulation target unavailable")?;
                Some(Texaccum { view: &c.view, guest_size: target.size })
            }
            _ => crate::native_objects_glue::texaccum_level(t, self.texaccum)?,
        };
        let view_copy = copy.as_ref().filter(|_| self.object_draws().any(|d| d.view))
            .map(|c| Texaccum { view: &c.view, guest_size: view.guest_size });
        let scene_copy = scene.as_ref().map(|c| Texaccum { view: &c.view, guest_size: native_render::object_program::SCENE_COPY_SIZE });
        // The structure runs culled at once, in order, with the dispatch
        // recorded before every pass.
        let (mut culled, dispatch) = if self.culls() {
            let camera = self.camera.ok_or("layer camera unavailable")?;
            let ready = self.ready.as_ref().ok_or("world geometry unavailable")?;
            let lens = native_render::camera::display_camera(camera, display_factor());
            let [x, y, width, height] = [0.0, 0.0, t.size.0 as f32, t.size.1 as f32];
            let occlusion = Some((t.depth.texture(), &lens, PassViewport { x, y, width, height }, self.token));
            let world_runs: Vec<&[(Owner, u8)]> = self.world_runs().collect();
            let mut guard = ready.gpu();
            let gpu = guard.as_mut().ok_or("world geometry unavailable")?;
            let (world, dispatch) = gpu.culled_layer_draws(
                self.layer, &t.uploads, &t.device, &t.queue, t.color_format, t.depth_format, t.size, &world_runs, view,
                &self.fogs, &self.stencil, &self.samples, &lens, occlusion,
            )?;
            (Some(world.into_iter()), Some(dispatch))
        } else {
            (None, None)
        };
        let mut writes = Vec::new();
        let mut recorded = Vec::with_capacity(segments.len());
        let (mut ranges, mut count) = (0, 0);
        for s in &segments {
            let mut out = ExternalDraws { ranges: 0, packets: Vec::new(), discards: false, writes: Vec::new() };
            // Each run's draws within the segment, by their unit offsets.
            let mut at = 0usize;
            for run in &self.runs {
                let len = match run {
                    LayerRun::World(parts) => parts.len(),
                    LayerRun::Objects(draws) => draws.len(),
                };
                let (from, to) = (at.max(s.units.start), (at + len).min(s.units.end));
                let first = at;
                at += len;
                if from >= to {
                    continue;
                }
                let draws = match run {
                    LayerRun::World(parts) => {
                        if (from, to) != (first, first + len) {
                            return Err("a structure run split by a segment");
                        }
                        match culled.as_mut() {
                            Some(world) => world.next().ok_or("a structure run drew nothing")?,
                            None => self.world_run(t, view, parts)?,
                        }
                    }
                    LayerRun::Objects(draws) => {
                        self.object_run(t, view, &draws[from - first..to - first], texaccum, view_copy, scene_copy)?
                    }
                };
                out.ranges += draws.ranges;
                out.packets.extend(draws.packets);
                out.discards |= draws.discards;
                out.writes.extend(draws.writes);
            }
            if out.discards {
                return Err("a discarding draw in a layer recorded as commands");
            }
            for (dst, offset, bytes) in out.writes {
                let (src, from) = t.uploads.stage(&t.device, &t.queue, &bytes);
                writes.push((src, from, dst, offset, bytes.len() as u64));
            }
            ranges += out.ranges;
            count += out.packets.len();
            recorded.push((s.copies_colour, s.copies_target, out.packets));
        }
        let snapshots = usize::from(scene.is_some()) + usize::from(structure.is_some());
        let passes = recorded.len() + snapshots;
        let (color, depth) = (t.color.clone(), t.depth.clone());
        let stencil = t.depth_format.has_stencil_aspect();
        let device = t.device.clone();
        let copy = copy.map(|c| (c.from, c.to, c.extent));
        let commands = move |encoder: &mut wgpu::CommandEncoder| {
            for (src, at, dst, offset, len) in &writes {
                encoder.copy_buffer_to_buffer(src, *at, dst, *offset, *len);
            }
            if let Some(dispatch) = &dispatch {
                dispatch.record(encoder);
            }
            if let Some(scene) = &scene {
                scene.record(&device, encoder);
            }
            for (copies_colour, copies_target, packets) in &recorded {
                if *copies_colour && let Some((from, to, extent)) = &copy {
                    encoder.copy_texture_to_texture(from.as_image_copy(), to.as_image_copy(), *extent);
                }
                if *copies_target && let Some(structure) = &structure {
                    structure.record(&device, encoder);
                }
                let mut pass = load_pass(encoder, &color, &depth, stencil);
                // Checked when the packets were made: every pipeline
                // built, no counted draw.
                let _ = gfx::renderer::encode_in_pass(&mut pass, packets);
            }
        };
        Ok(ExternalEncoded { ranges, draws: count + snapshots, passes, commands: Box::new(commands) })
    }
}

#[cfg(test)]
mod restore_tests {
    use super::*;
    use engine::memory::FlatMemory;
    use engine::render_loop::states::{DIRTY, SHADOW};
    use render_bridge::open_shader::Write;

    fn memory() -> FlatMemory {
        FlatMemory::new(0x0040_0000, 0x18_0000)
    }

    /// The replay writes each entry as `0x00018900`'s call leaves guest
    /// memory: the simple state's shadow, the deferred state's mask and
    /// shadow.
    #[test]
    fn the_open_shaders_restore_writes_each_entry_as_its_call_would() {
        let mut m = memory();
        m.set_u32(DIRTY, 0x1).unwrap();
        let writes = [
            Write::Simple { state: 0x10, header: (1 << 18) | (4 * 0x11), value: 1 },
            Write::Deferred { state: 0x60, mask: 0x20, value: 7 },
        ];
        restore_list(&mut m, &writes).unwrap();
        assert_eq!(m.u32(SHADOW + 0x40).unwrap(), 1);
        assert_eq!(m.u32(SHADOW + 0x180).unwrap(), 7);
        assert_eq!(m.u32(DIRTY).unwrap(), 0x21);
    }

    /// A write through a setter is a named gap, before any write.
    #[test]
    #[should_panic(expected = "layers: restore list write through a render state setter")]
    fn a_setter_write_is_a_named_gap() {
        let mut m = memory();
        let writes = [Write::Setter { state: 147, setter: 0x003f_53d0, value: 0x405 }];
        let _ = restore_list(&mut m, &writes);
    }

    /// A sorted run's item making the frame's first `0x0003bea0` call is
    /// skipped as layer 15's entry is: the cache's restore list written
    /// before the pass, then with the commit the copy
    /// flag, the draw byte and the cleared cache, so the next entry's
    /// shader draw finds no open shader.
    #[test]
    fn a_sorted_run_item_making_the_copy_writes_the_restore_list_and_clears_the_cache() {
        use engine::render_loop::layer::DRAW_FLAG;
        use engine::render_loop::states::{HEADERS, MASKS};
        use render_bridge::frame_step::layer::{SETUP, SETUP_FLOAT, SETUP_OPEN};
        use render_bridge::open_shader::{DESCRIPTOR_STATES, IMPLEMENTATION_RESTORE, SETUP_DESCRIPTOR};
        let mut m = memory();
        let (implementation, descriptor, table) = (0x0050_0000, 0x0050_1000, 0x0050_2000);
        m.set_u32(SETUP + SETUP_OPEN, implementation).unwrap();
        m.set_u32(SETUP + SETUP_DESCRIPTOR, descriptor).unwrap();
        m.set_u32(descriptor + DESCRIPTOR_STATES, table).unwrap();
        // Two entries from the table's fourth: a simple state, a deferred one.
        m.set_u16(implementation + IMPLEMENTATION_RESTORE, (2 << 9) | 3).unwrap();
        for (i, (state, value)) in [(0x10u8, 1u32), (0x60, 7)].into_iter().enumerate() {
            let at = table + 5 * (3 + i as u32);
            m.set_u8(at, state).unwrap();
            m.set_u32(at + 1, value).unwrap();
        }
        m.set_u32(HEADERS + 4 * 0x10, (1 << 18) | (4 * 0x11)).unwrap();
        m.set_u32(MASKS + 4 * 0x60, 0x20).unwrap();
        m.set_u32(SETUP_FLOAT, 0x3f80_0000).unwrap();

        let first = FirstCopy::read(&m).unwrap();
        first.restore(&mut m).unwrap();
        assert_eq!(m.u32(SHADOW + 0x40).unwrap(), 1, "the simple state");
        assert_eq!(m.u32(SHADOW + 0x180).unwrap(), 7, "the deferred state");
        assert_eq!(m.u32(DIRTY).unwrap() & 0x20, 0x20, "its mask");
        assert_eq!(m.u32(SETUP + SETUP_OPEN).unwrap(), implementation, "held until the commit");
        assert_eq!(m.u8(TARGET_COPIED).unwrap(), 0);

        FirstCopy::finish(&mut m).unwrap();
        assert_eq!(m.u8(TARGET_COPIED).unwrap(), 1);
        assert_eq!(m.u8(DRAW_FLAG).unwrap(), 1);
        assert_eq!(m.u32(SETUP + SETUP_OPEN).unwrap(), 0, "the cache cleared");
        assert_eq!(m.u32(SETUP + 0x1330).unwrap(), 0x3f80_0000, "as 0x00016b10 clears it");
        assert!(FirstCopy::read(&m).unwrap().0.is_none(), "no open shader left");
    }
}

#[cfg(test)]
mod accumulation_tests {
    use super::{Segment, layer_segments};

    /// Each draw sampling the accumulation target starts a segment with a
    /// copy of the bound colour, so it reads every draw before it; the
    /// draws after it up to the next one join its segment; leading draws
    /// that sample nothing take no copy.
    #[test]
    fn each_draw_sampling_the_accumulation_target_reads_a_copy_made_just_before_it() {
        let seg = |copies_colour, copies_target, units| Segment { copies_colour, copies_target, units };
        let got = layer_segments([false, false, true, false, true, true, false].map(|s| (s, false)));
        assert_eq!(got, [seg(false, false, 0..2), seg(true, false, 2..4), seg(true, false, 4..5), seg(true, false, 5..7)]);
        assert_eq!(layer_segments([(true, false)]), [seg(true, false, 0..1)]);
        assert!(layer_segments([]).is_empty());
        // The structure target copy starts a segment of its own, and a
        // draw that samples there takes both.
        let got = layer_segments([(false, false), (false, true), (false, false), (true, false)]);
        assert_eq!(got, [seg(false, false, 0..1), seg(false, true, 1..3), seg(true, false, 3..4)]);
        assert_eq!(layer_segments([(true, true)]), [seg(true, true, 0..1)]);
    }
}

#[cfg(test)]
mod target_copy_tests {
    use super::*;

    /// An item whose guest draw would set the frame's copy flag is taken
    /// as planned but never order-free: the skip draws it only ahead of
    /// every kept item, where its `0x0003bea0` would leave nothing but
    /// the flag.
    #[test]
    fn an_item_setting_the_copy_flag_is_never_order_free() {
        let item = |sets_copied| ObjectItem { shader: 7, marked: false, sets_copied, copies: false, draws: Vec::new() };
        let plain = object_skip(Some(&item(false)), 7, true);
        assert!(plain.take && plain.free);
        let copies = object_skip(Some(&item(true)), 7, true);
        assert!(copies.take && !copies.free);
        assert!(!object_skip(Some(&item(false)), 8, true).take, "another shader");
        assert!(!object_skip(None, 7, true).take);
        assert_eq!(TARGET_COPIED, 0x0050_9430);
    }

    /// The copy is made before the first draw of the first drawn item
    /// that copies, counted over every object draw before it; a later one
    /// finds it made.
    #[test]
    fn the_structure_target_copy_is_made_before_the_first_item_that_copies() {
        assert_eq!(copy_point([(false, 2), (true, 3), (true, 1)]), Some(2));
        assert_eq!(copy_point([(true, 1), (false, 4)]), Some(0), "the layer's first draw");
        assert_eq!(copy_point([(false, 2), (false, 1)]), None, "nothing copies");
        assert_eq!(STRUCTURE_COPY_TARGET, 0x16, "0x000352e0(0x16, 1) at 0x0003bed4");
    }


}
