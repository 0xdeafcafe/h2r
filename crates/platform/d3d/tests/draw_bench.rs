//! Synthetic draw throughput through the device and `GfxBackend` on a real
//! GPU: untextured API draws (the built-in shader), textured API draws (the
//! `Nv2aShaders` provider) and inline push-buffer quads, each timed per draw
//! and per frame (draws then a `Swap`). Every resource is synthetic.
//! Inline quads are drained by `MakeRequestedSpace`, which rewinds the push
//! buffer for the next frame.
//!
//! ```sh
//! cargo test --release -p d3d --test draw_bench -- --ignored --nocapture
//! ```

mod common;

use std::time::{Duration, Instant};

use common::*;
use d3d::abi::Regs;
use d3d::{Device, GfxBackend, GuestMemory, SparseMemory, UploadPolicy};
use gfx::GfxContext;
use gfx::vertex::fvf;
use shaders::psh::PixelShaderDef;
use shaders::psh::bits::*;
use shaders::vsh::encode::{Inst, Operand, program};
use shaders::vsh::{MacOp, OutReg};

/// Draws per frame (Halo's menu draws about 600).
const DRAWS: u32 = 600;
/// Frames per scenario (after one warm-up frame); `H2_BENCH_FRAMES`
/// overrides it (for profiling).
fn frames() -> u32 {
    std::env::var("H2_BENCH_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20)
}

fn swap(d: &mut Device<GfxBackend>, m: &mut SparseMemory) {
    call(d, m, 0x3f9d00, Regs::default(), &[]).unwrap();
}

/// Runs `frame` (which issues `DRAWS` draws) [`frames`] times, then waits for
/// the GPU; prints the mean wall time per frame and per draw.
fn time(
    name: &str,
    d: &mut Device<GfxBackend>,
    m: &mut SparseMemory,
    mut frame: impl FnMut(&mut Device<GfxBackend>, &mut SparseMemory),
) -> Duration {
    frame(d, m);
    swap(d, m);
    let frames = frames();
    let start = Instant::now();
    for _ in 0..frames {
        frame(d, m);
        swap(d, m);
    }
    let _ = d
        .backend
        .ctx
        .device
        .poll(wgpu::PollType::wait_indefinitely());
    let per_frame = start.elapsed() / frames;
    let s = d.backend.batch_stats();
    println!(
        "{name:<28} {:>8.2} ms/frame {:>7.1} us/draw  (passes {}, submits {})",
        per_frame.as_secs_f64() * 1e3,
        per_frame.as_secs_f64() * 1e6 / DRAWS as f64,
        s.passes,
        s.submits,
    );
    assert!(
        d.take_events()
            .iter()
            .all(|e| !matches!(e, d3d::Event::DrawSkipped { .. } | d3d::Event::BadState(_)))
    );
    per_frame
}

fn device() -> Option<(Device<GfxBackend>, SparseMemory)> {
    match GfxContext::headless() {
        Ok(ctx) => Some(create(GfxBackend::new(ctx), UploadPolicy::Tracked)),
        Err(e) => {
            eprintln!("skipping GPU benchmark: {e}");
            None
        }
    }
}

#[test]
#[ignore = "benchmark: run with --release -- --ignored --nocapture"]
fn draw_throughput() {
    // Untextured strip quads from a vertex buffer: the built-in shader.
    if let Some((mut d, mut m)) = device() {
        set_states(&mut d, &mut m);
        quad_vb(&mut m, 0.0, 0.0, 8.0, 8.0, 0xFFFF_0000);
        bind_quad(&mut d, &mut m);
        time("DrawVertices, built-in", &mut d, &mut m, |d, m| {
            d.clear(m, 0, 0, 0xF0, 0xFF00_0000, 1.0, 0).unwrap();
            for _ in 0..DRAWS {
                d.draw_vertices(m, TRIANGLESTRIP, 0, 4).unwrap();
            }
        });
    }

    // Textured fixed-function quads: the Nv2aShaders provider.
    if let Some((mut d, mut m)) = device() {
        set_states(&mut d, &mut m);
        texture(&mut m, 0x80);
        d.set_texture(&mut m, 0, TEX_HEADER).unwrap();
        let mut vb = Vec::new();
        for (x, y) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
            for f in [8.0 * x, 8.0 * y, 0.5, 1.0] {
                vb.extend_from_slice(&f.to_le_bytes());
            }
            vb.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
            for f in [16.0 * x, 4.0 * y] {
                vb.extend_from_slice(&f.to_le_bytes());
            }
        }
        m.write(0x8000_0000 | VB_DATA, &vb).unwrap();
        m.write_u32(VB_HEADER, 1 | 0x0100_0000).unwrap();
        m.write_u32(VB_HEADER + 4, VB_DATA).unwrap();
        m.write_u32(VB_HEADER + 8, 0).unwrap();
        d.set_vertex_shader(&mut m, fvf::XYZRHW | fvf::DIFFUSE | 0x100)
            .unwrap();
        d.set_stream_source(&mut m, 0, VB_HEADER, 28).unwrap();
        time("DrawVertices, textured", &mut d, &mut m, |d, m| {
            d.clear(m, 0, 0, 0xF0, 0xFF00_0000, 1.0, 0).unwrap();
            for _ in 0..DRAWS {
                d.draw_vertices(m, TRIANGLESTRIP, 0, 4).unwrap();
            }
        });
    }

    // Inline push-buffer quads (BEGIN_END / VERTEX4F), drained at Swap.
    if let Some((mut d, mut m)) = device() {
        set_states(&mut d, &mut m);
        d.set_vertex_shader(&mut m, fvf::XYZRHW | fvf::DIFFUSE)
            .unwrap();
        time("inline push-buffer quads", &mut d, &mut m, |d, m| {
            d.clear(m, 0, 0, 0xF0, 0xFF00_0000, 1.0, 0).unwrap();
            for i in 0..DRAWS {
                let x = (i % 8) as f32 * 8.0;
                inline_quad(m, x, 0.0, x + 8.0, 8.0, [0, 255, 0, 255]);
            }
            // MakeRequestedSpace: run what was pushed and rewind to the start.
            let regs = Regs {
                eax: 0x100,
                ..Regs::default()
            };
            call(d, m, 0x3faae0, regs, &[0x4000]).unwrap();
        });
    }

    // Halo-like object draws: a vertex program reading constants, a
    // register-combiner pixel shader and a texture, with what objects change
    // between draws: 8 constant registers, the texture (two alternate) and a
    // render state.
    if let Some((mut d, mut m)) = device() {
        set_states(&mut d, &mut m);
        programmable(&mut d, &mut m);
        texture(&mut m, 0x80);
        let second = TEX_HEADER + 0x40;
        for i in 0..5u32 {
            let w = m.read_u32(TEX_HEADER + 4 * i).unwrap();
            m.write_u32(second + 4 * i, w).unwrap();
        }
        let mut n = 0u32;
        time("programmable, per-draw churn", &mut d, &mut m, |d, m| {
            d.clear(m, 0, 0, 0xF0, 0xFF00_0000, 1.0, 0).unwrap();
            for i in 0..DRAWS {
                n = n.wrapping_add(1);
                for r in 0..32u32 {
                    let v = ((n + r) % 7) as f32 * 0.001;
                    m.write_u32(CONSTS + 32 + 4 * r, v.to_bits()).unwrap();
                }
                d.set_vertex_shader_constant(m, 98, CONSTS + 32, 8, true).unwrap();
                d.set_texture(m, 0, if i % 2 == 0 { TEX_HEADER } else { second }).unwrap();
                d.set_render_state(m, 61, (i % 4) * 8).unwrap(); // ALPHAREF
                d.draw_vertices(m, TRIANGLESTRIP, 0, 4).unwrap();
            }
        });
    }
}

