#![cfg(feature = "gpu")]

use std::sync::{Arc, Mutex};

use d3d::backend::{
    ExternalPassRequest, ExternalSlot, ExternalSurfaces, PixelRect, StreamBytes, TargetInfo,
    TextureInfo, Viewport,
};
use d3d::shader::{CONSTANTS, ShaderInputs, VertexProcessing};
use d3d::state::StateWords;
use d3d::{Backend, DrawOutcome, ExternalPass, ExternalPlan, GfxBackend, ResolvedDraw, TargetCopy};
use gfx::encode::HostPixelOrder;
use gfx::state::{CompareFunc, CullMode, DepthBufferMode, StencilOp};
use gfx::surface::SurfaceId;
use gfx::texture::TextureKind;
use gfx::vertex::{
    PrimitiveType, StreamLayout, VertexAttribute, VertexDataType, VertexLayout, VertexRegister,
};
use gfx::xdk::RenderStateTable;
use gfx::{Clear, D3DFormat, GfxContext};
use native_render::world_layers::Texaccum;
use native_render::world_layers_gpu::{Cache, IndexedPass, pool_lens_stage, pool_recording_counters, pool_refusal_counter};
use shaders::psh::{PixelShaderDef, bits::*};
use shaders::vsh::encode::{Inst, Operand, program};
use shaders::vsh::{MacOp, OutReg, Program};

// The frame counter capture is process-wide, including threaded backends.
static CAPTURE_LOCK: Mutex<()> = Mutex::new(());

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
        retail_constants: None,
        ui_fit: None,
        glyph_cache: 0,
    }
}

/// Present synthetic frames until the worker-built variant becomes admissible.
fn after_compile(mut attempt: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if attempt() { return true; }
        if std::time::Instant::now() >= deadline { return false; }
        std::thread::sleep(std::time::Duration::from_millis(16));
        timeline::frame();
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
    after_compile(|| {
        let pass = IndexedPass::new(draw, Arc::clone(cache), 1, sample.then_some(ACCUM)).unwrap();
        record_pass(backend, surfaces.clone(), textures, timeline::native::Stage::World, Box::new(pass))
    })
}

fn record_pass(
    backend: &mut GfxBackend, surfaces: ExternalSurfaces<SurfaceId>,
    textures: [Option<&d3d::gfx_backend::HostTexture>; 4],
    stage: timeline::native::Stage, pass: Box<dyn ExternalPass>,
) -> bool {
    record_copy(backend, surfaces, textures, stage, TargetCopy::Reference, Arc::new(Mutex::new(pass)))
}

