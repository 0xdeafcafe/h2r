use std::sync::{Arc, Mutex};

use wgpu::TextureFormat as F;
use wgpu::TextureUsages as U;
use wgpu::util::DeviceExt;

use super::*;
use crate::gpu::Image;
use crate::light_gpu::{LightGpu, LightTarget};
use crate::light_look::FRAME_DWORDS;
use crate::light_tiles::{Segment, bin_cpu, plan};
use crate::program::{INPUT_MASK, PassParts, WORLD_FETCH, translate_pass};

const COLOUR: F = F::Rgba16Float;
const DEPTH: F = F::Depth32Float;
const BASE: [f32; 4] = [0.1, 0.1, 0.1, 1.0];
const GEL: u32 = 0xe100_0040;

/// `dp4 oPos, v0, c[0..3]; mov oT0, v1; mov oT3, c[31]`: the window
/// position and W from the view's rows, the gel's coordinates, and the
/// light frame's first row as a colour.
fn microcode() -> Vec<u8> {
    use shaders::vsh::encode::{Inst, Operand as O, program};
    use shaders::vsh::{MacOp, OutReg};
    let row = |c, mask| {
        Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(O::c(c)), None).to_out(false, OutReg::Pos, mask)
    };
    program(&[
        row(0, 8),
        row(1, 4),
        row(2, 2),
        row(3, 1),
        Inst::mac(MacOp::Mov, Some(O::v(1)), None, None).to_out(false, OutReg::T(0), 0xf),
        Inst::mac(MacOp::Mov, Some(O::c(31)), None, None).to_out(false, OutReg::T(3), 0xf),
    ])
}

/// A lit pass translated as the game's are: stage 0 (the gel, or a white
/// bitmap) times stage 3 (the light's colour, passed through), added to
/// the target, tested `LessEqual` against the scene's W-buffer.
fn program(gel: bool) -> LightProgram {
    use shaders::psh::bits::*;
    let mut def = shaders::PixelShaderDef {
        texture_modes: texture_modes([TEX_PROJECT2D, TEX_NONE, TEX_NONE, TEX_PASSTHRU]),
        final_inputs_abcd: inputs(REG_T0, REG_T3, REG_ZERO, REG_ZERO),
        final_inputs_efg: final_efg(REG_ZERO, REG_ZERO, REG_T3 | CHAN_ALPHA, 0),
        explicit_final: true,
        ..Default::default()
    };
    def.combiner_count = 1;
    let t = translate_pass(&PassParts {
        microcode: &microcode(),
        pixel_shader: &def.to_bytes(),
        dims: [Some(shaders::psh::TextureDim::D2), None, None, None],
        biased: [false; 4],
        stage_states: &[],
        vertex_constants: &[],
        vertex_defaults: &[],
        alpha_test: None,
        provided_inputs: INPUT_MASK,
        input_fetch: WORLD_FETCH,
        w_buffer: true,
        shadow: [None; 4],
    })
    .expect("translates");
    let add = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    };
    LightProgram {
        wgsl: t.wgsl,
        words: Vec::new(),
        inputs: t.inputs,
        def: t.def,
        pixel_options: t.pixel_options,
        stages: [
            if gel {
                LightStage::Gel
            } else {
                LightStage::Bitmap {
                    bitmap: map::TagId(7),
                    image: 0,
                }
            },
            LightStage::Empty,
            LightStage::Empty,
            LightStage::Empty,
        ],
        samplers: t.samplers,
        dims: t.dims,
        signed: t.signed,
        vertex: t.vertex,
        pixel: t.pixel,
        colours: Vec::new(),
        state: crate::pass_state::PassState {
            blend: Some(wgpu::BlendState {
                color: add,
                alpha: add,
            }),
            blend_constant: wgpu::Color::BLACK,
            color_writes: wgpu::ColorWrites::ALL,
            depth_write: false,
            depth_compare: wgpu::CompareFunction::LessEqual,
            primitive: wgpu::PrimitiveState::default(),
            alpha_test: None,
            stencil: None,
            depth_offset: None,
        },
        inherits_cull: false,
        copies_target: false,
        animation: None,
    }
}

