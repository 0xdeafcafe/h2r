//! The host of `frame-scene.layers` (docs/graphics/framescene.md, slice 4):
//! the scene's world layers 1, 5, 7, 8 and 20 drawn from the scene's plan
//! (`native_render::layer_pass`) over the native world's loaded BSP, not
//! from the draws the guest resolves under the layer call.
//!
//! - **At load** ([`load`], `native_render::layers_load`, on the world's
//!   worker, with the fog's): each
//!   world material's passes of each layer compiled with their own
//!   programs (`fog_gpu::layer_passes` and `FogProgram`, the same
//!   translation the fog's layer 16 takes), all or nothing per material
//!   and layer, their stage images decoded; the parts that can draw them;
//!   and each material resolved once, with its instance's animated
//!   properties and input scales, for the plan.
//! - **Per layer call** ([`Glue::ask`], before the layer draw of a scene
//!   view): the bridge extracts the layer's pass from guest memory
//!   (`render_bridge::native_layers::extract`); the plan takes each item
//!   or names why not; a planned item is drawn when it is a structure
//!   item (a cluster's or a placement's), its draw is static (the
//!   material as resolved at the load, no animation: what the compiled
//!   programs hold), and the layer's set draws its part with its shader.
//!   The skip clears the layer's bit on the run of such items from the
//!   list's first (`layer_pass::clear_prefix`); the rest, and every item
//!   after the first refused one, keep their D3D draws. One external pass
//!   records them.
//! - **The pass** ([`LayerViewPass`]): the device's view constants and W
//!   clip at the issue, then `GpuWorld::layer_draws` in the D3D pass open
//!   on the bound attachments.
//!
//! Each ask's outcomes are counted by layer and cause
//! (`render_bridge::native_layers::count`) for `render.scene`.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use engine::memory::GuestMemory;
use frame_scene::FrameScene;
use native_render::layer_pass::{self, Instance, LayerRefusal, Materials};
use native_render::program::ViewInputs;
use native_render::world::Owner;
use render_bridge::native_layers::{Host, count};
use tags::material::{EngineInputs, Material};

use crate::native_fog_glue::{device_fog_words, fog_of};
use crate::native_world_glue::{ReadMemory, Ready, State, current_ready, palettes_hold};

// The load moved to native-render, so the editor loads the same layers
// (docs/editor/polish-plan.md, P15).
pub use native_render::layers_load::{LayerData, LayersView, load, upload_layers};
use native_render::layers_load::PER_DRAW;

/// The plan's view of the load's resolved shaders.
struct Loaded<'a>(&'a LayersView);

impl Materials for Loaded<'_> {
    fn material(&mut self, shader: u32, inputs: Option<&EngineInputs>) -> Result<Arc<Material>, String> {
        if inputs.is_some() {
            return Err(PER_DRAW.to_owned());
        }
        self.0.materials.get(&shader).cloned().ok_or_else(|| "not a world material".to_owned())
    }

    fn instance(&mut self, shader: u32) -> Result<Arc<Instance>, String> {
        self.0.instances.get(&shader).cloned().ok_or_else(|| "not a world material".to_owned())
    }
}

/// A refusal's cause as `render.scene` counts it.
fn cause(r: &LayerRefusal) -> &'static str {
    match r {
        LayerRefusal::Material(why) if why == PER_DRAW => "per_draw",
        r => r.name(),
    }
}

struct Glue<B: d3d::backend::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
    state: State,
}

