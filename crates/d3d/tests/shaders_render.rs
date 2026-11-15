//! Draws through the device, `GfxBackend` and the default `Nv2aShaders`
//! provider on a real GPU, checking pixels: a UI-like quad (vertex program
//! with constants and a default vertex input, register-combiner pixel
//! shader, linear texture, alpha blending and alpha test) and a
//! fixed-function textured quad. Every shader and texture is synthetic.
//! Skips when no adapter is available.

mod common;

use common::*;
use d3d::{Event, GfxBackend, GuestMemory, SparseMemory, UploadPolicy};
use gfx::GfxContext;
use gfx::surface::HostPixelOrder;
use gfx::vertex::fvf;
use shaders::psh::PixelShaderDef;
use shaders::psh::bits::*;
use shaders::vsh::encode::{Inst, Operand, program};
use shaders::vsh::{MacOp, OutReg};

const VAF: u32 = MISC + 0x3000;
const STREAM_INPUTS: u32 = MISC + 0x3200;
const FUNCTION: u32 = MISC + 0x3400;
const PSDEF: u32 = MISC + 0x3800;
const PS_OBJECT: u32 = MISC + 0x3900;
const CONSTS: u32 = MISC + 0x3A00;

// Guest render-state slots (canonical numbering on 5849).
const RS_ALPHAFUNC: u32 = 58;
const RS_ALPHATESTENABLE: u32 = 60;
const RS_ALPHAREF: u32 = 61;
const RS_SRCBLEND: u32 = 62;
const RS_DESTBLEND: u32 = 63;

/// Texel colours of the 16x4 test texture, by column: 0..8 opaque red,
/// 8..12 green at alpha 0x80, 12..16 blue at alpha 0x10.
fn texel(x: u32) -> u32 {
    match x {
        0..8 => 0xFFFF_0000,
        8..12 => 0x8000_FF00,
        _ => 0x1000_00FF,
    }
}

/// A 16x4 `LIN_A8R8G8B8` texture at `TEX_HEADER` holding [`texel`].
fn ui_texture(m: &mut SparseMemory) {
    texture(m, 0);
    let bytes: Vec<u8> = (0..4)
        .flat_map(|_| (0..16).flat_map(|x| texel(x).to_le_bytes()))
        .collect();
    m.write(0x8000_0000 | TEX_DATA, &bytes).unwrap();
}

fn gpu() -> Option<(d3d::Device<GfxBackend>, SparseMemory, wgpu::ErrorScopeGuard)> {
    let ctx = match GfxContext::headless() {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            return None;
        }
    };
    let scope = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let (d, m) = create(GfxBackend::new(ctx), UploadPolicy::Tracked);
    Some((d, m, scope))
}

/// The back buffer as RGBA rows.
fn back_buffer(d: &mut d3d::Device<GfxBackend>) -> impl Fn(u32, u32) -> [u8; 4] + use<> {
    let (px, order) = d
        .backend
        .read_color(d.back_buffer().unwrap())
        .unwrap();
    move |x, y| {
        let p = pixel(&px, x, y);
        match order {
            HostPixelOrder::Bgra8 => [p[2], p[1], p[0], p[3]],
            _ => p,
        }
    }
}

fn close(got: [u8; 4], want: [u8; 4]) -> bool {
    got.iter()
        .zip(want)
        .all(|(g, w)| (*g as i32 - w as i32).abs() <= 2)
}

fn no_skips(d: &mut d3d::Device<GfxBackend>) {
    let events = d.take_events();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::DrawSkipped { .. } | Event::BadState(_))),
        "{events:?}"
    );
}

