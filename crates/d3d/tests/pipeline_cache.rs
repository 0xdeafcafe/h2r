//! The persistent pipeline cache end to end, on synthetic shaders: a run
//! records every pipeline its draws needed; the next run, from the same
//! file, translates those programs and builds their pipelines ahead of time
//! on worker threads, so its draws translate nothing, miss no pipeline, build
//! nothing on the drawing thread and render the same pixels.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use d3d::backend::{StreamBytes, TargetInfo, Viewport};
use d3d::shader::{CONSTANTS, ShaderInputs, VertexProcessing};
use d3d::state::StateWords;
use d3d::{Backend, DrawOutcome, GfxBackend, ResolvedDraw};
use gfx::vertex::{
    PrimitiveType, StreamLayout, VertexAttribute, VertexDataType, VertexLayout, VertexRegister,
};
use gfx::xdk::RenderStateTable;
use gfx::{D3DFormat, GfxContext};
use shaders::psh::PixelShaderDef;
use shaders::psh::bits::*;
use shaders::vsh::encode::{Inst, Operand, program};
use shaders::vsh::{self, MacOp, OutReg};

fn draw(microcode: Vec<u32>, reads: u16, psdef: [u32; 60], data: Vec<u8>) -> ResolvedDraw {
    let regs: Vec<u8> = (0..16u8).filter(|r| reads & (1 << r) != 0).collect();
    let stride = 16 * regs.len().max(1) as u32;
    let layout = VertexLayout {
        streams: vec![StreamLayout {
            stream: 0,
            attributes: regs
                .iter()
                .enumerate()
                .map(|(i, &r)| VertexAttribute {
                    register: VertexRegister(r),
                    data_type: VertexDataType::Float4,
                    offset: 16 * i as u32,
                })
                .collect(),
            stride,
        }],
        pretransformed: false,
    };
    let mut words = StateWords::defaults(false);
    words.render[..57].copy_from_slice(&psdef[..57]);
    words.render[136] = psdef[54];
    ResolvedDraw {
        entry: "corpus",
        primitive: PrimitiveType::TriangleList,
        vertex_count: 3,
        layout,
        streams: vec![StreamBytes {
            data: data.into(),
            stride,
        }],
        indices: None,
        state: words.translate(&RenderStateTable::default()),
        extras: words.stage_extras(),
        target: TargetInfo {
            width: 64,
            height: 64,
            format: D3DFormat::A8R8G8B8,
            depth_format: None,
        },
        texture_info: [None; 4],
        shader: ShaderInputs {
            vertex: VertexProcessing::Program {
                start: 0,
                microcode,
            },
            provided_inputs: reads,
            vertex_defaults: [[0.0, 0.0, 0.0, 1.0]; 16],
            constants: Box::new([[0.0; 4]; CONSTANTS]),
            pixel_shader: Some(Box::new(psdef)),
            combiner_state: Box::new(psdef[..57].try_into().unwrap()),
            final_combiner_latch: psdef[8] | psdef[9] != 0,
            affine_projection: false,
        },
        raw: Box::new(words),
        viewport: Viewport {
            x: 0,
            y: 0,
            width: 64,
            height: 64,
            min_z: 0.0,
            max_z: 1.0,
        },
        textures: [None; 4],
        glyph_cache: 0,
        clip: None,
        clip_rects: Vec::new(),
        hw: d3d::hw::HwState::default(),
    }
}

fn words(def: &PixelShaderDef) -> [u32; 60] {
    let b = def.to_bytes();
    std::array::from_fn(|i| u32::from_le_bytes(b[4 * i..4 * i + 4].try_into().unwrap()))
}

