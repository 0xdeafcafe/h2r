#![cfg(feature = "gpu")]

use std::sync::{Arc, Mutex};

use d3d::backend::{
    ExternalPassRequest, ExternalSlot, ExternalSurfaces, PixelRect, StreamBytes, TargetInfo,
    TextureInfo, Viewport,
};
use d3d::shader::{CONSTANTS, ShaderInputs, VertexProcessing};
use d3d::state::StateWords;
use d3d::{Backend, DrawOutcome, ExternalPass, ExternalPlan, GfxBackend, ResolvedDraw, TargetCopy};
use gfx::state::{CompareFunc, CullMode, DepthBufferMode, StencilOp};
use gfx::surface::SurfaceId;
use gfx::texture::TextureKind;
use gfx::vertex::{
    PrimitiveType, StreamLayout, VertexAttribute, VertexDataType, VertexLayout, VertexRegister,
};
use gfx::xdk::RenderStateTable;
use gfx::{Clear, D3DFormat, GfxContext};
use native_render::world_layers::Texaccum;
use native_render::world_layers_gpu::{Cache, IndexedPass};
use shaders::psh::{PixelShaderDef, bits::*};
use shaders::vsh::encode::{Inst, Operand, program};
use shaders::vsh::{MacOp, OutReg, Program};

const SIZE: u32 = 32;
const FORMAT: D3DFormat = D3DFormat::LinA8R8G8B8;
const DEPTH: D3DFormat = D3DFormat::LinD24S8;
const ACCUM: Texaccum = Texaccum {
    surface: Some(0x2000),
    texture: 0x1000,
    size: (SIZE, SIZE),
};