#[test]
fn ui_quad_with_program_combiner_blend_and_alpha_test() {
    let Some((mut d, mut m, scope)) = gpu() else {
        return;
    };
    set_states(&mut d, &mut m);
    d.clear(&mut m, 0, 0, 0xF0, 0xFFFF_FFFF, 1.0, 0).unwrap();

    // Vertex program: oPos = v0 * c0 + c1 (D3D numbering; hardware 96, 97),
    // oT0 = v9, oD0 = v3 (not in the stream: the SetVertexData value).
    let code = program(&[
        // One constant per instruction: mul then add (A + C).
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
    m.write(FUNCTION, &code).unwrap();
    d.load_vertex_shader_program(&mut m, FUNCTION, 0).unwrap();
    // Quad from (0, 16) to (64, 48).
    for (i, v) in [64.0f32, 32.0, 0.0, 0.0, 0.0, 16.0, 0.0, 1.0]
        .iter()
        .enumerate()
    {
        m.write_u32(CONSTS + 4 * i as u32, v.to_bits()).unwrap();
    }
    d.set_vertex_shader_constant(&mut m, 96, CONSTS, 2, true)
        .unwrap();
    d.set_vertex_data(&mut m, 3, [1.0, 1.0, 1.0, 1.0]).unwrap();

    // Attribute format: v0 FLOAT4 at 0, v9 FLOAT2 at 16; the rest NONE.
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
    d.select_vertex_shader_direct(&mut m, VAF, 0).unwrap();
    // Strip quad: unit square, texel-addressed coordinates (linear texture).
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
    d.set_vertex_shader_input(&mut m, handle, 1, STREAM_INPUTS)
        .unwrap();

    // Combiner: r0.rgb = t0 * c0 (c0 halves green), r0.a = t0.a * v0.a.
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
    d.set_pixel_shader(&mut m, PS_OBJECT).unwrap();

    ui_texture(&mut m);
    d.set_texture(&mut m, 0, TEX_HEADER).unwrap();
    for (slot, v) in [
        (RS_ALPHABLENDENABLE, 1),
        (RS_SRCBLEND, 0x302),  // SRCALPHA
        (RS_DESTBLEND, 0x303), // INVSRCALPHA
        (RS_ALPHATESTENABLE, 1),
        (RS_ALPHAFUNC, 0x204), // GREATER
        (RS_ALPHAREF, 0x20),
    ] {
        d.set_render_state(&mut m, slot, v).unwrap();
    }
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    no_skips(&mut d);

    let px = back_buffer(&mut d);
    assert_eq!(px(16, 32), [255, 0, 0, 255], "opaque red texel");
    // Green 0x80 tinted by c0 to 0x80, blended at alpha 0x80 over white.
    assert!(close(px(40, 32), [127, 191, 127, 191]), "{:?}", px(40, 32));
    assert_eq!(
        px(56, 32),
        [255, 255, 255, 255],
        "alpha 0x10 fails the test"
    );
    assert_eq!(px(16, 8), [255, 255, 255, 255], "outside the quad");
    assert_eq!(px(16, 48), [255, 255, 255, 255], "outside the quad");

    // Without the alpha test the faint blue texels blend in.
    d.set_render_state(&mut m, RS_ALPHATESTENABLE, 0).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    no_skips(&mut d);
    let px = back_buffer(&mut d);
    assert!(close(px(56, 32), [239, 239, 255, 240]), "{:?}", px(56, 32));

    let err = pollster::block_on(scope.pop());
    assert!(err.is_none(), "wgpu validation error: {err:?}");
}

#[test]
fn fixed_function_textured_quad() {
    let Some((mut d, mut m, scope)) = gpu() else {
        return;
    };
    set_states(&mut d, &mut m);
    d.clear(&mut m, 0, 0, 0xF0, 0xFF00_0000, 1.0, 0).unwrap();
    ui_texture(&mut m);
    d.set_texture(&mut m, 0, TEX_HEADER).unwrap();
    // XYZRHW | DIFFUSE | TEX1 over the whole target; texel coordinates.
    let mut vb = Vec::new();
    for (x, y) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        for f in [W as f32 * x, H as f32 * y, 0.5, 1.0] {
            vb.extend_from_slice(&f.to_le_bytes());
        }
        vb.extend_from_slice(&0xFF80_FFFFu32.to_le_bytes());
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
    // Default stage 0: COLOROP MODULATE(texture, diffuse), ALPHAOP
    // SELECTARG1(texture).
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    no_skips(&mut d);
    let px = back_buffer(&mut d);
    assert_eq!(px(16, 32), [128, 0, 0, 255]);
    assert_eq!(px(40, 32), [0, 255, 0, 0x80]);
    assert_eq!(px(56, 32), [0, 0, 255, 0x10]);

    // TFACTOR as the colour, diffuse alpha: no texture read at all.
    let l = d3d::GuestLayout::HALO2;
    m.write_u32(l.texture_state_slot(0, 14), 3).unwrap(); // COLORARG1 = TFACTOR
    m.write_u32(l.texture_state_slot(0, 12), 2).unwrap(); // COLOROP = SELECTARG1
    m.write_u32(l.texture_state_slot(0, 18), 0).unwrap(); // ALPHAARG1 = DIFFUSE
    d.set_render_state(&mut m, 148, 0xFF20_4060).unwrap(); // TEXTUREFACTOR
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    no_skips(&mut d);
    let px = back_buffer(&mut d);
    assert_eq!(px(5, 5), [0x20, 0x40, 0x60, 0xFF]);

    let err = pollster::block_on(scope.pop());
    assert!(err.is_none(), "wgpu validation error: {err:?}");
}

#[test]
fn untransformed_fixed_function_is_skipped_with_a_reason() {
    let Some((mut d, mut m, _scope)) = gpu() else {
        return;
    };
    set_states(&mut d, &mut m);
    m.write(0x8000_0000 | VB_DATA, &[0u8; 64]).unwrap();
    m.write_u32(VB_HEADER, 1 | 0x0100_0000).unwrap();
    m.write_u32(VB_HEADER + 4, VB_DATA).unwrap();
    d.set_vertex_shader(&mut m, fvf::XYZ | fvf::DIFFUSE)
        .unwrap();
    d.set_stream_source(&mut m, 0, VB_HEADER, 16).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    let events = d.take_events();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::DrawSkipped { reason, .. }
            if reason.contains("transform and lighting"))),
        "{events:?}"
    );
}