#[test]
fn the_tiled_rewrite_indexes_uniforms_by_instance() {
    let p = program(true);
    let tiled = tiled_wgsl(&p.wgsl).expect("the shape shaders::link gives");
    shaders::validate(&tiled).expect("valid WGSL");
    assert!(
        !tiled.contains("var<uniform> xvs") && !tiled.contains("xvs.") && !tiled.contains("xps.")
    );
    assert!(
        tiled.contains("xvs_light = xl_instance;") && tiled.contains("out.xl_light = xl_instance;")
    );
    assert!(tiled.contains("xl_tile_colour(v, r.color)"));
    assert_eq!(
        tiled_wgsl("@vertex fn vs_main() {}"),
        Err(TileRefusal::Program)
    );
    assert_eq!(
        admit_pass(&LitPass {
            program: &p,
            gel_target: false
        }),
        Ok(())
    );
    assert_eq!(
        admit_pass(&LitPass {
            program: &p,
            gel_target: true
        }),
        Err(TileRefusal::GelTarget)
    );
    let mut replace = p.clone();
    replace.state.blend = None;
    assert_eq!(
        admit_pass(&LitPass {
            program: &replace,
            gel_target: false
        }),
        Err(TileRefusal::Blend)
    );
    let mut reads = p.clone();
    reads.state.blend.as_mut().unwrap().color.src_factor = wgpu::BlendFactor::DstAlpha;
    assert_eq!(
        admit_pass(&LitPass {
            program: &reads,
            gel_target: false
        }),
        Err(TileRefusal::Blend)
    );
    let mut masked = reads.clone();
    masked.state.color_writes = wgpu::ColorWrites::COLOR;
    assert_eq!(alpha_read(&masked), AlphaRead::Dst);
    assert_eq!(alpha_read(&p), AlphaRead::None);
    assert_eq!(
        admit_pass(&LitPass {
            program: &masked,
            gel_target: false
        }),
        Ok(()),
        "a read of the destination's alpha writing no alpha reads its mask"
    );
    let reading = tiled_wgsl_reading(&masked.wgsl, AlphaRead::OneMinusDst, true).unwrap();
    shaders::validate(&reading).expect("valid WGSL");
    assert!(reading.contains("saturate(c.rgb) * (1.0 - a)"));
    assert!(tiled.contains("fn xl_alpha_factor(v: XboxVaryings, c: vec4f) -> vec4f { return c; }"));
    let mut writes = p.clone();
    writes.state.depth_write = true;
    assert_eq!(
        admit_pass(&LitPass {
            program: &writes,
            gel_target: false
        }),
        Err(TileRefusal::Depth)
    );
    let mut shadowed = p.clone();
    shadowed.stages[0] = LightStage::DepthShadow;
    assert_eq!(
        admit_pass(&LitPass {
            program: &shadowed,
            gel_target: false
        }),
        Err(TileRefusal::Shadow)
    );
    let v = TileVolume {
        rect: [0.0; 4],
        near: 0.0,
    };
    let pass = [LitPass {
        program: &p,
        gel_target: false,
    }];
    assert_eq!(
        admit_light(&[10, 11, 14, 12, 13], &pass, Some(v)),
        Err(TileRefusal::Shadow)
    );
    assert_eq!(
        admit_light(&[14, 12, 13], &pass, Some(v)),
        Err(TileRefusal::Opaque)
    );
    assert_eq!(
        admit_light(&[12, 13], &pass, None),
        Err(TileRefusal::Bounds)
    );
    assert_eq!(admit_light(&[12, 13], &pass, Some(v)), Ok(v));
    let alpha = wgpu::ColorWrites::ALPHA;
    assert_eq!(
        admit_shadowed_light(&[10, 11, 12, 13], &[alpha, alpha]),
        Ok(())
    );
    assert_eq!(
        admit_shadowed_light(&[10, 11, 12], &[wgpu::ColorWrites::ALL]),
        Err(TileRefusal::Shadow),
        "a receiver that multiplies the colour"
    );
    assert_eq!(
        admit_shadowed_light(&[10, 11, 14, 12, 13], &[alpha]),
        Err(TileRefusal::Opaque)
    );
    assert_eq!(
        admit_shadowed_light(&[11, 12], &[alpha]),
        Err(TileRefusal::Shadow),
        "receivers with no caster"
    );
}

/// A depth target holding `tiles` (row by row, `across` a row) as the
/// stored depth of each 16x16 tile.
fn scene_depth(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: (u32, u32),
    tiles: &[f32],
    across: u32,
) -> wgpu::Texture {
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH,
        usage: U::RENDER_ATTACHMENT | U::TEXTURE_BINDING,
        view_formats: &[],
    });
    let mut values = [[0.0f32; 4]; 4];
    for (i, d) in tiles.iter().enumerate() {
        values[i / 4][i % 4] = *d;
    }
    let mut bytes: Vec<u8> = bytemuck::cast_slice(&values).to_vec();
    bytes.extend_from_slice(&[across, 0, 0, 0].map(u32::to_le_bytes).concat());
    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: &bytes,
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(
            "struct D { d: array<vec4f, 4>, across: vec4u }
            @group(0) @binding(0) var<uniform> u: D;
            @vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
                let p = vec2f(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
                return vec4f(p, 0.5, 1.0);
            }
            @fragment fn fs(@builtin(position) p: vec4f) -> @builtin(frag_depth) f32 {
                let t = vec2u(p.xy) / 16u;
                let i = t.y * u.across.x + t.x;
                return u.d[i / 4u][i % 4u];
            }"
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
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH,
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
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: buffer.as_entire_binding(),
        }],
    });
    let view = depth.create_view(&Default::default());
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
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
    }
    queue.submit([enc.finish()]);
    depth
}