fn triangle(x: f32, color: [f32; 4], reference: u8, sample: bool) -> ResolvedDraw {
    let code = program(&[
        Inst::mac(MacOp::Mov, Some(Operand::v(0)), None, None).to_out(false, OutReg::Pos, 0xf),
        Inst::mac(MacOp::Mov, Some(Operand::v(3)), None, None).to_out(false, OutReg::D0, 0xf),
        Inst::mac(MacOp::Mov, Some(Operand::v(9)), None, None)
            .to_out(false, OutReg::T(0), 0xf)
            .last(),
    ]);
    let microcode: Vec<u32> = Program::parse(&code)
        .unwrap()
        .instructions
        .iter()
        .flat_map(|i| i.words)
        .collect();
    let source = if sample { REG_T0 } else { REG_V0 };
    let def = PixelShaderDef {
        texture_modes: if sample {
            texture_modes([TEX_PROJECT2D, 0, 0, 0])
        } else {
            0
        },
        final_inputs_abcd: inputs(0, 0, 0, source),
        final_inputs_efg: final_efg(0, 0, source | CHAN_ALPHA, 0),
        ..Default::default()
    };
    let ps = def.to_bytes();
    let ps: [u32; 60] =
        std::array::from_fn(|n| u32::from_le_bytes(ps[n * 4..n * 4 + 4].try_into().unwrap()));
    let mut words = StateWords::defaults(false);
    words.render[..57].copy_from_slice(&ps[..57]);
    words.render[136] = ps[54];
    let mut state = words.translate(&RenderStateTable::default());
    let render = &mut state.render;
    render.cull_mode = CullMode::None;
    render.depth_mode = DepthBufferMode::W;
    render.depth_write = !sample;
    render.depth_func = if sample {
        CompareFunc::Equal
    } else {
        CompareFunc::Less
    };
    render.stencil.enable = true;
    render.stencil.reference = reference;
    render.stencil.func = if sample {
        CompareFunc::Equal
    } else {
        CompareFunc::Always
    };
    render.stencil.pass = if sample {
        StencilOp::Keep
    } else {
        StencilOp::Replace
    };

    let stride = 48;
    let mut bytes = Vec::new();
    for (x, y) in [(x, 2.0), (x + 12.0, 2.0), (x, 14.0)] {
        let values = [
            x, y, 0.0, 4096.0, color[0], color[1], color[2], color[3], x, y, 0.0, 1.0,
        ];
        bytes.extend_from_slice(bytemuck::cast_slice(&values));
    }
    let mut info = [None; 4];
    let mut headers = [None; 4];
    if sample {
        info[0] = Some(TextureInfo {
            header: ACCUM.texture,
            palette: 0,
            format: FORMAT,
            kind: TextureKind::D2,
            width: SIZE,
            height: SIZE,
            depth: 1,
            levels: 1,
        });
        headers[0] = Some(ACCUM.texture);
    }
    ResolvedDraw {
        entry: "D3DDevice_DrawIndexedVertices",
        primitive: PrimitiveType::TriangleList,
        vertex_count: 3,
        layout: VertexLayout {
            streams: vec![StreamLayout {
                stream: 0,
                stride,
                attributes: [0, 3, 9]
                    .into_iter()
                    .enumerate()
                    .map(|(n, r)| VertexAttribute {
                        register: VertexRegister(r),
                        data_type: VertexDataType::Float4,
                        offset: n as u32 * 16,
                    })
                    .collect(),
            }],
            pretransformed: false,
        },
        streams: vec![StreamBytes {
            stride,
            data: bytes.into(),
        }],
        indices: Some(vec![0, 1, 2].into()),
        state,
        extras: words.stage_extras(),
        raw: Arc::new(words),
        target: TargetInfo {
            width: SIZE,
            height: SIZE,
            format: FORMAT,
            depth_format: Some(DEPTH),
        },
        texture_info: info,
        shader: ShaderInputs {
            vertex: VertexProcessing::Program {
                start: 0,
                microcode: microcode.into(),
            },
            provided_inputs: (1 << 0) | (1 << 3) | (1 << 9),
            vertex_defaults: [[0.0, 0.0, 0.0, 1.0]; 16],
            constants: Arc::new([[0.0; 4]; CONSTANTS]),
            pixel_shader: Some(Arc::new(ps)),
            combiner_state: Arc::new(ps[..57].try_into().unwrap()),
            final_combiner_latch: true,
            affine_projection: false,
        },
        viewport: Viewport {
            x: 0,
            y: 0,
            width: SIZE,
            height: SIZE,
            min_z: 0.0,
            max_z: 1.0,
        },
        clip: Some(PixelRect {
            x: 0,
            y: 0,
            width: SIZE,
            height: SIZE,
        }),
        clip_rects: Vec::new(),
        hw: Default::default(),
        textures: headers,
        glyph_cache: 0,
    }
}

fn record(
    backend: &mut GfxBackend,
    surfaces: ExternalSurfaces<SurfaceId>,
    draw: &ResolvedDraw,
    textures: [Option<&d3d::gfx_backend::HostTexture>; 4],
    cache: &Arc<Cache>,
) -> bool {
    let sample = !surfaces.sampled.is_empty();
    let pass: Box<dyn ExternalPass> =
        Box::new(IndexedPass::new(draw, Arc::clone(cache), 1, sample.then_some(ACCUM)).unwrap());
    let plan = ExternalPlan {
        stage: timeline::native::Stage::World,
        view: 0,
        ranges: 1,
        slots: ACCUM
            .surface
            .filter(|_| sample)
            .map(|header| {
                Some(ExternalSlot {
                    header,
                    written: false,
                })
            })
            .into_iter()
            .collect(),
    };
    let request = ExternalPassRequest {
        rect: (0, 0, SIZE, SIZE),
        surface_size: (SIZE, SIZE),
        z_scale: 16_777_215.0,
    };
    let Some(recordings) = backend
        .prepare_external(
            surfaces,
            request,
            textures,
            &plan,
            TargetCopy::Reference,
            Arc::new(Mutex::new(pass)),
        )
        .unwrap()
    else {
        return false;
    };
    assert_eq!(recordings.len(), 1);
    for recording in recordings {
        backend.commit_external(recording);
    }
    true
}