const RS_FOGENABLE: u32 = 92;
const RS_FOGTABLEMODE: u32 = 93;
const RS_FOGSTART: u32 = 94;
const RS_FOGEND: u32 = 95;
const RS_SPECULARENABLE: u32 = 103;
const RS_FOGCOLOR: u32 = 138;
const RS_TEXTUREFACTOR: u32 = 148;

/// Whether the provider has registered `n` programs (ids are sequential).
fn programs(d: &d3d::Device<GfxBackend>) -> u32 {
    (0..)
        .take_while(|n| {
            d.backend
                .renderer
                .has_shader(gfx::pipeline::ShaderId(d3d::nv2a::SHADER_ID_BASE + n))
        })
        .count() as u32
}

#[test]
fn untextured_fixed_function_beyond_diffuse_uses_the_provider() {
    let Some((mut d, mut m, scope)) = gpu() else {
        return;
    };
    set_states(&mut d, &mut m);
    // XYZRHW | DIFFUSE | SPECULAR over the whole target: half-transparent
    // red diffuse, green-ish specular whose alpha (the vertex fog) is 0.
    let mut vb = Vec::new();
    for (x, y) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        for f in [W as f32 * x, H as f32 * y, 0.5, 1.0] {
            vb.extend_from_slice(&f.to_le_bytes());
        }
        vb.extend_from_slice(&0x80FF_0000u32.to_le_bytes());
        vb.extend_from_slice(&0x0000_4000u32.to_le_bytes());
    }
    m.write(0x8000_0000 | VB_DATA, &vb).unwrap();
    m.write_u32(VB_HEADER, 1 | 0x0100_0000).unwrap();
    m.write_u32(VB_HEADER + 4, VB_DATA).unwrap();
    m.write_u32(VB_HEADER + 8, 0).unwrap();
    d.set_vertex_shader(&mut m, fvf::XYZRHW | fvf::DIFFUSE | fvf::SPECULAR)
        .unwrap();
    d.set_stream_source(&mut m, 0, VB_HEADER, 24).unwrap();
    let draw = |d: &mut d3d::Device<GfxBackend>, m: &mut SparseMemory| {
        d.clear(m, 0, 0, 0xF0, 0xFF00_0000, 1.0, 0).unwrap();
        d.draw_vertices(m, TRIANGLESTRIP, 0, 4).unwrap();
        no_skips(d);
        back_buffer(d)(5, 5)
    };

    // Default stages, no texture: diffuse passes through (the built-in
    // shader's case, no program translated).
    assert_eq!(draw(&mut d, &mut m), [255, 0, 0, 0x80]);
    assert_eq!(programs(&d), 0);

    // Specular add.
    d.set_render_state(&mut m, RS_SPECULARENABLE, 1).unwrap();
    assert_eq!(draw(&mut d, &mut m), [255, 0x40, 0, 0x80]);
    d.set_render_state(&mut m, RS_SPECULARENABLE, 0).unwrap();

    // Vertex fog: factor 0 is all fog colour.
    d.set_render_state(&mut m, RS_FOGCOLOR, 0xFF00_00FF)
        .unwrap();
    d.set_render_state(&mut m, RS_FOGENABLE, 1).unwrap();
    assert_eq!(draw(&mut d, &mut m), [0, 0, 255, 0x80]);
    d.set_render_state(&mut m, RS_FOGENABLE, 0).unwrap();

    // TFACTOR as the colour.
    let l = d3d::GuestLayout::HALO2;
    m.write_u32(l.texture_state_slot(0, 14), 3).unwrap(); // COLORARG1 = TFACTOR
    m.write_u32(l.texture_state_slot(0, 12), 2).unwrap(); // COLOROP = SELECTARG1
    d.set_render_state(&mut m, RS_TEXTUREFACTOR, 0xFF20_4060)
        .unwrap();
    assert_eq!(draw(&mut d, &mut m), [0x20, 0x40, 0x60, 0x80]);
    let translated = programs(&d);
    assert_eq!(translated, 3);

    // Another TFACTOR is a uniform: same program.
    d.set_render_state(&mut m, RS_TEXTUREFACTOR, 0xFF60_4020)
        .unwrap();
    assert_eq!(draw(&mut d, &mut m), [0x60, 0x40, 0x20, 0x80]);
    assert_eq!(programs(&d), translated, "constant-only change reuses");

    let err = pollster::block_on(scope.pop());
    assert!(err.is_none(), "wgpu validation error: {err:?}");
}

