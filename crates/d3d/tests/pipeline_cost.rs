//! Optional and ignored: where the time of a first-time NV2A pipeline goes.
//!
//! For each vertex program of a shader corpus (`h2 extract-shaders`, see
//! `shader_corpus.rs`; nothing from it is in the repository), paired with a
//! pass-through combiner, this times the steps of a cold compile:
//!
//! * `translate`: `vsh`/`psh` translation, linking and naga validation (ours),
//! * `module`: `create_shader_module` (wgpu parses and validates the WGSL),
//! * `msl`: naga's MSL back end alone, both stages (what wgpu-hal runs per
//!   pipeline on Metal),
//! * `pipeline`: `create_render_pipeline` (MSL plus the Metal compiler).
//!
//! The entry points are renamed per run so the Metal compiler's own cache
//! cannot hold the MSL. Then it builds the same set again, salted afresh, on
//! worker threads, to see whether pipeline creation scales.
//!
//! ```text
//! H2_SHADER_CORPUS=$TMPDIR/h2-shaders cargo test --release -p d3d --test pipeline_cost -- --ignored --nocapture
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gfx::GfxContext;
use gfx::pipeline::{PipelineKey, ShaderId, ShaderProgram, TargetFormats, create_render_pipeline};
use gfx::program::ProgramBindings;
use gfx::state::RenderState;
use gfx::translate::Capabilities;
use gfx::vertex::{
    PrimitiveType, StreamLayout, VertexAttribute, VertexDataType, VertexLayout, VertexRegister,
};
use shaders::psh::bits::*;
use shaders::psh::{self, PixelOptions, PixelShaderDef};
use shaders::vsh::{self, VertexOptions};

const SAMPLE: usize = 24;

struct Case {
    wgsl: String,
    layout: VertexLayout,
}

fn salted(wgsl: &str, salt: &str) -> String {
    wgsl.replace("vs_main", &format!("vs_main_{salt}"))
        .replace("fs_main", &format!("fs_main_{salt}"))
}

fn build(
    ctx: &GfxContext,
    bindings: &mut ProgramBindings,
    case: &Case,
    salt: &str,
) -> (ShaderProgram, PipelineKey) {
    let wgsl = salted(&case.wgsl, salt);
    let module = ctx
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cost"),
            source: wgpu::ShaderSource::Wgsl(wgsl.into()),
        });
    let mut program = bindings.program(&ctx.device, module, [None; 4]);
    program.vs_entry = format!("vs_main_{salt}");
    program.fs_entry = format!("fs_main_{salt}");
    let key = PipelineKey::new(
        ShaderId(1),
        &case.layout,
        &[],
        PrimitiveType::TriangleList,
        &RenderState::default(),
        TargetFormats {
            color: wgpu::TextureFormat::Bgra8Unorm,
            depth: Some(wgpu::TextureFormat::Depth24PlusStencil8),
            samples: 1,
        },
        Capabilities::ALL,
    );
    (program, key)
}