impl<B: d3d::backend::Backend + Send + 'static> Host for Glue<B> {
    fn ask(&self, mem: &mut dyn GuestMemory, layer: u32, scene: &FrameScene) -> Option<native_render::skip::Cleared> {
        let l = u8::try_from(layer).ok()?;
        let ready = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            Arc::clone(current_ready(&state, mem)?)
        };
        let (view, palettes) = ready.layers.get()?;
        let data = view.layers.get(&l)?;
        let pass = scene.layer_passes.first()?;
        // The plan, from the scene alone.
        let plan = layer_pass::plan(scene, pass, &mut Loaded(view));
        let mut outcomes: BTreeMap<&'static str, u64> = BTreeMap::new();
        for (_, r) in &plan.refused {
            *outcomes.entry(cause(r)).or_default() += 1;
        }
        let mut taken: HashMap<u32, u32> = HashMap::new();
        for d in &plan.draws {
            let why = if layer_pass::world_owner(d.owner).is_none() {
                "object"
            } else if !layer_pass::is_static(d) {
                "animated"
            } else {
                taken.insert(d.slot, d.shader);
                continue;
            };
            *outcomes.entry(why).or_default() += 1;
        }
        let report = |outcomes: &BTreeMap<&'static str, u64>, drawn: u64| {
            for (why, &n) in outcomes {
                count(layer, &format!("refused_{why}"), n);
            }
            count(layer, "taken", drawn);
        };
        if data.programs.is_empty() {
            *outcomes.entry("program").or_default() += taken.len() as u64;
        }
        if taken.is_empty() || data.programs.is_empty() {
            report(&outcomes, 0);
            return None;
        }
        let Ok(mut d) = self.device.lock() else { return None };
        if !d.can_external_pass() || !matches!(d.begin_external_boundary(&mut crate::native_lights_glue::DeviceMemory(mem)), Ok(true)) {
            *outcomes.entry("device").or_default() += taken.len() as u64;
            report(&outcomes, 0);
            return None;
        }
        let read = ReadMemory(mem);
        // The passes draw without the stencil and with the palettes the
        // load expanded through: a view that holds anything else stays on
        // the D3D path (layers 1 and 3 begin in stencil mode 1).
        if !d.stencil_state(&read).is_some_and(|s| !s.enable) {
            *outcomes.entry("stencil").or_default() += taken.len() as u64;
            report(&outcomes, 0);
            return None;
        }
        if !palettes_hold(palettes, &|n| d.stage_palette(&read, n)) {
            *outcomes.entry("palettes").or_default() += taken.len() as u64;
            report(&outcomes, 0);
            return None;
        }
        ready.with_pipeline_target(&d, mem, |gpu, device, color, depth| {
            gpu.prewarm_layer(l, device, color, depth).then_some(())
        })?;
        let device_words = device_fog_words(&d, mem);
        let mut fogs = HashMap::new();
        for (&material, words) in &data.words {
            if let Some(resolved) = words.resolve(&device_words) {
                fogs.insert(material, fog_of(&resolved));
            }
        }
        let read = ReadMemory(mem);
        let mut preview = native_render::skip::MaskPreview::new(&read)?;
        let (drawn, cleared) = layer_pass::clear_prefix(&mut preview, layer, &|item| {
            taken.get(&item.slot) == Some(&item.shader)
                && data.part_set.get(&(item.owner, item.part)) == Some(&item.shader)
                && data.parts.iter().any(|&(k, (_, _, _, m))| {
                    k == (item.owner, item.part) && fogs.contains_key(&m)
                })
        })?;
        let masks = preview.finish()?;
        let ranges = drawn.len();
        // What the plan took that the skip did not: from the first item
        // the layer's set cannot draw (no compiled program, part or fog),
        // in the list's order.
        *outcomes.entry("order").or_default() += (taken.len() - ranges) as u64;
        report(&outcomes, ranges as u64);
        if ranges == 0 {
            return None;
        }
        let view_index = mem.u32(engine::render_loop::occlusion::guest::VIEW_INDEX).unwrap_or(u32::MAX);
        let plan = d3d::ExternalPlan { stage: timeline::native::Stage::World, view: view_index, ranges, slots: Vec::new() };
        let pass = Box::new(LayerViewPass {
            ready: Arc::clone(&ready),
            layer: l,
            selected: drawn.iter().map(|i| (i.owner, i.part)).collect(),
            fogs,
            view: None,
            w_clip: None,
            writes_depth: data.writes_depth,
            run: u64::from(d.swap_count()).wrapping_add(1),
        });
        let reservation = d
            .prepare_external_bound(&mut crate::native_lights_glue::DeviceMemory(mem), plan, pass, Vec::new())
            .ok()??;
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !current_ready(&state, mem).is_some_and(|current| Arc::ptr_eq(current, &ready)) {
            return None;
        }
        if !masks.apply(&mut render_bridge::native_fog::Mem(mem)) {
            return None;
        }
        reservation.commit();
        Some(cleared)
    }
}

/// Installs the host over the world's state (`native_world_glue::install`).
pub fn install<B: d3d::backend::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>, state: State) {
    render_bridge::native_layers::set_host(Arc::new(Glue { device: Arc::clone(device), state }));
}

/// One view's replacement of one layer.
struct LayerViewPass {
    ready: Arc<Ready>,
    layer: u8,
    /// The parts the masks surrendered, in the list's order.
    selected: Vec<(Owner, u8)>,
    /// Each material's fog unit and colour, the device's at the ask under
    /// the material's own words.
    fogs: HashMap<u16, (shaders::psh::fog::Fog, [f32; 4])>,
    view: Option<[[f32; 4]; d3d::VIEW_CONSTANTS]>,
    w_clip: Option<([f32; 2], bool)>,
    writes_depth: bool,
    run: u64,
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
        let mut guard = self.ready.gpu();
        let gpu = guard.as_mut().ok_or("world geometry unavailable")?;
        let set = gpu.layer(self.layer).ok_or("layer materials unavailable")?;
        if self.selected.iter().any(|&(owner, part)| !set.draws_part(owner, part)) {
            return Err("a selected layer part did not bind");
        }
        Ok(())
    }

    fn writes(&self) -> d3d::ExternalWrites {
        d3d::ExternalWrites { color: true, depth_stencil: self.writes_depth }
    }

    /// The layer's draws join the D3D render pass already open on these
    /// attachments. A refusal here leaves the D3D draws.
    fn record_draws(
        &mut self, t: &d3d::backend::ExternalAttachments, request: d3d::ExternalPassRequest,
    ) -> Option<Result<d3d::backend::ExternalDraws, &'static str>> {
        let Some(constants) = self.view else { return Some(Err("layer view constants unavailable")) };
        if self.w_clip.is_none() {
            return Some(Err("layer W-buffer clip unavailable"));
        }
        let mut guard = self.ready.gpu();
        let Some(gpu) = guard.as_mut() else { return Some(Err("world geometry unavailable")) };
        let view = ViewInputs { constants: request.view_constants(t.copy, constants), guest_size: request.surface_size, size: t.size, z_scale: request.z_scale, w_clip: self.w_clip };
        Some(gpu.layer_draws(self.layer, &t.device, t.color_format, t.depth_format, t.size, &self.selected, &view, &self.fogs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_per_draw_refusal_counts_apart_from_a_missing_material() {
        assert_eq!(cause(&LayerRefusal::Material(PER_DRAW.to_owned())), "per_draw");
        assert_eq!(cause(&LayerRefusal::Material("not a world material".to_owned())), "material");
        assert_eq!(cause(&LayerRefusal::NoPass), "no_pass");
    }

    #[test]
    fn the_plan_reads_only_what_the_load_resolved() {
        let view = LayersView::default();
        let mut loaded = Loaded(&view);
        assert_eq!(loaded.material(7, None).unwrap_err(), "not a world material");
        assert_eq!(loaded.material(7, Some(&EngineInputs::default())).unwrap_err(), PER_DRAW);
        assert!(loaded.instance(7).is_err());
    }
}
