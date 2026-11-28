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
        ..Default::default()
    }
}

fn camera() -> RenderCamera {
    RenderCamera {
        position: [0.0; 3],
        forward: [1.0, 0.0, 0.0],
        up: [0.0, 0.0, 1.0],
        fov: 60f32.to_radians(),
        projection_xy: None,
        viewport: [0, 0, SIZE as i16, SIZE as i16],
        near: 0.0625,
        far: 1024.0,
    }
}

fn read(ctx: &gfx::GfxContext, texture: &wgpu::Texture, aspect: wgpu::TextureAspect) -> Vec<u8> {
    let (width, height) = (texture.width(), texture.height());
    let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(width * height * 4),
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
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
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
    let page = LightmapKey { page: 0, palette: Some(0) };
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
    let arena = gfx::ring::UploadPool::default().begin();
    let mut uploads = gfx::ring::EncoderUploads { device: &ctx.device, queue: &ctx.queue, encoder: &mut enc, arena: &arena };
    let color_format = color.format();
    let depth_format = depth.format();
    let front = [(Owner::Cluster(0), 0)];
    let back = [(Owner::Cluster(1), 0)];
    assert!(gpu.prepare_parts(&ctx.device, color_format, depth_format, &front, &camera()));
    assert!(!gpu.prepare_parts(&ctx.device, color_format, depth_format, &back, &camera()));
    assert!(!gpu.prepare_parts(&ctx.device, color_format, depth_format, &[(Owner::Cluster(0), 99)], &camera()));
    let viewport = PassViewport {
        x: 0.0,
        y: 0.0,
        width: SIZE as f32,
        height: SIZE as f32,
    };
    gpu.prepare(&mut uploads, &camera(), viewport, DepthRule::D24);
    gpu.encode(
        &ctx.device,
        &mut enc,
        &cv,
        wgpu::TextureFormat::Rgba8Unorm,
        &dv,
        wgpu::TextureFormat::Depth32Float,
        viewport,
        (SIZE, SIZE),
    );
    arena.flush(&ctx.queue);
    ctx.queue.submit([enc.finish()]);

    let stats = gpu.read_cull_stats(&ctx.device, &ctx.queue);
    assert_eq!((stats.visible, stats.drawn, stats.total), (1, 1, 2));
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

#[test]
fn selected_and_indirect_world_draws_match_for_both_cameras_at_both_scales_in_one_submit() {
    use wgpu::{TextureFormat as F, TextureUsages as U};
    let Ok(ctx) = gfx::GfxContext::headless() else { return };
    let mut b = WorldBuilder::new();
    let page = Some(LightmapKey { page: 0, palette: Some(0) });
    b.add_section(Owner::Cluster(0), &wall(10.0, 0), &Placement::IDENTITY, page);
    b.add_section(Owner::Cluster(1), &wall(-10.0, 1), &Placement::IDENTITY, page);
    let mut world = GpuWorld::new(&ctx.device, &ctx.queue, b.finish(), &mut |m| {
        Some(Image::solid(if m == 0 { [255, 0, 0, 255] } else { [0, 255, 0, 255] }))
    }, &mut |_| Some(Image::solid([255; 4])));
    let arena = gfx::ring::UploadPool::default().begin();
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    let mut images = Vec::new();
    for (size, selected) in [(SIZE, false), (SIZE, true), (2 * SIZE, false), (2 * SIZE, true)] {
        let texture = |format| ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: None, size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format,
            // TEXTURE_BINDING: the indirect views' cull reads the depth pyramid.
            usage: U::RENDER_ATTACHMENT | U::COPY_SRC | U::TEXTURE_BINDING, view_formats: &[],
        });
        let color = texture(F::Rgba8Unorm);
        let depth = texture(F::Depth32Float);
        let cv = color.create_view(&Default::default());
        let dv = depth.create_view(&Default::default());
        drop(enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &cv, depth_slice: None, resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &dv, stencil_ops: None, depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.), store: wgpu::StoreOp::Store,
                }),
            }),
            timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
        }));
        for view in 0..2 {
            let forward = if view == 0 { [1., 0., 0.] } else { [-1., 0., 0.] };
            let camera = RenderCamera { forward, ..camera() };
            let mut uploads = gfx::ring::EncoderUploads { device: &ctx.device, queue: &ctx.queue, arena: &arena, encoder: &mut enc };
            let half = size as f32 / 2.;
            let vp = PassViewport { x: view as f32 * half, y: 0., width: half, height: size as f32 };
            if selected {
                let selection = world.select_parts(&[(Owner::Cluster(view), 0); 2], &camera).unwrap();
                assert!(world.prepare_selection(&ctx.device, F::Rgba8Unorm, F::Depth32Float, &selection));
                assert!(world.write_selection(&mut uploads, &selection, None, &camera, DepthRule::D24));
                assert_eq!(world.encode_selection(&mut enc, &cv, F::Rgba8Unorm, &dv, F::Depth32Float, vp, (size, size), &selection), 2);
            } else {
                world.prepare(&mut uploads, &camera, vp, DepthRule::D24);
                world.encode(&ctx.device, &mut enc, &cv, F::Rgba8Unorm, &dv, F::Depth32Float, vp, (size, size));
            }
        }
        images.push((color, depth));
    }
    arena.flush(&ctx.queue);
    ctx.queue.submit([enc.finish()]);
    ctx.queue.on_submitted_work_done(move || drop(arena));
    for pair in images.as_chunks::<2>().0 {
        let (reference, selected) = (&pair[0], &pair[1]);
        let px = read(&ctx, &reference.0, wgpu::TextureAspect::All);
        assert_eq!(px, read(&ctx, &selected.0, wgpu::TextureAspect::All));
        assert_eq!(read(&ctx, &reference.1, wgpu::TextureAspect::DepthOnly), read(&ctx, &selected.1, wgpu::TextureAspect::DepthOnly));
        let size = reference.0.width() as usize;
        for (x, wanted) in [(size / 4, [255, 0, 0, 255]), (3 * size / 4, [0, 255, 0, 255])] {
            let i = 4 * (size / 2 * size + x);
            assert_eq!(&px[i..i + 4], &wanted, "centre of view at x={x} on {size}px copy");
        }
    }
}
