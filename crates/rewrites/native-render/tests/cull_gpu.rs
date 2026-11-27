//! The world's GPU cull on a headless device (skips without an adapter):
//! the compute pass keeps exactly the draws the reference box test keeps,
//! a box behind a wall is culled by the depth pyramid, and a box that
//! comes into view is drawn the frame it does.

use native_render::camera::RenderCamera;
use native_render::cull::Frustum;
use native_render::gpu::{DepthRule, GpuWorld, Image, PassViewport};
use native_render::world::{Owner, PartInput, Placement, SectionInput, WorldBuilder};

const SIZE: u32 = 64;
const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];

fn camera(position: [f32; 3], forward: [f32; 3]) -> RenderCamera {
    RenderCamera {
        position,
        forward,
        up: [0.0, 0.0, 1.0],
        fov: 60f32.to_radians(),
        projection_xy: None,
        viewport: [0, 0, SIZE as i16, SIZE as i16],
        near: 0.0625,
        far: 1024.0,
    }
}

/// A square facing -x at `x`, centred on `(y, z)`, `half` wide each way.
fn square(x: f32, y: f32, z: f32, half: f32, material: u16) -> SectionInput {
    SectionInput {
        positions: vec![
            [x, y + half, z - half],
            [x, y - half, z - half],
            [x, y + half, z + half],
            [x, y - half, z + half],
        ],
        uvs: vec![[0.5; 2]; 4],
        normals: vec![[-1.0, 0.0, 0.0]; 4],
        parts: vec![PartInput { material, transparent: false, triangles: vec![[0, 1, 2], [1, 3, 2]] }],
        ..Default::default()
    }
}

/// One triangle whose bounds are the box `min..max`.
fn spanning(min: [f32; 3], max: [f32; 3]) -> SectionInput {
    SectionInput {
        positions: vec![min, max, [min[0], max[1], min[2]]],
        uvs: vec![[0.5; 2]; 3],
        normals: vec![[-1.0, 0.0, 0.0]; 3],
        parts: vec![PartInput { material: 0, transparent: false, triangles: vec![[0, 1, 2]] }],
        ..Default::default()
    }
}

fn upload(ctx: &gfx::GfxContext, world: native_render::world::World) -> GpuWorld {
    GpuWorld::new(&ctx.device, &ctx.queue, world, &mut |m| Some(Image::solid(if m == 0 { RED } else { GREEN })), &mut |_| None)
}

struct Target {
    color: wgpu::Texture,
    depth: wgpu::Texture,
}

impl Target {
    fn new(ctx: &gfx::GfxContext, sampled_depth: bool) -> Self {
        let texture = |format, usage| {
            ctx.device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let u = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC;
        let sampled = if sampled_depth { wgpu::TextureUsages::TEXTURE_BINDING } else { wgpu::TextureUsages::empty() };
        Self {
            color: texture(wgpu::TextureFormat::Rgba8Unorm, u),
            depth: texture(wgpu::TextureFormat::Depth32Float, u | sampled),
        }
    }

    /// One frame: clear to black and the far plane, prepare and encode the
    /// whole world, submit.
    fn frame(&self, ctx: &gfx::GfxContext, gpu: &mut GpuWorld, camera: &RenderCamera) {
        let (cv, dv) = (self.color.create_view(&Default::default()), self.depth.create_view(&Default::default()));
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
        let viewport = PassViewport { x: 0.0, y: 0.0, width: SIZE as f32, height: SIZE as f32 };
        {
            let mut uploads = gfx::ring::EncoderUploads { device: &ctx.device, queue: &ctx.queue, encoder: &mut enc, arena: &arena };
            gpu.prepare(&mut uploads, camera, viewport, DepthRule::D24);
        }
        let (cf, df) = (self.color.format(), self.depth.format());
        gpu.encode(&ctx.device, &mut enc, &cv, cf, &dv, df, viewport, (SIZE, SIZE));
        arena.flush(&ctx.queue);
        ctx.queue.submit([enc.finish()]);
        ctx.queue.on_submitted_work_done(move || drop(arena));
    }

    fn pixels(&self, ctx: &gfx::GfxContext) -> Vec<[u8; 4]> {
        let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(SIZE * SIZE * 4),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            self.color.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(SIZE * 4), rows_per_image: Some(SIZE) },
            },
            wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
        );
        ctx.queue.submit([enc.finish()]);
        buffer.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
        ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let bytes = buffer.slice(..).get_mapped_range().unwrap().to_vec();
        bytes.chunks_exact(4).map(|p| [p[0], p[1], p[2], p[3]]).collect()
    }
}

/// A small deterministic generator (xorshift).
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0 as f32 / u32::MAX as f32
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next()
    }
}

/// How far the box is from the nearest plane decision: boxes within
/// `0.01` of flipping are left out, where the GPU's fused multiply-adds
/// may round the other way.
fn margin(f: &Frustum, min: [f32; 3], max: [f32; 3]) -> f32 {
    f.planes
        .iter()
        .map(|p| {
            let c = |k: usize| if p[k] >= 0.0 { max[k] } else { min[k] };
            (p[0] * c(0) + p[1] * c(1) + p[2] * c(2) + p[3]).abs()
        })
        .fold(f32::INFINITY, f32::min)
}