/// Table fog on pre-transformed vertices takes its coordinate where D3D's
/// pass-through program does (`0x3f6f40`): z with an affine projection
/// (device flag 2), w = 1/rhw otherwise, and the specular alpha without a
/// table. Linear fog from 0 to 8 over red, with blue fog: z = 0.5 gives
/// factor 0.9375, w = 4 gives 0.5, and the specular alpha 0 gives 0.
#[test]
fn pretransformed_table_fog_reads_z_or_w() {
    let Some((mut d, mut m, scope)) = gpu() else {
        return;
    };
    set_states(&mut d, &mut m);
    let mut vb = Vec::new();
    for (x, y) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        for f in [W as f32 * x, H as f32 * y, 0.5, 0.25] {
            vb.extend_from_slice(&f.to_le_bytes());
        }
        vb.extend_from_slice(&0x80FF_0000u32.to_le_bytes());
        vb.extend_from_slice(&0x0000_0000u32.to_le_bytes());
    }
    m.write(0x8000_0000 | VB_DATA, &vb).unwrap();
    m.write_u32(VB_HEADER, 1 | 0x0100_0000).unwrap();
    m.write_u32(VB_HEADER + 4, VB_DATA).unwrap();
    m.write_u32(VB_HEADER + 8, 0).unwrap();
    d.set_vertex_shader(&mut m, fvf::XYZRHW | fvf::DIFFUSE | fvf::SPECULAR)
        .unwrap();
    d.set_stream_source(&mut m, 0, VB_HEADER, 24).unwrap();
    let draw = |d: &mut d3d::Device<GfxBackend>, m: &mut SparseMemory| {
        d.clear(m, 0, 0, 0xF0, 0xFF00_0000, 1.0, 0).unwrap();
        d.draw_vertices(m, TRIANGLESTRIP, 0, 4).unwrap();
        no_skips(d);
        back_buffer(d)(5, 5)
    };
    let flags = d3d::GuestLayout::HALO2.device + 0x8;
    let set_affine = |m: &mut SparseMemory, on: bool| {
        let f = m.read_u32(flags).unwrap();
        m.write_u32(flags, if on { f | 2 } else { f & !2 }).unwrap();
    };
    d.set_render_state(&mut m, RS_FOGCOLOR, 0xFF00_00FF)
        .unwrap();
    d.set_render_state(&mut m, RS_FOGENABLE, 1).unwrap();
    d.set_render_state(&mut m, RS_FOGSTART, 0.0f32.to_bits())
        .unwrap();
    d.set_render_state(&mut m, RS_FOGEND, 8.0f32.to_bits())
        .unwrap();

    // No table: the specular alpha (0) is the factor.
    set_affine(&mut m, false);
    let got = draw(&mut d, &mut m);
    assert!(close(got, [0, 0, 255, 0x80]), "vertex fog: {got:?}");
    // Linear table, perspective: w = 4.
    d.set_render_state(&mut m, RS_FOGTABLEMODE, 3).unwrap();
    let got = draw(&mut d, &mut m);
    assert!(close(got, [128, 0, 127, 0x80]), "w: {got:?}");
    // Linear table, affine: z = 0.5.
    set_affine(&mut m, true);
    let got = draw(&mut d, &mut m);
    assert!(close(got, [239, 0, 16, 0x80]), "z: {got:?}");
    // FOGENABLE off: no fog whatever the table says.
    d.set_render_state(&mut m, RS_FOGENABLE, 0).unwrap();
    let got = draw(&mut d, &mut m);
    assert!(close(got, [255, 0, 0, 0x80]), "fog off: {got:?}");

    let err = pollster::block_on(scope.pop());
    assert!(err.is_none(), "wgpu validation error: {err:?}");
}

