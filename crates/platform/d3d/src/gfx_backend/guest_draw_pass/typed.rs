//! Record a retained stage at the same point in a replay as on the guest's
//! render thread. Each recording resolves its own target views and upload
//! arena; the immutable mesh and material allocations are shared.

use frame_scene::TargetRef;

use super::GuestDrawPass;
use crate::backend::{Backend, ExternalCopies, SceneStage, TargetCopy};

impl GuestDrawPass<'_> {
    pub(super) fn record_stage(&mut self, stage: &SceneStage<TargetRef>) -> Result<(), String> {
        let mut pass = stage.pass.lock().map_err(|_| "scene stage poisoned")?;
        if !pass.replays_scene() {
            return Err("scene stage cannot be recorded again".into());
        }
        let surfaces = stage.surfaces.try_map(|r| self.surface(*r).ok()).ok_or("missing scene stage target")?;
        let ExternalCopies::Ready(targets) = self.gpu.external_attachments(surfaces, [None; 4], TargetCopy::Display)? else {
            return Err("scene stage attachment is unavailable".into());
        };
        let [target] = targets.as_slice() else {
            return Err("expected one scene stage target copy".into());
        };
        // Stage uploads and commands belong to this submission. Never call
        // the guest reservation, skip, submission callback or drop counters.
        let ranges = stage.plan.ranges;
        if ranges == 0 {
            return Err("scene stage has no ranges".into());
        }
        let (issued, draws) = if let Some(draws) = pass.record_draws(target, stage.request) {
            let draws = draws?;
            if draws.ranges != ranges || draws.packets.is_empty() || draws.packets.iter().any(gfx::RecordedDraw::counts_samples) {
                return Err("scene stage did not produce the promised draw packets".into());
            }
            let count = draws.packets.len();
            (self.gpu.append_native_draws(stage.plan.stage, target.copy, target.targets, draws, &target.uploads), count)
        } else if let Some(encoded) = pass.record_encoded(target, stage.request) {
            let encoded = encoded?;
            if encoded.ranges != ranges {
                return Err("scene stage did not encode the promised ranges".into());
            }
            let count = encoded.draws;
            (self.gpu.append_native_encoded(stage.plan.stage, target.copy, encoded), count)
        } else {
            return Err("scene stage has no repeatable recording".into());
        };
        if !issued {
            return Err(self.gpu.external_ordering_error.take().unwrap_or_else(|| "could not record scene stage".into()));
        }
        self.gpu.stats.draws += draws as u64;
        self.gpu.retain_uploads(target.uploads.clone());
        self.gpu.submit_full_external_batch();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{ExternalAttachments, ExternalEncoded};
    use crate::scene_sink::{ReplayFrame, ReplayOp};
    use crate::{ExternalPass, ExternalPassRequest, ExternalPlan, ExternalSurfaces, GfxBackend};
    use frame_scene::{AuxTarget, Slot, SlotAt};
    use gfx::{Clear, D3DFormat, GfxContext};
    use std::sync::{Arc, Mutex};

    /// The entire effect of this invented stage is copying its sampled
    /// level to its bound colour. All target handles come from the call.
    struct CopyLevel;

    impl ExternalPass for CopyLevel {
        fn replays_scene(&self) -> bool { true }

        fn record_encoded(
            &mut self, target: &ExternalAttachments, _request: ExternalPassRequest,
        ) -> Option<Result<ExternalEncoded, &'static str>> {
            let source = target.sampled[0].as_ref()?.view.texture().clone();
            let dest = target.color.texture().clone();
            let size = source.size();
            Some(Ok(ExternalEncoded {
                ranges: 1,
                draws: 0,
                passes: 0,
                commands: Box::new(move |enc| {
                    enc.copy_texture_to_texture(source.as_image_copy(), dest.as_image_copy(), size);
                }),
            }))
        }
    }

    #[test]
    fn retained_stage_uses_the_current_private_attachments_on_every_draw() {
        let Ok(ctx) = GfxContext::headless() else { return };
        let mut gpu = GfxBackend::new(ctx);
        let source = TargetRef::Aux(0);
        let dest = TargetRef::SceneColour;
        let desc = AuxTarget { width: 8, height: 4, format: D3DFormat::LinA8R8G8B8 as u32, depth: false };
        let at = SlotAt { slot: Slot::AfterLights, view: 0 };
        let clear = |colour, rgba| ReplayOp::Clear {
            colour, depth: TargetRef::None, clear: Clear { color: Some(rgba), ..Clear::NONE },
        };
        let stage = SceneStage {
            surfaces: ExternalSurfaces {
                color: dest, depth: dest, sampled: vec![source], slots: vec![], slot_sizes: vec![],
            },
            request: ExternalPassRequest { rect: (0, 0, 8, 4), surface_size: (8, 4), z_scale: 1.0 },
            plan: ExternalPlan { stage: timeline::native::Stage::Water, view: 0, ranges: 1, slots: vec![] },
            pass: Arc::new(Mutex::new(Box::new(CopyLevel))),
        };
        let frame = ReplayFrame {
            ops: vec![
                (at, clear(source, [1., 0., 0., 1.])),
                (at, clear(dest, [0., 0., 1., 1.])),
                (at, ReplayOp::Stage(stage)),
            ],
            presented: dest,
            colour: Some(desc),
            aux: vec![desc],
            ..Default::default()
        };
        for scale in [1, 4] {
            let mut pass = GuestDrawPass::new(&mut gpu, &frame, scale).unwrap();
            for (_, op) in &frame.ops {
                pass.apply(op).unwrap();
            }
            let (size, rgba) = pass.read_rgba().unwrap();
            assert_eq!(size, [8 * scale, 4 * scale]);
            assert!(rgba.chunks_exact(4).all(|pixel| pixel == [255, 0, 0, 255]));
            assert!(gpu.last_presented().is_none());
        }
    }
}