fn read_masks(ctx: &gfx::GfxContext, grid: &TileGrid) -> Vec<u32> {
    let size = grid.masks.size();
    let staging = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    enc.copy_buffer_to_buffer(&grid.masks, 0, &staging, 0, size);
    ctx.queue.submit([enc.finish()]);
    staging.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    
    bytemuck::cast_slice::<u8, u32>(&staging.slice(..).get_mapped_range().unwrap()).to_vec()
}

#[test]
fn a_tiles_list_holds_exactly_the_lights_whose_volume_touches_it() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        return;
    };
    let (device, queue) = (&ctx.device, &ctx.queue);
    // 40x24: three tiles across (the last 8 wide), two down (the last 8
    // high).
    let size = (40, 24);
    let far = [0.9, 0.2, 0.6, 0.5, 0.9, 0.3];
    let depth = scene_depth(device, queue, size, &far, 3);
    let volumes = [
        // Everything at depth 0.25: tiles but the 0.2 one.
        TileVolume {
            rect: [0.0, 0.0, 40.0, 24.0],
            near: 0.25,
        },
        // Inside tile (0, 0) only.
        TileVolume {
            rect: [3.0, 3.0, 9.0, 9.0],
            near: 0.0,
        },
        // Its right edge at 16, the first column's end: not the second.
        TileVolume {
            rect: [0.0, 0.0, 16.0, 24.0],
            near: 0.0,
        },
        // Over tiles (1..3, 1) but behind the 0.3 one.
        TileVolume {
            rect: [17.0, 17.0, 40.0, 24.0],
            near: 0.4,
        },
        // Past the target: no tile.
        TileVolume {
            rect: [40.0, 0.0, 60.0, 24.0],
            near: 0.0,
        },
    ];
    let tiles = LightTiles::new(device);
    let mut enc = device.create_command_encoder(&Default::default());
    let grid = tiles.bin(
        device,
        &mut enc,
        &depth.create_view(&Default::default()),
        size,
        &volumes,
    );
    queue.submit([enc.finish()]);
    assert_eq!(grid.tiles, (3, 2));
    let masks = read_masks(&ctx, &grid);
    let by_hand = [
        0b00111, 0b00000, 0b00001, //
        0b00101, 0b01001, 0b00001, //
    ];
    assert_eq!(masks, by_hand);
    let mut cpu_depth = vec![0.0; (size.0 * size.1) as usize];
    for y in 0..size.1 {
        for x in 0..size.0 {
            cpu_depth[(y * size.0 + x) as usize] = far[((y / 16) * 3 + x / 16) as usize];
        }
    }
    assert_eq!(
        bin_cpu(&cpu_depth, size, &volumes),
        masks,
        "the CPU reference agrees"
    );
}

fn half(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = i32::from((bits >> 10) & 0x1f);
    let frac = f32::from(bits & 0x3ff);
    sign * match exp {
        0 => frac * 2f32.powi(-24),
        31 => f32::INFINITY,
        e => (1.0 + frac / 1024.0) * 2f32.powi(e - 15),
    }
}

/// A quad of two triangles over window `[x0, x1] x [y0, y1]` at W `w`, its
/// texture coordinates the window's over 32.
fn quad(x0: f32, y0: f32, x1: f32, y1: f32, w: f32) -> [WorldVertex; 6] {
    let v = |x: f32, y: f32| WorldVertex {
        position: [x, y, w],
        uv: [x / 32.0, y / 32.0],
        ..Default::default()
    };
    [
        v(x0, y0),
        v(x1, y0),
        v(x0, y1),
        v(x0, y1),
        v(x1, y0),
        v(x1, y1),
    ]
}