#[test]
fn program_reads_set_vertex_data_and_fog_follows_fogenable() {
    let Some((mut d, mut m, scope)) = gpu() else {
        return;
    };
    set_states(&mut d, &mut m);
    // oPos = v0 (window coordinates), oD0 = v3 (not in the stream: the
    // SetVertexData value), oFog = c[96].x (D3D numbering; 0: all fog).
    let code = program(&[
        Inst::mac(MacOp::Mov, Some(Operand::v(0)), None, None).to_out(false, OutReg::Pos, 0xF),
        Inst::mac(MacOp::Mov, Some(Operand::c(96).swz("x")), None, None).to_out(
            false,
            OutReg::Fog,
            8,
        ),
        Inst::mac(MacOp::Mov, Some(Operand::v(3)), None, None)
            .to_out(false, OutReg::D0, 0xF)
            .last(),
    ]);
    m.write(FUNCTION, &code).unwrap();
    d.load_vertex_shader_program(&mut m, FUNCTION, 0).unwrap();
    for i in 0..4u32 {
        m.write_u32(CONSTS + 4 * i, 0).unwrap();
    }
    d.set_vertex_shader_constant(&mut m, 96, CONSTS, 1, true)
        .unwrap();
    d.set_vertex_data(&mut m, 3, [0.25, 0.5, 0.75, 1.0])
        .unwrap();
    for reg in 0..16u32 {
        let format = if reg == 0 { 0x42 } else { 0x02 };
        for (j, w) in [0, 0, format, 0].iter().enumerate() {
            m.write_u32(VAF + 16 * reg + 4 * j as u32, *w).unwrap();
        }
    }
    d.select_vertex_shader_direct(&mut m, VAF, 0).unwrap();
    let mut vb = Vec::new();
    for (x, y) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        for f in [W as f32 * x, H as f32 * y, 0.0, 1.0] {
            vb.extend_from_slice(&f.to_le_bytes());
        }
    }
    m.write(0x8000_0000 | VB_DATA, &vb).unwrap();
    m.write_u32(VB_HEADER, 1 | 0x0100_0000).unwrap();
    m.write_u32(VB_HEADER + 4, VB_DATA).unwrap();
    m.write_u32(VB_HEADER + 8, 0).unwrap();
    for (i, w) in [VB_HEADER, 16, 0].iter().enumerate() {
        m.write_u32(STREAM_INPUTS + 4 * i as u32, *w).unwrap();
    }
    let handle = d3d::GuestLayout::HALO2.static_vertex_shader | 1;
    d.set_vertex_shader_input(&mut m, handle, 1, STREAM_INPUTS)
        .unwrap();

    // r0 = v0; final combiner: lerp(fog.a, r0, fog.rgb), alpha r0.a.
    let mut def = PixelShaderDef {
        combiner_count: 1,
        ..PixelShaderDef::default()
    };
    def.rgb_inputs[0] = inputs(REG_V0, ONE, 0, 0);
    def.rgb_outputs[0] = outputs(REG_R0, 0, 0, 0);
    def.alpha_inputs[0] = inputs(REG_V0 | CHAN_ALPHA, ONE | CHAN_ALPHA, 0, 0);
    def.alpha_outputs[0] = outputs(REG_R0, 0, 0, 0);
    def.final_inputs_abcd = inputs(REG_FOG | CHAN_ALPHA, REG_R0, REG_FOG, REG_ZERO);
    def.final_inputs_efg = final_efg(0, 0, REG_R0 | CHAN_ALPHA, 0);
    m.write(PSDEF, &def.to_bytes()).unwrap();
    for (i, w) in [1, 0, PSDEF].iter().enumerate() {
        m.write_u32(PS_OBJECT + 4 * i as u32, *w).unwrap();
    }
    d.set_pixel_shader(&mut m, PS_OBJECT).unwrap();
    d.set_render_state(&mut m, RS_FOGCOLOR, 0xFF00_FF00)
        .unwrap();

    let draw = |d: &mut d3d::Device<GfxBackend>, m: &mut SparseMemory| {
        d.clear(m, 0, 0, 0xF0, 0xFF00_0000, 1.0, 0).unwrap();
        d.draw_vertices(m, TRIANGLESTRIP, 0, 4).unwrap();
        no_skips(d);
        back_buffer(d)(20, 20)
    };
    // Fog off: the program's fog register reads factor 1.
    let want = [64, 128, 191, 255];
    assert!(close(draw(&mut d, &mut m), want), "{want:?}");
    // Fog on, factor from the vertex (0): fog colour.
    d.set_render_state(&mut m, RS_FOGENABLE, 1).unwrap();
    assert_eq!(draw(&mut d, &mut m), [0, 255, 0, 255]);
    // Linear table fog with the fog coordinate (0) before the start: none.
    d.set_render_state(&mut m, RS_FOGTABLEMODE, 3).unwrap();
    d.set_render_state(&mut m, RS_FOGSTART, 1.0f32.to_bits())
        .unwrap();
    d.set_render_state(&mut m, RS_FOGEND, 2.0f32.to_bits())
        .unwrap();
    assert!(close(draw(&mut d, &mut m), want));
    assert_eq!(programs(&d), 1, "fog state is uniforms");

    let err = pollster::block_on(scope.pop());
    assert!(err.is_none(), "wgpu validation error: {err:?}");
}

