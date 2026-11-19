//! The native objects' pass on a headless device (skips without an
//! adapter): an invented two-node model whose square hangs on a turned
//! node, posed by node matrices as the game would hand them over, drawn
//! into an offscreen target. The square lands where the node puts it, lit
//! by its skinned normal, at the game's W depth; an object behind the
//! camera is culled, and one whose node count disagrees is not drawn. Its
//! transparent part blends over the target with the run's own node copy
//! and leaves depth alone.

use native_render::camera::{D24_SCALE, RenderCamera, w_depth};
use native_render::gpu::{DepthRule, Image, PassViewport};
use native_render::instances::ObjectInput;
use native_render::model::{Model, ModelInput, ModelPartInput, ModelSectionInput, NodeMatrix};
use native_render::model_gpu::{GpuModels, Lighting, TransparentItem, TransparentRefusal};
use native_render::pass_state::PassState;

const SIZE: u32 = 64;

/// Node 1's square: in rest space the plane y = 0, facing +y, 4 wide,
/// around the origin.
fn model() -> Model {
    Model::build(&ModelInput {
        inverse_binds: vec![NodeMatrix::IDENTITY; 2],
        sections: vec![ModelSectionInput {
            positions: vec![[-2.0, 0.0, -2.0], [2.0, 0.0, -2.0], [-2.0, 0.0, 2.0], [2.0, 0.0, 2.0]],
            uvs: vec![[0.5; 2]; 4],
            normals: vec![[0.0, 1.0, 0.0]; 4],
            node_indices: vec![[1, 0, 0, 0]; 4],
            node_weights: vec![[1.0, 0.0, 0.0, 0.0]; 4],
            node_map: vec![],
            parts: vec![ModelPartInput {
                material: 0,
                transparent: false,
                triangles: vec![[0, 1, 2], [1, 3, 2]],
            }],
        }],
    })
}

/// A quarter turn about z (x goes to y, y to -x), at `position`.
fn turned(position: [f32; 3]) -> NodeMatrix {
    NodeMatrix {
        scale: 1.0,
        forward: [0.0, 1.0, 0.0],
        left: [-1.0, 0.0, 0.0],
        up: [0.0, 0.0, 1.0],
        position,
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
fn a_posed_object_draws_skinned_and_lit_with_the_games_w_depth() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        eprintln!("no wgpu adapter; skipping");
        return;
    };
    let mut gpu = GpuModels::new(&ctx.device, &ctx.queue);
    // Half ambient, half from straight behind the camera: a square facing
    // the camera is lit 1.0, one facing away 0.5.
    gpu.lighting = Lighting {
        ambient: [0.5; 3],
        direction: [-1.0, 0.0, 0.0],
        diffuse: 0.5,
    };
    let m = gpu.add_model(&ctx.device, &ctx.queue, model(), &mut |_| Some(Image::solid([200, 100, 50, 255])));
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
    let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC;
    let color = target(wgpu::TextureFormat::Rgba8Unorm, usage);
    let depth = target(wgpu::TextureFormat::Depth32Float, usage);
    let (cv, dv) = (color.create_view(&Default::default()), depth.create_view(&Default::default()));
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
    // Node 0 somewhere irrelevant; node 1 turned and 10 ahead: the square
    // stands in the plane x = 10 facing -x, towards the camera.
    let posed = [NodeMatrix { position: [50.0, 50.0, 50.0], ..NodeMatrix::IDENTITY }, turned([10.0, 0.0, 0.0])];
    let behind = [NodeMatrix::IDENTITY, turned([-10.0, 0.0, 0.0])];
    let one = [NodeMatrix::IDENTITY];
    let objects = [
        ObjectInput { model: m, nodes: &posed, sphere: ([10.0, 0.0, 0.0], 3.0) },
        ObjectInput { model: m, nodes: &behind, sphere: ([-10.0, 0.0, 0.0], 3.0) },
        ObjectInput { model: m, nodes: &one, sphere: ([10.0, 0.0, 0.0], 3.0) },
    ];
    let arena = gfx::ring::UploadPool::default().begin();
    let mut uploads = gfx::ring::EncoderUploads { device: &ctx.device, queue: &ctx.queue, encoder: &mut enc, arena: &arena };
    let f = gpu.prepare(&ctx.device, &mut uploads, &camera(), DepthRule::D24, &objects);
    assert_eq!((f.instances.len(), f.culled, f.mismatched, f.draws.len()), (1, 1, 1, 1));
    gpu.encode(
        &ctx.device,
        &mut enc,
        &cv,
        wgpu::TextureFormat::Rgba8Unorm,
        &dv,
        wgpu::TextureFormat::Depth32Float,
        PassViewport {
            x: 0.0,
            y: 0.0,
            width: SIZE as f32,
            height: SIZE as f32,
        },
    );
    arena.flush(&ctx.queue);
    ctx.queue.submit([enc.finish()]);

    let px = read(&ctx, &color, wgpu::TextureAspect::All);
    let at = |x: u32, y: u32| {
        let i = ((y * SIZE + x) * 4) as usize;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    };
    let c = at(SIZE / 2, SIZE / 2);
    for (k, want) in [200i32, 100, 50].into_iter().enumerate() {
        assert!((i32::from(c[k]) - want).abs() <= 1, "centre {c:?}: the base map lit 1.0 by the turned normal");
    }
    assert_eq!(at(0, 0), [0, 0, 0, 255], "corner clear: the square is 0.35 of the half-width");

    let d = read(&ctx, &depth, wgpu::TextureAspect::DepthOnly);
    let i = ((SIZE / 2 * SIZE + SIZE / 2) * 4) as usize;
    let got = f32::from_le_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]);
    let want = w_depth(10.0, 1024.0, D24_SCALE);
    assert!((got - want).abs() <= 1.0 / D24_SCALE, "depth {got} want {want}");
}

