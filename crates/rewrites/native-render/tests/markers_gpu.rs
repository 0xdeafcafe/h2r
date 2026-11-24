//! Synthetic only: the marker skin on the headless target.
#![cfg(feature = "gpu")]
use native_render::markers::{Batch, Pass, Targets};

const SIZE: u32 = 32;
const COLOR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const IDENTITY: [[f32; 4]; 4] =
    [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [0., 0., 0., 1.]];

fn texture(ctx: &gfx::GfxContext, format: wgpu::TextureFormat) -> wgpu::Texture {
    ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// A black target whose depth is `world` everywhere (the floor or the
/// nearer wall the markers are tested against), then the pass over it in
/// clip space. Returns the colour.
fn draw(ctx: &gfx::GfxContext, world: f32, batch: &Batch) -> Vec<u8> {
    let (color, depth) = (texture(ctx, COLOR), texture(ctx, DEPTH));
    let (cv, dv) = (color.create_view(&Default::default()), depth.create_view(&Default::default()));
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: None,
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &cv,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
        })],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: &dv,
            depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(world), store: wgpu::StoreOp::Store }),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    }));
    Pass::new(&ctx.device, COLOR, DEPTH, false).encode(
        &ctx.device,
        &mut encoder,
        &Targets { color: &cv, depth: &dv },
        IDENTITY,
        batch,
    );
    ctx.queue.submit([encoder.finish()]);
    ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    gfx::readback::read_texture(ctx, &color, wgpu::TextureAspect::All, 4, (SIZE, SIZE))
}

fn px(image: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIZE + x) * 4) as usize;
    [image[i], image[i + 1], image[i + 2], image[i + 3]]
}

/// A red wall over the middle of the target at clip depth `z`.
fn wall(z: f32) -> Batch {
    let mut batch = Batch::default();
    let colour = [1., 0., 0., 0.5];
    batch.push_tri([[-0.5, -0.5, z], [0.5, -0.5, z], [0.5, 0.5, z]], colour);
    batch.push_tri([[-0.5, -0.5, z], [0.5, 0.5, z], [-0.5, 0.5, z]], colour);
    batch
}

#[test]
fn a_wall_changes_pixels_in_front_of_a_floor_and_none_behind_a_nearer_wall() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        eprintln!("no GPU adapter; skipped");
        return;
    };
    // The floor is at depth 0.5; the wall at 0.3 is in front of it.
    let front = draw(&ctx, 0.5, &wall(0.3));
    let red = px(&front, SIZE / 2, SIZE / 2)[0];
    assert!((120..=136).contains(&red), "half red over black: {red}");
    assert_eq!(px(&front, 1, 1), [0, 0, 0, 255], "nothing outside the wall");
    // A nearer wall (0.1) hides a marker wall at 0.3: no pixel changes.
    let behind = draw(&ctx, 0.1, &wall(0.3));
    assert!(behind.chunks(4).all(|p| p == [0, 0, 0, 255]), "the skin is not drawn behind a nearer wall");
}

#[test]
fn a_line_behind_the_world_draws_at_a_third_of_its_alpha() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        eprintln!("no GPU adapter; skipped");
        return;
    };
    let mut line = Batch::default();
    line.push_line([-0.9, 0.03, 0.3], [0.9, 0.03, 0.3], [1., 1., 1., 1.]);
    let lit = |image: &[u8]| (0..SIZE).map(|y| px(image, SIZE / 2, y)[0]).max().unwrap();
    let in_front = lit(&draw(&ctx, 0.5, &line));
    let hidden = lit(&draw(&ctx, 0.1, &line));
    assert_eq!(in_front, 255, "a line in front is full");
    assert!((80..=90).contains(&hidden), "a line behind keeps a third: {hidden}");
}
