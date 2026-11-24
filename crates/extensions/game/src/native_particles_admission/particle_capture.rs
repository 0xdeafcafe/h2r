//! The original emitter's published quad when its live declaration or
//! carried registers differ from the direct producer's reconstruction.
use std::sync::{Arc, Mutex};

use d3d::backend::{ExternalAttachments, ExternalDraws};
use d3d::{ExternalPass, ExternalPassRequest, ExternalWrites, ResolvedDraw};
use gfx::vertex::PrimitiveType;
use native_render::ui_gpu::UiGpu;

pub(super) fn corner_bytes() -> [u8; 32] {
    let corners = [[0i16, 0, 0, 0], [1, 0, 0, 0], [1, 1, 0, 0], [0, 1, 0, 0]];
    let mut bytes = [0; 32];
    for (value, pair) in corners.into_iter().flatten().zip(bytes.chunks_exact_mut(2)) {
        pair.copy_from_slice(&value.to_le_bytes());
    }
    bytes
}

pub(super) fn owns(draw: &ResolvedDraw) -> bool {
    draw.entry == "inline Begin/End"
        && draw.primitive == PrimitiveType::QuadList
        && draw.vertex_count >= 4
        && draw.indices.is_none()
        && matches!(draw.streams.as_slice(), [stream] if stream.data.as_ref() == corner_bytes())
}

pub(super) struct Pass {
    draw: Arc<ResolvedDraw>,
    gpu: Arc<Mutex<UiGpu>>,
    frame: u32,
}

impl Pass {
    pub(super) fn new(draw: &ResolvedDraw, gpu: &Arc<Mutex<UiGpu>>) -> Self {
        Self {
            draw: Arc::new(draw.clone()),
            gpu: Arc::clone(gpu),
            frame: crate::d3d_diag::presents() as u32,
        }
    }
}

impl ExternalPass for Pass {
    fn record_draws(
        &mut self,
        target: &ExternalAttachments,
        request: ExternalPassRequest,
    ) -> Option<Result<ExternalDraws, &'static str>> {
        if request.surface_size != (self.draw.target.width, self.draw.target.height) {
            return Some(Err("captured particle target size"));
        }
        let draw = self.draw.for_copy(target.copy);
        Some(
            self.gpu
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .prepare(target, draw.as_ref(), self.frame)
                .map_err(|_| "captured particle declaration or state"),
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
}
