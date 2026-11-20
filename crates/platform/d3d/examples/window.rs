//! The device rendering into a real window through `crates/apps/app`:
//! `GfxBackend::new(host.gfx_context()).on_present(host.present_hook())`.
//!
//! A synthetic guest (no game data) drives the D3D device the way Halo's
//! UI does: every frame it clears, draws a textured quad through a vertex
//! program and a register-combiner pixel shader with alpha blending and
//! alpha test (sliding left and right), draws a fixed-function textured
//! quad, and calls `Swap`.
//!
//! ```text
//! cargo run -p d3d --example window
//! cargo run -p d3d --example window -- --frames 300   # closes after 5 s
//! ```
//!
//! What to look for: a dark blue background; at the top a 256x128 quad of a
//! red/green checkerboard whose green squares are see-through and whose
//! lower-right corner is cut away by the alpha test, moving left and right;
//! at the bottom a static, opaque copy of the checkerboard tinted by the
//! vertex colour (fixed-function MODULATE).

use app::{AppConfig, Control, GuestHost};
use d3d::device::{DeviceConfig, GuestRange, PresentParameters};
use d3d::{Device, Event, GfxBackend, GuestLayout, GuestMemory, SparseMemory};
use gfx::vertex::fvf;
use shaders::psh::PixelShaderDef;
use shaders::psh::bits::*;
use shaders::vsh::encode::{Inst, Operand, program};
use shaders::vsh::{MacOp, OutReg};

const W: u32 = 640;
const H: u32 = 480;
const PB: GuestRange = GuestRange {
    addr: 0x0100_0000,
    len: 0x4_0000,
};
const SCRATCH: GuestRange = GuestRange {
    addr: 0x0200_0000,
    len: 0x1000,
};
const MISC: u32 = 0x0300_0000;
const PARAMS: u32 = MISC;
const PP_DEVICE: u32 = MISC + 0x100;
const VB_HEADER: u32 = MISC + 0x1000;
const VB2_HEADER: u32 = MISC + 0x1100;
const TEX_HEADER: u32 = MISC + 0x2000;
const VAF: u32 = MISC + 0x3000;
const STREAM_INPUTS: u32 = MISC + 0x3200;
const FUNCTION: u32 = MISC + 0x3400;
const PSDEF: u32 = MISC + 0x3800;
const PS_OBJECT: u32 = MISC + 0x3900;
const CONSTS: u32 = MISC + 0x3A00;
/// Physical addresses (seen at `0x8000_0000 | pa`).
const VB_DATA: u32 = 0x0010_0000;
const VB2_DATA: u32 = 0x0010_0800;
const TEX_DATA: u32 = 0x0020_0000;
const TEX: u32 = 64;

fn memory() -> anyhow::Result<SparseMemory> {
    let mut m = SparseMemory::new();
    m.map(0x0040_4000, 0x5000);
    // A stand-in `D3DSIMPLERENDERSTATEENCODE` (the real one is in the XBE):
    // every slot shares a non-invertible method except a few.
    let encode = GuestLayout::HALO2.simple_encode;
    m.map(encode & !0xFFF, 0x1000);
    let mut table = [(1u32 << 18) | 0x1D90; d3d::pushbuf::SIMPLE_ENCODE_SLOTS];
    for (slot, method) in [(57usize, 0x0354u32), (59, 0x0304), (64, 0x035C)] {
        table[slot] = (1 << 18) | method;
    }
    let bytes: Vec<u8> = table.iter().flat_map(|w| w.to_le_bytes()).collect();
    m.write(encode, &bytes)?;
    m.map(PB.addr, PB.len);
    m.map(SCRATCH.addr, SCRATCH.len);
    m.map(MISC, 0x1_0000);
    m.map(0x8000_0000 | VB_DATA, 0x1000);
    m.map(0x8000_0000 | TEX_DATA, TEX * TEX * 4);
    Ok(m)
}

fn write_words(m: &mut SparseMemory, at: u32, words: &[u32]) -> anyhow::Result<()> {
    for (i, w) in words.iter().enumerate() {
        m.write_u32(at + 4 * i as u32, *w)?;
    }
    Ok(())
}

