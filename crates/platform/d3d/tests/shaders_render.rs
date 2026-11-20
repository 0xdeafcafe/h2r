//! Draws through the device, `GfxBackend` and the default `Nv2aShaders`
//! provider on a real GPU, checking pixels: a UI-like quad (vertex program
//! with constants and a default vertex input, register-combiner pixel
//! shader, linear texture, alpha blending and alpha test) and a
//! fixed-function textured quad. Every shader and texture is synthetic.
//! Skips when no adapter is available.

use crate::common::*;
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

/// `oPos = v0, oD0 = v3`.
fn plain_program() -> Vec<u8> {
    program(&[
        Inst::mac(MacOp::Mov, Some(Operand::v(0)), None, None).to_out(false, OutReg::Pos, 0xF),
        Inst::mac(MacOp::Mov, Some(Operand::v(3)), None, None)
            .to_out(false, OutReg::D0, 0xF)
            .last(),
    ])
}

/// Draws a triangle list of `verts` (v0 = window x, y, z, w; v3 = RGBA)
/// with vertex program `code` and `r0 = v0` combiners, depth function
/// `zfunc` (a `D3DCMP_*`) and depth writes `zwrite`. No clear.
fn draw_pass(
    d: &mut d3d::Device<GfxBackend>,
    m: &mut SparseMemory,
    code: &[u8],
    verts: &[[f32; 8]],
    zfunc: u32,
    zwrite: bool,
) {
    let mut def = PixelShaderDef {
        combiner_count: 1,
        ..PixelShaderDef::default()
    };
    def.rgb_inputs[0] = inputs(REG_V0, ONE, 0, 0);
    def.rgb_outputs[0] = outputs(REG_R0, 0, 0, 0);
    def.alpha_inputs[0] = inputs(REG_V0 | CHAN_ALPHA, ONE | CHAN_ALPHA, 0, 0);
    def.alpha_outputs[0] = outputs(REG_R0, 0, 0, 0);
    draw_pass_with(d, m, code, verts, zfunc, zwrite, &def);
}

/// [`draw_pass`] with pixel shader `def`.
fn draw_pass_with(
    d: &mut d3d::Device<GfxBackend>,
    m: &mut SparseMemory,
    code: &[u8],
    verts: &[[f32; 8]],
    zfunc: u32,
    zwrite: bool,
    def: &PixelShaderDef,
) {
    m.write(FUNCTION, code).unwrap();
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
    let vb: Vec<u8> = verts.iter().flatten().flat_map(|f| f.to_le_bytes()).collect();
    m.write(0x8000_0000 | VB_DATA, &vb).unwrap();
    m.write_u32(VB_HEADER, 1 | 0x0100_0000).unwrap();
    m.write_u32(VB_HEADER + 4, VB_DATA).unwrap();
    m.write_u32(VB_HEADER + 8, 0).unwrap();
    for (i, w) in [VB_HEADER, 32, 0].iter().enumerate() {
        m.write_u32(STREAM_INPUTS + 4 * i as u32, *w).unwrap();
    }
    let handle = d3d::GuestLayout::HALO2.static_vertex_shader | 1;
    d.set_vertex_shader_input(m, handle, 1, STREAM_INPUTS).unwrap();
    m.write(PSDEF, &def.to_bytes()).unwrap();
    for (i, w) in [1, 0, PSDEF].iter().enumerate() {
        m.write_u32(PS_OBJECT + 4 * i as u32, *w).unwrap();
    }
    d.set_pixel_shader(m, PS_OBJECT).unwrap();
    d.set_render_state(m, RS_ZFUNC, zfunc).unwrap();
    d.set_render_state(m, 64, zwrite as u32).unwrap(); // ZWRITEENABLE
    d.draw_vertices(m, 5, 0, verts.len() as u32).unwrap(); // TRIANGLELIST
    no_skips(d);
}

