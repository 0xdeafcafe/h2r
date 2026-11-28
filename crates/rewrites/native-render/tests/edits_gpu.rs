//! A live edit's native refresh on a headless device (skips without an
//! adapter; docs/editor/realtime-edits.md, "The native refresh"): a
//! section replaced in an uploaded world draws, culls and bounds exactly
//! as a world loaded from the edited section, and a texture rewritten in
//! place samples its new texels while a size or level change is refused.

use native_render::camera::RenderCamera;
use native_render::gpu::{self, DepthRule, GpuWorld, Image, ImageRefusal, PassViewport};
use native_render::world::{LightmapKey, Owner, PartInput, Placement, SectionInput, SectionRefusal, World, WorldBuilder};

const SIZE: u32 = 64;

/// A 4x4 square facing the camera at view depth `x`, centred at `y`.
fn wall(x: f32, y: f32) -> SectionInput {
    SectionInput {
        positions: vec![[x, y + 2.0, -2.0], [x, y - 2.0, -2.0], [x, y + 2.0, 2.0], [x, y - 2.0, 2.0]],
        uvs: vec![[0.5; 2]; 4],
        lightmap_uvs: vec![[0.5; 2]; 4],
        normals: vec![[-1.0, 0.0, 0.0]; 4],
        parts: vec![PartInput { material: 0, transparent: false, triangles: vec![[0, 1, 2], [1, 3, 2]] }],
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

const PAGE: LightmapKey = LightmapKey { page: 0, palette: Some(0) };

/// Cluster 0 as `first`, cluster 1 a fixed wall to the side.
fn world(first: &SectionInput) -> World {
    let mut b = WorldBuilder::new();
    b.add_section(Owner::Cluster(0), first, &Placement::IDENTITY, Some(PAGE));
    b.add_section(Owner::Cluster(1), &wall(20.0, -9.0), &Placement::IDENTITY, Some(PAGE));
    b.finish()
}

fn upload(ctx: &gfx::GfxContext, world: World) -> GpuWorld {
    GpuWorld::new(&ctx.device, &ctx.queue, world, &mut |_| Some(Image::solid([200, 100, 50, 255])), &mut |_| {
        Some(Image::solid([255; 4]))
    })
}

fn read(ctx: &gfx::GfxContext, texture: &wgpu::Texture, level: u32) -> Vec<u8> {
    let (width, height) = ((texture.width() >> level).max(1), (texture.height() >> level).max(1));
    let row = (width * 4).next_multiple_of(256);
    let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(row * height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture, mip_level: level, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(height) },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    ctx.queue.submit([enc.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
    ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let bytes = buffer.slice(..).get_mapped_range().unwrap().to_vec();
    bytes.chunks(row as usize).flat_map(|r| r[..(width * 4) as usize].to_vec()).collect()
}

/// Draws the whole world (indirect, frustum-culled) into a cleared target
/// and returns its pixels and how many draws the cull kept.
fn draw(ctx: &gfx::GfxContext, gpu: &mut GpuWorld) -> (Vec<u8>, u32) {
    let target = |format| {
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
    };
    let color = target(wgpu::TextureFormat::Rgba8Unorm);
    let depth = target(wgpu::TextureFormat::Depth32Float);
    let (cv, dv) = (color.create_view(&Default::default()), depth.create_view(&Default::default()));
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    drop(enc.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: None,
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &cv,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
        })],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: &dv,
            depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    }));
    let arena = gfx::ring::UploadPool::default().begin();
    let mut uploads = gfx::ring::EncoderUploads { device: &ctx.device, queue: &ctx.queue, encoder: &mut enc, arena: &arena };
    let viewport = PassViewport { x: 0.0, y: 0.0, width: SIZE as f32, height: SIZE as f32 };
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
    (read(ctx, &color, 0), gpu.read_cull_stats(&ctx.device, &ctx.queue).visible)
}

/// A cluster behind the camera moved in front of it: with the bounds left
/// as uploaded the cull would still drop it. After the replacement the
/// world equals one loaded from the edited section in its vertices,
/// draws, bounds, culling and pixels.
#[test]
fn a_replaced_section_draws_as_a_world_loaded_from_the_edited_bytes() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        eprintln!("no wgpu adapter; skipping");
        return;
    };
    let edited = wall(10.0, 1.0);
    let mut live = upload(&ctx, world(&wall(-10.0, 0.0)));
    let mut loaded = upload(&ctx, world(&edited));
    let (_, visible) = draw(&ctx, &mut live);
    assert_eq!(visible, 1, "before: only the side wall");

    live.set_lights(native_render::light_gpu::LightSet::new(&ctx.device, &ctx.queue, None, None));
    let (_, original_vertices, original_indices) = live.retain_light(&ctx.device, None).unwrap();
    let run = live.replace_section(&ctx.queue, Owner::Cluster(0), &edited).unwrap();
    let (_, edited_vertices, edited_indices) = live.retain_light(&ctx.device, None).unwrap();
    assert_ne!(original_vertices, edited_vertices, "retained scene still owns the old vertices");
    assert_eq!(original_indices, edited_indices, "section edits preserve topology");
    assert_eq!((run.first_vertex, run.vertex_count), (0, 4));
    assert_eq!(live.world().vertices, loaded.world().vertices);
    assert_eq!(live.world().draws, loaded.world().draws, "draw bounds recomputed");
    assert_eq!(live.world().bounds, loaded.world().bounds);

    let parts = [(Owner::Cluster(0), 0)];
    let (color, depth) = (wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureFormat::Depth32Float);
    assert!(live.prepare_parts(&ctx.device, color, depth, &parts, &camera()), "the guest's admission sees it");
    let (live_px, live_visible) = draw(&ctx, &mut live);
    let (loaded_px, loaded_visible) = draw(&ctx, &mut loaded);
    assert_eq!((live_visible, loaded_visible), (2, 2));
    assert!(live_px == loaded_px, "the edited world draws as the loaded one");
    let centre = ((SIZE / 2 * SIZE + SIZE / 2) * 4) as usize;
    assert_ne!(live_px[centre..centre + 3], [0, 0, 0], "the moved wall covers the centre");

    // A changed count or triangle list is the world reload's.
    let mut grown = edited.clone();
    grown.positions.push([10.0, 0.0, 0.0]);
    assert_eq!(live.replace_section(&ctx.queue, Owner::Cluster(0), &grown).unwrap_err(), SectionRefusal::VertexCount);
    let mut flipped = edited.clone();
    flipped.parts[0].triangles = vec![[0, 2, 1], [1, 2, 3]];
    assert_eq!(live.replace_section(&ctx.queue, Owner::Cluster(0), &flipped).unwrap_err(), SectionRefusal::Triangles);
    assert_eq!(live.world().vertices, loaded.world().vertices, "a refusal changes nothing");
}

