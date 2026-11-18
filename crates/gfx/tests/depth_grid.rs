//! `gfx::depth_grid` against a CPU max over the same cells: random depths
//! (with far texels and a wall), rectangles at offsets and grids that do
//! and do not divide the rectangle. Skipped without an adapter.

use gfx::GfxContext;
use gfx::depth_grid::{DepthGrid, Rect};

fn ctx() -> Option<GfxContext> {
    match GfxContext::headless() {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            None
        }
    }
}

/// A `Depth32Float` texture of `w x h` holding `depths` (row-major).
fn depth_texture(ctx: &GfxContext, depths: &[f32], w: u32, h: u32) -> wgpu::Texture {
    let device = &ctx.device;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("test depth"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let values = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("test depths"),
        size: (depths.len() * 4) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bytes: Vec<u8> = depths.iter().flat_map(|d| d.to_le_bytes()).collect();
    ctx.queue.write_buffer(&values, 0, &bytes);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(
            format!(
                "@group(0) @binding(0) var<storage, read> v: array<f32>;
                 @vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {{
                     let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
                     return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
                 }}
                 @fragment fn fs(@builtin(position) p: vec4<f32>) -> @builtin(frag_depth) f32 {{
                     return v[u32(p.y) * {w}u + u32(p.x)];
                 }}"
            )
            .into(),
        ),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None,
        layout: None,
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: Default::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[],
        }),
        multiview_mask: None,
        cache: None,
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: values.as_entire_binding(),
        }],
    });
    let view = texture.create_view(&Default::default());
    let mut enc = device.create_command_encoder(&Default::default());
    {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..3, 0..1);
    }
    ctx.queue.submit([enc.finish()]);
    texture
}

/// The CPU rule: pixel `p` in cell `floor((p + 0.5) · cells / src)`; an
/// empty cell is 1.0.
fn cpu_grid(depths: &[f32], tw: u32, rect: Rect, gw: u32, gh: u32) -> Vec<f32> {
    let cell = |p: u32, src: u32, cells: u32| (((2 * p + 1) * cells) / (2 * src)).min(cells - 1);
    let mut g = vec![-1.0f32; (gw * gh) as usize];
    for y in 0..rect.height {
        for x in 0..rect.width {
            let d = depths[((rect.y + y) * tw + rect.x + x) as usize];
            let i = (cell(y, rect.height, gh) * gw + cell(x, rect.width, gw)) as usize;
            g[i] = g[i].max(d);
        }
    }
    for v in &mut g {
        if *v < 0.0 {
            *v = 1.0;
        }
    }
    g
}

fn gpu_grid(
    ctx: &GfxContext,
    grid: &DepthGrid,
    t: &wgpu::Texture,
    rect: Rect,
    gw: u32,
    gh: u32,
) -> Vec<f32> {
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    let mut pending = grid.reduce(ctx, &mut enc, t, rect, gw, gh);
    ctx.queue.submit([enc.finish()]);
    pending.map();
    for _ in 0..1000 {
        ctx.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        if let Some(v) = pending.try_take() {
            return v;
        }
        assert!(!pending.failed());
    }
    panic!("grid never mapped");
}

#[test]
fn depth_grid_matches_the_cpu_max_per_cell() {
    let Some(ctx) = ctx() else { return };
    let (tw, th) = (301u32, 173u32);
    let mut seed = 0x9e37_79b9u32;
    let mut depths: Vec<f32> = (0..tw * th)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed >> 8) as f32 / (1u32 << 24) as f32
        })
        .collect();
    // Far texels and a near wall.
    for y in 0..th {
        for x in 0..tw {
            let i = (y * tw + x) as usize;
            if x % 37 == 3 {
                depths[i] = 1.0;
            }
            if (100..180).contains(&x) && (40..120).contains(&y) {
                depths[i] = 0.01 + (x as f32) * 1e-5;
            }
        }
    }
    let t = depth_texture(&ctx, &depths, tw, th);
    let grid = DepthGrid::new(&ctx);
    let cases = [
        (
            Rect {
                x: 0,
                y: 0,
                width: tw,
                height: th,
            },
            64,
            32,
        ),
        (
            Rect {
                x: 13,
                y: 7,
                width: 256,
                height: 128,
            },
            64,
            32,
        ),
        (
            Rect {
                x: 50,
                y: 20,
                width: 200,
                height: 100,
            },
            7,
            5,
        ),
        // Finer than the rectangle: empty cells are far.
        (
            Rect {
                x: 90,
                y: 30,
                width: 20,
                height: 10,
            },
            64,
            32,
        ),
    ];
    for (rect, gw, gh) in cases {
        let want = cpu_grid(&depths, tw, rect, gw, gh);
        let got = gpu_grid(&ctx, &grid, &t, rect, gw, gh);
        assert_eq!(got.len(), want.len());
        for (i, (g, w)) in got.iter().zip(&want).enumerate() {
            assert_eq!(
                g.to_bits(),
                w.to_bits(),
                "{rect:?} {gw}x{gh} cell {i}: GPU {g} CPU {w}"
            );
        }
    }
}
