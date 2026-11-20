//! `gfx::depth_bits` against the CPU path it replaces: a `D24S8` read-back
//! (`encode_depth`'s `round(clamp(d, 0, 1) * 16777215)`) decoded as raw
//! bits (`convert::depth_channels`). Every 24-bit depth value, and a sweep
//! of other `f32` depths with the values next to each rounding tie, must
//! give the same bytes. Skipped without an adapter or without
//! `DEPTH32FLOAT_STENCIL8`.

use gfx::GfxContext;
use gfx::depth_bits::DepthBits;
use gfx::encode::{DepthPlane, encode_depth};
use gfx::format::D3DFormat;
use gfx::surface::SurfaceLayout;

const W: u32 = 4096;

/// A `Depth32FloatStencil8` texture of `W` x `rows` whose depth texels are
/// `depths` (row-major; written as `frag_depth`) and stencil `stencil`.
fn depth_texture(ctx: &GfxContext, depths: &[f32], stencil: u32) -> wgpu::Texture {
    let rows = depths.len() as u32 / W;
    let device = &ctx.device;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("test depth"),
        size: wgpu::Extent3d {
            width: W,
            height: rows,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32FloatStencil8,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
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
                     return v[u32(p.y) * {W}u + u32(p.x)];
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
            format: wgpu::TextureFormat::Depth32FloatStencil8,
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
                    load: wgpu::LoadOp::Clear(0.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(stencil),
                    store: wgpu::StoreOp::Store,
                }),
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

/// The CPU path's bytes for the texture's stored depths: read back, encode
/// as linear `D24S8`, decode as raw bits.
fn cpu_bits(
    ctx: &GfxContext,
    texture: &wgpu::Texture,
    rows: u32,
    stencil: u8,
) -> (Vec<u8>, Vec<f32>) {
    let raw =
        gfx::readback::read_texture(ctx, texture, wgpu::TextureAspect::DepthOnly, 4, (W, rows));
    let depth: Vec<f32> = raw
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect();
    let n = (W * rows) as usize;
    let plane = DepthPlane {
        depth: depth.clone(),
        stencil: Some(vec![stencil; n]),
    };
    let words = encode_depth(
        D3DFormat::LinD24S8,
        &plane,
        W,
        rows,
        &SurfaceLayout::Linear { pitch: W * 4 },
    )
    .unwrap();
    (
        gfx::convert::depth_channels(D3DFormat::LinD24S8, &words, n).unwrap(),
        depth,
    )
}

fn gpu_bits(ctx: &GfxContext, texture: &wgpu::Texture, rows: u32) -> Vec<u8> {
    let bits = DepthBits::new(ctx);
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    let out = bits.convert(ctx, &mut enc, texture, (W, rows));
    ctx.queue.submit([enc.finish()]);
    gfx::readback::read_texture(ctx, &out, wgpu::TextureAspect::All, 4, (W, rows))
}

fn check(ctx: &GfxContext, depths: &[f32], stencil: u8, what: &str) {
    let rows = depths.len() as u32 / W;
    let t = depth_texture(ctx, depths, stencil as u32);
    let (want, stored) = cpu_bits(ctx, &t, rows, stencil);
    let got = gpu_bits(ctx, &t, rows);
    assert_eq!(got.len(), want.len());
    if got != want {
        let i = got
            .chunks(4)
            .zip(want.chunks(4))
            .position(|(a, b)| a != b)
            .unwrap();
        panic!(
            "{what}: texel {i} (depth {:e}, bits {:#010x}) GPU {:?} CPU {:?}",
            stored[i],
            stored[i].to_bits(),
            &got[i * 4..i * 4 + 4],
            &want[i * 4..i * 4 + 4]
        );
    }
}

fn ctx() -> Option<GfxContext> {
    let ctx = match GfxContext::headless() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            return None;
        }
    };
    if !ctx.caps.depth32_stencil8 {
        eprintln!("skipping GPU test: no DEPTH32FLOAT_STENCIL8");
        return None;
    }
    Some(ctx)
}

/// Every 24-bit depth `z` as the surface holds it after a fill
/// (`z / 16777215` in `f32`, what `decode_depth` writes), so a level filled
/// from guest memory converts back to its own bits.
#[test]
fn depth_bits_match_the_readback_for_every_24_bit_depth() {
    let Some(ctx) = ctx() else { return };
    let depths: Vec<f32> = (0..1u32 << 24).map(|z| z as f32 / 16_777_215.0).collect();
    check(&ctx, &depths, 0x5A, "every z / 16777215");
}

/// Depths a draw can leave: a sweep over every `f32` in `[0, 1]` at a
/// stride, the values either side of each rounding tie
/// (`(k + 0.5) / 16777215` and its neighbours), and the ends of the range
/// (0, -0, the smallest subnormal, 1).
#[test]
fn depth_bits_match_the_readback_for_other_depths() {
    let Some(ctx) = ctx() else { return };
    let one = 1.0f32.to_bits();
    let mut depths: Vec<f32> = (0..one).step_by(127).map(f32::from_bits).collect();
    let mut k = 0u32;
    while !depths.len().is_multiple_of(W as usize) || depths.len() < 12 << 20 {
        let tie = (k as f64 + 0.5) / 16_777_215.0;
        let t = tie as f32;
        depths.extend([
            f32::from_bits(t.to_bits() - 1),
            t,
            f32::from_bits(t.to_bits() + 1),
        ]);
        k = (k + 7919) % (1 << 24);
    }
    depths.truncate(depths.len() / W as usize * W as usize);
    depths[..8].copy_from_slice(&[1.0, 0.0, -0.0, 1.0e-45, 0.999_999_94, 0.5, 0.25, 1.0 / 3.0]);
    for stencil in [0x00, 0xA5] {
        check(&ctx, &depths, stencil, "sweep");
    }
}
