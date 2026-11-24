//! One colour fan at its original target and sampler bindings.
use std::sync::{Arc, Mutex};

use d3d::backend::{ExternalAttachments, ExternalDraws};
use d3d::{
    DrawSnapshot, ExternalPass, ExternalPassRequest, ExternalPlan, ExternalWrites, ResolvedDraw,
};
use native_render::flares::{FlareGpu, Site};
use timeline::native::Stage;

pub(super) fn plan(
    site: Site,
    snapshot: &DrawSnapshot<'_>,
    gpu: &Arc<Mutex<FlareGpu>>,
) -> Option<(ExternalPlan, Box<dyn ExternalPass>)> {
    if !matches!(snapshot.draw.entry, "D3DDevice_End" | "inline Begin/End")
        || snapshot.color_header == 0
        || snapshot.draw.hw.zpass_count_enable
        || native_render::flares::check(site, snapshot.draw).is_err()
    {
        return None;
    }
    Some((
        ExternalPlan {
            stage: Stage::Flares,
            view: 4,
            ranges: 1,
            slots: Vec::new(),
        },
        Box::new(FlarePass {
            site,
            draw: Arc::new(snapshot.draw.clone()),
            gpu: Arc::clone(gpu),
            frame: crate::d3d_diag::presents() as u32,
        }),
    ))
}

struct FlarePass {
    site: Site,
    draw: Arc<ResolvedDraw>,
    gpu: Arc<Mutex<FlareGpu>>,
    frame: u32,
}

impl ExternalPass for FlarePass {
    fn replays_scene(&self) -> bool {
        // Immutable issue-time state. prepare obtains every attachment and
        // sampled texture from the copy currently being recorded.
        true
    }

    fn record_draws(
        &mut self,
        target: &ExternalAttachments,
        request: ExternalPassRequest,
    ) -> Option<Result<ExternalDraws, &'static str>> {
        if request.surface_size != (self.draw.target.width, self.draw.target.height) {
            return Some(Err("flare target size"));
        }
        let draw = self.draw.for_copy(target.copy);
        Some(
            self.gpu
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .prepare(self.site, target, draw.as_ref(), self.frame)
                .map_err(|_| "flare fan preparation"),
        )
    }

    fn resolved_fallback(&self) -> Option<Arc<ResolvedDraw>> {
        Some(Arc::clone(&self.draw))
    }

    fn writes(&self) -> ExternalWrites {
        ExternalWrites {
            color: self.draw.state.render.color_write.xbox_value() != 0,
            depth_stencil: self.draw.writes_depth_stencil(),
        }
    }

    fn colour_only(&self) -> bool {
        self.draw.target.depth_format.is_none()
    }
}