/// Whether the final combiner runs comes from device `+0x788`, latched by
/// `SetPixelShader` from the definition it binds, not from the definition's
/// buffer at the draw. These set up a red `r0 = v0` quad for them.
fn latch_setup(d: &mut d3d::Device<GfxBackend>, m: &mut SparseMemory) {
    set_states(d, m);
    // oPos = v0 (window coordinates), oD0 = v3 (SetVertexData: red).
    let code = program(&[
        Inst::mac(MacOp::Mov, Some(Operand::v(0)), None, None).to_out(false, OutReg::Pos, 0xF),
        Inst::mac(MacOp::Mov, Some(Operand::v(3)), None, None)
            .to_out(false, OutReg::D0, 0xF)
            .last(),
    ]);
    m.write(FUNCTION, &code).unwrap();
    d.load_vertex_shader_program(m, FUNCTION, 0).unwrap();
    d.set_vertex_data(m, 3, [1.0, 0.0, 0.0, 1.0]).unwrap();
    for reg in 0..16u32 {
        let format = if reg == 0 { 0x42 } else { 0x02 };
        for (j, w) in [0, 0, format, 0].iter().enumerate() {
            m.write_u32(VAF + 16 * reg + 4 * j as u32, *w).unwrap();
        }
    }
    d.select_vertex_shader_direct(m, VAF, 0).unwrap();
    let mut vb = Vec::new();
    for (x, y) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        for f in [W as f32 * x, H as f32 * y, 0.0, 1.0] {
            vb.extend_from_slice(&f.to_le_bytes());
        }
    }
    m.write(0x8000_0000 | VB_DATA, &vb).unwrap();
    m.write_u32(VB_HEADER, 1 | 0x0100_0000).unwrap();
    m.write_u32(VB_HEADER + 4, VB_DATA).unwrap();
    m.write_u32(VB_HEADER + 8, 0).unwrap();
    for (i, w) in [VB_HEADER, 16, 0].iter().enumerate() {
        m.write_u32(STREAM_INPUTS + 4 * i as u32, *w).unwrap();
    }
    let handle = d3d::GuestLayout::HALO2.static_vertex_shader | 1;
    d.set_vertex_shader_input(m, handle, 1, STREAM_INPUTS)
        .unwrap();
    for (i, w) in [1, 0, PSDEF].iter().enumerate() {
        m.write_u32(PS_OBJECT + 4 * i as u32, *w).unwrap();
    }
}

/// `r0 = v0`; with `final_combiner`, rgb = D = r0 and alpha = G = r0.a.
fn latch_def(final_combiner: bool) -> PixelShaderDef {
    let mut def = PixelShaderDef {
        combiner_count: 1,
        ..PixelShaderDef::default()
    };
    def.rgb_inputs[0] = inputs(REG_V0, ONE, 0, 0);
    def.rgb_outputs[0] = outputs(REG_R0, 0, 0, 0);
    def.alpha_inputs[0] = inputs(REG_V0 | CHAN_ALPHA, ONE | CHAN_ALPHA, 0, 0);
    def.alpha_outputs[0] = outputs(REG_R0, 0, 0, 0);
    if final_combiner {
        def.final_inputs_abcd = inputs(0, 0, 0, REG_R0);
        def.final_inputs_efg = final_efg(0, 0, REG_R0 | CHAN_ALPHA, 0);
    }
    def
}

/// Zero in both final-combiner slots of the render-state mirror (what
/// pushing zero to `SET_COMBINER_SPECULAR_FOG_CW0/1` leaves there).
fn zero_final_slots(m: &mut SparseMemory) {
    for slot in [8u32, 9] {
        m.write_u32(d3d::GuestLayout::HALO2.render_state_slot(slot), 0)
            .unwrap();
    }
}