/// An RGBA8 texture of `width`x`height` with `levels` levels.
fn rgba_texture(ctx: &gfx::GfxContext, width: u32, height: u32, levels: u32) -> wgpu::Texture {
    ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// Same size and levels: every level's texels change. A changed size or
/// level count is refused before anything is written.
#[test]
fn a_rewritten_texture_samples_its_new_texels_and_a_size_change_is_refused() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        eprintln!("no wgpu adapter; skipping");
        return;
    };
    let t = rgba_texture(&ctx, 4, 4, 3);
    let image = |a: u8, b: u8, c: u8| {
        Image::from_levels(vec![(4, 4, vec![a; 64]), (2, 2, vec![b; 16]), (1, 1, vec![c; 4])]).unwrap()
    };
    gpu::rewrite_texture(&ctx.queue, &t, &image(1, 2, 3)).unwrap();
    assert_eq!(read(&ctx, &t, 0), vec![1; 64]);
    gpu::rewrite_texture(&ctx.queue, &t, &image(40, 50, 60)).unwrap();
    assert_eq!(read(&ctx, &t, 0), vec![40; 64]);
    assert_eq!(read(&ctx, &t, 1), vec![50; 16]);
    assert_eq!(read(&ctx, &t, 2), vec![60; 4]);

    let bigger = Image::level(8, 8, vec![9; 256]).unwrap();
    assert_eq!(gpu::rewrite_texture(&ctx.queue, &t, &bigger), Err(ImageRefusal::Size));
    let one_level = Image::level(4, 4, vec![9; 64]).unwrap();
    assert_eq!(gpu::rewrite_texture(&ctx.queue, &t, &one_level), Err(ImageRefusal::Levels));
    assert_eq!(read(&ctx, &t, 0), vec![40; 64], "refused: nothing written");

    // A block-compressed texture takes only blocks of its own format.
    if ctx.device.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC) {
        let bc = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: 4, height: 4, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bc1RgbaUnorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        assert_eq!(gpu::check_rewrite(&bc, &one_level), Err(ImageRefusal::Format));
        let blocks = one_level.clone().with_blocks(wgpu::TextureFormat::Bc1RgbaUnorm, vec![vec![0; 8]]);
        assert_eq!(gpu::rewrite_texture(&ctx.queue, &bc, &blocks), Ok(()));
        let other = one_level.with_blocks(wgpu::TextureFormat::Bc3RgbaUnorm, vec![vec![0; 16]]);
        assert_eq!(gpu::check_rewrite(&bc, &other), Err(ImageRefusal::Format));
    }
}

/// What every holder shares: a held texture is written once however many
/// holders list it, a signed stage is biased as at load, a stage with no
/// decode refuses the lot, and a size change writes nothing
/// (docs/editor/realtime-edits.md, "The native refresh").
#[test]
fn held_textures_are_rewritten_all_or_nothing_and_each_once() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        eprintln!("no wgpu adapter; skipping");
        return;
    };
    let (a, b) = (rgba_texture(&ctx, 1, 1, 1), rgba_texture(&ctx, 1, 1, 1));
    let held: Vec<gpu::Held> = vec![
        (a.clone(), 0, [false; 4]),
        (a.clone(), 0, [false; 4]),
        (b.clone(), 2, [true, false, false, false]),
    ];
    assert_eq!(gpu::held_stages(&held), vec![0, 2]);
    let decoded = std::collections::HashMap::from([
        (0, Image::solid([10, 20, 30, 40])),
        (2, Image::solid([10, 20, 30, 40])),
    ]);
    assert_eq!(gpu::rewrite_held(&ctx.queue, &held, &decoded), Ok(2), "the shared texture once");
    assert_eq!(read(&ctx, &a, 0), vec![10, 20, 30, 40]);
    assert_eq!(read(&ctx, &b, 0), vec![10 ^ 0x80, 20, 30, 40], "the signed channel is biased");

    let bigger = std::collections::HashMap::from([
        (0, Image::solid([1, 1, 1, 1])),
        (2, Image::level(2, 2, vec![9; 16]).unwrap()),
    ]);
    assert_eq!(gpu::rewrite_held(&ctx.queue, &held, &bigger), Err(ImageRefusal::Size));
    assert_eq!(read(&ctx, &a, 0), vec![10, 20, 30, 40], "refused: the first texture is untouched too");
    let missing = std::collections::HashMap::from([(0, Image::solid([1, 1, 1, 1]))]);
    assert_eq!(gpu::rewrite_held(&ctx.queue, &held, &missing), Err(ImageRefusal::NotDecoded));
    assert_eq!(gpu::rewrite_held(&ctx.queue, &[], &decoded), Ok(0));
}
