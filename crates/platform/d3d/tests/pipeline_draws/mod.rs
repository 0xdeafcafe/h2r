//! Synthetic NV2A draws for the pipeline cache tests (`pipeline_cache.rs`,
//! `pipeline_scope.rs`): one vertex program that writes every output from
//! a register, and one combiner program per draw.
#![allow(dead_code)]

use std::path::PathBuf;

use d3d::backend::{StreamBytes, TargetInfo, Viewport};
use d3d::shader::{CONSTANTS, ShaderInputs, VertexProcessing};
use d3d::state::StateWords;
use d3d::{Backend, DrawOutcome, GfxBackend, ResolvedDraw};
use gfx::D3DFormat;
use gfx::vertex::{
    PrimitiveType, StreamLayout, VertexAttribute, VertexDataType, VertexLayout, VertexRegister,
};
use gfx::xdk::RenderStateTable;
use shaders::psh::PixelShaderDef;
use shaders::psh::bits::*;
use shaders::vsh::encode::{Inst, Operand, program};
use shaders::vsh::{self, MacOp, OutReg};

/// A draw of three vertices through `microcode` and the combiner words
/// `psdef`, onto a 64x64 target.
pub fn draw(microcode: Vec<u32>, reads: u16, psdef: [u32; 60], data: Vec<u8>) -> ResolvedDraw {
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

/// The render-state words of a combiner program.
pub fn words(def: &PixelShaderDef) -> [u32; 60] {
    let b = def.to_bytes();
    std::array::from_fn(|i| u32::from_le_bytes(b[4 * i..4 * i + 4].try_into().unwrap()))
}

/// One draw, and one program, per combiner: `(rgb inputs, alpha-tests)`.
pub fn draws_with(combiners: &[(u32, bool)]) -> Vec<ResolvedDraw> {
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
    combiners
        .iter()
        .map(|&(rgb, kills)| {
            let mut def = PixelShaderDef {
                combiner_count: 1,
                ..PixelShaderDef::default()
            };
            def.rgb_inputs[0] = rgb;
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

/// A fresh cache file path for test `name`, in a directory of its own.
pub fn scratch(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("d3d-pipeline-cache-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.join(d3d::pipeline_store::FILE_NAME)
}

/// Draws everything, presents, and returns the pixels.
pub fn frame(b: &mut GfxBackend, draws: &[ResolvedDraw]) -> Vec<u8> {
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