#[test]
fn tiled_shading_equals_the_per_light_passes() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        return;
    };
    let (device, queue) = (&ctx.device, &ctx.queue);
    const SIZE: (u32, u32) = (32, 32);
    // Tiles' scene depth: the full quad (0.5) passes in (0, 0) and (1, 1)
    // and fails in the others; the small quad (0.4) passes in (0, 0).
    let depth = scene_depth(device, queue, SIZE, &[0.8, 0.3, 0.45, 0.8], 2);
    let depth_view = depth.create_view(&Default::default());
    let mut constants = [[0.0; 4]; gfx::external::VIEW_CONSTANTS];
    constants[0] = [1.0, 0.0, 0.0, 0.0];
    constants[1] = [0.0, 1.0, 0.0, 0.0];
    constants[2] = [0.0, 0.0, 1.0, 0.0];
    constants[3] = [0.0, 0.0, 1.0, 0.0];
    let view = ViewInputs {
        constants,
        guest_size: SIZE,
        size: SIZE,
        z_scale: 100.0,
        w_clip: Some(([0.0, 100.0], true)),
    };

    let set = {
        let mut set = LightSet::new(device, queue, None, None);
        let checker = [
            255, 255, 255, 255, 64, 64, 64, 255, 64, 64, 64, 255, 255, 255, 255, 255,
        ];
        set.upload_gel(
            device,
            queue,
            GEL,
            &Image::from_rgba(2, 2, checker.to_vec(), Vec::new()),
        );
        set
    };
    let gpu = |p: &LightProgram| {
        let mut gpu = LightGpu::new(
            device,
            queue,
            Arc::new(p.clone()),
            set.uniforms_layout(),
            &mut |_, _| Some(Image::solid([255; 4])),
        )
        .unwrap();
        assert!(
            gpu.pipeline(device, COLOUR, DEPTH, None, None)
                .1
                .get()
                .is_some()
        );
        Mutex::new(gpu)
    };
    let (plain, gel) = (program(false), program(true));
    let (plain_gpu, gel_gpu) = (gpu(&plain), gpu(&gel));
    timeline::frame();

    // G1 the whole target at W 50, G2 a small quad in tile (0, 0) at W 40,
    // G3 the right half at W 50.
    let mut vertices = Vec::new();
    vertices.extend(quad(0.0, 0.0, 32.0, 32.0, 50.0));
    vertices.extend(quad(2.0, 2.0, 12.0, 12.0, 40.0));
    vertices.extend(quad(16.0, 0.0, 32.0, 32.0, 50.0));
    let init = |usage, bytes: &[u8]| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytes,
            usage,
        })
    };
    let vertex_buffer = init(wgpu::BufferUsages::VERTEX, bytemuck::cast_slice(&vertices));
    let index: Vec<u32> = (0..vertices.len() as u32).collect();
    let index_buffer = init(wgpu::BufferUsages::INDEX, bytemuck::cast_slice(&index));
    let (g1, g2, g3) = ([0..6], [6..12], [12..18]);

    let frame = |rgba: [f32; 4]| {
        let mut f = [0u32; FRAME_DWORDS];
        for (w, c) in f.iter_mut().zip(rgba) {
            *w = c.to_bits();
        }
        f
    };
    // A, B (gel) and C overlap; S is shadowed. C draws G2 twice (a
    // duplicate sorted entry) and has a layer 13 over G1.
    let (fa, fb, fc, fs) = (
        frame([0.5, 0.1, 0.0, 0.25]),
        frame([0.2, 0.6, 0.1, 0.5]),
        frame([0.05, 0.3, 0.7, 0.125]),
        frame([0.4, 0.4, 0.4, 1.0]),
    );
    let draw = |program: &'static LightProgram, gpu, frame, ranges, gel: Option<u32>| LightDraw {
        program,
        gpu,
        cull: None,
        sample: None,
        colours: [[0.0; 3]; 2],
        frame,
        projection: None,
        gel,
        gel_view: None,
        gel_scale: None,
        shadow_map: None,
        ranges,
        placement: None,
        model: None,
    };
    let plain: &'static LightProgram = Box::leak(Box::new(plain));
    let gel: &'static LightProgram = Box::leak(Box::new(gel));
    let a = || {
        [
            draw(plain, &plain_gpu, &fa, &g1, None),
            draw(plain, &plain_gpu, &fa, &g2, None),
        ]
    };
    let b = || draw(gel, &gel_gpu, &fb, &g3, Some(GEL));
    let c12 = || {
        [
            draw(plain, &plain_gpu, &fc, &g1, None),
            draw(plain, &plain_gpu, &fc, &g2, None),
            draw(plain, &plain_gpu, &fc, &g2, None),
        ]
    };
    let c13 = || draw(plain, &plain_gpu, &fc, &g1, None);
    let s = || [draw(plain, &plain_gpu, &fs, &g1, None)];

    let colour = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: SIZE.0,
            height: SIZE.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COLOUR,
        usage: U::RENDER_ATTACHMENT | U::COPY_SRC,
        view_formats: &[],
    });
    let colour_view = colour.create_view(&Default::default());
    let pool = gfx::ring::UploadPool::default();
    let per_light = |draws: &[LightDraw<'_>], clear: Option<[f32; 4]>| {
        let uploads = pool.begin();
        let target = LightTarget {
            device,
            queue,
            uploads: &uploads,
            color: &colour_view,
            color_format: COLOUR,
            depth: &depth_view,
            depth_format: DEPTH,
            size: SIZE,
        };
        let packets = set
            .draws(
                &target,
                &vertex_buffer,
                &index_buffer,
                &view,
                [0, 0, SIZE.0, SIZE.1],
                draws,
            )
            .expect("per-light draws");
        let front = uploads.upload_commands(queue);
        let rt = gfx::RenderTarget {
            color: &colour_view,
            color_format: COLOUR,
            depth: Some(&depth_view),
            depth_format: Some(DEPTH),
            size: SIZE,
            samples: 1,
        };
        let mut enc = device.create_command_encoder(&Default::default());
        let clear = gfx::Clear {
            color: clear,
            depth: None,
            stencil: None,
        };
        gfx::Renderer::encode_pass(&ctx, &mut enc, &rt, clear, &packets.packets, None).unwrap();
        queue.submit(front.into_iter().chain([enc.finish()]));
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    };
    let read = || {
        gfx::readback::read_texture(&ctx, &colour, wgpu::TextureAspect::All, 8, SIZE)
            .chunks_exact(2)
            .map(|h| half(u16::from_le_bytes([h[0], h[1]])))
            .collect::<Vec<f32>>()
    };

    // The reference: each light's passes in the game's order.
    let reference: Vec<LightDraw<'_>> = a()
        .into_iter()
        .chain([b()])
        .chain(c12())
        .chain([c13()])
        .chain(s())
        .collect();
    per_light(&reference, Some(BASE));
    let expected = read();

    // Tiled: admission, the plan, the run binned and drawn, then S.
    let geometry = |ranges: &[Range<u32>]| {
        let mut corners = Vec::new();
        for r in ranges {
            for v in &vertices[r.start as usize..r.end as usize] {
                corners.push(v.position);
            }
        }
        TileVolume::from_window(&corners, SIZE, SIZE, view.z_scale)
    };
    let volume_a = geometry(&[0..6, 6..12]);
    let volume_b = geometry(&[12..18]);
    let pass = |p| {
        [LitPass {
            program: p,
            gel_target: false,
        }]
    };
    let lights = [
        (
            frame_scene::ViewSlot::LightAccumulation,
            admit_light(&[12], &pass(plain), volume_a),
        ),
        (
            frame_scene::ViewSlot::LightAccumulation,
            admit_light(&[12], &pass(gel), volume_b),
        ),
        (
            frame_scene::ViewSlot::LightAccumulation,
            admit_light(&[12, 13], &pass(plain), volume_a),
        ),
        (
            frame_scene::ViewSlot::LightAccumulation,
            admit_light(&[10, 11, 12], &pass(plain), volume_a),
        ),
    ];
    let segments = plan(
        &lights
            .iter()
            .map(|(s, r)| (*s, r.map(|_| ())))
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        segments,
        [
            Segment::Tiled(0..3),
            Segment::PerLight {
                light: 3,
                refused: Some(TileRefusal::Shadow)
            }
        ]
    );
    let volumes: Vec<TileVolume> = lights[..3].iter().map(|(_, v)| v.unwrap()).collect();

    let mut tiles = LightTiles::new(device);
    let mut enc = device.create_command_encoder(&Default::default());
    let grid = tiles.bin(device, &mut enc, &depth_view, SIZE, &volumes);
    queue.submit([enc.finish()]);
    assert_eq!(read_masks(&ctx, &grid), [0b111, 0b000, 0b101, 0b111]);
    let ([a0, a1], [c0, c1, c2]) = (a(), c12());
    let run = [
        TiledDraw {
            set: None,
            layer: 13,
            lights: vec![(2, c13())],
        },
        TiledDraw {
            set: None,
            layer: 12,
            lights: vec![(0, a0), (2, c0)],
        },
        TiledDraw {
            set: None,
            layer: 12,
            lights: vec![(0, a1), (2, c1), (2, c2)],
        },
        TiledDraw {
            set: None,
            layer: 12,
            lights: vec![(1, b())],
        },
    ];
    let tiled = tiles
        .record(device, &set, &grid, &view, COLOUR, DEPTH, &run)
        .expect("records");
    assert_eq!(
        tiled.draws(),
        4,
        "one draw per geometry range, whatever its lights"
    );
    let encode = |tiled: &TiledPass| {
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let [r, g, b, a] = BASE.map(f64::from);
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &colour_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            tiled.encode(
                &mut pass,
                &vertex_buffer,
                &index_buffer,
                [0, 0, SIZE.0, SIZE.1],
            );
        }
        queue.submit([enc.finish()]);
    };
    encode(&tiled);
    // The refused light on its per-light pass, at its place after the run.
    per_light(&s(), None);
    let got = read();

    let lit = expected
        .chunks_exact(4)
        .filter(|p| (p[0] - BASE[0]).abs() > 0.01)
        .count();
    assert!(lit > 100, "the scene is lit ({lit} pixels)");
    let texel = |v: &[f32], x: u32, y: u32| v[(4 * (y * SIZE.0 + x)) as usize..][..4].to_vec();
    assert_ne!(
        texel(&expected, 20, 5),
        texel(&expected, 20, 25),
        "the gel and tile depths differ"
    );
    for (i, (e, g)) in expected.iter().zip(&got).enumerate() {
        let tolerance = 2e-3 * e.abs().max(1.0);
        assert!(
            (e - g).abs() <= tolerance,
            "texel {} channel {}: per light {e}, tiled {g}",
            i / 4,
            i % 4
        );
    }

    // The fragments read their tile's list: with no tile listing any
    // light, the run adds nothing.
    let none = volumes
        .iter()
        .map(|v| TileVolume { near: 2.0, ..*v })
        .collect::<Vec<_>>();
    let mut enc = device.create_command_encoder(&Default::default());
    let grid = tiles.bin(device, &mut enc, &depth_view, SIZE, &none);
    queue.submit([enc.finish()]);
    encode(
        &tiles
            .record(device, &set, &grid, &view, COLOUR, DEPTH, &run)
            .expect("records"),
    );
    let unlit = read();
    for (i, v) in unlit.iter().enumerate() {
        assert!(
            (v - BASE[i % 4]).abs() < 1e-3,
            "texel {} channel {} is {v}",
            i / 4,
            i % 4
        );
    }
}

