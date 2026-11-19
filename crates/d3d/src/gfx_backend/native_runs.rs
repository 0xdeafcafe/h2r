//! Ordered native commands share the pending draw list when they can use
//! its attachments. Opaque bundles keep their own run.
use super::{GfxBackend, PendingPass, SurfaceId};
use crate::backend::{ExternalBundles, ExternalDraws, TargetCopy};
use std::sync::atomic::{AtomicU64, Ordering};
use timeline::native::{Metric, Stage, Timer};

pub(super) struct Run {
    key: (u64, SurfaceId, SurfaceId),
    copy: TargetCopy,
    stage: Stage,
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
    stencil: bool,
    draws: Vec<ExternalBundles>,
}

static PASSES: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

fn metric(copy: TargetCopy) -> Metric {
    match copy {
        TargetCopy::Reference => Metric::ReferencePasses,
        TargetCopy::Display => Metric::DisplayPasses,
    }
}

impl PendingPass {
    pub(super) fn new(color: SurfaceId, depth: Option<SurfaceId>) -> Self {
        Self {
            color,
            depth,
            clear: gfx::Clear::NONE,
            draws: Vec::new(),
            encoded: 1,
            tags: Vec::new(),
            native: Vec::new(),
        }
    }

    pub(super) fn count_native_passes(&self) {
        if self.native.is_empty() || !timeline::phase::enabled() {
            return;
        }
        let mut native = self.native.iter().peekable();
        let mut seen = 0u64;
        for (i, draw) in self.draws.iter().enumerate() {
            if draw.starts_pass() || draw.counts_samples() {
                seen = 0;
            }
            let Some((range, stage, copy)) = native.peek().copied() else {
                break;
            };
            if range.contains(&i) {
                let bit = 1u64 << (*stage as usize * 2 + usize::from(*copy == TargetCopy::Display));
                if seen & bit == 0 {
                    stage.count(metric(*copy), 1);
                    seen |= bit;
                }
                if range.end == i + 1 {
                    native.next();
                }
            }
        }
    }
}

impl GfxBackend {
    pub fn native_bundle_passes() -> [u64; 2] {
        std::array::from_fn(|i| PASSES[i].load(Ordering::Relaxed))
    }

    pub(super) fn encode_before_external(&mut self) -> bool {
        // A commit has no result channel. Preserve a fatal backend error for
        // the next operation with one, as the threaded backend does.
        if let Err(error) = self.encode_pending() {
            self.external_ordering_error = Some(error);
            return false;
        }
        true
    }

    pub(super) fn append_native(
        &mut self,
        stage: Stage,
        copy: TargetCopy,
        targets: (SurfaceId, SurfaceId),
        color: wgpu::TextureView,
        depth: wgpu::TextureView,
        stencil: bool,
        draws: ExternalBundles,
    ) {
        let key = (draws.run, targets.0, targets.1);
        if let Some(run) = &mut self.native_run
            && (run.key, run.copy, run.stage) == (key, copy, stage)
            && run.color == color
            && run.depth == depth
            && run.stencil == stencil
        {
            run.draws.push(draws);
            return;
        }
        self.finish_native_run();
        self.native_run = Some(Run {
            key,
            copy,
            stage,
            color,
            depth,
            stencil,
            draws: vec![draws],
        });
    }

    pub(super) fn append_native_draws(
        &mut self,
        stage: Stage,
        copy: TargetCopy,
        (color, depth): (SurfaceId, SurfaceId),
        draws: ExternalDraws,
    ) -> bool {
        self.finish_native_run();
        let depth = (depth != color).then_some(depth);
        if self.pending.as_ref().is_some_and(|p| (p.color, p.depth) != (color, depth))
            && !self.encode_before_external()
        {
            return false;
        }
        let _encode = Timer::start(stage, Metric::EncodeNs);
        let start = self.pending.as_ref().map_or(0, |p| p.draws.len());
        self.frame_draw += 1;
        for (i, packet) in draws.packets.into_iter().enumerate() {
            self.append_recorded(color, depth, packet, i == 0, draws.discards);
        }
        let pending = self.pending.as_mut().expect("at least one validated draw");
        pending.native.push((start..pending.draws.len(), stage, copy));
        true
    }

    pub(super) fn finish_native_run(&mut self) {
        let Some(run) = self.native_run.take() else { return };
        let _encode = Timer::start(run.stage, Metric::EncodeNs);
        let (encoder, passes) = self.open.get_or_insert_with(||
            (self.ctx.device.create_command_encoder(&Default::default()), 0));
        ExternalBundles::encode_run(encoder, &run.color, &run.depth, run.stencil, &run.draws);
        *passes += 1;
        self.stats.render_passes += 1;
        run.stage.count(metric(run.copy), 1);
        if run.stage == Stage::Lights {
            PASSES[usize::from(run.copy == TargetCopy::Display)].fetch_add(1, Ordering::Relaxed);
        }
    }
}