fn floats(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// One-time guest setup: device, texture, vertex program, pixel shader.
fn setup(d: &mut Device<GfxBackend>, m: &mut SparseMemory) -> anyhow::Result<()> {
    PresentParameters {
        width: W,
        height: H,
        back_buffer_format: 0x06, // A8R8G8B8
        back_buffer_count: 1,
        multisample_type: 0,
        swap_effect: 0,
        enable_auto_depth_stencil: true,
        auto_depth_stencil_format: 0x2A, // D24S8
        flags: 0,
        refresh_rate: 0,
        presentation_interval: 0x8000_0000,
        buffer_surface: 0,
        depth_stencil_surface: 0,
    }
    .write(m, PARAMS)?;
    d.create_device(m, 0x40, PARAMS, PP_DEVICE)?;
    d.set_render_state(m, 143, 0)?; // ZENABLE
    d.set_render_state(m, 147, 0)?; // CULLMODE = NONE

    // 64x64 LIN_A8R8G8B8 checkerboard: opaque red and translucent green
    // 8x8 squares, alpha falling to zero towards the lower-right corner.
    let mut texels = Vec::new();
    for y in 0..TEX {
        for x in 0..TEX {
            let fade = 255 - ((x + y) * 255 / (2 * TEX - 2));
            let c = if (x / 8 + y / 8) % 2 == 0 {
                (fade << 24) | 0x00FF_2020
            } else {
                ((fade / 2) << 24) | 0x0020_FF20
            };
            texels.extend(c.to_le_bytes());
        }
    }
    m.write(0x8000_0000 | TEX_DATA, &texels)?;
    let pitch = TEX * 4;
    write_words(
        m,
        TEX_HEADER,
        &[
            1 | (4 << 16) | 0x0100_0000,
            TEX_DATA,
            0,
            (2 << 4) | (0x12 << 8) | (1 << 16),
            (TEX - 1) | ((TEX - 1) << 12) | ((pitch / 64 - 1) << 24),
        ],
    )?;
    d.set_texture(m, 0, TEX_HEADER)?;
    d.set_texture_stage_state(m, 0, 3, 2)?; // MAGFILTER = LINEAR
    d.set_texture_stage_state(m, 0, 4, 2)?; // MINFILTER = LINEAR

    // Vertex program: oPos = v0 * c0 + c1, oT0 = v9, oD0 = v3 (default).
    let code = program(&[
        Inst::mac(MacOp::Mul, Some(Operand::v(0)), Some(Operand::c(96)), None).to_temp(0, 0xF),
        Inst::mac(MacOp::Add, Some(Operand::r(0)), None, Some(Operand::c(97))).to_out(
            false,
            OutReg::Pos,
            0xF,
        ),
        Inst::mac(MacOp::Mov, Some(Operand::v(9)), None, None).to_out(false, OutReg::T(0), 0xF),
        Inst::mac(MacOp::Mov, Some(Operand::v(3)), None, None)
            .to_out(false, OutReg::D0, 0xF)
            .last(),
    ]);
    m.write(FUNCTION, &code)?;
    d.load_vertex_shader_program(m, FUNCTION, 0)?;
    for reg in 0..16u32 {
        let (offset, format) = match reg {
            0 => (0, 0x42),  // FLOAT4
            9 => (16, 0x22), // FLOAT2
            _ => (0, 0x02),  // NONE
        };
        write_words(m, VAF + 16 * reg, &[0, offset, format, 0])?;
    }
    // Unit square, texel-addressed coordinates (linear texture).
    let mut vb = Vec::new();
    for (x, y) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        vb.extend(floats(&[x, y, 0.0, 1.0, TEX as f32 * x, TEX as f32 * y]));
    }
    m.write(0x8000_0000 | VB_DATA, &vb)?;
    write_words(m, VB_HEADER, &[1 | 0x0100_0000, VB_DATA, 0])?;
    // Fixed-function quad: XYZRHW | DIFFUSE | TEX1.
    let mut vb2 = Vec::new();
    for (x, y) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        vb2.extend(floats(&[192.0 + 256.0 * x, 300.0 + 128.0 * y, 0.5, 1.0]));
        vb2.extend(0xFFFF_FFA0u32.to_le_bytes());
        vb2.extend(floats(&[TEX as f32 * x, TEX as f32 * y]));
    }
    m.write(0x8000_0000 | VB2_DATA, &vb2)?;
    write_words(m, VB2_HEADER, &[1 | 0x0100_0000, VB2_DATA, 0])?;

    // Combiner: rgb = t0 * c0, alpha = t0.a * v0.a.
    let mut def = PixelShaderDef {
        combiner_count: 1,
        texture_modes: texture_modes([TEX_PROJECT2D, 0, 0, 0]),
        ..PixelShaderDef::default()
    };
    def.rgb_inputs[0] = inputs(REG_T0, REG_C0, 0, 0);
    def.rgb_outputs[0] = outputs(REG_R0, 0, 0, 0);
    def.alpha_inputs[0] = inputs(REG_T0 | CHAN_ALPHA, REG_V0 | CHAN_ALPHA, 0, 0);
    def.alpha_outputs[0] = outputs(REG_R0, 0, 0, 0);
    def.constant0[0] = 0xFFFF_FFFF;
    m.write(PSDEF, &def.to_bytes())?;
    write_words(m, PS_OBJECT, &[1, 0, PSDEF])?;
    Ok(())
}