fn latch(m: &SparseMemory) -> u32 {
    m.read_u32(d3d::GuestLayout::HALO2.device + 0x788).unwrap()
}

fn latch_draw(d: &mut d3d::Device<GfxBackend>, m: &mut SparseMemory) -> [u8; 4] {
    d.clear(m, 0, 0, 0xF0, 0xFF00_00FF, 1.0, 0).unwrap();
    d.draw_vertices(m, TRIANGLESTRIP, 0, 4).unwrap();
    no_skips(d);
    back_buffer(d)(20, 20)
}

/// Bound with a final combiner, then zero in both final slots: the
/// hardware runs an all-zero final combiner (black, alpha 0). Clearing the
/// buffer's final words afterwards, without rebinding, keeps it so.
#[test]
fn final_combiner_latched_present_survives_zero_pushes_and_buffer_reuse() {
    let Some((mut d, mut m, scope)) = gpu() else {
        return;
    };
    latch_setup(&mut d, &mut m);
    m.write(PSDEF, &latch_def(true).to_bytes()).unwrap();
    d.set_pixel_shader(&mut m, PS_OBJECT).unwrap();
    assert_eq!(latch_draw(&mut d, &mut m), [255, 0, 0, 255]);
    zero_final_slots(&mut m);
    assert_eq!(
        latch_draw(&mut d, &mut m),
        [0, 0, 0, 0],
        "explicit zero final"
    );
    m.write(PSDEF, &latch_def(false).to_bytes()).unwrap();
    assert_ne!(latch(&m), 0);
    assert_eq!(latch_draw(&mut d, &mut m), [0, 0, 0, 0], "latch kept");
    assert!(pollster::block_on(scope.pop()).is_none());
}

/// Bound without a final combiner: D3D's default one (`r0`, no fog), even
/// after the buffer is given a final combiner without rebinding.
#[test]
fn final_combiner_latched_absent_ignores_buffer_reuse() {
    let Some((mut d, mut m, scope)) = gpu() else {
        return;
    };
    latch_setup(&mut d, &mut m);
    m.write(PSDEF, &latch_def(false).to_bytes()).unwrap();
    d.set_pixel_shader(&mut m, PS_OBJECT).unwrap();
    assert_eq!(latch(&m), 0);
    assert_eq!(latch_draw(&mut d, &mut m), [255, 0, 0, 255]);
    m.write(PSDEF, &latch_def(true).to_bytes()).unwrap();
    zero_final_slots(&mut m);
    assert_eq!(latch(&m), 0);
    assert_eq!(
        latch_draw(&mut d, &mut m),
        [255, 0, 0, 255],
        "default final"
    );
    assert!(pollster::block_on(scope.pop()).is_none());
}