/// The square as material 1's transparent part.
fn transparent_model() -> Model {
    let mut input = ModelInput {
        inverse_binds: vec![NodeMatrix::IDENTITY; 2],
        sections: vec![ModelSectionInput {
            positions: vec![[-2.0, 0.0, -2.0], [2.0, 0.0, -2.0], [-2.0, 0.0, 2.0], [2.0, 0.0, 2.0]],
            uvs: vec![[0.5; 2]; 4],
            normals: vec![[0.0, 1.0, 0.0]; 4],
            node_indices: vec![[1, 0, 0, 0]; 4],
            node_weights: vec![[1.0, 0.0, 0.0, 0.0]; 4],
            node_map: vec![],
            parts: vec![],
        }],
    };
    input.sections[0].parts.push(ModelPartInput {
        material: 1,
        transparent: true,
        triangles: vec![[0, 1, 2], [1, 3, 2]],
    });
    Model::build(&input)
}

fn alpha_blended() -> PassState {
    PassState {
        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
        blend_constant: wgpu::Color::BLACK,
        color_writes: wgpu::ColorWrites::ALL,
        depth_write: false,
        depth_compare: wgpu::CompareFunction::LessEqual,
        primitive: wgpu::PrimitiveState::default(),
        alpha_test: None,
    }
}

#[test]
fn an_objects_transparent_part_blends_with_the_runs_own_node_copy_and_keeps_depth() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        eprintln!("no wgpu adapter; skipping");
        return;
    };
    let mut gpu = GpuModels::new(&ctx.device, &ctx.queue);
    gpu.lighting = Lighting {
        ambient: [0.5; 3],
        direction: [-1.0, 0.0, 0.0],
        diffuse: 0.5,
    };
    let m = gpu.add_model(&ctx.device, &ctx.queue, transparent_model(), &mut |_| None);
    let (dev, q) = (&ctx.device, &ctx.queue);
    // Refusals: no such model, no transparent part of material 0, an
    // alpha-tested pass.
    assert_eq!(gpu.set_transparent(dev, q, 9, 1, None, alpha_blended()), Err(TransparentRefusal::NoModel));
    assert_eq!(gpu.set_transparent(dev, q, m, 0, None, alpha_blended()), Err(TransparentRefusal::NoParts));
    let tested = PassState { alpha_test: Some((wgpu::CompareFunction::Greater, 0x80)), ..alpha_blended() };
    assert_eq!(gpu.set_transparent(dev, q, m, 1, None, tested), Err(TransparentRefusal::AlphaTest));
    assert!(!gpu.has_transparent(m, 1));
    let half = Some(Image::solid([200, 100, 50, 128]));
    assert_eq!(gpu.set_transparent(dev, q, m, 1, half, alpha_blended()), Ok(()));
    assert!(gpu.has_transparent(m, 1));

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
    let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC;
    let color = target(wgpu::TextureFormat::Rgba8Unorm, usage);
    let depth = target(wgpu::TextureFormat::Depth32Float, usage);
    let (cv, dv) = (color.create_view(&Default::default()), depth.create_view(&Default::default()));
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
    // Node 1 turned and 10 ahead, as in the opaque test.
    let posed = [NodeMatrix::IDENTITY, turned([10.0, 0.0, 0.0])];
    let one = [NodeMatrix::IDENTITY];
    let items = [
        TransparentItem { model: m, material: 1, nodes: &posed },
        TransparentItem { model: m, material: 1, nodes: &one },
        TransparentItem { model: m, material: 0, nodes: &posed },
    ];
    let viewport = PassViewport {
        x: 0.0,
        y: 0.0,
        width: SIZE as f32,
        height: SIZE as f32,
    };
    let drawn = gpu.encode_transparents(
        &ctx.device,
        &mut enc,
        &cv,
        wgpu::TextureFormat::Rgba8Unorm,
        &dv,
        wgpu::TextureFormat::Depth32Float,
        viewport,
        &camera(),
        DepthRule::D24,
        &items,
    );
    assert_eq!(drawn, 1, "the wrong node count and the material not taken are left out");
    ctx.queue.submit([enc.finish()]);

    let px = read(&ctx, &color, wgpu::TextureAspect::All);
    let i = ((SIZE / 2 * SIZE + SIZE / 2) * 4) as usize;
    let c = [px[i], px[i + 1], px[i + 2]];
    for (k, want) in [100i32, 50, 25].into_iter().enumerate() {
        assert!((i32::from(c[k]) - want).abs() <= 2, "centre {c:?}: half the base map over black");
    }
    assert_eq!([px[0], px[1], px[2]], [0, 0, 0], "corner clear");

    let d = read(&ctx, &depth, wgpu::TextureAspect::DepthOnly);
    let got = f32::from_le_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]);
    assert_eq!(got, 1.0, "no depth write");
}