const VAF: u32 = MISC + 0x3000;
const STREAM_INPUTS: u32 = MISC + 0x3200;
const FUNCTION: u32 = MISC + 0x3400;
const PSDEF: u32 = MISC + 0x3800;
const PS_OBJECT: u32 = MISC + 0x3900;
const CONSTS: u32 = MISC + 0x3A00;

/// A vertex program (oPos = v0 * c96 + c97 plus terms reading c98..c105,
/// oT0 = v9, oD0 = v3) with its stream, and a one-stage combiner shader.
fn programmable(d: &mut Device<GfxBackend>, m: &mut SparseMemory) {
    let mut insts = vec![
        Inst::mac(MacOp::Mul, Some(Operand::v(0)), Some(Operand::c(96)), None).to_temp(0, 0xF),
    ];
    // Eight constant reads folded in at zero weight-ish values.
    for c in 98..106 {
        insts.push(
            Inst::mac(MacOp::Add, Some(Operand::r(0)), None, Some(Operand::c(c))).to_temp(0, 0xF),
        );
    }
    insts.push(
        Inst::mac(MacOp::Add, Some(Operand::r(0)), None, Some(Operand::c(97))).to_out(
            false,
            OutReg::Pos,
            0xF,
        ),
    );
    insts.push(Inst::mac(MacOp::Mov, Some(Operand::v(9)), None, None).to_out(false, OutReg::T(0), 0xF));
    insts.push(
        Inst::mac(MacOp::Mov, Some(Operand::v(3)), None, None)
            .to_out(false, OutReg::D0, 0xF)
            .last(),
    );
    m.write(FUNCTION, &program(&insts)).unwrap();
    d.load_vertex_shader_program(m, FUNCTION, 0).unwrap();
    for (i, v) in [8.0f32, 8.0, 0.0, 0.0, 0.0, 16.0, 0.0, 1.0]
        .iter()
        .enumerate()
    {
        m.write_u32(CONSTS + 4 * i as u32, v.to_bits()).unwrap();
    }
    d.set_vertex_shader_constant(m, 96, CONSTS, 2, true).unwrap();
    d.set_vertex_data(m, 3, [1.0, 1.0, 1.0, 1.0]).unwrap();
    for reg in 0..16u32 {
        let (offset, format) = match reg {
            0 => (0, 0x42),
            9 => (16, 0x22),
            _ => (0, 0x02),
        };
        for (j, w) in [0, offset, format, 0].iter().enumerate() {
            m.write_u32(VAF + 16 * reg + 4 * j as u32, *w).unwrap();
        }
    }
    d.select_vertex_shader_direct(m, VAF, 0).unwrap();
    let mut vb = Vec::new();
    for (x, y) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        for f in [x, y, 0.0, 1.0, 16.0 * x, 4.0 * y] {
            vb.extend_from_slice(&f.to_le_bytes());
        }
    }
    m.write(0x8000_0000 | VB_DATA, &vb).unwrap();
    m.write_u32(VB_HEADER, 1 | 0x0100_0000).unwrap();
    m.write_u32(VB_HEADER + 4, VB_DATA).unwrap();
    m.write_u32(VB_HEADER + 8, 0).unwrap();
    for (i, w) in [VB_HEADER, 24, 0].iter().enumerate() {
        m.write_u32(STREAM_INPUTS + 4 * i as u32, *w).unwrap();
    }
    let handle = d3d::GuestLayout::HALO2.static_vertex_shader | 1;
    d.set_vertex_shader_input(m, handle, 1, STREAM_INPUTS).unwrap();
    let mut def = PixelShaderDef {
        combiner_count: 1,
        texture_modes: texture_modes([TEX_PROJECT2D, 0, 0, 0]),
        ..PixelShaderDef::default()
    };
    def.rgb_inputs[0] = inputs(REG_T0, REG_C0, 0, 0);
    def.rgb_outputs[0] = outputs(REG_R0, 0, 0, 0);
    def.alpha_inputs[0] = inputs(REG_T0 | CHAN_ALPHA, REG_V0 | CHAN_ALPHA, 0, 0);
    def.alpha_outputs[0] = outputs(REG_R0, 0, 0, 0);
    def.constant0[0] = 0xFFFF_80FF;
    m.write(PSDEF, &def.to_bytes()).unwrap();
    for (i, w) in [1, 0, PSDEF].iter().enumerate() {
        m.write_u32(PS_OBJECT + 4 * i as u32, *w).unwrap();
    }
    d.set_pixel_shader(m, PS_OBJECT).unwrap();
}