fn scene(backend: &mut GfxBackend, mixed: bool) -> Vec<u8> {
    let color = backend.create_surface(FORMAT, SIZE, SIZE, false).unwrap();
    let depth = backend.create_surface(DEPTH, SIZE, SIZE, true).unwrap();
    let accum = backend.create_texture_target(FORMAT, SIZE, SIZE).unwrap();
    let cache = Arc::new(Cache::default());
    let surfaces = ExternalSurfaces {
        color: accum,
        depth,
        sampled: vec![],
        slots: vec![],
        slot_sizes: vec![],
    };
    backend
        .clear(accum, Some(depth), Clear::all([0.0, 0.0, 0.0, 1.0]))
        .unwrap();
    let red = triangle(2.0, [1.0, 0.0, 0.0, 1.0], 0xff, false);
    let green = triangle(18.0, [0.0, 1.0, 0.0, 1.0], 4, false);
    if mixed {
        assert!(record(backend, surfaces, &red, [None; 4], &cache));
    } else {
        assert_eq!(
            backend.draw(accum, Some(depth), &red, [None; 4]),
            Ok(DrawOutcome::Drawn)
        );
    }
    assert_eq!(
        backend.draw(accum, Some(depth), &green, [None; 4]),
        Ok(DrawOutcome::Drawn)
    );

    let texture = backend.surface_texture(accum).unwrap().unwrap();
    let sampled = [Some(&texture), None, None, None];
    backend
        .clear(
            color,
            Some(depth),
            Clear {
                color: Some([0.0, 0.0, 1.0, 1.0]),
                depth: None,
                stencil: None,
            },
        )
        .unwrap();
    let red = triangle(2.0, [1.0; 4], 0xff, true);
    let green = triangle(18.0, [1.0; 4], 4, true);
    assert_eq!(
        backend.draw(color, Some(depth), &red, sampled),
        Ok(DrawOutcome::Drawn)
    );
    if mixed {
        let decoy = backend.create_texture_target(FORMAT, SIZE, SIZE).unwrap();
        let mut surfaces = ExternalSurfaces {
            color,
            depth,
            sampled: vec![accum],
            slots: vec![Some(decoy)],
            slot_sizes: vec![Some((SIZE, SIZE))],
        };
        assert!(
            !record(backend, surfaces.clone(), &green, sampled, &cache),
            "same dimensions cannot substitute a texaccum target"
        );
        surfaces.slots[0] = Some(accum);
        assert!(record(backend, surfaces, &green, sampled, &cache));
    } else {
        assert_eq!(
            backend.draw(color, Some(depth), &green, sampled),
            Ok(DrawOutcome::Drawn)
        );
    }
    // Each test alone would overwrite the right triangle if its guard
    // were lost. The wrong-stencil draw has the right W; the wrong-depth
    // draw has the right stencil and fails its comparison against equal W.
    let mut wrong = triangle(18.0, [1.0, 0.0, 1.0, 1.0], 0xff, false);
    wrong.state.render.depth_write = false;
    wrong.state.render.depth_func = CompareFunc::Equal;
    wrong.state.render.stencil.func = CompareFunc::Equal;
    wrong.state.render.stencil.pass = StencilOp::Keep;
    for (reference, depth_func) in [(0xff, CompareFunc::Equal), (4, CompareFunc::Greater)] {
        wrong.state.render.stencil.reference = reference;
        wrong.state.render.depth_func = depth_func;
        if mixed {
            let surfaces = ExternalSurfaces {
                color,
                depth,
                sampled: vec![],
                slots: vec![],
                slot_sizes: vec![],
            };
            assert!(record(backend, surfaces, &wrong, [None; 4], &cache));
        } else {
            assert_eq!(
                backend.draw(color, Some(depth), &wrong, [None; 4]),
                Ok(DrawOutcome::Drawn)
            );
        }
    }
    backend.read_color(color).unwrap().0
}

