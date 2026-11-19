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

    pub(super) fn append_native(&mut self, copy: TargetCopy, setup: wgpu::CommandBuffer, targets: (SurfaceId, SurfaceId),
        color: wgpu::TextureView, depth: wgpu::TextureView, stencil: bool, draws: ExternalBundles) {
        let key = (draws.run, targets.0, targets.1);
        if self.native_run.as_ref().is_some_and(|r| r.key != key || r.copy != copy) {
            self.finish_native_run();
        }
        // Setup commands may initialize this packet's private uniform
        // buffers. All members of a run promise immutable draw resources.
        if let Some((enc, _)) = self.open.take() {
            self.external_prefix.push(enc.finish());
        }
        self.external_prefix.push(setup);
        self.native_run.get_or_insert_with(|| Run {
            key, copy, color, depth, stencil, draws: Vec::new(),
        }).draws.push(draws);
    }

    pub(super) fn finish_native_run(&mut self) {
        let Some(run) = self.native_run.take() else { return };
        let (encoder, passes) = self.open.get_or_insert_with(||
            (self.ctx.device.create_command_encoder(&Default::default()), 0));
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("native light run"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &run.color, depth_slice: None, resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &run.depth,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                stencil_ops: run.stencil.then_some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
            }),
            ..Default::default()
        });
        for draws in &run.draws {
            let [x, y, width, height] = draws.scissor;
            pass.set_scissor_rect(x, y, width, height);
            for (blend, bundle) in &draws.groups {
                pass.set_blend_constant(*blend);
                pass.execute_bundles(std::iter::once(bundle));
            }
        }
        *passes += 1;
        PASSES[usize::from(run.copy == TargetCopy::Display)].fetch_add(1, Ordering::Relaxed);
    }
}
