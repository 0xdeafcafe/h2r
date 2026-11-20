//! Optional and ignored: every vertex program and pixel shader definition
//! from your own disc, drawn through `GfxBackend` and the default
//! `Nv2aShaders` provider (translation, naga validation, the program
//! layouts, pipeline creation and a draw, under a wgpu validation scope).
//!
//! The corpus is the directory `h2 extract-shaders` writes from the disc
//! image (`*.xvu`, `*.psdef`); nothing from it is in the repository. Give it
//! as an absolute path:
//!
//! ```text
//! cargo run --release -p cli -- extract-shaders --out $TMPDIR/h2-shaders
//! H2_SHADER_CORPUS=$TMPDIR/h2-shaders cargo test -p d3d --test shader_corpus -- --ignored
//! ```
//!
//! See `docs/graphics/shaders.md`.

use std::path::{Path, PathBuf};

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

fn corpus() -> PathBuf {
    let dir = PathBuf::from(std::env::var("H2_SHADER_CORPUS").expect(
        "H2_SHADER_CORPUS is not set. Build the corpus from your disc with \
         `cargo run --release -p cli -- extract-shaders --out $TMPDIR/h2-shaders`, \
         then set H2_SHADER_CORPUS to that absolute path (docs/graphics/shaders.md)",
    ));
    assert!(
        dir.is_absolute(),
        "H2_SHADER_CORPUS must be an absolute path (made by `h2 extract-shaders --out DIR`)"
    );
    dir
}

fn files(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == ext))
        .collect();
    v.sort();
    v
}

/// A draw of one triangle with `microcode` and `psdef`, every register the
/// program reads supplied as FLOAT4 in one stream.
fn draw(microcode: Vec<u32>, reads: u16, psdef: [u32; 60]) -> ResolvedDraw {
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
            data: vec![0; 3 * stride as usize].into(),
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
                microcode: microcode.into(),
            },
            provided_inputs: reads,
            vertex_defaults: [[0.0, 0.0, 0.0, 1.0]; 16],
            constants: std::sync::Arc::new([[0.0; 4]; CONSTANTS]),
            pixel_shader: Some(std::sync::Arc::new(psdef)),
            combiner_state: std::sync::Arc::new(psdef[..57].try_into().unwrap()),
            final_combiner_latch: psdef[8] | psdef[9] != 0,
            affine_projection: false,
        },
        raw: std::sync::Arc::new(words),
        viewport: Viewport {
            x: 0,
            y: 0,
            width: 64,
            height: 64,
            min_z: 0.0,
            max_z: 1.0,
        },
        textures: [None; 4],
        retail_constants: None,
        ui_fit: None,
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

#[test]
#[ignore = "reads a shader corpus extracted from the disc (H2_SHADER_CORPUS, absolute path)"]
fn every_retail_shader_draws() {
    let dir = corpus();
    let ctx = GfxContext::headless().expect("a GPU adapter");
    let mut b = GfxBackend::new(ctx);
    let color = b
        .create_surface(D3DFormat::A8R8G8B8, 64, 64, false)
        .unwrap();

    // Pairs each retail program with a pass-through combiner, and each
    // retail combiner with a program that writes every output from v0..v7.
    let mut pass = PixelShaderDef {
        combiner_count: 1,
        ..PixelShaderDef::default()
    };
    pass.rgb_inputs[0] = inputs(REG_V0, ONE, 0, 0);
    pass.rgb_outputs[0] = outputs(REG_R0, 0, 0, 0);
    pass.alpha_inputs[0] = inputs(REG_V0 | CHAN_ALPHA, ONE | CHAN_ALPHA, 0, 0);
    pass.alpha_outputs[0] = outputs(REG_R0, 0, 0, 0);
    let pass = words(&pass);
    let outs = [
        OutReg::Pos,
        OutReg::D0,
        OutReg::D1,
        OutReg::B0,
        OutReg::B1,
        OutReg::T(0),
        OutReg::T(1),
        OutReg::T(2),
        OutReg::T(3),
    ];
    let mut insts: Vec<Inst> = outs
        .iter()
        .enumerate()
        .map(|(i, &o)| {
            Inst::mac(MacOp::Mov, Some(Operand::v(i as u8 % 8)), None, None).to_out(false, o, 0xF)
        })
        .collect();
    let last = insts.pop().unwrap().last();
    insts.push(last);
    let simple = vsh::Program::parse(&program(&insts)).unwrap();
    let simple_words: Vec<u32> = simple.instructions.iter().flat_map(|i| i.words).collect();

    let mut cases = Vec::new();
    for f in files(&dir, "xvu") {
        let p = vsh::Program::parse(&std::fs::read(&f).unwrap()).unwrap();
        let words: Vec<u32> = p.instructions.iter().flat_map(|i| i.words).collect();
        cases.push((f, draw(words, p.inputs_read(), pass)));
    }
    for f in files(&dir, "psdef") {
        let def = PixelShaderDef::from_bytes(&std::fs::read(&f).unwrap()).unwrap();
        cases.push((
            f,
            draw(simple_words.clone(), simple.inputs_read(), words(&def)),
        ));
    }
    assert!(
        cases.len() > 2,
        "{}: no *.xvu or *.psdef files",
        dir.display()
    );

    let mut failures = Vec::new();
    for (f, d) in &cases {
        let scope = b.ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let r = b.draw(color, None, d, [None; 4]);
        let err = pollster::block_on(scope.pop());
        match (r, err) {
            (Ok(DrawOutcome::Drawn), None) => {}
            (r, err) => failures.push(format!("{}: {r:?} {err:?}", f.display())),
        }
    }
    eprintln!("{} shaders drawn, {} failed", cases.len(), failures.len());
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