#[test]
#[ignore = "reads a shader corpus extracted from the disc (H2_SHADER_CORPUS, absolute path)"]
fn first_pipeline_cost() {
    let dir = PathBuf::from(std::env::var("H2_SHADER_CORPUS").expect("H2_SHADER_CORPUS"));
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "xvu"))
        .collect();
    paths.sort();
    // Spread the sample over the corpus.
    let step = (paths.len() / SAMPLE).max(1);
    let paths: Vec<PathBuf> = paths.into_iter().step_by(step).take(SAMPLE).collect();

    let mut pass = PixelShaderDef {
        combiner_count: 1,
        ..PixelShaderDef::default()
    };
    pass.rgb_inputs[0] = inputs(REG_V0, ONE, 0, 0);
    pass.rgb_outputs[0] = outputs(REG_R0, 0, 0, 0);
    pass.alpha_inputs[0] = inputs(REG_V0 | CHAN_ALPHA, ONE | CHAN_ALPHA, 0, 0);
    pass.alpha_outputs[0] = outputs(REG_R0, 0, 0, 0);

    let ctx = GfxContext::headless().expect("a GPU adapter");
    let mut bindings = ProgramBindings::new(&ctx.device);
    let salt0 = Instant::now().elapsed().as_nanos() ^ std::process::id() as u128;
    let (mut t_tr, mut t_mod, mut t_msl, mut t_pipe) = (
        Duration::ZERO,
        Duration::ZERO,
        Duration::ZERO,
        Duration::ZERO,
    );
    let mut cases = Vec::new();
    let mut lines = 0usize;
    for (i, p) in paths.iter().enumerate() {
        let t = Instant::now();
        let prog = vsh::Program::parse(&std::fs::read(p).unwrap()).unwrap();
        if prog.writes_constants() {
            continue;
        }
        let reads = prog.inputs_read();
        let vs = vsh::translate(
            &prog,
            &VertexOptions {
                provided_inputs: reads,
                flat_shading: false,
                ..Default::default()
            },
        )
        .unwrap();
        let ps = psh::translate(&pass, &PixelOptions::default());
        let wgsl = shaders::link(&vs, &ps).unwrap();
        shaders::validate(&wgsl).unwrap();
        t_tr += t.elapsed();
        lines += wgsl.lines().count();

        let regs: Vec<u8> = (0..16u8).filter(|r| reads & (1 << r) != 0).collect();
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
                stride: 16 * regs.len().max(1) as u32,
            }],
            pretransformed: false,
        };
        let case = Case { wgsl, layout };
        let salt = format!("{salt0:x}_{i}");

        let t = Instant::now();
        let (program, key) = build(&ctx, &mut bindings, &case, &salt);
        t_mod += t.elapsed();

        let t = Instant::now();
        let (module, info) = shaders::validate(&salted(&case.wgsl, &salt)).unwrap();
        for (stage, name) in [
            (naga::ShaderStage::Vertex, &program.vs_entry),
            (naga::ShaderStage::Fragment, &program.fs_entry),
        ] {
            naga::back::msl::write_string(
                &module,
                &info,
                &naga::back::msl::Options::default(),
                &naga::back::msl::PipelineOptions {
                    entry_point: Some((stage, name.clone())),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        t_msl += t.elapsed();

        let t = Instant::now();
        let _p = create_render_pipeline(&ctx.device, &key, &program, "cost");
        t_pipe += t.elapsed();
        cases.push(case);
    }
    let n = cases.len() as u32;
    assert!(n > 0, "no vertex programs in {}", dir.display());
    let ms = |d: Duration| d.as_secs_f64() * 1e3 / n as f64;
    eprintln!(
        "{n} programs ({} WGSL lines each), ms per program: translate {:.2}, module {:.2}, msl {:.2}, pipeline {:.2} (Metal {:.2})",
        lines / n as usize,
        ms(t_tr),
        ms(t_mod),
        ms(t_msl),
        ms(t_pipe),
        ms(t_pipe.saturating_sub(t_msl)),
    );

    // The same set, salted afresh, on worker threads.
    for threads in [1usize, 4] {
        let built: Vec<_> = cases
            .iter()
            .enumerate()
            .map(|(i, c)| build(&ctx, &mut bindings, c, &format!("{salt0:x}_t{threads}_{i}")))
            .collect();
        let t = Instant::now();
        std::thread::scope(|s| {
            for chunk in built.chunks(built.len().div_ceil(threads)) {
                let device = ctx.device.clone();
                s.spawn(move || {
                    for (program, key) in chunk {
                        create_render_pipeline(&device, key, program, "cost");
                    }
                });
            }
        });
        eprintln!(
            "{threads} thread(s): {:.1} ms wall for {n} pipelines ({:.2} ms each)",
            t.elapsed().as_secs_f64() * 1e3,
            t.elapsed().as_secs_f64() * 1e3 / n as f64
        );
    }
}