/// Draws `quads` (window x0, x1, z, w, RGBA each; full height) with
/// `oPos = v0, oD0 = v3`, depth tested LessEqual and written, after a
/// clear to black and depth 1, and returns the back buffer.
fn depth_quads(
    d: &mut d3d::Device<GfxBackend>,
    m: &mut SparseMemory,
    quads: &[(f32, f32, f32, f32, [f32; 4])],
) -> impl Fn(u32, u32) -> [u8; 4] + use<> {
    let code = program(&[
        Inst::mac(MacOp::Mov, Some(Operand::v(0)), None, None).to_out(false, OutReg::Pos, 0xF),
        Inst::mac(MacOp::Mov, Some(Operand::v(3)), None, None)
            .to_out(false, OutReg::D0, 0xF)
            .last(),
    ]);
    m.write(FUNCTION, &code).unwrap();
    d.load_vertex_shader_program(m, FUNCTION, 0).unwrap();
    for reg in 0..16u32 {
        let (offset, format) = match reg {
            0 => (0, 0x42),
            3 => (16, 0x42),
            _ => (0, 0x02),
        };
        for (j, w) in [0, offset, format, 0].iter().enumerate() {
            m.write_u32(VAF + 16 * reg + 4 * j as u32, *w).unwrap();
        }
    }
    d.select_vertex_shader_direct(m, VAF, 0).unwrap();
    let mut vb = Vec::new();
    for &(x0, x1, z, w, rgba) in quads {
        for (x, y) in [(x0, 0.0f32), (x1, 0.0), (x0, H as f32), (x1, 0.0), (x1, H as f32), (x0, H as f32)] {
            for f in [x, y, z, w].into_iter().chain(rgba) {
                vb.extend_from_slice(&f.to_le_bytes());
            }
        }
    }
    m.write(0x8000_0000 | VB_DATA, &vb).unwrap();
    m.write_u32(VB_HEADER, 1 | 0x0100_0000).unwrap();
    m.write_u32(VB_HEADER + 4, VB_DATA).unwrap();
    m.write_u32(VB_HEADER + 8, 0).unwrap();
    for (i, w) in [VB_HEADER, 32, 0].iter().enumerate() {
        m.write_u32(STREAM_INPUTS + 4 * i as u32, *w).unwrap();
    }
    let handle = d3d::GuestLayout::HALO2.static_vertex_shader | 1;
    d.set_vertex_shader_input(m, handle, 1, STREAM_INPUTS).unwrap();
    let mut def = PixelShaderDef {
        combiner_count: 1,
        ..PixelShaderDef::default()
    };
    def.rgb_inputs[0] = inputs(REG_V0, ONE, 0, 0);
    def.rgb_outputs[0] = outputs(REG_R0, 0, 0, 0);
    def.alpha_inputs[0] = inputs(REG_V0 | CHAN_ALPHA, ONE | CHAN_ALPHA, 0, 0);
    def.alpha_outputs[0] = outputs(REG_R0, 0, 0, 0);
    m.write(PSDEF, &def.to_bytes()).unwrap();
    for (i, w) in [1, 0, PSDEF].iter().enumerate() {
        m.write_u32(PS_OBJECT + 4 * i as u32, *w).unwrap();
    }
    d.set_pixel_shader(m, PS_OBJECT).unwrap();
    d.set_render_state(m, RS_ZFUNC, 0x203).unwrap(); // LESSEQUAL
    d.set_render_state(m, 64, 1).unwrap(); // ZWRITEENABLE
    d.clear(m, 0, 0, 0xF3, 0xFF00_0000, 1.0, 0).unwrap();
    d.draw_vertices(m, 5, 0, 6 * quads.len() as u32).unwrap(); // TRIANGLELIST
    no_skips(d);
    back_buffer(d)
}

/// Halo runs W-buffered (`ZEnable(2)` at boot): the NV2A tests and stores
/// the interpolated w, not the program's z. Its first-person models use
/// another projection than the level, so their z does not order against
/// the level's, but their w (the distance) does. With z, a weapon at
/// w = 1 drawn after a wall at w = 10 lost to the wall wherever its z was
/// larger: the see-through gun.
#[test]
fn w_buffered_programs_depth_test_w_not_z() {
    let Some((mut d, mut m, scope)) = gpu() else {
        return;
    };
    set_states(&mut d, &mut m);
    // SetRenderState_ZEnable(D3DZB_USEW).
    call(&mut d, &mut m, 0x3f54f0, d3d::abi::Regs::default(), &[2]).unwrap();
    const D24: f32 = 16_777_215.0;
    let green = [0.0, 1.0, 0.0, 1.0];
    let red = [1.0, 0.0, 0.0, 1.0];
    let px = depth_quads(
        &mut d,
        &mut m,
        &[
            // The wall: far (w = 10) but small z.
            (0.0, W as f32, 0.2 * D24, 10.0, green),
            // The weapon: near (w = 1), larger z, in front on the left.
            (0.0, 32.0, 0.9 * D24, 1.0, red),
            // Near (w = 2) with z past the depth maximum: z does not clip.
            (32.0, 48.0, 2.0 * D24, 2.0, red),
        ],
    );
    assert_eq!(px(16, 32), [255, 0, 0, 255], "the nearer w wins");
    assert_eq!(px(40, 32), [255, 0, 0, 255], "z past the maximum is not clipped");
    assert_eq!(px(56, 32), [0, 255, 0, 255]);
    // Z-buffered, the same quads order by z (the wall wins on the left)
    // and z past the maximum clips.
    call(&mut d, &mut m, 0x3f54f0, d3d::abi::Regs::default(), &[1]).unwrap();
    let px = depth_quads(
        &mut d,
        &mut m,
        &[
            (0.0, W as f32, 0.2 * D24, 10.0, green),
            (0.0, 32.0, 0.9 * D24, 1.0, red),
            (32.0, 48.0, 2.0 * D24, 2.0, red),
        ],
    );
    assert_eq!(px(16, 32), [0, 255, 0, 255]);
    assert_eq!(px(40, 32), [0, 255, 0, 255]);

    let err = pollster::block_on(scope.pop());
    assert!(err.is_none(), "wgpu validation error: {err:?}");
}
