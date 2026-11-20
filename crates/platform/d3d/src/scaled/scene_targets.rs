//! A native game frame borrowing the display store. At a single common
//! scale the live colour and depth images are already authoritative for
//! later reads, shadows and resizes. Only accept work that needs no
//! transfer to a guest-size image. Export the same shared sampled image
//! and display-owned visibility ticket that a single-copy draw produces.

use std::collections::HashMap;

use frame_scene::TargetRef;
use gfx::surface::SurfaceId;

use super::{DISPLAY_TICKET, Lighting, ScaledBackend, ScaledSurface, ScaledTexture};
use crate::gfx_backend::{GfxBackend, HostTexture};
use crate::scene_sink::{ReplayFrame, ReplayOp, ReplaySample, ReplayTexture};
use crate::threaded::owner::Outputs;

impl ScaledBackend<GfxBackend, GfxBackend> {
    /// Resolve every attachment on the render thread, before changing any
    /// surface. A mixed-size pair can require a proxy, downsample or refill
    /// in `draw_single`. The regular scaled path retains those frames.
    pub fn scene_targets(
        &self,
        frame: &ReplayFrame,
        targets: &HashMap<TargetRef, ScaledSurface<SurfaceId, SurfaceId>>,
    ) -> Option<HashMap<TargetRef, SurfaceId>> {
        if !self.single()
            || self.frame_failed
            || !frame.refusals.is_empty()
            || !frame.ops.iter().all(|(_, op)| eligible_op(op))
        {
            return None;
        }

        let physical: HashMap<_, _> = targets
            .iter()
            .map(|(&name, surface)| self.view_surface(surface).map(|id| (name, id)))
            .collect::<Option<_>>()?;
        self.view
            .matches_scene_targets_at(frame, &physical, self.scale)
            .then_some(physical)
    }

    /// Uniform targets use the baseline carried receiver in both graph
    /// modes. Restore the scaled backend's receiver for subsequent views
    /// before the normal Swap boundary can apply new settings.
    pub fn with_scene_view<T>(&mut self, record: impl FnOnce(&mut GfxBackend) -> T) -> T {
        let hard = self.lighting == Lighting::E3;
        if hard {
            self.view.set_hard_shadows(false);
        }
        let result = record(&mut self.view);
        if hard {
            self.view.set_hard_shadows(true);
        }
        result
    }

    /// The graph has already presented the physical view and applied
    /// gamma. Account once and run the same live scale/lighting boundary
    /// as a single-copy Swap, including preserving old surface contents.
    pub fn scene_presented(
        &mut self,
        frame: &ReplayFrame,
        outputs: Outputs<HostTexture>,
    ) -> Outputs<ScaledTexture<HostTexture, HostTexture>> {
        let outputs = Outputs {
            tickets: outputs
                .tickets
                .into_iter()
                .map(|(proxy, id)| (proxy, id | DISPLAY_TICKET))
                .collect(),
            // GfxBackend::export_texture shares this same GPU allocation.
            samples: outputs
                .samples
                .into_iter()
                .map(|(id, texture)| {
                    (
                        id,
                        ScaledTexture {
                            reference: texture.shared(),
                            view: Some(texture),
                        },
                    )
                })
                .collect(),
        };
        self.stats.scaled_depth_aliases += frame
            .ops
            .iter()
            .filter(|(_, op)| {
                matches!(
                    op,
                    ReplayOp::Sample {
                        how: ReplaySample::Alias { .. },
                        ..
                    }
                )
            })
            .count() as u64;
        self.stats.display_draws += frame
            .ops
            .iter()
            .map(|(_, op)| match op {
                ReplayOp::Draw { .. } => 1,
                ReplayOp::Stage(stage) => stage.plan.ranges as u64,
                _ => 0,
            })
            .sum::<u64>();
        self.stats.view_frames += 1;
        self.frame_failed = false;
        self.finish_frame();
        outputs
    }
}

fn eligible_op(op: &ReplayOp) -> bool {
    match op {
        ReplayOp::Bind { .. }
        | ReplayOp::Clear { .. }
        | ReplayOp::ClearRects { .. }
        | ReplayOp::Copy { .. }
        | ReplayOp::Sample { .. }
        | ReplayOp::BeginFlareQuery
        | ReplayOp::EndFlareQuery(_) => true,
        ReplayOp::Draw { textures, .. } => textures
            .iter()
            .flatten()
            .all(|t| !matches!(t, ReplayTexture::Placeholder(_))),
        // The stage receives the same display attachments and material
        // inputs it would get in the single-copy worker.
        ReplayOp::Stage(stage) => stage.pass.lock().is_ok_and(|pass| pass.replays_scene()),
        // The original upload also refreshes the guest-sized surface.
        ReplayOp::Upload { .. } => false,
    }
}
