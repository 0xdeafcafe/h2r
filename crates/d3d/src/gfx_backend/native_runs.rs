//! Join already confirmed, immutable render bundles in submission order.
use super::{GfxBackend, SurfaceId};
use crate::backend::{ExternalBundles, TargetCopy};
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) struct Run {
    key: (u64, SurfaceId, SurfaceId),
    copy: TargetCopy,
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
    stencil: bool,
    draws: Vec<ExternalBundles>,
}

static PASSES: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

impl GfxBackend {
    pub fn native_bundle_passes() -> [u64; 2] {
        std::array::from_fn(|i| PASSES[i].load(Ordering::Relaxed))
    }

    pub(super) fn append_native(&mut self, copy: TargetCopy, targets: (SurfaceId, SurfaceId),
        color: wgpu::TextureView, depth: wgpu::TextureView, stencil: bool, draws: ExternalBundles) {
        let key = (draws.run, targets.0, targets.1);
        if self.native_run.as_ref().is_some_and(|r| r.key != key || r.copy != copy) {
            self.finish_native_run();
        }
        self.native_run.get_or_insert_with(|| Run {
            key, copy, color, depth, stencil, draws: Vec::new(),
        }).draws.push(draws);
    }

    pub(super) fn finish_native_run(&mut self) {
        let Some(run) = self.native_run.take() else { return };
        let (encoder, passes) = self.open.get_or_insert_with(||
            (self.ctx.device.create_command_encoder(&Default::default()), 0));
        ExternalBundles::encode_run(encoder, &run.color, &run.depth, run.stencil, &run.draws);
        *passes += 1;
        let metric = match run.copy {
            TargetCopy::Reference => timeline::native::Metric::ReferencePasses,
            TargetCopy::Display => timeline::native::Metric::DisplayPasses,
        };
        timeline::native::Stage::Lights.count(metric, 1);
        PASSES[usize::from(run.copy == TargetCopy::Display)].fetch_add(1, Ordering::Relaxed);
    }
}