#[test]
fn the_compute_cull_keeps_what_the_reference_box_test_keeps() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        eprintln!("no wgpu adapter; skipping");
        return;
    };
    let cameras = [
        camera([0.0; 3], [1.0, 0.0, 0.0]),
        camera([5.0, -3.0, 1.0], [0.6, 0.8, 0.0]),
        camera([-20.0, 10.0, 4.0], [0.0, -0.8, -0.6]),
    ];
    let mut rng = Rng(0x9e37_79b9);
    for (n, view) in cameras.iter().enumerate() {
        let frustum = Frustum::from_view_projection(&view.view_projection());
        // Boxes ahead, behind, beside, straddling the eye and past far.
        let mut boxes = Vec::new();
        while boxes.len() < 300 {
            let at = [rng.range(-1200.0, 1200.0), rng.range(-400.0, 400.0), rng.range(-200.0, 200.0)];
            let scale = if rng.next() < 0.5 { 2.0 } else { 60.0 };
            let size = [rng.range(0.1, scale), rng.range(0.1, scale), rng.range(0.1, scale)];
            let max = [at[0] + size[0], at[1] + size[1], at[2] + size[2]];
            if margin(&frustum, at, max) > 0.01 {
                boxes.push((at, max));
            }
        }
        boxes.push(([-1.0; 3], [1.0; 3]));
        let mut b = WorldBuilder::new();
        for (i, &(min, max)) in boxes.iter().enumerate() {
            b.add_section(Owner::Cluster(i as u32), &spanning(min, max), &Placement::IDENTITY, None);
        }
        let world = b.finish();
        let want: Vec<bool> = world.draws.iter().map(|d| frustum.may_see(&d.bounds)).collect();
        let kept = want.iter().filter(|&&k| k).count() as u32;
        assert!(kept > 5 && kept < want.len() as u32 - 5, "camera {n}: a mixed scene ({kept} kept)");
        // With the pyramid (the first frame: nothing drawn early, the
        // pyramid of the clear hides nothing) and without it.
        for sampled in [true, false] {
            let mut gpu = upload(&ctx, world.clone());
            Target::new(&ctx, sampled).frame(&ctx, &mut gpu, view);
            assert_eq!(gpu.read_drawn(&ctx.device, &ctx.queue), want, "camera {n}, sampled depth {sampled}");
            let stats = gpu.read_cull_stats(&ctx.device, &ctx.queue);
            assert_eq!((stats.visible, stats.drawn, stats.occluded), (kept, kept, 0), "camera {n}");
            assert_eq!(gpu.occlusion_skips().depth_unsampled, u64::from(!sampled));
        }
    }
}

#[test]
fn a_box_behind_a_wall_is_culled_and_drawn_the_frame_it_comes_into_view() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        eprintln!("no wgpu adapter; skipping");
        return;
    };
    let mut b = WorldBuilder::new();
    b.add_section(Owner::Cluster(0), &square(10.0, 0.0, 0.0, 2.0, 0), &Placement::IDENTITY, None);
    b.add_section(Owner::Cluster(1), &square(20.0, 0.0, 0.0, 0.5, 1), &Placement::IDENTITY, None);
    let world = b.finish();
    let wall = world.draws.iter().position(|d| d.owner == Owner::Cluster(0)).unwrap();
    let hidden = world.draws.iter().position(|d| d.owner == Owner::Cluster(1)).unwrap();
    let mut gpu = upload(&ctx, world);
    let target = Target::new(&ctx, true);
    let ahead = camera([0.0; 3], [1.0, 0.0, 0.0]);
    let drawn = |gpu: &GpuWorld| {
        let d = gpu.read_drawn(&ctx.device, &ctx.queue);
        (d[wall], d[hidden])
    };

    // The first frame has no history: both draw late, against the clear.
    target.frame(&ctx, &mut gpu, &ahead);
    assert_eq!(drawn(&gpu), (true, true));
    // The second draws both early; the pyramid of their depth hides the box
    // (it was drawn already, and leaves the history).
    target.frame(&ctx, &mut gpu, &ahead);
    let stats = gpu.read_cull_stats(&ctx.device, &ctx.queue);
    assert_eq!((stats.visible, stats.occluded), (2, 1));
    // From the third on, the box is culled.
    target.frame(&ctx, &mut gpu, &ahead);
    assert_eq!(drawn(&gpu), (true, false), "the box behind the wall is culled");
    let stats = gpu.read_cull_stats(&ctx.device, &ctx.queue);
    assert_eq!((stats.visible, stats.drawn, stats.occluded), (2, 1, 1));
    let px = target.pixels(&ctx);
    assert_eq!(px[(SIZE / 2 * SIZE + SIZE / 2) as usize], RED, "the wall covers the centre");
    assert_eq!(gpu.occlusion_skips(), Default::default());

    // Stepped aside, the box is in view past the wall's edge: it is not in
    // the early set, and the late test draws it this frame.
    target.frame(&ctx, &mut gpu, &camera([0.0, 6.0, 0.0], [1.0, 0.0, 0.0]));
    assert_eq!(drawn(&gpu), (true, true), "the revealed box is drawn the same frame");
    let stats = gpu.read_cull_stats(&ctx.device, &ctx.queue);
    assert_eq!((stats.drawn, stats.occluded), (2, 0));
    let px = target.pixels(&ctx);
    assert!(px.contains(&GREEN), "the box's pixels");
    assert!(px.contains(&RED), "the wall's");
}
