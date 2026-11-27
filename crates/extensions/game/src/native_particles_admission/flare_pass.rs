//! One flare fan at its original targets, with a count pipeline for queries.
//! A fan left to the device is `scene_kept_flare_<site>_<cause>` ([`kept`]).
use std::sync::{Arc, Mutex};

use d3d::backend::{ExternalAttachments, ExternalDraws};
use d3d::{
    DrawSnapshot, ExternalPass, ExternalPassRequest, ExternalPlan, ExternalWrites, ResolvedDraw,
};
use native_render::flares::{self, FlareGpu, Refusal, Site};
use timeline::native::Stage;

pub(super) fn plan(
    site: Site,
    snapshot: &DrawSnapshot<'_>,
    gpu: &Arc<Mutex<FlareGpu>>,
) -> Option<(ExternalPlan, Box<dyn ExternalPass>)> {
    if !matches!(snapshot.draw.entry, "D3DDevice_End" | "inline Begin/End")
        || snapshot.color_header == 0
    {
        kept(site, "entry");
        return None;
    }
    if let Err(reason) = flares::check(site, snapshot.draw) {
        refuse_visibility(site, reason);
        kept(site, reason.cause());
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

/// The fan at `site` keeps its guest draw for `cause`.
pub(super) fn kept(site: Site, cause: &'static str) {
    render_bridge::native_hud::kept::count(render_bridge::native_hud::kept::LateKind::Flare(site.name()), cause);
}

fn refuse_visibility(site: Site, reason: Refusal) {
    if site == Site::Visibility {
        let label = match reason {
            Refusal::QueryState => "VisibilityState",
            Refusal::Geometry => "VisibilityFanGeometry",
            Refusal::Layout => "VisibilityFanDeclaration",
            Refusal::Texture => "VisibilityTexture",
            Refusal::Ui(_) => "VisibilityCounterPreparation",
        };
        super::REFUSALS.count("Flare", label);
    }
}

struct FlarePass {
    site: Site,
    draw: Arc<ResolvedDraw>,
    gpu: Arc<Mutex<FlareGpu>>,
    frame: u32,
}

impl ExternalPass for FlarePass {
    fn counts_samples(&self) -> bool {
        self.site == Site::Visibility
    }

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
                .map_err(|reason| {
                    refuse_visibility(self.site, reason);
                    kept(self.site, "preparation");
                    "flare fan preparation"
                }),
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