#[test]
fn the_compare_finds_the_largest_channel_difference() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        return;
    };
    let (device, queue) = (&ctx.device, &ctx.queue);
    let size = (12u32, 9u32);
    let texture = |rgba: &[u8]| {
        device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: F::Rgba8Unorm,
                usage: U::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            rgba,
        )
    };
    let base: Vec<u8> = (0..size.0 * size.1 * 4).map(|i| (i % 251) as u8).collect();
    let mut changed = base.clone();
    changed[4 * (7 * size.0 + 10) as usize + 2] += 10;
    let compare = Compare::new(device);
    let run = |a: &wgpu::Texture, b: &wgpu::Texture| {
        let diff = compare.prepare(
            device,
            &a.create_view(&Default::default()),
            &b.create_view(&Default::default()),
            size,
        );
        let mut enc = device.create_command_encoder(&Default::default());
        diff.encode(&mut enc);
        queue.submit([enc.finish()]);
        diff.readback()
            .slice(..)
            .map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        
        difference(&diff.readback().slice(..).get_mapped_range().unwrap())
    };
    let (a, b, c) = (texture(&base), texture(&base), texture(&changed));
    assert_eq!(run(&a, &b), 0.0);
    let d = run(&a, &c);
    assert!((d - 10.0 / 255.0).abs() < 1e-4, "{d}");
    assert!(
        d > crate::light_tiles::tolerance(3),
        "ten steps exceed three lights' rounding"
    );
}