fn frame(d: &mut Device<GfxBackend>, m: &mut SparseMemory, n: u64) -> anyhow::Result<()> {
    d.set_render_state(m, 59, 0)?; // ALPHABLENDENABLE
    d.set_render_state(m, 60, 0)?; // ALPHATESTENABLE
    d.clear(m, 0, 0, 0xF1, 0xFF10_2040, 1.0, 0)?;

    // Program + combiner quad, sliding: alpha blend and alpha test on.
    let t = n as f32 / 60.0;
    let x = 192.0 + 160.0 * (t * 1.5).sin();
    let c = [256.0f32, 128.0, 0.0, 0.0, x, 60.0, 0.0, 1.0];
    m.write(CONSTS, &floats(&c))?;
    d.set_vertex_shader_constant(m, 96, CONSTS, 2, true)?;
    d.set_vertex_data(m, 3, [1.0, 1.0, 1.0, 1.0])?;
    d.select_vertex_shader_direct(m, VAF, 0)?;
    write_words(m, STREAM_INPUTS, &[VB_HEADER, 24, 0])?;
    d.set_vertex_shader_input(
        m,
        GuestLayout::HALO2.static_vertex_shader | 1,
        1,
        STREAM_INPUTS,
    )?;
    d.set_pixel_shader(m, PS_OBJECT)?;
    for (slot, v) in [
        (59, 1),
        (62, 0x302),
        (63, 0x303),
        (60, 1),
        (58, 0x204),
        (61, 0x30),
    ] {
        d.set_render_state(m, slot, v)?;
    }
    d.draw_vertices(m, 6, 0, 4)?; // TRIANGLESTRIP

    // Fixed-function quad, opaque.
    d.set_vertex_shader_input(m, 0, 0, 0)?;
    d.set_pixel_shader(m, 0)?;
    d.set_render_state(m, 59, 0)?;
    d.set_render_state(m, 60, 0)?;
    d.set_vertex_shader(m, fvf::XYZRHW | fvf::DIFFUSE | 0x100)?;
    d.set_stream_source(m, 0, VB2_HEADER, 28)?;
    d.draw_vertices(m, 6, 0, 4)?;

    d.swap(m, 0)?;
    for e in d.take_events() {
        if matches!(e, Event::DrawSkipped { .. } | Event::BadState(_)) {
            eprintln!("window: {e:?}");
        }
    }
    Ok(())
}

fn guest(mut host: GuestHost, frames: Option<u64>) -> anyhow::Result<()> {
    let backend = GfxBackend::new(host.gfx_context()).on_present(host.present_hook());
    let mut m = memory()?;
    let mut d = Device::new(DeviceConfig::halo2(PB, SCRATCH), backend);
    setup(&mut d, &mut m)?;
    let ticks_per_frame = 10_000_000 / 60;
    let mut n = 0u64;
    while host.tick(n * ticks_per_frame) == Control::Continue {
        frame(&mut d, &mut m, n)?;
        n += 1;
        if frames.is_some_and(|f| n >= f) {
            break;
        }
    }
    println!("window: {n} frames, {:?}", d.stats());
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let mut frames = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--frames" => frames = args.next().and_then(|v| v.parse().ok()),
            _ => anyhow::bail!("usage: window [--frames N]"),
        }
    }
    let config = AppConfig {
        title: "h2recomp d3d window".into(),
        ..AppConfig::default()
    };
    app::run(config, move |host| guest(host, frames))?;
    Ok(())
}
