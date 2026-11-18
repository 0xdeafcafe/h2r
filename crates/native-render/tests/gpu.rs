//! The native world pass on a headless device (skips without an adapter):
//! an invented world drawn into an offscreen target, its colour the base
//! map times the lightmap page, its depth the game's W-buffer value, a
//! culled draw absent.

use native_render::camera::{D24_SCALE, RenderCamera, w_depth};
use native_render::gpu::{DepthRule, GpuWorld, Image, PassViewport};
use native_render::world::{LightmapKey, Owner, PartInput, Placement, SectionInput, WorldBuilder};

const SIZE: u32 = 64;

/// A 4x4 square facing the camera at view depth `x`, across the centre.
fn wall(x: f32, material: u16) -> SectionInput {
    SectionInput {
        positions: vec![[x, 2.0, -2.0], [x, -2.0, -2.0], [x, 2.0, 2.0], [x, -2.0, 2.0]],
        uvs: vec![[0.5; 2]; 4],
        lightmap_uvs: vec![[0.5; 2]; 4],
        normals: vec![[-1.0, 0.0, 0.0]; 4],
        parts: vec![PartInput {
            material,
            transparent: false,
            triangles: vec![[0, 1, 2], [1, 3, 2]],
        }],
    }
}

fn camera() -> RenderCamera {
    RenderCamera {
        position: [0.0; 3],
        forward: [1.0, 0.0, 0.0],
        up: [0.0, 0.0, 1.0],
        fov: 60f32.to_radians(),
        viewport: [0, 0, SIZE as i16, SIZE as i16],
        near: 0.0625,
        far: 1024.0,
    }
}

fn read(ctx: &gfx::GfxContext, texture: &wgpu::Texture, aspect: wgpu::TextureAspect) -> Vec<u8> {
    let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(SIZE * SIZE * 4),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(SIZE * 4),
                rows_per_image: Some(SIZE),
            },
        },
        wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
    );
    ctx.queue.submit([enc.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
    ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    buffer.slice(..).get_mapped_range().unwrap().to_vec()
}

#[test]
fn the_world_draws_lit_with_the_games_w_depth() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        eprintln!("no wgpu adapter; skipping");
        return;
    };
    let page = LightmapKey { page: 0, palette: 0 };
    let mut b = WorldBuilder::new();
    b.add_section(Owner::Cluster(0), &wall(10.0, 0), &Placement::IDENTITY, Some(page));
    // Behind the camera: culled.
    b.add_section(Owner::Cluster(1), &wall(-10.0, 1), &Placement::IDENTITY, Some(page));
    let world = b.finish();
    let mut gpu = GpuWorld::new(
        &ctx.device,
        &ctx.queue,
        world,
        &mut |m| Some(Image::solid(if m == 0 { [200, 100, 50, 255] } else { [0, 255, 0, 255] })),
        &mut |_| Some(Image::solid([128, 255, 64, 255])),
    );
    let target = |format, usage| {
        ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let color = target(
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
    );
    let depth = target(
        wgpu::TextureFormat::Depth32Float,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
    );
    let (cv, dv) = (color.create_view(&Default::default()), depth.create_view(&Default::default()));
    // The game's clear: far depth, black.
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    drop(enc.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: None,
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &cv,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: &dv,
            depth_ops: Some(wgpu::Operations {
                load: wgpu::LoadOp::Clear(1.0),
                store: wgpu::StoreOp::Store,
            }),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    }));
    let stats = gpu.prepare(&ctx.queue, &camera(), DepthRule::D24);
    assert_eq!((stats.visible, stats.total), (1, 2));
    let viewport = PassViewport {
        x: 0.0,
        y: 0.0,
        width: SIZE as f32,
        height: SIZE as f32,
    };
    gpu.encode(
        &ctx.device,
        &mut enc,
        &cv,
        wgpu::TextureFormat::Rgba8Unorm,
        &dv,
        wgpu::TextureFormat::Depth32Float,
        viewport,
    );
    ctx.queue.submit([enc.finish()]);

    let px = read(&ctx, &color, wgpu::TextureAspect::All);
    let at = |x: u32, y: u32| {
        let i = ((y * SIZE + x) * 4) as usize;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    };
    // 200·128/255, 100·255/255, 50·64/255.
    let c = at(SIZE / 2, SIZE / 2);
    let want = [100i32, 100, 13];
    for k in 0..3 {
        assert!((i32::from(c[k]) - want[k]).abs() <= 1, "centre {c:?} want {want:?}");
    }
    // The wall spans tan-half-angle 0.2 / tan(30°) ≈ 0.35 of the half-width:
    // the corner is still clear.
    assert_eq!(at(0, 0), [0, 0, 0, 255], "corner");

    let d = read(&ctx, &depth, wgpu::TextureAspect::DepthOnly);
    let i = ((SIZE / 2 * SIZE + SIZE / 2) * 4) as usize;
    let got = f32::from_le_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]);
    let want = w_depth(10.0, 1024.0, D24_SCALE);
    assert!(
        (got - want).abs() <= 1.0 / D24_SCALE,
        "depth {got} want {want} ({} steps apart)",
        (got - want) * D24_SCALE
    );
    let corner = f32::from_le_bytes([d[0], d[1], d[2], d[3]]);
    assert_eq!(corner, 1.0);
}