/// Writes `alpha` into `view`'s alpha over `rect` (x, y, w, h), its
/// colour kept: a stand-in for a shadowed light's layer 11 marks.
fn fill_alpha(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    view: &wgpu::TextureView,
    rect: [u32; 4],
    alpha: f32,
) {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(
            format!(
                "@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {{
                    let p = vec2f(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
                    return vec4f(p, 0.5, 1.0);
                }}
                @fragment fn fs() -> @location(0) vec4f {{ return vec4f(0.0, 0.0, 0.0, {alpha:?}); }}"
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
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: COLOUR,
                blend: None,
                write_mask: wgpu::ColorWrites::ALPHA,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
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
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        let [x, y, w, h] = rect;
        pass.set_scissor_rect(x, y, w, h);
        pass.set_pipeline(&pipeline);
        pass.draw(0..3, 0..1);
    }
    queue.submit([enc.finish()]);
}

#[test]
fn a_masked_run_reads_each_lights_alpha_as_its_per_light_pass_did() {
    let Ok(ctx) = gfx::GfxContext::headless() else {
        return;
    };
    let (device, queue) = (&ctx.device, &ctx.queue);
    const SIZE: (u32, u32) = (32, 32);
    let depth = scene_depth(device, queue, SIZE, &[0.8; 4], 2);
    let depth_view = depth.create_view(&Default::default());
    let mut constants = [[0.0; 4]; gfx::external::VIEW_CONSTANTS];
    constants[0] = [1.0, 0.0, 0.0, 0.0];
    constants[1] = [0.0, 1.0, 0.0, 0.0];
    constants[2] = [0.0, 0.0, 1.0, 0.0];
    constants[3] = [0.0, 0.0, 1.0, 0.0];
    let view = ViewInputs {
        constants,
        guest_size: SIZE,
        size: SIZE,
        z_scale: 100.0,
        w_clip: Some(([0.0, 100.0], true)),
    };
    let set = LightSet::new(device, queue, None, None);
    // `reads` adds its colour times the destination's alpha; `plain` adds
    // it as is. Neither writes alpha.
    let mut reads = program(false);
    reads.state.blend.as_mut().unwrap().color.src_factor = wgpu::BlendFactor::DstAlpha;
    reads.state.color_writes = wgpu::ColorWrites::COLOR;
    let mut plain = program(false);
    plain.state.color_writes = wgpu::ColorWrites::COLOR;
    for p in [&reads, &plain] {
        assert_eq!(
            admit_pass(&LitPass {
                program: p,
                gel_target: false
            }),
            Ok(())
        );
    }
    let gpu = |p: &LightProgram| {
        let mut gpu = LightGpu::new(
            device,
            queue,
            Arc::new(p.clone()),
            set.uniforms_layout(),
            &mut |_, _| Some(Image::solid([255; 4])),
        )
        .unwrap();
        assert!(
            gpu.pipeline(device, COLOUR, DEPTH, None, None)
                .1
                .get()
                .is_some()
        );
        Mutex::new(gpu)
    };
    let (reads_gpu, plain_gpu) = (gpu(&reads), gpu(&plain));
    timeline::frame();
    let vertices = quad(0.0, 0.0, 32.0, 32.0, 50.0);
    let init = |usage, bytes: &[u8]| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytes,
            usage,
        })
    };
    let vertex_buffer = init(wgpu::BufferUsages::VERTEX, bytemuck::cast_slice(&vertices));
    let index: Vec<u32> = (0..vertices.len() as u32).collect();
    let index_buffer = init(wgpu::BufferUsages::INDEX, bytemuck::cast_slice(&index));
    let g1 = [0..6];
    let frame = |rgba: [f32; 4]| {
        let mut f = [0u32; FRAME_DWORDS];
        for (w, c) in f.iter_mut().zip(rgba) {
            *w = c.to_bits();
        }
        f
    };
    // A reads the alpha the run began with; S's receivers mark the left
    // half 0.25 before its passes read it; B reads nothing.
    let (fa, fs, fb) = (
        frame([0.5, 0.1, 0.0, 1.0]),
        frame([0.4, 0.4, 0.4, 1.0]),
        frame([0.05, 0.3, 0.7, 1.0]),
    );
    let reads: &'static LightProgram = Box::leak(Box::new(reads));
    let plain: &'static LightProgram = Box::leak(Box::new(plain));
    let draw = |program: &'static LightProgram, gpu, frame| LightDraw {
        program,
        gpu,
        cull: None,
        sample: None,
        colours: [[0.0; 3]; 2],
        frame,
        projection: None,
        gel: None,
        gel_view: None,
        gel_scale: None,
        shadow_map: None,
        ranges: &g1,
        placement: None,
        model: None,
    };
    let a = || draw(reads, &reads_gpu, &fa);
    let s = || draw(reads, &reads_gpu, &fs);
    let b = || draw(plain, &plain_gpu, &fb);

    let colour = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: SIZE.0,
            height: SIZE.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COLOUR,
        usage: U::RENDER_ATTACHMENT | U::COPY_SRC,
        view_formats: &[],
    });
    let colour_view = colour.create_view(&Default::default());
    let pool = gfx::ring::UploadPool::default();
    let per_light = |draws: &[LightDraw<'_>], clear: Option<[f32; 4]>| {
        if draws.is_empty() {
            // `LightSet::draws` refuses a run of no draws (no uniforms to
            // take a slot for): the clear alone is the pass.
            let rt = gfx::RenderTarget {
                color: &colour_view,
                color_format: COLOUR,
                depth: Some(&depth_view),
                depth_format: Some(DEPTH),
                size: SIZE,
                samples: 1,
            };
            let mut enc = device.create_command_encoder(&Default::default());
            let clear = gfx::Clear { color: clear, depth: None, stencil: None };
            gfx::Renderer::encode_pass(&ctx, &mut enc, &rt, clear, &[], None).unwrap();
            queue.submit([enc.finish()]);
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            return;
        }
        let uploads = pool.begin();
        let target = LightTarget {
            device,
            queue,
            uploads: &uploads,
            color: &colour_view,
            color_format: COLOUR,
            depth: &depth_view,
            depth_format: DEPTH,
            size: SIZE,
        };
        let packets = set
            .draws(
                &target,
                &vertex_buffer,
                &index_buffer,
                &view,
                [0, 0, SIZE.0, SIZE.1],
                draws,
            )
            .expect("per-light draws");
        let front = uploads.upload_commands(queue);
        let rt = gfx::RenderTarget {
            color: &colour_view,
            color_format: COLOUR,
            depth: Some(&depth_view),
            depth_format: Some(DEPTH),
            size: SIZE,
            samples: 1,
        };
        let mut enc = device.create_command_encoder(&Default::default());
        let clear = gfx::Clear {
            color: clear,
            depth: None,
            stencil: None,
        };
        gfx::Renderer::encode_pass(&ctx, &mut enc, &rt, clear, &packets.packets, None).unwrap();
        queue.submit(front.into_iter().chain([enc.finish()]));
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    };
    let read = || {
        gfx::readback::read_texture(&ctx, &colour, wgpu::TextureAspect::All, 8, SIZE)
            .chunks_exact(2)
            .map(|h| half(u16::from_le_bytes([h[0], h[1]])))
            .collect::<Vec<f32>>()
    };
    let left = [0, 0, SIZE.0 / 2, SIZE.1];

    // Per light: A, S's marks, S, B.
    per_light(&[a()], Some(BASE));
    fill_alpha(device, queue, &colour_view, left, 0.25);
    per_light(&[s(), b()], None);
    let expected = read();

    // Tiled: the alpha before the run into layer 0, S's marks, the alpha
    // after them into layer 1, then the run reading A's layer 0 and S's 1.
    per_light(&[], Some(BASE));
    let masks = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: SIZE.0,
            height: SIZE.1,
            depth_or_array_layers: 2,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COLOUR,
        usage: U::COPY_DST | U::TEXTURE_BINDING,
        view_formats: &[],
    });
    let snapshot = |layer: u32| {
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_texture_to_texture(
            colour.as_image_copy(),
            wgpu::TexelCopyTextureInfo {
                texture: &masks,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: layer,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: SIZE.0,
                height: SIZE.1,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([enc.finish()]);
    };
    snapshot(0);
    fill_alpha(device, queue, &colour_view, left, 0.25);
    snapshot(1);
    let whole = TileVolume {
        rect: [0.0, 0.0, SIZE.0 as f32, SIZE.1 as f32],
        near: 0.0,
    };
    let mut tiles = LightTiles::new(device);
    let mut enc = device.create_command_encoder(&Default::default());
    let grid = tiles.bin(device, &mut enc, &depth_view, SIZE, &[whole; 3]);
    queue.submit([enc.finish()]);
    let run = [
        TiledDraw {
            set: None,
            layer: 12,
            lights: vec![(0, a()), (1, s())],
        },
        TiledDraw {
            set: None,
            layer: 12,
            lights: vec![(2, b())],
        },
    ];
    assert!(
        tiles
            .record(device, &set, &grid, &view, COLOUR, DEPTH, &run)
            .is_err(),
        "a read of the destination's alpha needs the masks"
    );
    let masks_view = masks.create_view(&mask_view());
    let tiled = tiles
        .record_masked(
            device,
            &set,
            &grid,
            &view,
            COLOUR,
            DEPTH,
            &run,
            Some((&masks_view, &[0u32, 1, 1][..])),
        )
        .expect("records");
    let mut enc = device.create_command_encoder(&Default::default());
    {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &colour_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        tiled.encode(
            &mut pass,
            &vertex_buffer,
            &index_buffer,
            [0, 0, SIZE.0, SIZE.1],
        );
    }
    queue.submit([enc.finish()]);
    let got = read();

    let texel = |v: &[f32], x: u32, y: u32| v[(4 * (y * SIZE.0 + x)) as usize..][..4].to_vec();
    let (l, r) = (texel(&expected, 4, 4), texel(&expected, 28, 4));
    assert!(
        (l[0] - (0.1 + 0.5 + 0.4 * 0.25 + 0.05)).abs() < 2e-3,
        "the left half reads S's marks: {l:?}"
    );
    assert!(
        (r[0] - (0.1 + 0.5 + 0.4 + 0.05)).abs() < 2e-3,
        "the right half the alpha before them: {r:?}"
    );
    for (i, (e, g)) in expected.iter().zip(&got).enumerate() {
        assert!(
            (e - g).abs() <= 2e-3 * e.abs().max(1.0),
            "texel {} channel {}: per light {e}, tiled {g}",
            i / 4,
            i % 4
        );
    }
}
