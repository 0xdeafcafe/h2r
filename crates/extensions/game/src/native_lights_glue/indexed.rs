//! A gathered handler can draw a visible subpart or apply live material
//! state. Its resolved emission contains that exact index window, streams,
//! constants and stage bindings. Record just this window in the light layer.

use std::sync::{Arc, Mutex};

use d3d::{DrawAdmission, DrawSnapshot, ExternalPass, ExternalPlan};
use native_render::gels::{TARGET, TARGET_STRIDE, TARGETS};
use native_render::world_layers_gpu::IndexedPass;
use render_bridge::native_lights::{IndexedLight, indexed_light};

use super::{Binding, Checkpoint, Outputs};

/// The embedded texture header returned by the target getter for target 13.
const GEL_TEXTURE: u32 = TARGETS + TARGET_STRIDE * TARGET + 4;

pub(super) struct Admission {
    cache: crate::native_map::MapIndexedCache,
    gels: Arc<Outputs>,
    checkpoint: Arc<Mutex<Option<(u64, Checkpoint)>>>,
}

impl Admission {
    pub(super) fn new(
        gels: Arc<Outputs>,
        checkpoint: Arc<Mutex<Option<(u64, Checkpoint)>>>,
    ) -> Self {
        Self {
            cache: Default::default(),
            gels,
            checkpoint,
        }
    }

    fn gel(&self, light: &IndexedLight) -> Option<Binding> {
        let (serial, before) = (*self.checkpoint.lock().ok()?)?;
        if serial != light.serial {
            return None;
        }
        let (definition, shader) = light.shader_gel?;
        self.gels.produced_after(before, definition, shader)
    }
}

/// A read of the colour or depth attachment cannot use the open render
/// pass. A sampled target-13 header needs the current invocation's slot.
fn samples_gel(
    targets: [u32; 2],
    sampled: [bool; 4],
    bound: [u32; 4],
    resolved: [Option<u32>; 4],
) -> Option<bool> {
    let mut gel = false;
    for (n, reads) in sampled.into_iter().enumerate() {
        if reads {
            if bound[n] == 0 || targets.contains(&bound[n]) || resolved[n] != Some(bound[n]) {
                return None;
            }
            gel |= bound[n] == GEL_TEXTURE;
        }
    }
    Some(gel)
}

impl DrawAdmission for Admission {
    fn admit(&self, snapshot: &DrawSnapshot<'_>) -> Option<(ExternalPlan, Box<dyn ExternalPass>)> {
        let ctx = crate::native_world_layers_glue::indexed_draw()?;
        let layer = ctx.layer;
        let light = indexed_light(layer.number, layer.caller_return, layer.list)?;
        ctx.geometry?; // Standard geometry handler and one of its indexed emitter calls.
        let draw = snapshot.draw;
        if render_bridge::native_map_work::Stage::Lights.standing_down()
            || hot_render::global().active()
            || light.targets != [snapshot.color_header, snapshot.depth_header]
            || draw.entry != "D3DDevice_DrawIndexedVertices"
            || draw.indices.as_ref()?.len() != ctx.index_count? as usize
            || !draw.state.bad.is_empty()
            || draw.glyph_cache != 0
        {
            return None;
        }
        let generated = samples_gel(
            light.targets,
            snapshot.sampled_stages,
            snapshot.bound_textures,
            draw.textures,
        )?;
        let mut plan = ExternalPlan {
            stage: timeline::native::Stage::Lights,
            view: layer.view,
            ranges: 1,
            slots: Vec::new(),
        };
        let mut pass = IndexedPass::new(draw, self.cache.get(), ctx.frame, None)?;
        if generated {
            let binding = self.gel(&light)?;
            binding.append(&mut plan);
            pass.pin_sampled_surface(GEL_TEXTURE, binding.guest_size);
        }
        Some((plan, Box::new(pass)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexed_light_pins_only_a_sampled_gel_and_refuses_feedback() {
        let targets = [10, 11];
        let bound = [20, GEL_TEXTURE, 10, 11];
        let resolved = bound.map(Some);
        assert_eq!(
            samples_gel(targets, [true, false, false, false], bound, resolved),
            Some(false)
        );
        assert_eq!(
            samples_gel(targets, [true, true, false, false], bound, resolved),
            Some(true)
        );
        for i in 2..4 {
            let mut sampled = [false; 4];
            sampled[i] = true;
            assert_eq!(samples_gel(targets, sampled, bound, resolved), None);
        }
        let mut stale = resolved;
        stale[1] = Some(99);
        assert_eq!(
            samples_gel(targets, [true, true, false, false], bound, stale),
            None
        );
    }
}
