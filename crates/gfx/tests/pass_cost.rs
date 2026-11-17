//! What a render pass costs on the CPU against the number of buffers the
//! device holds. wgpu-core sizes each pass's usage scope (and each command
//! buffer's tracker) to the highest buffer tracker index handed out so
//! far: `UsageScope::new_pooled` calls `BufferUsageScope::set_size(
//! tracker_indices.buffers.size())`, and the scope is cleared when the pass
//! ends, so every pass pays per buffer ever live at once, used or not.
//! Ignored (a timing, not a check):
//!
//! ```sh
//! cargo test --release -p gfx --test pass_cost -- --ignored --nocapture
//! ```

use gfx::GfxContext;

/// Microseconds per pass (one draw-less pass with a load and a store),
/// with `held` other buffers alive, submitting every `per_submit` passes.
fn bench(ctx: &GfxContext, view: &wgpu::TextureView, passes: usize, per_submit: usize) -> f64 {
    let start = std::time::Instant::now();
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    for i in 0..passes {
        {
            let _pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
        }
        if (i + 1) % per_submit == 0 {
            let done = std::mem::replace(
                &mut enc,
                ctx.device.create_command_encoder(&Default::default()),
            );
            ctx.queue.submit([done.finish()]);
        }
    }
    ctx.queue.submit([enc.finish()]);
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    start.elapsed().as_secs_f64() * 1e6 / passes as f64
}

#[test]
#[ignore = "timing; run with --ignored --nocapture"]
fn pass_cost_by_live_buffers() {
    let Ok(ctx) = GfxContext::headless() else {
        eprintln!("skipping: no adapter");
        return;
    };
    let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    bench(&ctx, &view, 200, 8);
    let mut held = Vec::new();
    for want in [0usize, 1_000, 10_000, 30_000, 100_000] {
        while held.len() < want {
            held.push(ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 256,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        let us = bench(&ctx, &view, 600, 8);
        eprintln!(
            "{want:>7} live buffers: {us:7.1} us per pass (incl. its share of a submit per 8)"
        );
    }
}