/// Three draws, two programs with different combiners over one vertex program that writes
/// every output from a register: a triangle in window coordinates, coloured
/// by `v1`.
fn draws() -> Vec<ResolvedDraw> {
    let outs = [OutReg::Pos, OutReg::D0, OutReg::D1, OutReg::T(0)];
    let mut insts: Vec<Inst> = outs
        .iter()
        .enumerate()
        .map(|(i, &o)| {
            Inst::mac(MacOp::Mov, Some(Operand::v(i as u8)), None, None).to_out(false, o, 0xF)
        })
        .collect();
    let last = insts.pop().unwrap().last();
    insts.push(last);
    let p = vsh::Program::parse(&program(&insts)).unwrap();
    let microcode: Vec<u32> = p.instructions.iter().flat_map(|i| i.words).collect();
    let reads = p.inputs_read();
    let regs = (0..16).filter(|r| reads & (1 << r) != 0).count();
    let mut data = Vec::new();
    for pos in [
        [0.0f32, 0.0, 0.5, 1.0],
        [64.0, 0.0, 0.5, 1.0],
        [0.0, 64.0, 0.5, 1.0],
    ] {
        for r in 0..regs {
            let v = if r == 0 { pos } else { [0.25, 0.5, 0.75, 1.0] };
            for f in v {
                data.extend_from_slice(&f.to_le_bytes());
            }
        }
    }
    // The second one alpha-tests, so its program discards: merged passes
    // under the exact rule give it a render pass of its own, which must
    // hold when its program comes from the cache too.
    [(REG_V0, false), (REG_V1, true), (REG_V0, false)]
        .into_iter()
        .map(|(src, kills)| {
            let mut def = PixelShaderDef {
                combiner_count: 1,
                ..PixelShaderDef::default()
            };
            def.rgb_inputs[0] = inputs(src, ONE, 0, 0);
            def.rgb_outputs[0] = outputs(REG_R0, 0, 0, 0);
            def.alpha_inputs[0] = inputs(REG_V0 | CHAN_ALPHA, ONE | CHAN_ALPHA, 0, 0);
            def.alpha_outputs[0] = outputs(REG_R0, 0, 0, 0);
            let mut d = draw(microcode.clone(), reads, words(&def), data.clone());
            if kills {
                d.state.render.alpha_test_enable = true;
                d.state.render.alpha_func = gfx::state::CompareFunc::Greater;
                d.state.render.alpha_ref = 0;
            }
            d
        })
        .collect()
}

fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("d3d-pipeline-cache-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.join(d3d::pipeline_store::FILE_NAME)
}

/// `b` with the exact merge rule: every discarding draw starts a render pass.
fn exact(mut b: GfxBackend) -> GfxBackend {
    b.set_merge_discards(false);
    b
}

/// Draws everything, presents, and returns the pixels.
fn frame(b: &mut GfxBackend, draws: &[ResolvedDraw]) -> Vec<u8> {
    let color = b
        .create_surface(D3DFormat::A8R8G8B8, 64, 64, false)
        .unwrap();
    for d in draws {
        assert_eq!(b.draw(color, None, d, [None; 4]), Ok(DrawOutcome::Drawn));
    }
    let px = b.read_color(color).unwrap().0;
    b.present(color, 0).unwrap();
    px
}

#[test]
fn second_run_builds_nothing_on_the_drawing_thread() {
    let ctx = || GfxContext::headless();
    if let Err(e) = ctx() {
        eprintln!("skipping GPU test: {e}");
        return;
    }
    let ctx = || ctx().unwrap();
    let path = scratch();
    let draws = draws();
    let me = std::thread::current().id();

    // First run: cold. Every pipeline is a miss and gets recorded.
    let mut b = exact(GfxBackend::new(ctx())).with_pipeline_cache(path.clone());
    let want = frame(&mut b, &draws);
    assert!(
        want.chunks(4).any(|p| p != [0, 0, 0, 0]),
        "the draws drew something"
    );
    let passes = b.batch_stats().render_passes;
    let built = b.renderer.cache().len();
    assert!(built >= 2, "{built} pipelines");
    assert_eq!(b.renderer.cache().misses() as usize, built);
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        text.lines()
            .filter(|l| l.starts_with("{\"Pipeline\""))
            .count(),
        built
    );
    assert_eq!(
        text.lines()
            .filter(|l| l.starts_with("{\"Program\""))
            .count(),
        2
    );
    drop(b);

    // Second run: warm. Prewarming finishes before the frame (presents
    // drive it, as frames do in the game).
    let mut b = exact(GfxBackend::new(ctx())).with_pipeline_cache(path.clone());
    let warm = b.create_surface(D3DFormat::A8R8G8B8, 8, 8, false).unwrap();
    let start = Instant::now();
    while b.renderer.cache().len() < built || b.renderer.cache().iter().any(|(_, p)| !p.is_ready())
    {
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "prewarm never finished"
        );
        b.present(warm, 0).unwrap();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(frame(&mut b, &draws), want);
    assert_eq!(
        b.renderer.cache().misses(),
        0,
        "every pipeline was prewarmed"
    );
    assert!(
        b.renderer
            .cache()
            .iter()
            .all(|(_, p)| p.built_on().is_some_and(|t| t != me))
    );
    // The prewarmed discarding program still gets its own pass.
    assert_eq!(b.batch_stats().render_passes, passes);
    // Nothing new to record.
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);

    // Without the cache: same pixels, built at draw time, and the same as
    // one render pass per draw.
    let mut b = exact(GfxBackend::new(ctx()));
    assert_eq!(frame(&mut b, &draws), want);
    assert_eq!(b.batch_stats().render_passes, passes);
    let mut b = GfxBackend::new(ctx());
    b.set_merge_passes(false);
    assert_eq!(frame(&mut b, &draws), want);
    assert!(b.batch_stats().render_passes >= passes);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}