fn record_copy(
    backend: &mut GfxBackend, surfaces: ExternalSurfaces<SurfaceId>,
    textures: [Option<&d3d::gfx_backend::HostTexture>; 4],
    stage: timeline::native::Stage, copy: TargetCopy, pass: d3d::backend::SharedReplacement,
) -> bool {
    let sample = !surfaces.sampled.is_empty();
    let plan = ExternalPlan {
        stage,
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
        display_projection_factor: 1.0, z_scale: 16_777_215.0,
    };
    let Some(recordings) = backend
        .prepare_external(
            surfaces,
            request,
            textures,
            &plan,
            copy,
            pass,
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
            slots: vec![Some(accum)],
            slot_sizes: vec![Some((SIZE, SIZE))],
        };
        assert!(record(backend, surfaces.clone(), &green, sampled, &cache));
        surfaces.slots[0] = Some(decoy);
        let pass = IndexedPass::new(&green, Arc::clone(&cache), 1, Some(ACCUM)).unwrap();
        assert!(
            !record_pass(backend, surfaces, sampled, timeline::native::Stage::World, Box::new(pass)),
            "same dimensions cannot substitute a texaccum target, even with a ready pipeline"
        );
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
fn resolved_light_window_samples_its_pinned_gel_on_the_indexed_draw() {
    let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(ctx) = GfxContext::headless() else { return };
    let mut backend = GfxBackend::new(ctx);
    let color = backend.create_surface(FORMAT, SIZE, SIZE, false).unwrap();
    let depth = backend.create_surface(DEPTH, SIZE, SIZE, true).unwrap();
    let gel = backend.create_texture_target(FORMAT, SIZE, SIZE).unwrap();
    backend.clear(gel, None, Clear {
        color: Some([1.0, 0.0, 0.0, 1.0]),
        ..Clear::default()
    }).unwrap();
    let texture = backend.surface_texture(gel).unwrap().unwrap();
    let sampled = [Some(&texture), None, None, None];
    let mut draw = triangle(2.0, [1.0; 4], 0, true);
    draw.state.render.depth_func = CompareFunc::Always;
    draw.state.render.stencil.enable = false;
    // The handler emitted only the triangle in the second half of the stream.
    let mut vertices = triangle(18.0, [1.0; 4], 0, true).streams[0].data.to_vec();
    vertices.extend_from_slice(&draw.streams[0].data);
    draw.streams[0].data = vertices.into();
    draw.vertex_count = 6;
    draw.indices = Some(vec![3, 4, 5].into());
    let clear = Clear::all([0.0, 0.0, 0.0, 1.0]);
    backend.clear(color, Some(depth), clear).unwrap();
    assert_eq!(backend.draw(color, Some(depth), &draw, sampled), Ok(DrawOutcome::Drawn));
    let expected = backend.read_color(color).unwrap().0;
    let pixel = |x, y| 4 * (y * SIZE + x) as usize;
    assert_ne!(expected[pixel(4, 4)..][..4], expected[pixel(20, 4)..][..4]);
    backend.clear(color, Some(depth), clear).unwrap();
    let cache = Arc::new(Cache::default());
    let mut surfaces = ExternalSurfaces {
        color, depth, sampled: vec![gel], slots: vec![Some(gel)], slot_sizes: vec![Some((SIZE, SIZE))],
    };
    let make_pass = || {
        let mut pass = IndexedPass::new(&draw, Arc::clone(&cache), 1, None).unwrap();
        pass.pin_sampled_surface(ACCUM.texture, (SIZE, SIZE));
        Box::new(pass)
    };
    let stage = timeline::native::Stage::Lights;
    assert!(after_compile(|| record_pass(&mut backend, surfaces.clone(), sampled, stage, make_pass())));
    assert_eq!(backend.read_color(color).unwrap().0, expected);
    surfaces.slots[0] = Some(backend.create_texture_target(FORMAT, SIZE, SIZE).unwrap());
    assert!(!record_pass(&mut backend, surfaces, sampled, stage, make_pass()));
}

#[test]
fn world_layers_gpu_mixed_w_depth_stencil_and_texaccum_match_d3d() {
    let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
    let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
                        display_projection_factor: 1.0, z_scale: 16_777_215.0,
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
    assert!(baseline.as_chunks::<4>().0.iter().any(|p| p[1] != 0));
    assert_eq!(render(true), baseline);
    assert_eq!(
        *outcomes.lock().unwrap(),
        [(0, Some(Ok(DrawOutcome::Drawn))), (1, Some(Ok(DrawOutcome::Drawn))), (2, Some(Ok(DrawOutcome::Drawn)))],
        "a cold pipeline leaves all three draws on D3D through this frame"
    );
    assert!(after_compile(|| {
        outcomes.lock().unwrap().clear();
        assert_eq!(render(true), baseline);
        *outcomes.lock().unwrap() == [(0, None), (1, Some(Ok(DrawOutcome::Drawn))), (2, None)]
    }));
}

#[test]
fn uncached_world_geometry_shares_one_upload_and_one_pass() {
    let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(ctx) = GfxContext::headless() else {
        return;
    };
    let mut backend = GfxBackend::new(ctx);
    backend.set_merge_passes(true);
    backend.set_merge_masks(true);
    let color = backend.create_surface(FORMAT, SIZE, SIZE, false).unwrap();
    let depth = backend.create_surface(DEPTH, SIZE, SIZE, true).unwrap();
    let surfaces = ExternalSurfaces {
        color,
        depth,
        sampled: vec![],
        slots: vec![],
        slot_sizes: vec![],
    };
    let cache = Arc::new(Cache::default());
    // Churn is scoped to this thread, and pass counts to this backend.
    let before = gfx::churn::snapshot();
    let passes = backend.batch_stats().render_passes;
    backend.clear(color, Some(depth), Clear::all([0.0; 4])).unwrap();
    for i in 0..8 {
        let x = if i % 2 == 0 { 2.0 } else { 18.0 };
        let mut draw = triangle(x, [1.0, f32::from(i as u8 % 2), 0.0, 1.0], 1, false);
        draw.state.render.depth_func = CompareFunc::Always;
        assert!(draw.streams.iter().all(|s| s.data.cache_id().is_none()));
        assert!(draw.indices.as_ref().unwrap().cache_id().is_none());
        if i == 7 {
            // One guest range can have multiple clip pieces and still counts
            // as one complete reservation with the same staged buffers.
            draw.clip_rects = vec![
                PixelRect { x: 16, y: 0, width: 8, height: SIZE },
                PixelRect { x: 24, y: 0, width: 8, height: SIZE },
            ];
        }
        assert!(record(&mut backend, surfaces.clone(), &draw, [None; 4], &cache));
    }
    backend.flush().unwrap();
    let churn = gfx::churn::snapshot() - before;
    assert_eq!(churn.buffers_created, 1);
    assert_eq!(churn.queue_writes, 1);
    assert_eq!(backend.batch_stats().render_passes - passes, 1);
    let (pixels, order) = backend.read_color(color).unwrap();
    let pixel = |x: usize| {
        let start = 4 * (4 * SIZE as usize + x);
        let [c0, g, c2, a]: [u8; 4] = pixels[start..start + 4].try_into().unwrap();
        match order {
            HostPixelOrder::Bgra8 => [c2, g, c0, a],
            HostPixelOrder::Rgba8 => [c0, g, c2, a],
        }
    };
    assert_eq!(pixel(4), [255, 0, 0, 255]);
    assert_eq!(pixel(20), [255, 255, 0, 255]);
}

#[test]
fn resident_section_reuses_geometry_with_copy_specific_uniforms_and_scissors() {
    use native_render::world_layers::{Geometry, Owner};
    use timeline::native::Stage;

    let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(ctx) = GfxContext::headless() else { return };
    let mut backend = GfxBackend::new(ctx);
    let color = backend.create_surface(FORMAT, SIZE, SIZE, false).unwrap();
    let depth = backend.create_surface(DEPTH, SIZE, SIZE, true).unwrap();
    let surfaces = ExternalSurfaces { color, depth, sampled: vec![], slots: vec![], slot_sizes: vec![] };
    let cache = Arc::new(Cache::default());
    let geometry = Geometry { owner: Owner::Cluster(3), record: 0, sorted_slot: 1,
        part: 1, shader: 9, variant: 0, implementation: 1 };
    let mut draw = triangle(2.0, [1.; 4], 1, false);
    let words = program(&[
        Inst::mac(MacOp::Mov, Some(Operand::v(0)), None, None).to_out(false, OutReg::Pos, 0xf),
        Inst::mac(MacOp::Mov, Some(Operand::c(12)), None, None).to_out(false, OutReg::D0, 0xf).last(),
    ]);
    draw.shader.vertex = VertexProcessing::Program {
        start: 0, microcode: Program::parse(&words).unwrap().instructions.iter()
            .flat_map(|i| i.words).collect::<Vec<_>>().into(),
    };
    // Probe the two constant blocks through colour, independently of position.
    Arc::make_mut(&mut draw.shader.constants)[12] = [0., 1., 0., 1.];
    draw.retail_constants = Some((draw.shader.constants.clone(), 0.5));
    Arc::make_mut(&mut draw.shader.constants)[12] = [1.; 4];
    draw.clip = Some(PixelRect { x: 2, y: 0, width: 6, height: SIZE });
    draw.state.render.depth_func = CompareFunc::Always;
    // Both copies exercise the same resident pass after its pipeline is visible.
    assert!(record(&mut backend, surfaces.clone(), &draw, [None; 4], &cache));
    let pass = IndexedPass::for_section(&draw, cache, 1, None, (1, 4), geometry, 3).unwrap();
    let pass: d3d::backend::SharedReplacement = Arc::new(Mutex::new(Box::new(pass)));
    for copy in [TargetCopy::Reference, TargetCopy::Display, TargetCopy::Reference] {
        backend.clear(color, Some(depth), Clear::all([0.; 4])).unwrap();
        assert!(record_copy(&mut backend, surfaces.clone(), [None; 4], Stage::World, copy, pass.clone()));
        let pixels = backend.read_color(color).unwrap().0;
        let pixel = |x: usize| &pixels[4 * (3 * SIZE as usize + x)..][..4];
        if copy == TargetCopy::Reference {
            assert_eq!(pixel(5), &[0, 255, 0, 255]);
            assert_eq!(pixel(10), &[0; 4]);
        } else {
            assert_eq!(pixel(5), &[0; 4]);
            assert_eq!(pixel(10), &[255; 4]);
        }
    }
}

#[test]
fn resident_sections_upload_only_changed_constants_across_swaps() {
    let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    use native_render::world_layers::{Geometry, Owner};
    use timeline::{native::Stage, phase};
    let Ok(ctx) = GfxContext::headless() else { return };
    let mut backend = GfxBackend::new(ctx);
    let color = backend.create_surface(FORMAT, SIZE, SIZE, false).unwrap();
    let depth = backend.create_surface(DEPTH, SIZE, SIZE, true).unwrap();
    let first = backend.create_texture_target(FORMAT, SIZE, SIZE).unwrap();
    let next = backend.create_texture_target(FORMAT, SIZE, SIZE).unwrap();
    let lightmaps = [first, next].map(|surface| backend.surface_texture(surface).unwrap().unwrap());
    let surfaces = ExternalSurfaces { color, depth, sampled: vec![], slots: vec![], slot_sizes: vec![] };
    let cache = Arc::new(Cache::default());
    // Warm the two shader/PSO variants on transient geometry. The capture
    // begins with no resident section and includes only its six swaps.
    for sampled in [false, true] {
        let draw = triangle(2.0, [1.0; 4], 1, sampled);
        let texture = sampled.then_some(&lightmaps[0]);
        assert!(record(&mut backend, surfaces.clone(), &draw, [texture, None, None, None], &cache));
    }
    backend.flush().unwrap();
    phase::start(phase::Config::live(u64::MAX)).unwrap();
    phase::frame_end(0);
    assert!(phase::capture_begin());

    for swap in 1..=6 {
        for (stage, owner) in [
            (Stage::World, Owner::Cluster(3)),
            (Stage::Objects, Owner::Object { handle: swap, model: 8, section: 3 }),
        ] {
            let geometry = Geometry { owner, record: swap as u16, sorted_slot: 50 + swap,
                part: 1, shader: 9, variant: 0, implementation: 1 };
            // Owned snapshots have no stable D3D cache ID.
            let world = stage == Stage::World;
            let mut draw = triangle(if swap >= 4 { 8.0 } else { 2.0 }, [1.0; 4], 1, world);
            if swap >= 3 { Arc::make_mut(&mut draw.shader.constants)[11] = [if world { 0.25 } else { 0.75 }; 4]; }
            let generation = if swap >= 5 { 2 } else { 1 };
            let pass = IndexedPass::for_section(&draw, cache.clone(), u64::from(swap), None,
                (generation, 4), geometry, 3).unwrap();
            let texture = world.then_some(&lightmaps[usize::from(swap == 6)]);
            assert!(record_pass(&mut backend, surfaces.clone(), [texture, None, None, None], stage, Box::new(pass)));
        }
        backend.flush().unwrap();
        backend.ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        phase::frame_end(u64::from(swap));
    }
    let frames = phase::capture_end().unwrap();
    phase::stop();
    assert_eq!(frames.len(), 6);
    let count = |frame: usize, label: &str| {
        let (i, _) = phase::counter_columns().find(|(_, name)| *name == label).unwrap();
        frames[frame].counts[i]
    };
    for stage in ["world", "objects"] {
        let metric = |f, name| count(f, &format!("native_{stage}_{name}"));
        assert_eq!(metric(0, "cache_misses"), 1);
        for frame in [1, 2] {
            assert_eq!(metric(frame, "cache_hits"), 1);
            assert_eq!(metric(frame, "cache_misses"), 0);
            assert_eq!(metric(frame, "buffers"), 0);
        }
        assert_eq!(metric(1, "upload_bytes"), 0);
        assert_eq!(metric(2, "upload_bytes"), 16);
        // Same-address mutation, then a BSP reload at the same address.
        for frame in [3, 4] {
            assert_eq!(metric(frame, "cache_misses"), 1);
            assert!(metric(frame, "upload_bytes") >= 3 * 48 + 3 * 4);
        }
    }
    // A new lightmap behind the same guest header rebinds the world material.
    assert_eq!(count(5, "native_world_cache_misses"), 1);
    assert_eq!(count(5, "native_world_upload_bytes"), 0);
    assert_eq!(count(5, "native_objects_cache_hits"), 1);
    assert_eq!(count(1, "queue_writes"), 0);
    assert_eq!(count(2, "queue_writes"), 1);
}

#[path = "world_layers_gpu/frame_graph.rs"]
mod frame_graph;

#[path = "world_layers_gpu/sun_tiles.rs"]
mod sun_tiles;

#[path = "world_layers_gpu/flares.rs"]
mod flares;

#[path = "world_layers_gpu/cone_shadows.rs"]
mod cone_shadows;

#[test]
fn sorted_pool_translates_the_resolved_ripple_and_scene_stages() {
    let mut draw = triangle(2.0, [1.0; 4], 0, true);
    let def = PixelShaderDef {
        texture_modes: texture_modes([TEX_PROJECT2D, TEX_BUMPENVMAP, TEX_BUMPENVMAP, TEX_BUMPENVMAP]),
        final_inputs_abcd: inputs(REG_T3, REG_T1, REG_T2, 0),
        final_inputs_efg: final_efg(0, 0, REG_T0 | CHAN_ALPHA, 0),
        ..Default::default()
    };
    let bytes = def.to_bytes();
    let words: [u32; 60] = std::array::from_fn(|i| u32::from_le_bytes(bytes[i*4..i*4+4].try_into().unwrap()));
    draw.shader.pixel_shader = Some(Arc::new(words));
    draw.shader.combiner_state = Arc::new(words[..57].try_into().unwrap());
    let raw = Arc::make_mut(&mut draw.raw);
    raw.render[..57].copy_from_slice(&words[..57]);
    raw.render[136] = words[54];
    draw.texture_info = [draw.texture_info[0]; 4];
    draw.textures = [draw.textures[0]; 4];
    draw.extras[2].bump_env_mat = [0.25, -0.1, 0.5, 0.2];
    // Delta Towers' tower_pool: t0 is the signed ripple bitmap, sampled
    // projectively; t2 reads the refraction target through it. Only the
    // frame sample is the lens: the ripple must keep its own image.
    assert_eq!(pool_lens_stage(&draw), None, "a projective ripple bitmap is not the lens");
    assert_eq!(IndexedPass::for_pool(&draw, Arc::new(Cache::default()), 1).err(), Some("no_lens"));
    draw.extras[2].samples_frame = true;
    let pass = IndexedPass::for_pool(&draw, Arc::new(Cache::default()), 1).unwrap();
    assert_eq!(pool_lens_stage(&draw), Some(2));
    assert!(pass.replays_scene());
    assert_eq!(pass.resolved_fallback().unwrap().as_ref(), &draw);
    let program = d3d::nv2a::NativeDrawMaterial::resolve(&draw, [true; 4],
        gfx::translate::Capabilities::ALL, gfx::repack::FetchMode::Cpu).unwrap();
    assert_eq!(program.key.translate().unwrap().dims, [Some(wgpu::TextureViewDimension::D2); 4]);
    assert_eq!(program.uniforms(&draw, 1u32).1.bump_mat[2], draw.extras[2].bump_env_mat);
}

#[test]
fn sky_strip_draws_the_current_views_palette_and_its_own_stencil() {
    use native_render::object_state::{structure_stencil, STENCIL_MARK_REFERENCE};
    use native_render::program::MODEL_ROWS;
    use native_render::sky_material::SkyConstants;
    use scene_extract::layout::sky::SkyItem;

    let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(ctx) = GfxContext::headless() else { return };
    let mut backend = GfxBackend::new(ctx);
    let color = backend.create_surface(FORMAT, SIZE, SIZE, false).unwrap();
    let depth = backend.create_surface(DEPTH, SIZE, SIZE, true).unwrap();
    let surfaces = ExternalSurfaces { color, depth, sampled: vec![], slots: vec![], slot_sizes: vec![] };
    let cache = Arc::new(Cache::default());
    let mut draw = triangle(2.0, [0.0; 4], STENCIL_MARK_REFERENCE, false);
    draw.primitive = PrimitiveType::TriangleStrip;
    draw.state.render.stencil = structure_stencil(STENCIL_MARK_REFERENCE);
    draw.state.render.depth_func = CompareFunc::Always;
    let words = program(&[
        Inst::mac(MacOp::Mov, Some(Operand::v(0)), None, None).to_out(false, OutReg::Pos, 0xf),
        Inst::mac(MacOp::Mov, Some(Operand::c(MODEL_ROWS as u8)), None, None)
            .to_out(false, OutReg::D0, 0xf).last(),
    ]);
    draw.shader.vertex = VertexProcessing::Program {
        start: 0, microcode: Program::parse(&words).unwrap().instructions.iter()
            .flat_map(|i| i.words).collect::<Vec<_>>().into(),
    };
    let pixel = 4 * (4 * SIZE as usize + 4);
    let mut template = None;
    for value in [[0.0, 1.0, 0.0, 1.0], [1.0; 4]] {
        let rows = vec![value; 3];
        Arc::make_mut(&mut draw.shader.constants)[MODEL_ROWS..MODEL_ROWS + 3].copy_from_slice(&rows);
        let item = SkyItem {
            model: 3, section: 1, part: 0, shader: 5, pass: 0,
            rows, decompression: [[0.0; 4]; 3],
        };
        backend.clear(color, Some(depth), Clear::all([0.0; 4])).unwrap();
        assert!(after_compile(|| {
            let pass = IndexedPass::for_sky(&draw, cache.clone(), 1, 9, item.clone(), SkyConstants::default(), None).unwrap();
            assert!(pass.replays_scene());
            template = pass.sky_template();
            record_pass(&mut backend, surfaces.clone(), [None; 4], timeline::native::Stage::Sky, Box::new(pass))
        }));
        let pixels = backend.read_color(color).unwrap().0;
        let expected = value.map(|c| (c * 255.0) as u8);
        assert_eq!(&pixels[pixel..pixel + 4], &expected);
        // A guest draw after the sky only reaches the pixels marked by it.
        let mut probe = triangle(2.0, [0.0, 0.0, 0.0, 1.0], STENCIL_MARK_REFERENCE, false);
        probe.state.render.depth_func = CompareFunc::Always;
        probe.state.render.stencil.func = CompareFunc::Equal;
        probe.state.render.stencil.pass = StencilOp::Keep;
        assert_eq!(backend.draw(color, Some(depth), &probe, [None; 4]), Ok(DrawOutcome::Drawn));
        let pixels = backend.read_color(color).unwrap().0;
        assert_eq!(&pixels[pixel..pixel + 4], &[0, 0, 0, 255]);
    }

    // The cut: the last admitted strip replayed with the guest's sky passed
    // over, the made rows over its resident `SectionKey::Sky` entry.
    use native_render::world_layers_gpu::SkyCut;
    let template = template.expect("an admitted strip is a template");
    let replay = |rows: Vec<[f32; 4]>| IndexedPass::sky_replay(&template, 1, rows, [[0.0; 4]; 3], SkyConstants::default());
    assert_eq!(replay(vec![[0.5; 4]; 2]).err(), Some("transform"));
    assert_eq!(replay(Vec::new()).err(), Some("transform"));
    assert!(SkyCut::new(Vec::new()).is_none());
    let item = SkyItem { model: 3, section: 1, part: 0, shader: 5, pass: 0, rows: vec![[1.0; 4]; 3], decompression: [[0.0; 4]; 3] };
    let admitted = IndexedPass::for_sky(&draw, cache.clone(), 1, 9, item, SkyConstants::default(), None).unwrap();
    assert!(SkyCut::new(vec![admitted]).is_none(), "only replays make a cut");
    backend.clear(color, Some(depth), Clear::all([0.0; 4])).unwrap();
    assert!(after_compile(|| {
        let cut = SkyCut::new(vec![replay(vec![[0.0, 1.0, 0.0, 1.0]; 3]).unwrap()]).unwrap();
        assert_eq!(cut.len(), 1);
        assert!(cut.replays_scene());
        record_pass(&mut backend, surfaces.clone(), [None; 4], timeline::native::Stage::Sky, Box::new(cut))
    }));
    let pixels = backend.read_color(color).unwrap().0;
    assert_eq!(&pixels[pixel..pixel + 4], &[0, 255, 0, 255], "the replay draws the made rows");

    // A strip whose state leaves the stencil off is drawn as well, with
    // its own stencil: it writes no mark, so the probe reaches nothing.
    let mut unmarked = draw.clone();
    unmarked.state.render.stencil.enable = false;
    let rows = vec![[1.0; 4]; 3];
    Arc::make_mut(&mut unmarked.shader.constants)[MODEL_ROWS..MODEL_ROWS + 3].copy_from_slice(&rows);
    let item = SkyItem { model: 3, section: 1, part: 0, shader: 5, pass: 0, rows, decompression: [[0.0; 4]; 3] };
    backend.clear(color, Some(depth), Clear::all([0.0; 4])).unwrap();
    let mut unmarked_template = None;
    assert!(after_compile(|| {
        // The test's one cache: a fresh one per attempt would request its
        // pipeline anew each time, never visible at the attempt's frame.
        let pass = IndexedPass::for_sky(&unmarked, cache.clone(), 1, 9, item.clone(), SkyConstants::default(), None)
            .expect("an unmarked strip is drawn with its own stencil");
        unmarked_template = pass.sky_template();
        record_pass(&mut backend, surfaces.clone(), [None; 4], timeline::native::Stage::Sky, Box::new(pass))
    }));
    let pixels = backend.read_color(color).unwrap().0;
    assert_eq!(&pixels[pixel..pixel + 4], &[255; 4]);
    let mut probe = triangle(2.0, [0.0, 0.0, 0.0, 1.0], STENCIL_MARK_REFERENCE, false);
    probe.state.render.depth_func = CompareFunc::Always;
    probe.state.render.stencil.func = CompareFunc::Equal;
    probe.state.render.stencil.pass = StencilOp::Keep;
    assert_eq!(backend.draw(color, Some(depth), &probe, [None; 4]), Ok(DrawOutcome::Drawn));
    let pixels = backend.read_color(color).unwrap().0;
    assert_eq!(&pixels[pixel..pixel + 4], &[255; 4], "the unmarked strip wrote no mark");
    let unmarked_template = unmarked_template.expect("an unmarked strip is a template");
    assert!(!unmarked_template.stencil().enable);
    assert_eq!(*template.stencil(), structure_stencil(STENCIL_MARK_REFERENCE));
}

/// A sky strip draws with its own cull mode, and each strip the stage
/// refuses names its cause (`scene_kept_sky_<cause>`), `constants` for a
/// bound register the plan's evaluated material does not hold. No GPU:
/// admission reads the resolved draw only.
#[test]
fn a_sky_strip_keeps_its_cull_and_each_refusal_is_named() {
    use native_render::object_state::{structure_stencil, STENCIL_MARK_REFERENCE};
    use native_render::program::MODEL_ROWS;
    use native_render::sky_material::SkyConstants;
    use scene_extract::layout::sky::SkyItem;
    use tags::material::{Origin, Register};

    let cache = Arc::new(Cache::default());
    let mut draw = triangle(2.0, [0.0; 4], STENCIL_MARK_REFERENCE, false);
    draw.primitive = PrimitiveType::TriangleStrip;
    draw.state.render.stencil = structure_stencil(STENCIL_MARK_REFERENCE);
    let rows = vec![[0.5; 4]; 3];
    Arc::make_mut(&mut draw.shader.constants)[MODEL_ROWS..MODEL_ROWS + 3].copy_from_slice(&rows);
    Arc::make_mut(&mut draw.shader.constants)[30] = [0.25; 4];
    Arc::make_mut(&mut draw.shader.combiner_state)[10] = 0x1122_3344;
    let item = SkyItem { model: 3, section: 1, part: 0, shader: 5, pass: 0, rows, decompression: [[0.0; 4]; 3] };
    let material = SkyConstants {
        vertex: vec![Register { index: 30, value: [0.25; 4], written: 0xf, origin: Origin::Instance }],
        pixel: vec![(0, 0, 0x1122_3344)],
    };
    let admit = |draw: &ResolvedDraw| IndexedPass::for_sky(draw, cache.clone(), 1, 9, item.clone(), material.clone(), None).err();
    assert_eq!(admit(&draw), None, "an unculled strip");
    for (cull, name) in [(CullMode::Cw, "cw"), (CullMode::Ccw, "ccw")] {
        let mut culled = draw.clone();
        culled.state.render.cull_mode = cull;
        assert_eq!(admit(&culled), None, "{name}: the pipeline takes the draw's cull");
    }
    // The stencil is the draw's own: off, or another test, is drawn; on a
    // target with no stencil it draws with the test off, as on D3D.
    let unmarked = { let mut d = draw.clone(); d.state.render.stencil.enable = false; d };
    assert_eq!(admit(&unmarked), None, "a strip with the stencil off");
    let mut tested = draw.clone();
    tested.state.render.stencil.func = CompareFunc::Equal;
    tested.state.render.stencil.pass = StencilOp::Keep;
    assert_eq!(admit(&tested), None, "a strip that tests the stencil");
    let unheld = { let mut d = draw.clone(); d.target.depth_format = Some(D3DFormat::LinD16); d };
    assert_eq!(admit(&unheld), None, "a mark on a target with no stencil");
    let mut cases: Vec<(&str, ResolvedDraw)> = Vec::new();
    cases.push(("topology", { let mut d = draw.clone(); d.primitive = PrimitiveType::TriangleList; d }));
    cases.push(("index_range", { let mut d = draw.clone(); d.indices = Some(vec![0, 1, 3].into()); d }));
    cases.push(("program", { let mut d = draw.clone(); d.shader.vertex = VertexProcessing::FixedFunction { fvf: 0x002 }; d }));
    cases.push(("transform", {
        let mut d = draw.clone();
        Arc::make_mut(&mut d.shader.constants)[MODEL_ROWS] = [9.0; 4];
        d
    }));
    cases.push(("constants", {
        let mut d = draw.clone();
        Arc::make_mut(&mut d.shader.constants)[30] = [0.5; 4];
        d
    }));
    cases.push(("constants", {
        let mut d = draw.clone();
        Arc::make_mut(&mut d.shader.combiner_state)[10] = 0x1122_3345;
        d
    }));
    cases.push(("decompression", {
        let mut d = draw.clone();
        Arc::make_mut(&mut d.shader.constants)[native_render::program::POSITION_DECOMPRESSION] = [9.0; 4];
        d
    }));
    for (cause, d) in cases {
        assert_eq!(admit(&d), Some(cause), "{cause}");
    }
}

/// The sky pass's program and stages from the material match the strip
/// the guest draws, or the first that differs is named. No GPU.
#[test]
fn the_sky_look_from_the_material_is_checked_against_the_strip() {
    use native_render::look::Stage;
    use native_render::object_state::STENCIL_MARK_REFERENCE;
    use native_render::program::MODEL_ROWS;
    use native_render::sky_material::SkyLook;

    let mut draw = triangle(2.0, [0.0; 4], STENCIL_MARK_REFERENCE, false);
    let words = program(&[
        Inst::mac(MacOp::Mov, Some(Operand::v(0)), None, None).to_out(false, OutReg::Pos, 0xf),
        Inst::mac(MacOp::Mov, Some(Operand::c(MODEL_ROWS as u8)), None, None).to_out(false, OutReg::D0, 0xf).last(),
    ]);
    draw.shader.vertex = VertexProcessing::Program {
        start: 0,
        microcode: Program::parse(&words).unwrap().instructions.iter().flat_map(|i| i.words).collect::<Vec<_>>().into(),
    };
    let stages = draw.sampled_texture_stages().map(|s| if s { Stage::Bitmap { bitmap: map::TagId(7), image: 0 } } else { Stage::Empty });
    let look = SkyLook::new(&words, stages).unwrap();
    assert_eq!(look.check(&draw), "matched");
    let mut other = look.clone();
    other.stages[3] = if matches!(stages[3], Stage::Empty) { Stage::Bitmap { bitmap: map::TagId(7), image: 1 } } else { Stage::Empty };
    assert_eq!(other.check(&draw), "textures");
    let mut wrong = look.clone();
    wrong.words[0] ^= 1;
    assert_eq!(wrong.check(&draw), "program");
    let mut fixed = draw.clone();
    fixed.shader.vertex = VertexProcessing::FixedFunction { fvf: 0x002 };
    assert_eq!(look.check(&fixed), "program");
}

#[test]
fn the_sky_section_from_the_load_is_checked_against_the_strip() {
    use d3d::backend::{StreamBytes, StreamData};
    use native_render::object_state::STENCIL_MARK_REFERENCE;
    use native_render::sky_material::SkySection;

    let positions: Vec<u8> = (0..6u8 * 4).collect();
    let colours: Vec<u8> = (100..100 + 6u8 * 8).collect();
    let runs = vec![vec![0, 1, 2], vec![3, 5, 4], Vec::new()];
    let section = SkySection::new(vec![(4, positions.clone()), (8, colours.clone())], runs.clone());
    let mut draw = triangle(2.0, [0.0; 4], STENCIL_MARK_REFERENCE, false);
    draw.indices = Some(vec![0u32, 2, 1].into());
    draw.streams = vec![
        StreamBytes { data: StreamData::Owned(positions[12..24].to_vec()), stride: 4 },
        StreamBytes { data: StreamData::Owned(colours[24..48].to_vec()), stride: 8 },
    ];
    assert_eq!(section.check(1, &draw), "matched");
    assert_eq!(section.check(0, &draw), "indices");
    assert_eq!(section.check(2, &draw), "part");
    assert_eq!(section.check(9, &draw), "part");
    let mut absolute = draw.clone();
    absolute.indices = Some(vec![3u32, 5, 4].into());
    absolute.streams[0].data = StreamData::Owned(positions.clone());
    absolute.streams[1].data = StreamData::Owned(colours.clone());
    assert_eq!(section.check(1, &absolute), "matched");
    let mut moved = draw.clone();
    moved.streams[1].data = StreamData::Owned(colours[16..40].to_vec());
    assert_eq!(section.check(1, &moved), "streams");
    let mut stride = draw.clone();
    stride.streams[0].stride = 12;
    assert_eq!(section.check(1, &stride), "streams");
    let short = SkySection::new(vec![(4, positions[..16].to_vec()), (8, colours.clone())], runs);
    assert_eq!(short.check(1, &draw), "window");

    // The resident entry's draw: the strip's, with the load's snapshots.
    let native = section.resident(1, &draw).unwrap();
    assert_eq!(native.vertex_count, 3);
    assert_eq!(native.indices.as_deref(), Some(&[0u32, 2, 1][..]));
    assert!(native.indices.as_ref().unwrap().cache_id().is_some());
    assert_eq!(native.streams[0].data[..], positions[12..24]);
    assert_eq!(native.streams[1].data[..], colours[24..48]);
    assert!(native.streams.iter().all(|s| s.data.cache_id().is_some()));
    let again = section.resident(1, &draw).unwrap();
    assert_eq!(again.streams[0].data.cache_id(), native.streams[0].data.cache_id());
    assert_eq!(again.indices.unwrap().cache_id(), native.indices.unwrap().cache_id());
    assert_eq!(section.resident(1, &absolute).err(), Some("absolute"));
    assert_eq!(section.resident(0, &draw).err(), Some("indices"));
}

/// A pool the native pass can draw: the lens (a frame sample) at stage 0,
/// the target's size, the depth tested without a write.
fn pool_triangle() -> ResolvedDraw {
    let mut draw = triangle(2.0, [1.0; 4], 0, true);
    draw.extras[0].samples_frame = true;
    draw.state.render.depth_func = CompareFunc::LessEqual;
    draw.state.render.stencil.enable = false;
    draw
}

#[test]
fn a_pool_without_its_inputs_is_a_named_refusal() {
    let cache = || Arc::new(Cache::default());
    let refused = |draw: &ResolvedDraw| IndexedPass::for_pool(draw, cache(), 1).err();
    assert_eq!(refused(&pool_triangle()), None);

    let mut draw = pool_triangle();
    draw.target.depth_format = None;
    assert_eq!(refused(&draw), Some("no_depth"));

    let mut draw = pool_triangle();
    draw.state.render.depth_func = CompareFunc::Always;
    assert_eq!(refused(&draw), Some("depth_not_tested"));
    let mut draw = pool_triangle();
    draw.state.render.depth_mode = DepthBufferMode::Disabled;
    assert_eq!(refused(&draw), Some("depth_not_tested"));

    let mut draw = pool_triangle();
    draw.state.render.depth_write = true;
    assert_eq!(refused(&draw), Some("writes_depth"));

    // A lens of another size than the target is still the lens: the
    // recording copies the bound colour, whatever the guest resolved.
    let mut draw = pool_triangle();
    draw.texture_info[0].as_mut().unwrap().width = SIZE / 2;
    draw.texture_info[0].as_mut().unwrap().height = SIZE / 2;
    assert_eq!(pool_lens_stage(&draw), Some(0));
    assert_eq!(refused(&draw), None);
    // A display copy larger than the guest's target, with the lens at guest size.
    let mut draw = pool_triangle();
    draw.target.width = SIZE * 3;
    draw.target.height = SIZE * 2;
    assert_eq!(pool_lens_stage(&draw), Some(0));
    assert_eq!(refused(&draw), None);
    // No sampled image: no lens.
    let mut draw = pool_triangle();
    draw.texture_info[0] = None;
    assert_eq!(refused(&draw), Some("no_lens"));
    // A projective stage whose image is not a frame sample (a ripple
    // bitmap) is not the lens either.
    let mut draw = pool_triangle();
    draw.extras[0].samples_frame = false;
    assert_eq!(pool_lens_stage(&draw), None);
    assert_eq!(refused(&draw), Some("no_lens"));

    assert_eq!(pool_refusal_counter("no lens"), "layer_plan_15_pool_refused_no_lens");
}

#[test]
fn a_pool_lens_samples_the_bound_colour_not_the_guests_sample() {
    let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(ctx) = GfxContext::headless() else { return };
    let mut backend = GfxBackend::new(ctx);
    let color = backend.create_surface(FORMAT, SIZE, SIZE, false).unwrap();
    let depth = backend.create_surface(DEPTH, SIZE, SIZE, true).unwrap();
    // The guest's sample of an earlier frame state: red. The scene the pool
    // draws over: blue.
    let guest_lens = backend.create_texture_target(FORMAT, SIZE, SIZE).unwrap();
    backend.clear(guest_lens, None, Clear { color: Some([1.0, 0.0, 0.0, 1.0]), depth: None, stencil: None }).unwrap();
    let guest_lens = backend.surface_texture(guest_lens).unwrap().unwrap();
    backend.clear(color, Some(depth), Clear::all([0.0, 0.0, 1.0, 1.0])).unwrap();
    let surfaces = ExternalSurfaces { color, depth, sampled: vec![], slots: vec![], slot_sizes: vec![] };
    let cache = Arc::new(Cache::default());
    let draw = pool_triangle();
    let recorded = |counters: &[(String, u64)]| counters.iter()
        .find(|(k, _)| k == "layer_plan_15_pool_recorded").map_or(0, |(_, n)| *n);
    let before = recorded(&pool_recording_counters());
    assert!(after_compile(|| {
        let pass = IndexedPass::for_pool(&draw, cache.clone(), 1).unwrap();
        assert!(pass.replays_scene());
        record_pass(&mut backend, surfaces.clone(), [Some(&guest_lens), None, None, None],
            timeline::native::Stage::Water, Box::new(pass))
    }));
    assert!(recorded(&pool_recording_counters()) > before);
    // The readback is in the host's pixel order: blue is the last colour
    // byte in RGBA and the first in BGRA.
    let (pixels, order) = backend.read_color(color).unwrap();
    let at = 4 * (4 * SIZE as usize + 4);
    let (blue, red) = match order {
        HostPixelOrder::Bgra8 => ([255, 0, 0, 255], [0, 0, 255, 255]),
        HostPixelOrder::Rgba8 => ([0, 0, 255, 255], [255, 0, 0, 255]),
    };
    assert_ne!(&pixels[at..at + 4], &red, "the lens sampled the guest's red");
    assert_eq!(&pixels[at..at + 4], &blue, "the lens is the bound colour");
}

#[test]
fn an_admitted_pool_dropped_without_a_recording_is_counted() {
    let unrecorded = || pool_recording_counters().iter()
        .find(|(k, _)| k == "layer_plan_15_pool_unrecorded").map_or(0, |(_, n)| *n);
    let before = unrecorded();
    drop(IndexedPass::for_pool(&pool_triangle(), Arc::new(Cache::default()), 1).unwrap());
    assert!(unrecorded() > before);
}

/// A standalone range replays in a kept scene, textured or not: its stage
/// views are the recording's own. A pinned texaccum surface does not.
#[test]
fn a_textured_standalone_range_replays_and_a_pinned_one_does_not() {
    let cache = Arc::new(Cache::default());
    let plain = IndexedPass::new(&triangle(2.0, [1.0; 4], 0, false), Arc::clone(&cache), 1, None).unwrap();
    assert!(plain.replays_scene());
    let draw = triangle(2.0, [1.0; 4], 0, true);
    assert!(draw.texture_info[0].is_some());
    let textured = IndexedPass::new(&draw, Arc::clone(&cache), 1, None).unwrap();
    assert!(textured.replays_scene());
    let pinned = IndexedPass::new(&draw, cache, 1, Some(ACCUM)).unwrap();
    assert!(!pinned.replays_scene());
}