#[test]
fn world_layers_gpu_mixed_w_depth_stencil_and_texaccum_match_d3d() {
    let Ok(ctx) = GfxContext::headless() else {
        eprintln!("skipping opaque layer test without a GPU adapter");
        return;
    };
    let mut backend = GfxBackend::new(ctx);
    let baseline = scene(&mut backend, false);
    let pixel = |x: usize, y: usize| -> &[u8] {
        let i = 4 * (y * SIZE as usize + x);
        &baseline[i..i + 4]
    };
    assert_ne!(
        pixel(4, 4),
        pixel(0, 0),
        "layer 3 saw the left depth and stencil"
    );
    assert_ne!(
        pixel(20, 4),
        pixel(0, 0),
        "layer 3 saw the right depth and stencil"
    );
    assert_ne!(
        pixel(4, 4),
        pixel(20, 4),
        "layer 3 sampled two distinct layer-1 colors"
    );
    assert_eq!(scene(&mut backend, true), baseline);
}

#[test]
fn queued_world_draws_choose_native_or_d3d_in_order_without_waiting() {
    let Ok(ctx) = GfxContext::headless() else {
        return;
    };
    let mut backend = d3d::ThreadedBackend::new(GfxBackend::new(ctx)).unwrap();
    let color = backend.create_surface(FORMAT, SIZE, SIZE, false).unwrap();
    let depth = backend.create_surface(DEPTH, SIZE, SIZE, true).unwrap();
    let cache = Arc::new(Cache::default());
    let outcomes = Arc::new(Mutex::new(Vec::new()));
    let mut render = |queued| {
        backend
            .clear(color, Some(depth), Clear::all([0.0, 0.0, 0.0, 1.0]))
            .unwrap();
        let waits = backend.waits().reads;
        // The second draw overwrites the first and deliberately refuses native
        // recording. The third writes separate pixels after that fallback.
        for (i, (x, rgba)) in [
            (2.0, [1.0, 0.0, 0.0, 1.0]),
            (2.0, [0.0, 1.0, 0.0, 1.0]),
            (18.0, [0.0, 0.0, 1.0, 1.0]),
        ]
        .into_iter()
        .enumerate()
        {
            let mut draw = triangle(x, rgba, 1, false);
            draw.state.render.depth_func = CompareFunc::Always;
            if queued {
                let pass = IndexedPass::new(&draw, cache.clone(), 1, None).unwrap();
                let results = outcomes.clone();
                let candidate = d3d::backend::QueuedExternalDraw {
                    surfaces: ExternalSurfaces {
                        color,
                        depth,
                        sampled: vec![],
                        slots: vec![],
                        slot_sizes: vec![],
                    },
                    request: ExternalPassRequest {
                        rect: (0, 0, SIZE, SIZE),
                        surface_size: (SIZE, SIZE),
                        z_scale: 16_777_215.0,
                    },
                    plan: ExternalPlan {
                        stage: timeline::native::Stage::World,
                        view: 0,
                        ranges: if i == 1 { 2 } else { 1 },
                        slots: vec![],
                    },
                    fallback: pass.resolved_fallback().unwrap(),
                    fallback_depth: Some(depth),
                    pass: Arc::new(Mutex::new(Box::new(pass))),
                    finished: Box::new(move |fallback| {
                        results.lock().unwrap().push((i, fallback.cloned()));
                    }),
                };
                assert!(backend.queue_external_draw(candidate, [None; 4]));
            } else {
                assert_eq!(
                    backend.draw(color, Some(depth), &draw, [None; 4]),
                    Ok(DrawOutcome::Drawn)
                );
            }
        }
        assert_eq!(backend.waits().reads, waits);
        backend
            .with_surface(color, |b, s| b.read_color(s).map(|image| image.0))
            .unwrap()
    };
    let baseline = render(false);
    assert!(baseline.chunks_exact(4).any(|p| p[1] != 0));
    assert_eq!(render(true), baseline);
    assert_eq!(
        *outcomes.lock().unwrap(),
        [(0, None), (1, Some(Ok(DrawOutcome::Drawn))), (2, None)]
    );
}