/// Draws `quads` (window x0, x1, z, w, RGBA each; full height) with
/// `oPos = v0, oD0 = v3`, depth tested LessEqual and written, after a
/// clear to black and depth 1, and returns the back buffer.
fn depth_quads(
    d: &mut d3d::Device<GfxBackend>,
    m: &mut SparseMemory,
    quads: &[(f32, f32, f32, f32, [f32; 4])],
) -> impl Fn(u32, u32) -> [u8; 4] + use<> {
    let mut verts = Vec::new();
    for &(x0, x1, z, w, [r, g, b, a]) in quads {
        for (x, y) in [(x0, 0.0f32), (x1, 0.0), (x0, H as f32), (x1, 0.0), (x1, H as f32), (x0, H as f32)] {
            verts.push([x, y, z, w, r, g, b, a]);
        }
    }
    d.clear(m, 0, 0, 0xF3, 0xFF00_0000, 1.0, 0).unwrap();
    draw_pass(d, m, &plain_program(), &verts, 0x203, true); // LESSEQUAL
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

/// A W-buffered program drawn to a colour target without a depth surface
/// (Ascension draws some): the program must not write depth, since a
/// pipeline whose fragment writes depth without a depth attachment fails
/// validation and wgpu panicked on it in `set_pipeline`. It still draws,
/// clipped by w.
#[test]
fn w_buffered_program_to_a_colour_only_target_draws() {
    let Some((mut d, mut m, scope)) = gpu() else {
        return;
    };
    set_states(&mut d, &mut m);
    let back = m
        .read_u32(d3d::GuestLayout::HALO2.device + d3d::layout::device_field::RENDER_TARGET)
        .unwrap();
    d.set_render_target(&mut m, back, 0).unwrap();
    call(&mut d, &mut m, 0x3f54f0, d3d::abi::Regs::default(), &[2]).unwrap();
    let red = [1.0, 0.0, 0.0, 1.0];
    let px = depth_quads(&mut d, &mut m, &[(0.0, 32.0, 0.5 * 16_777_215.0, 1.0, red)]);
    assert_eq!(px(16, 32), [255, 0, 0, 255]);
    assert_eq!(px(48, 32), [0, 0, 0, 255]);

    let err = pollster::block_on(scope.pop());
    assert!(err.is_none(), "wgpu validation error: {err:?}");
}

/// Halo draws a surface in several passes with different vertex programs:
/// the first writes depth, later ones test it with EQUAL. Programs that
/// compute w by different instructions can differ in the last bit or two.
/// The NV2A stores W-buffered depth as an integer (D24: the whole part of
/// w), which absorbs that, so the later passes cover the first; stored
/// as a float, every ulp fails EQUAL: the likely source of the stripes and
/// dither on Ascension's tower and the Banshee. Here the second program scales w by
/// 1 + 2^-22 (two ulps); x and y are window coordinates, so coverage is
/// unchanged. w runs from 100 to 5000 depth units across the triangles.
#[test]
fn w_buffered_passes_with_ulp_different_w_store_equal_depth() {
    let Some((mut d, mut m, scope)) = gpu() else {
        return;
    };
    set_states(&mut d, &mut m);
    call(&mut d, &mut m, 0x3f54f0, d3d::abi::Regs::default(), &[2]).unwrap();
    let corner = |x: f32, y: f32, w: f32| [x, y, 0.0, w, 1.0, 0.0, 0.0, 1.0];
    let (a, b, c, e) = (
        corner(1.0, 2.0, 100.3),
        corner(63.0, 1.0, 1_723.7),
        corner(2.0, 62.0, 911.1),
        corner(62.5, 63.0, 5_000.0),
    );
    let verts = [a, b, c, b, e, c];
    // c[96] = (1, 1, 1, 1 + 2^-22).
    let scale = [1.0f32, 1.0, 1.0, 1.0 + f32::EPSILON * 2.0];
    for (i, f) in scale.iter().enumerate() {
        m.write_u32(CONSTS + 4 * i as u32, f.to_bits()).unwrap();
    }
    d.set_vertex_shader_constant(&mut m, 96, CONSTS, 1, true).unwrap();
    // The position through a temporary and a multiply, another output
    // around it, and the colour's red and green swapped.
    let second = program(&[
        Inst::mac(MacOp::Mov, Some(Operand::v(3)), None, None).to_temp(2, 0xF),
        Inst::mac(MacOp::Mul, Some(Operand::v(0)), Some(Operand::c(96)), None).to_temp(0, 0xF),
        Inst::mac(MacOp::Mul, Some(Operand::v(0)), Some(Operand::r(2)), None)
            .to_out(false, OutReg::T(0), 0xF),
        Inst::mac(MacOp::Mov, Some(Operand::r(0)), None, None).to_out(false, OutReg::Pos, 0xF),
        Inst::mac(MacOp::Mov, Some(Operand::r(2).swz("yxzw")), None, None)
            .to_out(false, OutReg::D0, 0xF)
            .last(),
    ]);
    d.clear(&mut m, 0, 0, 0xF3, 0xFF00_0000, 1.0, 0).unwrap();
    draw_pass(&mut d, &mut m, &plain_program(), &verts, 0x203, true); // LESSEQUAL
    draw_pass(&mut d, &mut m, &second, &verts, 0x202, false); // EQUAL
    let px = back_buffer(&mut d);
    let (mut red, mut green) = (0, 0);
    for y in 0..H {
        for x in 0..W {
            match px(x, y) {
                [255, 0, 0, 255] => red += 1,
                [0, 255, 0, 255] => green += 1,
                _ => {}
            }
        }
    }
    assert!(red + green > 3000, "{} pixels drawn", red + green);
    // Only w within two ulps of a whole number can land on either side.
    assert!(red * 100 < red + green, "{red} of {} pixels failed the EQUAL pass", red + green);

    let err = pollster::block_on(scope.pop());
    assert!(err.is_none(), "wgpu validation error: {err:?}");
}

/// `D3DDCC_DEPTHCLAMP` in `D3DRS_DEPTHCLIPCONTROL` (`SET_ZMIN_MAX_CONTROL`
/// clamp mode): depth past the clip range is clamped to it, not culled.
/// Ascension's black sky is the likely case: sky past the far clip,
/// culled.
#[test]
fn w_buffered_depth_clamp_keeps_what_lies_past_the_clip_range() {
    let Some((mut d, mut m, scope)) = gpu() else {
        return;
    };
    set_states(&mut d, &mut m);
    const D24: f32 = 16_777_215.0;
    // The W-buffer parameters a projection with a near plane gives, so D3D
    // pushes CLIP_MIN/MAX = [1/WFar * WNear * D24, D24] = [1000, D24].
    use d3d::layout::device_field as f;
    let dev = d3d::GuestLayout::HALO2.device;
    m.write_u32(dev + f::INV_W_FAR, 1.0f32.to_bits()).unwrap();
    m.write_u32(dev + f::W_NEAR, (1000.0 / D24).to_bits()).unwrap();
    call(&mut d, &mut m, 0x3f54f0, d3d::abi::Regs::default(), &[2]).unwrap();
    let red = [1.0, 0.0, 0.0, 1.0];
    let green = [0.0, 1.0, 0.0, 1.0];
    // Culled (the default): the far quad is gone.
    d.set_render_state(&mut m, 82, 0x01).unwrap();
    let px = depth_quads(&mut d, &mut m, &[(0.0, W as f32, 0.0, 4.0 * D24, red)]);
    assert_eq!(px(32, 32), [0, 0, 0, 255]);
    // Clamped: it draws, at the far end of the range, so a nearer quad
    // drawn after it still passes LessEqual.
    d.set_render_state(&mut m, 82, 0x10).unwrap();
    let px = depth_quads(
        &mut d,
        &mut m,
        &[(0.0, W as f32, 0.0, 4.0 * D24, red), (0.0, 32.0, 0.0, 0.5 * D24, green)],
    );
    assert_eq!(px(48, 32), [255, 0, 0, 255]);
    assert_eq!(px(16, 32), [0, 255, 0, 255]);

    let err = pollster::block_on(scope.pop());
    assert!(err.is_none(), "wgpu validation error: {err:?}");
}

/// Halo's lens-flare occlusion test (the callback `0x2e540` that
/// `0x20bb0` runs between `BeginVisibilityTest` and `EndVisibilityTest`):
/// a screen-space quad around the flare drawn through a vertex program,
/// W-buffered, depth tested LessEqual without writes, its passing samples
/// divided by the quad's area. The scene's depth holds w (the W-buffered
/// programs write it from the fragment stage), so the count pass must test
/// w too. Testing the rasterized z (here `0.9 * D24`, above every stored
/// w) counted nothing in front of geometry, so flares there never showed:
/// the Armory's look-test light (`looker_light_red`, a lens flare on a
/// model-less device) stayed dark.
#[test]
fn w_buffered_visibility_test_counts_against_the_stored_w() {
    let Some((mut d, mut m, scope)) = gpu() else {
        return;
    };
    set_states(&mut d, &mut m);
    call(&mut d, &mut m, 0x3f54f0, d3d::abi::Regs::default(), &[2]).unwrap();
    const D24: f32 = 16_777_215.0;
    const DEV: u32 = 0x0040_4FE0;
    let green = [0.0, 1.0, 0.0, 1.0];
    // The wall: w = 10, z small.
    let _ = depth_quads(&mut d, &mut m, &[(0.0, W as f32, 0.1 * D24, 10.0, green)]);
    // One 16x16 test quad at window (8..24, 8..24) and w, counted.
    let test = |d: &mut d3d::Device<GfxBackend>, m: &mut SparseMemory, w: f32, index: u32| {
        let mut put = m.read_u32(DEV).unwrap();
        // 0x20cec: CLEAR_REPORT_VALUE 1, SET_ZPASS_PIXEL_COUNT_ENABLE 1.
        for word in [(2 << 18) | 0x17C8, 1, 1] {
            m.write_u32(put, word).unwrap();
            put += 4;
        }
        m.write_u32(DEV, put).unwrap();
        let v = |x: f32, y: f32| [x, y, 0.9 * D24, w, 1.0, 1.0, 1.0, 1.0];
        let verts = [
            v(8.0, 8.0),
            v(24.0, 8.0),
            v(8.0, 24.0),
            v(24.0, 8.0),
            v(24.0, 24.0),
            v(8.0, 24.0),
        ];
        // The quads share the vertex buffer: report the rewrite.
        d.note_write(0x8000_0000 | VB_DATA, 6 * 32);
        draw_pass(d, m, &plain_program(), &verts, 0x203, false); // LESSEQUAL
        let r = d3d::abi::Regs {
            eax: index,
            ..d3d::abi::Regs::default()
        };
        call(d, m, 0x3f85d0, r, &[]).unwrap();
        d.set_visibility_poll(false);
        d.get_visibility_test_result(m, index)
            .unwrap()
            .expect("waited")
            .count
    };
    assert_eq!(test(&mut d, &mut m, 20.0, 1), 0, "behind the wall: none");
    assert_eq!(
        test(&mut d, &mut m, 1.0, 2),
        16 * 16,
        "in front of the wall: every sample"
    );
    assert_eq!(test(&mut d, &mut m, 20.0, 3), 0, "behind again");

    let err = pollster::block_on(scope.pop());
    assert!(err.is_none(), "wgpu validation error: {err:?}");
}

// Hardware constant registers for the skinning programs below.
const SK_VIEWPROJ: u8 = 0;
const SK_SCALE: u8 = 4;
const SK_OFFSET: u8 = 5;
const SK_COLOUR: u8 = 6;
const SK_BONES: u8 = 8;

/// The tail both programs share: `dph` through the view-projection rows
/// into `oPos`, the viewport epilogue, and `oD0 = c[SK_COLOUR]`.
fn skin_tail(insts: &mut Vec<Inst>) {
    for (i, mask) in [8u8, 4, 2, 1].into_iter().enumerate() {
        insts.push(
            Inst::mac(MacOp::Dph, Some(Operand::r(0)), Some(Operand::c(SK_VIEWPROJ + i as u8)), None)
                .to_out(false, OutReg::Pos, mask),
        );
    }
    insts.push(
        Inst::mac(MacOp::Mul, Some(Operand::r(12)), Some(Operand::c(SK_SCALE)), None)
            .to_out(false, OutReg::Pos, 0xE)
            .with_ilu(shaders::vsh::IluOp::Rcc, Operand::r(12).swz("w"))
            .ilu_to_temp(1, 1),
    );
    insts.push(
        Inst::mac(MacOp::Mad, Some(Operand::r(12)), Some(Operand::r(1).swz("w")), Some(Operand::c(SK_OFFSET)))
            .to_out(false, OutReg::Pos, 0xE),
    );
    insts.push(
        Inst::mac(MacOp::Mov, Some(Operand::c(SK_COLOUR)), None, None)
            .to_out(false, OutReg::D0, 0xF)
            .last(),
    );
}

/// Rigid: `r0.xyz` = bone 0 (three rows) times `v0`.
fn rigid_program() -> Vec<u8> {
    let mut insts = Vec::new();
    for (row, mask) in [8u8, 4, 2].into_iter().enumerate() {
        insts.push(
            Inst::mac(MacOp::Dp4, Some(Operand::v(0)), Some(Operand::c(SK_BONES + row as u8)), None)
                .to_temp(0, mask),
        );
    }
    skin_tail(&mut insts);
    program(&insts)
}

/// Three bones weighted by `v3.xyz`, blended as matrices the way Halo's
/// skinning programs do: per row, `mul r5, c[bone 0 row], v3.x`, then
/// `mad r5, c[bone 1 row], v3.y, r5` and `mad r5, c[bone 2 row], v3.z, r5`;
/// then `dp4 r0.x, v0, r5` (and so on for y and z).
fn skinned_program() -> Vec<u8> {
    let mut insts = Vec::new();
    for (row, mask) in [8u8, 4, 2].into_iter().enumerate() {
        let r = 5 + row as u8;
        let bone = |b: u8| Operand::c(SK_BONES + 3 * b + row as u8);
        insts.push(
            Inst::mac(MacOp::Mul, Some(bone(0)), Some(Operand::v(3).swz("x")), None).to_temp(r, 0xF),
        );
        insts.push(
            Inst::mac(MacOp::Mad, Some(bone(1)), Some(Operand::v(3).swz("y")), Some(Operand::r(r)))
                .to_temp(r, 0xF),
        );
        insts.push(
            Inst::mac(MacOp::Mad, Some(bone(2)), Some(Operand::v(3).swz("z")), Some(Operand::r(r)))
                .to_temp(r, 0xF),
        );
        insts.push(
            Inst::mac(MacOp::Dp4, Some(Operand::v(0)), Some(Operand::r(r)), None).to_temp(0, mask),
        );
        // The blended rows also turn a direction into texture coordinates,
        // as the intro's later passes do for their tangent frame.
        insts.push(
            Inst::mac(MacOp::Dp3, Some(Operand::v(3)), Some(Operand::r(r)), None)
                .to_out(false, OutReg::T(1), mask),
        );
    }
    skin_tail(&mut insts);
    program(&insts)
}

/// Loads the skinning constants: a perspective view-projection with its
/// rows scaled by `w_scale` (the same picture, w times `w_scale`), the
/// viewport scale and offset for the 64x64 target, three bones with
/// awkward (non-dyadic) rotations and translations, and `colour`.
fn skin_constants(
    d: &mut d3d::Device<GfxBackend>,
    m: &mut SparseMemory,
    z_max: f32,
    w_scale: f32,
    colour: [f32; 4],
) {
    let mut c = [[0.0f32; 4]; 17];
    let (f, near, far) = (1.37f32, 0.0731f32, 913.7f32);
    let q = far / (far - near);
    c[0] = [f, 0.0131, 0.0, 0.0];
    c[1] = [0.0071, f, 0.0, 0.0];
    c[2] = [0.0, 0.0, q, -q * near];
    c[3] = [0.0, 0.0, 1.0, 0.0];
    for row in &mut c[..4] {
        *row = row.map(|v| v * w_scale);
    }
    let h = W as f32 / 2.0;
    c[SK_SCALE as usize] = [h, -h, z_max, 0.0];
    c[SK_OFFSET as usize] = [h + 0.531, h + 0.531, 0.0, 0.0];
    c[SK_COLOUR as usize] = colour;
    for bone in 0..3usize {
        let a = 0.3 + 0.71 * bone as f32;
        let (s, co) = a.sin_cos();
        let base = SK_BONES as usize + 3 * bone;
        c[base] = [co, -s * 0.37, s * 0.93, 0.113 * bone as f32 + 0.0417];
        c[base + 1] = [s, co * 0.37, -co * 0.93, -0.271];
        c[base + 2] = [0.0, 0.93, 0.37, 3.1371 + bone as f32];
    }
    for (i, v) in c.iter().flatten().enumerate() {
        m.write_u32(CONSTS + 4 * i as u32, v.to_bits()).unwrap();
    }
    d.set_vertex_shader_constant(m, 0, CONSTS, c.len() as u32, true).unwrap();
}

/// A 12x12 grid of object-space points with weights (1, 0, 0) in `v3`,
/// as a triangle list.
fn skin_mesh() -> Vec<[f32; 8]> {
    let n = 12;
    let p = |i: usize, j: usize| {
        let x = -1.9 + 3.8 * i as f32 / n as f32 + 0.0123 * ((i * 7 + j * 3) % 5) as f32;
        let y = -1.9 + 3.8 * j as f32 / n as f32 + 0.0171 * ((i * 3 + j * 5) % 7) as f32;
        let z = 0.37 * ((i as f32 * 0.61).sin() + (j as f32 * 0.43).cos());
        [x, y, z, 1.0, 1.0, 0.0, 0.0, 1.0]
    };
    let mut out = Vec::new();
    for j in 0..n {
        for i in 0..n {
            for (a, b) in [(0, 0), (1, 0), (0, 1), (1, 0), (1, 1), (0, 1)] {
                out.push(p(i + a, j + b));
            }
        }
    }
    out
}

/// Halo's multipass surfaces: the base pass with a rigid one-bone program
/// (depth LessEqual, written), later passes with a three-bone skinning
/// chain over the same vertices (depth Equal, not written). With weights
/// (1, 0, 0) the NV2A computes the same position and w in both, so the
/// later pass covers every pixel of the first, W-buffered and Z-buffered.
/// A guard: if the host compiler (Metal compiles with fast math) ever fused
/// or reassociated the two programs differently, Equal would fail along
/// iso-depth lines.
///
/// W-buffered depth is stored in whole steps of w, so a divergence only
/// shows where w crosses a whole number. Near w = 4 a 64x64 target barely
/// crosses any; the campaign intro's walls (frame 4450) span w = 10,000
/// to 22,000, crossing one every few pixels. Hence the second scale: the
/// same picture with w times 3913.3. The later pass also writes more
/// outputs and runs another pixel shader than the base pass, as the
/// intro's do. It holds on the Apple GPU this was written on, also with
/// `@invariant` taken off the position.
#[test]
fn rigid_and_skinned_passes_store_identical_depth() {
    let Some((mut d, mut m, scope)) = gpu() else {
        return;
    };
    set_states(&mut d, &mut m);
    let verts = skin_mesh();
    // More vertex memory than the common layout maps (VB_DATA..TEX_DATA).
    m.map(0x8000_1000 | VB_DATA, 0xF000);
    // The later pass's pixel shader: r0 = v0, then r0 = r0 + t1 * 0.
    let mut later = PixelShaderDef {
        combiner_count: 2,
        ..PixelShaderDef::default()
    };
    later.rgb_inputs[0] = inputs(REG_V0, ONE, 0, 0);
    later.rgb_outputs[0] = outputs(REG_R0, 0, 0, 0);
    later.alpha_inputs[0] = inputs(REG_V0 | CHAN_ALPHA, ONE | CHAN_ALPHA, 0, 0);
    later.alpha_outputs[0] = outputs(REG_R0, 0, 0, 0);
    later.rgb_inputs[1] = inputs(REG_R0, ONE, REG_V1, REG_ZERO);
    later.rgb_outputs[1] = outputs(0, 0, REG_R0, 0);
    later.alpha_inputs[1] = inputs(REG_R0 | CHAN_ALPHA, ONE | CHAN_ALPHA, 0, 0);
    later.alpha_outputs[1] = outputs(REG_R0, 0, 0, 0);
    for (zenable, w_scale) in [(2u32, 1.0), (2, 3913.3), (1, 1.0)] {
        call(&mut d, &mut m, 0x3f54f0, d3d::abi::Regs::default(), &[zenable]).unwrap();
        d.clear(&mut m, 0, 0, 0xF3, 0xFF00_0000, 1.0, 0).unwrap();
        skin_constants(&mut d, &mut m, 16_777_215.0, w_scale, [1.0, 0.0, 0.0, 1.0]);
        draw_pass(&mut d, &mut m, &rigid_program(), &verts, 0x203, true); // LESSEQUAL
        skin_constants(&mut d, &mut m, 16_777_215.0, w_scale, [0.0, 1.0, 0.0, 1.0]);
        draw_pass_with(&mut d, &mut m, &skinned_program(), &verts, 0x202, false, &later); // EQUAL
        let px = back_buffer(&mut d);
        let (mut red, mut green) = (0, 0);
        for y in 0..H {
            for x in 0..W {
                match px(x, y) {
                    [255, 0, 0, 255] => red += 1,
                    [0, 255, 0, 255] => green += 1,
                    _ => {}
                }
            }
        }
        let at = format!("ZENABLE {zenable}, w x {w_scale}");
        assert!(red + green > 1000, "{at}: {} pixels drawn", red + green);
        assert_eq!(red, 0, "{at}: {red} of {} pixels failed the Equal pass", red + green);
    }
    let err = pollster::block_on(scope.pop());
    assert!(err.is_none(), "wgpu validation error: {err:?}");
}

