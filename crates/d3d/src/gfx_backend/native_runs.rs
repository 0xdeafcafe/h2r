//! Join already confirmed, immutable render bundles in submission order.
use super::{GfxBackend, SurfaceId};
use crate::backend::{ExternalBundles, ExternalDraws, ExternalDrawTarget, TargetCopy};
use timeline::native::{Metric, Stage, Timer};
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) struct Run {
    key: (u64, SurfaceId, SurfaceId),
    copy: TargetCopy,
    stage: Stage,
    commands: Commands,
}

enum Commands {
    Bundles {
        color: wgpu::TextureView,
        depth: wgpu::TextureView,
        stencil: bool,
        draws: Vec<ExternalBundles>,
    },
    Draws {
        target: ExternalDrawTarget,
        packets: Vec<gfx::RecordedDraw>,
    },
}

static PASSES: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

impl GfxBackend {
    pub fn native_bundle_passes() -> [u64; 2] {
        std::array::from_fn(|i| PASSES[i].load(Ordering::Relaxed))
    }

    pub(super) fn append_native(&mut self, stage: Stage, copy: TargetCopy,
        targets: (SurfaceId, SurfaceId), color: wgpu::TextureView, depth: wgpu::TextureView,
        stencil: bool, draws: ExternalBundles) {
        let key = (draws.run, targets.0, targets.1);
        if let Some(Run { key: old, copy: old_copy, stage: old_stage,
            commands: Commands::Bundles { draws: run, .. } }) = &mut self.native_run
            && (*old, *old_copy, *old_stage) == (key, copy, stage)
        {
            run.push(draws);
            return;
        }
        self.finish_native_run();
        self.native_run = Some(Run {
            key, copy, stage,
            commands: Commands::Bundles { color, depth, stencil, draws: vec![draws] },
        });
    }

    pub(super) fn append_native_draws(&mut self, stage: Stage, copy: TargetCopy,
        targets: (SurfaceId, SurfaceId), target: ExternalDrawTarget, mut draws: ExternalDraws) {
        let key = (draws.run, targets.0, targets.1);
        if let Some(Run { key: old, copy: old_copy, stage: old_stage,
            commands: Commands::Draws { packets, .. } }) = &mut self.native_run
            && (*old, *old_copy, *old_stage) == (key, copy, stage)
        {
            if let Some(first) = draws.packets.first_mut() {
                let previous = packets.last().map(gfx::RecordedDraw::color_writes);
                // Keep the same pass breaks used for masked and discarding
                // D3D draws on backends where merging can change edge pixels.
                if !self.merge_passes || (!self.merge_discards && draws.discards)
                    || super::merge_split(previous, first.color_writes(), draws.discards)
                {
                    first.start_pass();
                }
            }
            packets.extend(draws.packets);
            return;
        }
        self.finish_native_run();
        self.native_run = Some(Run {
            key, copy, stage,
            commands: Commands::Draws { target, packets: draws.packets },
        });
    }

    pub(super) fn finish_native_run(&mut self) {
        let Some(run) = self.native_run.take() else { return };
        let _encode = Timer::start(run.stage, Metric::EncodeNs);
        let (encoder, passes) = self.open.get_or_insert_with(||
            (self.ctx.device.create_command_encoder(&Default::default()), 0));
        let encoded = match run.commands {
            Commands::Bundles { color, depth, stencil, draws } => {
                ExternalBundles::encode_run(encoder, &color, &depth, stencil, &draws);
                1
            }
            Commands::Draws { target, packets } => {
                let target = gfx::RenderTarget {
                    color: &target.color, color_format: target.color_format,
                    depth: target.depth.as_ref(), depth_format: target.depth_format,
                    size: target.size, samples: 1,
                };
                gfx::Renderer::encode_pass(&self.ctx, encoder, &target, gfx::Clear::NONE, &packets, None)
                    .expect("native draw packets were checked before commit")
            }
        };
        *passes += encoded;
        self.stats.render_passes += encoded as u64;
        let metric = match run.copy {
            TargetCopy::Reference => Metric::ReferencePasses,
            TargetCopy::Display => Metric::DisplayPasses,
        };
        run.stage.count(metric, encoded as u64);
        if run.stage == Stage::Lights {
            PASSES[usize::from(run.copy == TargetCopy::Display)].fetch_add(encoded as u64, Ordering::Relaxed);
        }
    }
}
