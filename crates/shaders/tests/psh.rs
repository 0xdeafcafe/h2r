//! Register combiners: decoding, reference semantics, texture-mode address
//! math, and WGSL validity for hand-built definitions.
// Fixtures start from Default and set the fields each case is about.
#![allow(clippy::field_reassign_with_default)]

mod common;

use shaders::psh::bits::*;
use shaders::psh::interp::{self, Inputs, Sampler};
use shaders::psh::{self, PixelOptions, PixelShaderDef, PixelUniforms, Reg, TexMode, TextureDim};

fn def(stages: &[(u32, u32, u32, u32)], tex: [u32; 4]) -> PixelShaderDef {
    let mut d = PixelShaderDef::default();
    for (i, (ri, ro, ai, ao)) in stages.iter().enumerate() {
        d.rgb_inputs[i] = *ri;
        d.rgb_outputs[i] = *ro;
        d.alpha_inputs[i] = *ai;
        d.alpha_outputs[i] = *ao;
    }
    d.combiner_count = stages.len() as u32 | COUNT_UNIQUE_C0 | COUNT_UNIQUE_C1;
    d.texture_modes = texture_modes(tex);
    d
}

/// Returns the coordinate it was given as the colour (w = 1), so tests can
/// see the address math.
struct Echo;
impl Sampler for Echo {
    fn sample(&self, _: usize, _: TextureDim, c: [f32; 3]) -> [f32; 4] {
        [c[0], c[1], c[2], 1.0]
    }
}

/// A constant colour per stage.
struct Flat([[f32; 4]; 4]);
impl Sampler for Flat {
    fn sample(&self, stage: usize, _: TextureDim, _: [f32; 3]) -> [f32; 4] {
        self.0[stage]
    }
}

fn approx(a: [f32; 4], b: [f32; 4]) {
    for i in 0..4 {
        assert!((a[i] - b[i]).abs() < 1e-5, "{a:?} != {b:?}");
    }
}

#[test]
fn bytes_round_trip_in_sdk_field_order() {
    let mut bytes = [0u8; psh::PSDEF_SIZE];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = i as u8;
    }
    let d = PixelShaderDef::from_bytes(&bytes).unwrap();
    assert_eq!(d.alpha_inputs[0], 0x03020100);
    assert_eq!(
        d.final_inputs_abcd,
        u32::from_le_bytes([0x20, 0x21, 0x22, 0x23])
    );
    assert_eq!(
        d.rgb_inputs[0],
        u32::from_le_bytes([0x88, 0x89, 0x8a, 0x8b])
    );
    assert_eq!(d.compare_mode, u32::from_le_bytes([0xa8, 0xa9, 0xaa, 0xab]));
    assert_eq!(
        d.rgb_outputs[0],
        u32::from_le_bytes([0xb4, 0xb5, 0xb6, 0xb7])
    );
    assert_eq!(
        d.combiner_count,
        u32::from_le_bytes([0xd4, 0xd5, 0xd6, 0xd7])
    );
    assert_eq!(
        d.texture_modes,
        u32::from_le_bytes([0xd8, 0xd9, 0xda, 0xdb])
    );
    assert_eq!(
        d.final_constants,
        u32::from_le_bytes([0xec, 0xed, 0xee, 0xef])
    );
    assert_eq!(d.to_bytes(), bytes);
    assert!(PixelShaderDef::from_bytes(&bytes[..10]).is_err());
}

#[test]
fn decodes_fields() {
    let mut d = def(
        &[(
            inputs(REG_T0, REG_C0 | CHAN_ALPHA | MAP_EXPAND_NORMAL, REG_V1, ONE),
            outputs(
                REG_R0,
                REG_R1,
                REG_T2,
                OUT_AB_DOT_PRODUCT | OUT_AB_CD_MUX | OUT_SHIFTLEFT_1 | OUT_AB_BLUE_TO_ALPHA,
            ),
            0,
            0,
        )],
        [TEX_PROJECT2D, TEX_DOTPRODUCT, TEX_DOT_ST, TEX_CLIPPLANE],
    );
    d.combiner_count |= COUNT_MUX_MSB;
    d.dot_mapping = dot_mapping(DOT_MINUS1_TO_1_D3D, DOT_MINUS1_TO_1, DOT_ZERO_TO_ONE);
    d.input_texture = input_texture(1, 0);
    d.compare_mode = 0b1010 << 12;
    d.final_inputs_efg = final_efg(
        REG_FOG | CHAN_ALPHA,
        REG_R0,
        REG_R0 | CHAN_ALPHA,
        FINAL_CLAMP_SUM | FINAL_COMPLEMENT_R0,
    );
    let c = d.decode();
    assert_eq!(c.stages.len(), 1);
    let rgb = c.stages[0].rgb;
    assert_eq!(rgb.inputs[0].reg, Reg::T(0));
    assert_eq!(rgb.inputs[1].reg, Reg::C0);
    assert!(rgb.inputs[1].alpha);
    assert_eq!(rgb.inputs[1].map, psh::InputMap::ExpandNormal);
    assert_eq!(rgb.inputs[3].map, psh::InputMap::UnsignedInvert);
    assert_eq!(
        (rgb.output.ab, rgb.output.cd, rgb.output.sum),
        (Reg::R0, Reg::R1, Reg::T(2))
    );
    assert!(
        rgb.output.ab_dot && !rgb.output.cd_dot && rgb.output.mux && rgb.output.ab_blue_to_alpha
    );
    assert_eq!(rgb.output.map, psh::OutMap::Shl1);
    assert!(c.mux_msb && c.unique_c0 && c.unique_c1);
    assert_eq!(
        c.texture_modes,
        [
            TexMode::Project2d,
            TexMode::DotProduct,
            TexMode::DotSt,
            TexMode::ClipPlane
        ]
    );
    assert_eq!(c.dot_mapping[1], psh::DotMap::Minus1To1D3d);
    assert_eq!(c.dot_mapping[2], psh::DotMap::Minus1To1);
    assert_eq!(c.input_texture, [0, 0, 1, 0]);
    assert_eq!(c.compare_mode[3], 0b1010);
    let f = c.final_combiner.unwrap();
    assert_eq!(f.e.reg, Reg::Fog);
    assert!(f.clamp_sum && f.complement_r0 && !f.complement_v1);
    assert_eq!(
        psh::argb(0x80FF4000),
        [1.0, 64.0 / 255.0, 0.0, 128.0 / 255.0]
    );
}

#[test]
fn modulate_texture_by_diffuse() {
    // tex t0; mul r0, t0, v0 (rgb and alpha), no final combiner.
    let d = def(
        &[(
            inputs(REG_T0, REG_V0, 0, 0),
            outputs(REG_R0, 0, 0, 0),
            inputs(REG_T0 | CHAN_ALPHA, REG_V0 | CHAN_ALPHA, 0, 0),
            outputs(REG_R0, 0, 0, 0),
        )],
        [TEX_PROJECT2D, 0, 0, 0],
    );
    let mut inp = Inputs::default();
    inp.d0 = [0.5, 1.0, 0.25, 0.5];
    inp.t[0] = [0.0, 0.0, 0.0, 1.0];
    let s = Flat([[0.5, 0.5, 1.0, 0.8], [0.0; 4], [0.0; 4], [0.0; 4]]);
    let out = interp::run(
        &d,
        &PixelOptions::default(),
        &inp,
        &PixelUniforms::default(),
        &s,
    )
    .unwrap();
    approx(out, [0.25, 0.5, 0.25, 0.4]);
    assert_eq!(
        psh::disassemble(&d),
        "combiners 1 mux=lsb c0=per-stage c1=per-stage\ntex t0 project2d\n  stage0.rgb: r0.rgb = t0.rgb * v0.rgb\n  stage0.a  : r0.a = t0.a * v0.a\nfinal: (none) out = r0\n"
    );
}

#[test]
fn dot3_bump_with_blue_to_alpha_and_scale() {
    // dp3_x2 r0.rgba, t0_bx2, v0_bx2 ; out = r0
    let d = def(
        &[(
            inputs(REG_T0 | MAP_EXPAND_NORMAL, REG_V0 | MAP_EXPAND_NORMAL, 0, 0),
            outputs(
                REG_R0,
                0,
                0,
                OUT_AB_DOT_PRODUCT | OUT_AB_BLUE_TO_ALPHA | OUT_SHIFTLEFT_1,
            ),
            0,
            0,
        )],
        [TEX_PROJECT2D, 0, 0, 0],
    );
    let mut inp = Inputs::default();
    inp.t[0][3] = 1.0;
    inp.d0 = [0.75, 0.5, 0.5, 1.0]; // (0.5, 0, 0)
    let s = Flat([[0.75, 0.5, 0.5, 1.0], [0.0; 4], [0.0; 4], [0.0; 4]]); // (0.5, 0, 0)
    let out = interp::run(&d, &Default::default(), &inp, &Default::default(), &s).unwrap();
    // dot = 0.25, x2 = 0.5, replicated, and blue copied to alpha.
    approx(out, [0.5, 0.5, 0.5, 0.5]);
}

#[test]
fn sum_and_mux_and_clamping() {
    let mut d = def(
        &[(
            // A*B + C*D with signed constants; bias then clamp.
            inputs(
                REG_C0 | MAP_SIGNED_IDENTITY,
                ONE,
                REG_C1 | MAP_SIGNED_IDENTITY,
                ONE,
            ),
            outputs(0, 0, REG_R1, OUT_BIAS),
            inputs(REG_C0 | CHAN_ALPHA, ONE, REG_C1 | CHAN_ALPHA, ONE),
            outputs(0, 0, REG_R0, OUT_AB_CD_MUX),
        )],
        [0, 0, 0, 0],
    );
    d.final_inputs_abcd = inputs(0, 0, 0, REG_R1);
    d.final_inputs_efg = final_efg(0, 0, REG_R0 | CHAN_ALPHA, 0);
    let mut u = PixelUniforms::default();
    u.c0[0] = [0.9, -0.5, 0.25, 0.1];
    u.c1[0] = [0.9, -0.75, 0.0, 0.7];
    // No texture 0, so r0.a starts at 1: LSB of 255 is 1 -> picks CD.
    let out = interp::run(
        &d,
        &Default::default(),
        &Inputs::default(),
        &u,
        &interp::NoTextures,
    )
    .unwrap();
    // r1 = clamp((c0+c1) - 0.5) = (1.0 clamped, -1 clamped, -0.25) -> final clamps to [0,1].
    approx(out, [1.0, 0.0, 0.0, 0.7]);
    d.combiner_count |= COUNT_MUX_MSB; // r0.a = 1 >= 0.5 -> CD again
    let out = interp::run(
        &d,
        &Default::default(),
        &Inputs::default(),
        &u,
        &interp::NoTextures,
    )
    .unwrap();
    approx(out, [1.0, 0.0, 0.0, 0.7]);
    // Shared constants: stage 0 reads c0[0] regardless of flags.
    let mut shared = d.clone();
    shared.combiner_count &= !(COUNT_UNIQUE_C0 | COUNT_UNIQUE_C1);
    let t = psh::translate(&shared, &Default::default());
    assert!(t.body.contains("xps.c0[0]"));
}

#[test]
fn final_combiner_fog_lerp_and_special_registers() {
    // The usual xfc: rgb = lerp(fog.rgb, r0.rgb, fog.a) + v1*c1? Here:
    // A = fog.a, B = r0, C = fog.rgb, D = EF (v1 * c1), G = r0.a.
    let mut d = def(
        &[(
            inputs(REG_V0, ONE, 0, 0),
            outputs(REG_R0, 0, 0, 0),
            inputs(REG_V0 | CHAN_ALPHA, ONE, 0, 0),
            outputs(REG_R0, 0, 0, 0),
        )],
        [0, 0, 0, 0],
    );
    d.final_inputs_abcd = inputs(REG_FOG | CHAN_ALPHA, REG_R0, REG_FOG, REG_EF_PROD);
    d.final_inputs_efg = final_efg(REG_V1, REG_C1, REG_R0 | CHAN_ALPHA, 0);
    let mut u = PixelUniforms::default();
    u.fog_color = [1.0, 0.0, 0.0, 0.0];
    u.c1[8] = [0.5, 0.5, 0.5, 0.0];
    let mut inp = Inputs::default();
    inp.d0 = [0.0, 0.0, 1.0, 0.6];
    inp.d1 = [0.2, 0.2, 0.2, 0.0];
    inp.fog = 0.25; // mode 0: factor straight from the vertex
    let out = interp::run(&d, &Default::default(), &inp, &u, &interp::NoTextures).unwrap();
    // 0.25 * (0,0,1) + 0.75 * (1,0,0) + 0.1
    approx(out, [0.85, 0.1, 0.35, 0.6]);

    // Linear table fog: start 10, end 20, coordinate 15 -> 0.5 (D3D's
    // scale -1/(end - start), bias 1 + end/(end - start)).
    u.fog = linear_fog(10.0, 20.0).uniform();
    inp.fog = 15.0;
    let out = interp::run(&d, &Default::default(), &inp, &u, &interp::NoTextures).unwrap();
    approx(out, [0.6, 0.1, 0.6, 0.6]);

    // V1R0 sum with complements and clamp.
    d.final_inputs_abcd = inputs(0, 0, 0, REG_V1R0_SUM);
    d.final_inputs_efg = final_efg(
        0,
        0,
        REG_R0 | CHAN_ALPHA,
        FINAL_COMPLEMENT_V1 | FINAL_CLAMP_SUM,
    );
    let out = interp::run(&d, &Default::default(), &inp, &u, &interp::NoTextures).unwrap();
    approx(out, [0.8, 0.8, 1.0, 0.6]);
}

/// `r0.rgb = -0.5`, `v1.rgb = 0.75`, final `A = B = C = 0, D = V1R0_SUM`.
/// Shared with the GPU test.
pub fn v1r0_sum_case() -> (PixelShaderDef, PixelUniforms, Inputs) {
    let mut d = def(
        &[(
            // r0.rgb = -c0 * 1
            inputs(REG_C0 | MAP_SIGNED_NEGATE, ONE, 0, 0),
            outputs(REG_R0, 0, 0, 0),
            0,
            0,
        )],
        [0, 0, 0, 0],
    );
    d.final_inputs_abcd = inputs(0, 0, 0, REG_V1R0_SUM);
    d.final_inputs_efg = final_efg(0, 0, 0, 0) | 1;
    let mut u = PixelUniforms::default();
    u.c0[0] = [0.5, 0.5, 0.5, 0.0];
    let mut inp = Inputs::default();
    inp.d1 = [0.75, 0.75, 0.75, 0.0];
    (d, u, inp)
}

/// Regression (review P1): V1R0_SUM unsigned-maps each operand before the
/// add (NV_register_combiners; nxdk_pgraph_tests SpecularR0SumFlags), so a
/// negative r0 contributes 0, not -0.5.
#[test]
fn v1r0_sum_clamps_each_operand() {
    let (d, u, inp) = v1r0_sum_case();
    let out = interp::run(&d, &Default::default(), &inp, &u, &interp::NoTextures).unwrap();
    approx(out, [0.75, 0.75, 0.75, 0.0]);
    let t = psh::translate(&d, &Default::default());
    assert!(t.warnings.is_empty(), "{:?}", t.warnings);
    assert!(t.body.contains("clamp(r0.rgb, vec3f(0.0), vec3f(1.0))"));
    common::check(&t.module());

    // Complemented operands clamp first too: 1 - clamp(-0.5) = 1, and with
    // v1 complemented 1 - 0.75 = 0.25; the output clamp caps 1.75 at 1.
    let mut d2 = d;
    d2.final_inputs_efg = final_efg(0, 0, 0, FINAL_COMPLEMENT_R0);
    let out = interp::run(&d2, &Default::default(), &inp, &u, &interp::NoTextures).unwrap();
    approx(out, [1.0, 1.0, 1.0, 0.0]);
    d2.final_inputs_efg = final_efg(0, 0, 0, FINAL_COMPLEMENT_V1);
    let out = interp::run(&d2, &Default::default(), &inp, &u, &interp::NoTextures).unwrap();
    approx(out, [0.25, 0.25, 0.25, 0.0]);
}

/// Regression (review P2): `PSInputTexture` stage 2 = 4 with a dependent
/// mode panicked in the interpreter and emitted an undefined `t4`.
#[test]
fn input_texture_selector_must_name_an_earlier_stage() {
    let mut d = def(
        &[(inputs(REG_T2, ONE, 0, 0), outputs(REG_R0, 0, 0, 0), 0, 0)],
        [TEX_PROJECT2D, TEX_PASSTHRU, TEX_DPNDNT_AR, TEX_NONE],
    );
    d.input_texture = 4 << 16;
    let t = psh::translate(&d, &Default::default());
    assert!(
        t.warnings.iter().any(|w| w.contains("input texture 4")),
        "{:?}",
        t.warnings
    );
    assert!(!t.body.contains("t4"), "{}", t.body);
    common::check(&t.module());
    let flat = Flat([[0.1, 0.2, 0.3, 0.4]; 4]);
    let out = interp::run(
        &d,
        &Default::default(),
        &Inputs::default(),
        &PixelUniforms::default(),
        &flat,
    );
    // t2 sampled at (0, 0); r0.a starts as t0.a.
    approx(out.unwrap(), [0.1, 0.2, 0.3, 0.4]);
    // Stage 3 may read t0..t2 but not t3.
    let mut d3 = def(
        &[(inputs(REG_T3, ONE, 0, 0), outputs(REG_R0, 0, 0, 0), 0, 0)],
        [TEX_PROJECT2D, TEX_PASSTHRU, TEX_PASSTHRU, TEX_DPNDNT_GB],
    );
    d3.input_texture = input_texture(0, 3);
    assert!(!psh::translate(&d3, &Default::default()).warnings.is_empty());
    d3.input_texture = input_texture(0, 2);
    assert!(psh::translate(&d3, &Default::default()).warnings.is_empty());

    // Selectors and dot mappings a stage does not use are not read.
    let mut unused = def(
        &[(inputs(REG_T2, ONE, 0, 0), outputs(REG_R0, 0, 0, 0), 0, 0)],
        [TEX_PROJECT2D, TEX_PROJECT2D, TEX_PROJECT2D, TEX_NONE],
    );
    unused.input_texture = 0xF << 16 | 0xF << 20;
    unused.dot_mapping = dot_mapping(DOT_HILO_1, DOT_HILO_1, DOT_HILO_1);
    let t = psh::translate(&unused, &Default::default());
    assert!(t.warnings.is_empty(), "{:?}", t.warnings);
    common::check(&t.module());
    interp::run(
        &unused,
        &Default::default(),
        &Inputs::default(),
        &PixelUniforms::default(),
        &flat,
    );
}

#[test]
fn two_sided_lighting_and_alpha_test() {
    let d = def(
        &[(
            inputs(REG_V0, ONE, 0, 0),
            outputs(REG_R0, 0, 0, 0),
            inputs(REG_V0 | CHAN_ALPHA, ONE, 0, 0),
            outputs(REG_R0, 0, 0, 0),
        )],
        [0; 4],
    );
    let mut inp = Inputs::default();
    inp.d0 = [1.0, 0.0, 0.0, 0.5];
    inp.b0 = [0.0, 1.0, 0.0, 0.2];
    let opts = PixelOptions {
        two_sided_lighting: true,
        alpha_test: Some(psh::AlphaFunc::Greater),
        ..Default::default()
    };
    let mut u = PixelUniforms::default();
    u.alpha_ref = [100.0, 0.0, 0.0, 0.0];
    inp.front = true;
    approx(
        interp::run(&d, &opts, &inp, &u, &interp::NoTextures).unwrap(),
        [1.0, 0.0, 0.0, 0.5],
    );
    inp.front = false; // back face: 0.2 * 255 = 51 fails > 100
    assert_eq!(interp::run(&d, &opts, &inp, &u, &interp::NoTextures), None);
}

#[test]
fn clip_plane_kills() {
    let d = def(
        &[(inputs(REG_V0, ONE, 0, 0), outputs(REG_R0, 0, 0, 0), 0, 0)],
        [TEX_CLIPPLANE, 0, 0, 0],
    );
    let mut inp = Inputs::default();
    inp.t[0] = [0.5, 0.5, 0.5, 0.5];
    assert!(
        interp::run(
            &d,
            &Default::default(),
            &inp,
            &Default::default(),
            &interp::NoTextures
        )
        .is_some()
    );
    inp.t[0][2] = -0.1;
    assert!(
        interp::run(
            &d,
            &Default::default(),
            &inp,
            &Default::default(),
            &interp::NoTextures
        )
        .is_none()
    );
    let mut ge = d.clone();
    ge.compare_mode = 0b0100; // r: kill when >= 0
    assert!(
        interp::run(
            &ge,
            &Default::default(),
            &inp,
            &Default::default(),
            &interp::NoTextures
        )
        .is_some()
    );
}

#[test]
fn texture_address_math() {
    let one = |tex: [u32; 4]| {
        def(
            &[(
                inputs(REG_T3, ONE, 0, 0),
                outputs(REG_R0, 0, 0, 0),
                inputs(REG_T3 | CHAN_ALPHA, ONE, 0, 0),
                outputs(REG_R0, 0, 0, 0),
            )],
            tex,
        )
    };
    let run = |d: &PixelShaderDef, inp: &Inputs, u: &PixelUniforms| {
        interp::run(d, &Default::default(), inp, u, &Echo).unwrap()
    };
    let mut inp = Inputs::default();
    let u = PixelUniforms::default();

    // Projective 2D divides by w and applies the stage's coordinate scale.
    let d = one([0, 0, 0, TEX_PROJECT2D]);
    inp.t[3] = [0.5, 0.25, 9.0, 2.0];
    let mut us = u;
    us.tex_scale[3] = [0.5, 2.0, 1.0, 1.0];
    approx(run(&d, &inp, &us), [0.125, 0.25, 0.0, 1.0]);

    // Dependent reads take (a, r) and (g, b) of the input texture.
    let d = one([TEX_PASSTHRU, TEX_DPNDNT_AR, TEX_DPNDNT_GB, TEX_DPNDNT_AR]);
    inp.t[0] = [0.1, 0.2, 0.3, 0.4];
    let mut d2 = d.clone();
    d2.input_texture = input_texture(0, 1);
    // t1 = sample(t0.a, t0.r) = (0.4, 0.1, 0, 1); t3 = sample(t1.a, t1.r) = (1, 0.4, 0, 1).
    approx(run(&d2, &inp, &u), [1.0, 0.4, 0.0, 1.0]);

    // Dot product chain: t1 dot, t2 dot -> (s, t).
    let mut d = one([TEX_PASSTHRU, TEX_DOTPRODUCT, TEX_DOT_ST, 0]);
    d.rgb_inputs[0] = inputs(REG_T2, ONE, 0, 0);
    d.alpha_inputs[0] = inputs(REG_T2 | CHAN_ALPHA, ONE, 0, 0);
    inp.t[0] = [0.5, 1.0, 0.0, 1.0];
    inp.t[1] = [1.0, 0.5, 7.0, 0.0];
    inp.t[2] = [0.0, 1.0, 1.0, 0.0];
    approx(run(&d, &inp, &u), [1.0, 1.0, 0.0, 1.0]);

    // Signed mapping (D3D): 255 -> 1, 128 -> 0, 1 -> -1.
    d.dot_mapping = dot_mapping(DOT_MINUS1_TO_1_D3D, DOT_MINUS1_TO_1_D3D, 0);
    inp.t[0] = [1.0, 128.0 / 255.0, 1.0 / 255.0, 1.0];
    inp.t[1] = [1.0, 1.0, 1.0, 0.0];
    inp.t[2] = [0.0, 0.0, -0.5, 0.0];
    // dots = (1 + 0 - 1, 0.5).
    approx(run(&d, &inp, &u), [0.0, 0.5, 0.0, 1.0]);

    // Reflection: normal (0, 0, 1) from three dots, eye in the q's.
    let mut d = one([
        TEX_PASSTHRU,
        TEX_DOTPRODUCT,
        TEX_DOT_RFLCT_DIFF,
        TEX_DOT_RFLCT_SPEC,
    ]);
    d.rgb_inputs[0] = inputs(REG_T3, ONE, 0, 0);
    inp.t[0] = [0.0, 0.0, 1.0, 1.0];
    inp.t[1] = [1.0, 0.0, 0.0, -1.0];
    inp.t[2] = [0.0, 1.0, 0.0, 0.0];
    inp.t[3] = [0.0, 0.0, 1.0, 1.0];
    // dots = (0, 0, 1) -> n = (0, 0, 1); e = (-1, 0, 1).
    // r = 2n(n.e)/(n.n) - e = (0, 0, 2) - (-1, 0, 1) = (1, 0, 1).
    approx(run(&d, &inp, &u), [1.0, 0.0, 1.0, 1.0]);

    // Bump env: uv + M * (du, dv), and the luminance variant.
    let mut d = one([TEX_PASSTHRU, 0, 0, TEX_BUMPENVMAP_LUM]);
    d.input_texture = input_texture(0, 0);
    inp.t[0] = [0.5, -0.25, 0.5, 1.0];
    inp.t[3] = [0.1, 0.2, 0.0, 1.0];
    let mut ub = u;
    ub.bump_mat[3] = [1.0, 0.0, 0.0, 2.0];
    ub.bump_lum[3] = [1.0, 0.25, 0.0, 0.0];
    // t0 passthru clamps to (0.5, 0, 0.5); uv = (0.1 + 0.5, 0.2 + 0) ; lum = 0.5 + 0.25.
    approx(run(&d, &inp, &ub), [0.45, 0.15, 0.0, 0.75]);
}

#[test]
fn unsupported_modes_warn() {
    let d = def(
        &[(inputs(REG_T2, ONE, 0, 0), outputs(REG_R0, 0, 0, 0), 0, 0)],
        [TEX_PROJECT2D, TEX_DOTPRODUCT, TEX_BRDF, TEX_DOT_ZW],
    );
    let mut d = d;
    // Mapping values 8 to 15 are reserved.
    d.dot_mapping = dot_mapping(8, 0, 0);
    let t = psh::translate(&d, &Default::default());
    let w = t.warnings.join("\n");
    assert!(w.contains("brdf"), "{w}");
    assert!(w.contains("dot_zw"), "{w}");
    assert!(w.contains("Reserved(8)"), "{w}");
    common::check(&t.module());
    // A mode in a stage that cannot hold it.
    let bad = def(
        &[(inputs(REG_T0, ONE, 0, 0), outputs(REG_R0, 0, 0, 0), 0, 0)],
        [TEX_DOT_ST, 0, 0, 0],
    );
    assert!(
        !psh::translate(&bad, &Default::default())
            .warnings
            .is_empty()
    );
}

#[test]
fn texture_bindings_follow_modes_and_adjust() {
    let d = def(
        &[(inputs(REG_T0, ONE, 0, 0), outputs(REG_R0, 0, 0, 0), 0, 0)],
        [TEX_PROJECT2D, TEX_CUBEMAP, TEX_PROJECT3D, TEX_NONE],
    );
    let t = psh::translate(&d, &Default::default());
    assert_eq!(
        t.textures,
        [
            Some(TextureDim::D2),
            Some(TextureDim::Cube),
            Some(TextureDim::D3),
            None
        ]
    );
    let opts = PixelOptions {
        texture_dims: [Some(TextureDim::Cube), None, Some(TextureDim::D2), None],
        ..Default::default()
    };
    let t = psh::translate(&d, &opts);
    assert_eq!(
        t.textures,
        [
            Some(TextureDim::Cube),
            Some(TextureDim::Cube),
            Some(TextureDim::D2),
            None
        ]
    );
    assert!(
        t.body
            .contains("@group(1) @binding(2) var xps_tex1: texture_cube<f32>;")
    );
    common::check(&t.module());
}

/// Every texture mode in each stage that allows it, every input mapping,
/// output mapping and flag, and every alpha test, through naga and MSL.
#[test]
fn generated_wgsl_validates() {
    let maps = [
        MAP_UNSIGNED_IDENTITY,
        MAP_UNSIGNED_INVERT,
        MAP_EXPAND_NORMAL,
        MAP_EXPAND_NEGATE,
        MAP_HALFBIAS_NORMAL,
        MAP_HALFBIAS_NEGATE,
        MAP_SIGNED_IDENTITY,
        MAP_SIGNED_NEGATE,
    ];
    let regs = [
        REG_ZERO, REG_C0, REG_C1, REG_FOG, REG_V0, REG_V1, REG_T0, REG_T1, REG_T2, REG_T3, REG_R0,
        REG_R1,
    ];
    let outs = [
        OUT_IDENTITY,
        OUT_BIAS,
        OUT_SHIFTLEFT_1,
        OUT_SHIFTLEFT_1_BIAS,
        OUT_SHIFTLEFT_2,
        OUT_SHIFTRIGHT_1,
    ];
    let mut stages = Vec::new();
    for s in 0..8usize {
        let r = |k: usize| {
            regs[(s * 3 + k) % regs.len()]
                | maps[(s + k) % 8]
                | if k % 2 == 1 { CHAN_ALPHA } else { 0 }
        };
        let flags = outs[s % outs.len()]
            | if s % 2 == 0 {
                OUT_AB_DOT_PRODUCT | OUT_AB_BLUE_TO_ALPHA
            } else {
                OUT_CD_DOT_PRODUCT | OUT_CD_BLUE_TO_ALPHA
            }
            | if s % 3 == 0 { OUT_AB_CD_MUX } else { 0 };
        stages.push((
            inputs(r(0), r(1), r(2), r(3)),
            outputs(
                REG_R0,
                REG_R1,
                if s % 2 == 0 { REG_T1 } else { REG_V0 },
                flags,
            ),
            inputs(r(1), r(2), r(3), r(0)),
            outputs(
                REG_R1,
                REG_ZERO,
                REG_R0,
                outs[(s + 1) % outs.len()] | if s % 2 == 1 { OUT_AB_CD_MUX } else { 0 },
            ),
        ));
    }
    let chains: &[[u32; 4]] = &[
        [TEX_PROJECT2D, TEX_PROJECT3D, TEX_CUBEMAP, TEX_PASSTHRU],
        [
            TEX_CLIPPLANE,
            TEX_BUMPENVMAP,
            TEX_BUMPENVMAP_LUM,
            TEX_DPNDNT_GB,
        ],
        [TEX_PROJECT2D, TEX_DOTPRODUCT, TEX_DOT_ST, TEX_DPNDNT_AR],
        [
            TEX_PROJECT2D,
            TEX_DOTPRODUCT,
            TEX_DOTPRODUCT,
            TEX_DOT_STR_3D,
        ],
        [
            TEX_PROJECT2D,
            TEX_DOTPRODUCT,
            TEX_DOTPRODUCT,
            TEX_DOT_STR_CUBE,
        ],
        [
            TEX_PROJECT2D,
            TEX_DOTPRODUCT,
            TEX_DOT_RFLCT_DIFF,
            TEX_DOT_RFLCT_SPEC,
        ],
        [
            TEX_PROJECT2D,
            TEX_DOTPRODUCT,
            TEX_DOTPRODUCT,
            TEX_DOT_RFLCT_SPEC_CONST,
        ],
        [TEX_NONE, TEX_NONE, TEX_NONE, TEX_NONE],
    ];
    let alpha_tests = [
        None,
        Some(psh::AlphaFunc::Never),
        Some(psh::AlphaFunc::Less),
        Some(psh::AlphaFunc::GreaterEqual),
        Some(psh::AlphaFunc::Always),
    ];
    for (k, chain) in chains.iter().enumerate() {
        for n in [0usize, 1, 8] {
            let mut d = def(&stages[..n], *chain);
            d.dot_mapping = dot_mapping(DOT_MINUS1_TO_1_D3D, DOT_MINUS1_TO_1_GL, DOT_MINUS1_TO_1);
            d.input_texture = input_texture(1, (k % 3) as u32);
            d.combiner_count |= if k % 2 == 0 { COUNT_MUX_MSB } else { 0 };
            if k % 2 == 0 {
                d.final_inputs_abcd = inputs(
                    REG_FOG | CHAN_ALPHA,
                    REG_V1R0_SUM,
                    REG_EF_PROD,
                    REG_C0 | MAP_UNSIGNED_INVERT,
                );
                d.final_inputs_efg = final_efg(
                    REG_R1,
                    REG_C1,
                    REG_T0 | CHAN_ALPHA,
                    FINAL_CLAMP_SUM | FINAL_COMPLEMENT_V1 | FINAL_COMPLEMENT_R0,
                );
            }
            let opts = PixelOptions {
                alpha_kill: [k % 2 == 0, true, false, true],
                alpha_test: alpha_tests[k % alpha_tests.len()],
                two_sided_lighting: k % 3 == 0,
                flat_shading: k % 4 == 0,
                ..Default::default()
            };
            let t = psh::translate(&d, &opts);
            assert!(t.warnings.is_empty(), "chain {k}: {:?}", t.warnings);
            common::check(&t.module());
        }
    }
}

#[test]
fn link_makes_one_module() {
    use shaders::vsh::encode::{Inst, Operand as O, program};
    use shaders::vsh::{self, MacOp, OutReg};
    let bytes = program(&[
        Inst::mac(MacOp::Mov, Some(O::v(0)), None, None).to_out(false, OutReg::Pos, 0xF),
        Inst::mac(MacOp::Mov, Some(O::v(3)), None, None).to_out(false, OutReg::D0, 0xF),
    ]);
    let vs = vsh::translate(&vsh::Program::parse(&bytes).unwrap(), &Default::default()).unwrap();
    let d = def(
        &[(inputs(REG_T0, REG_V0, 0, 0), outputs(REG_R0, 0, 0, 0), 0, 0)],
        [TEX_PROJECT2D, 0, 0, 0],
    );
    let ps = psh::translate(&d, &Default::default());
    let module = common::check(&shaders::link(&vs, &ps).unwrap());
    let names: Vec<_> = module
        .entry_points
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(names, ["vs_main", "fs_main"]);
    let flat = psh::translate(
        &d,
        &PixelOptions {
            flat_shading: true,
            ..Default::default()
        },
    );
    assert_eq!(
        shaders::link(&vs, &flat),
        Err(shaders::LinkError::FlatMismatch)
    );
}

/// `D3DTSS_COLORSIGN`: the host texture stores a masked channel's signed
/// byte `s` as `s + 128`, and the texture unit delivers `2s/255` (Xbox
/// captures: 64 -> 128/255, 127 -> 254/255), clamped at -1. Unmasked
/// channels stay unsigned. Read through r0 = 0.5 * t0 + 0.5 so negatives
/// survive the output clamp.
#[test]
fn color_sign_reads_signed_bytes() {
    let d = def(
        &[(
            inputs(REG_T0 | MAP_SIGNED_IDENTITY, REG_C0, ONE, REG_C0),
            outputs(0, 0, REG_R0, 0),
            inputs(REG_T0 | CHAN_ALPHA | MAP_SIGNED_IDENTITY, ONE, 0, 0),
            outputs(REG_R0, 0, 0, 0),
        )],
        [TEX_PROJECT2D, 0, 0, 0],
    );
    let mut u = PixelUniforms::default();
    u.c0[0] = [0.5; 4];
    // Stored (biased) bytes: s = 127, -128, -1; alpha unsigned 0x40.
    let stored = |b: u8| b as f32 / 255.0;
    let texel = [stored(0xFF), stored(0x00), stored(0x7F), stored(0x40)];
    let tex = Flat([texel; 4]);
    let mut inp = Inputs::default();
    inp.t[0] = [0.5, 0.5, 0.0, 1.0];
    let opts = PixelOptions {
        color_sign: [color_sign(0xE000_0000), 0, 0, 0],
        ..Default::default()
    };
    assert_eq!(opts.color_sign[0], SIGN_R | SIGN_G | SIGN_B);
    let out = interp::run(&d, &opts, &inp, &u, &tex).unwrap();
    approx(
        out,
        [
            0.5 * 254.0 / 255.0 + 0.5,
            0.0,
            0.5 - 1.0 / 255.0,
            0x40 as f32 / 255.0,
        ],
    );
    // Unsigned without the flag.
    let out = interp::run(&d, &Default::default(), &inp, &u, &tex).unwrap();
    approx(
        out,
        [
            1.0f32.min(0.5 * texel[0] + 0.5),
            0.5 * texel[1] + 0.5,
            0.5 * texel[2] + 0.5,
            texel[3],
        ],
    );
    // Positive capture points through t0 alone: s -> 2s/255.
    let copy = def(
        &[(
            inputs(REG_T0 | MAP_SIGNED_IDENTITY, ONE, 0, 0),
            outputs(REG_R0, 0, 0, 0),
            inputs(ONE, ONE, 0, 0),
            outputs(REG_R0, 0, 0, 0),
        )],
        [TEX_PROJECT2D, 0, 0, 0],
    );
    let rgb = PixelOptions {
        color_sign: [SIGN_R | SIGN_G | SIGN_B, 0, 0, 0],
        ..Default::default()
    };
    for (s, want) in [(1u8, 2u8), (64, 128), (100, 200), (126, 252), (127, 254)] {
        let t = [stored(s ^ 0x80); 4];
        let out = interp::run(&copy, &rgb, &inp, &u, &Flat([t; 4])).unwrap();
        approx(out, [stored(want), stored(want), stored(want), 1.0]);
    }
    // Filtering happens on the biased bytes: the average of 0x7F and 0x80
    // (stored 0xFF and 0x00) is about 0, not -1 or 1.
    let mid = [127.5 / 255.0; 4];
    let out = interp::run(&d, &opts, &inp, &u, &Flat([mid; 4])).unwrap();
    approx(
        out,
        [
            0.5 - 0.5 / 255.0,
            0.5 - 0.5 / 255.0,
            0.5 - 0.5 / 255.0,
            127.5 / 255.0,
        ],
    );
    // Signed alpha.
    let opts = PixelOptions {
        color_sign: [SIGN_A, 0, 0, 0],
        ..Default::default()
    };
    let t = [0.0, 0.0, 0.0, stored(0xC0 ^ 0x80)];
    let out = interp::run(&d, &opts, &inp, &u, &Flat([t; 4])).unwrap();
    approx(out, [0.5, 0.5, 0.5, 0.0]); // -128/255 clamps to 0 in r0's output
    let w = psh::translate(&d, &opts);
    assert!(w.body.contains("xps_signed("), "{}", w.body);
    common::check(&w.module());
}

/// `PixelOptions::border`: outside `[0, 1]` on a bordered axis the stage
/// reads the border uniform as is; inside, the texture (signed).
#[test]
fn shader_supplied_border() {
    let d = def(
        &[(
            inputs(REG_T0 | MAP_SIGNED_IDENTITY, REG_C0, ONE, REG_C0),
            outputs(0, 0, REG_R0, 0),
            inputs(REG_T0 | CHAN_ALPHA, ONE, 0, 0),
            outputs(REG_R0, 0, 0, 0),
        )],
        [TEX_PROJECT2D, 0, 0, 0],
    );
    let mut u = PixelUniforms::default();
    u.c0[0] = [0.5; 4];
    u.border[0] = [0.0, 0.0, 0.0, 1.0];
    let tex = Flat([[1.0; 4]; 4]); // stored 0xFF: signed 127
    let opts = PixelOptions {
        color_sign: [SIGN_R | SIGN_G | SIGN_B, 0, 0, 0],
        border: [1, 0, 0, 0],
        ..Default::default()
    };
    let mut inp = Inputs::default();
    inp.t[0] = [-0.25, 0.5, 0.0, 1.0];
    approx(
        interp::run(&d, &opts, &inp, &u, &tex).unwrap(),
        [0.5, 0.5, 0.5, 1.0],
    );
    // v is not bordered: only u decides.
    inp.t[0] = [0.5, 1.5, 0.0, 1.0];
    let inside = 0.5 * 254.0 / 255.0 + 0.5;
    approx(
        interp::run(&d, &opts, &inp, &u, &tex).unwrap(),
        [inside, inside, inside, 1.0],
    );
    let w = psh::translate(&d, &opts);
    assert!(w.body.contains("xps_outside("), "{}", w.body);
    common::check(&w.module());
}

/// `explicit_final`: an all-zero final combiner still runs (black, alpha
/// 0) and is not replaced by D3D's default.
#[test]
fn explicit_zero_final_combiner_runs() {
    let mut d = def(
        &[(
            inputs(REG_C0, ONE, 0, 0),
            outputs(REG_R0, 0, 0, 0),
            inputs(REG_C0 | CHAN_ALPHA, ONE, 0, 0),
            outputs(REG_R0, 0, 0, 0),
        )],
        [0, 0, 0, 0],
    );
    d.explicit_final = true;
    assert!(d.has_final_combiner());
    assert!(d.decode().final_combiner.is_some());
    let kept = d.with_default_final_combiner(true, true);
    assert_eq!((kept.final_inputs_abcd, kept.final_inputs_efg), (0, 0));
    let mut u = PixelUniforms::default();
    u.c0[0] = [1.0, 0.5, 0.25, 1.0];
    let out = interp::run(
        &d,
        &Default::default(),
        &Inputs::default(),
        &u,
        &Flat([[0.0; 4]; 4]),
    )
    .unwrap();
    approx(out, [0.0; 4]);
    d.explicit_final = false;
    assert!(d.decode().final_combiner.is_none());
    let out = interp::run(
        &d,
        &Default::default(),
        &Inputs::default(),
        &u,
        &Flat([[0.0; 4]; 4]),
    )
    .unwrap();
    approx(out, [1.0, 0.5, 0.25, 1.0]);
    // The 60-dword layout does not carry it.
    let mut e = d.clone();
    e.explicit_final = true;
    assert_eq!(PixelShaderDef::from_bytes(&e.to_bytes()).unwrap(), d);
}

/// Linear fog with `FOGSTART == FOGEND`: D3D programs a scale of 8192
/// instead of dividing by zero (`0x0040008e`).
#[test]
fn linear_fog_with_equal_start_and_end() {
    let mut d = PixelShaderDef::default();
    d.combiner_count = 1;
    d.rgb_inputs[0] = inputs(REG_FOG | CHAN_ALPHA, ONE, 0, 0);
    d.rgb_outputs[0] = outputs(REG_R0, 0, 0, 0);
    let mut u = PixelUniforms::default();
    u.fog = linear_fog(10.0, 10.0).uniform();
    let mut inp = Inputs::default();
    for (f, want) in [
        (9.0, 1.0),
        (10.0, 0.0),
        (11.0, 0.0),
        (10.0 - 1.0 / 16384.0, 0.5),
    ] {
        inp.fog = f;
        let out = interp::run(&d, &Default::default(), &inp, &u, &interp::NoTextures).unwrap();
        assert!((out[0] - want).abs() < 1e-3, "fog {f}: {out:?}");
    }
}

/// The final combiner D3D programs when a definition has none, from
/// FOGENABLE and SPECULARENABLE: the words Halo 2's D3D pushes
/// (`0x130C0300 + 0x20000` with specular, `0x0C`/`0x0E` without fog, and
/// `0x1C80`).
#[test]
fn default_final_combiner_matches_d3d() {
    use psh::d3d_default_final_combiner as dfc;
    assert_eq!(dfc(true, false), (0x130C_0300, 0x1C80));
    assert_eq!(dfc(true, true), (0x130E_0300, 0x1C80));
    assert_eq!(dfc(false, false), (0x0C, 0x1C80));
    assert_eq!(dfc(false, true), (0x0E, 0x1C80));

    let mut d = def(
        &[(
            inputs(REG_V0, ONE, 0, 0),
            outputs(REG_R0, 0, 0, 0),
            inputs(REG_V0 | CHAN_ALPHA, ONE, 0, 0),
            outputs(REG_R0, 0, 0, 0),
        )],
        [0, 0, 0, 0],
    );
    assert!(!d.has_final_combiner());
    let mut u = PixelUniforms::default();
    u.fog_color = [0.0, 0.0, 1.0, 0.0];
    let mut inp = Inputs::default();
    inp.d0 = [1.0, 0.0, 0.0, 0.5];
    inp.d1 = [0.0, 0.5, 0.0, 0.0];
    inp.fog = 0.25;
    let run = |d: &PixelShaderDef| {
        interp::run(d, &Default::default(), &inp, &u, &interp::NoTextures).unwrap()
    };
    approx(
        run(&d.with_default_final_combiner(false, false)),
        [1.0, 0.0, 0.0, 0.5],
    );
    approx(
        run(&d.with_default_final_combiner(false, true)),
        [1.0, 0.5, 0.0, 0.5],
    );
    // 0.25 * diffuse + 0.75 * blue fog.
    approx(
        run(&d.with_default_final_combiner(true, false)),
        [0.25, 0.0, 0.75, 0.5],
    );
    approx(
        run(&d.with_default_final_combiner(true, true)),
        [0.25, 0.125, 0.75, 0.5],
    );
    // A definition with its own final combiner is left alone.
    d.final_inputs_abcd = inputs(0, 0, 0, REG_V0);
    d.final_inputs_efg = final_efg(0, 0, ONE, 0);
    assert_eq!(d.with_default_final_combiner(true, true), d);
}

/// HILO dot mappings read the source texel as two 16-bit halves, the 32-bit
/// word presented as an A8R8G8B8 texture samples it: hi = a:r (bits
/// 31..16), lo = g:b (bits 15..0).
#[test]
fn hilo_dot_mappings_split_the_texel_word() {
    // t1 = dot(tc1, map(t0)), t2 = sample(dot1, dot2) echoed into r0.
    let mut d = def(
        &[(
            inputs(REG_T2, ONE, 0, 0),
            outputs(REG_R0, 0, 0, 0),
            inputs(REG_T2 | CHAN_ALPHA, ONE, 0, 0),
            outputs(REG_R0, 0, 0, 0),
        )],
        [TEX_PASSTHRU, TEX_DOTPRODUCT, TEX_DOT_ST, 0],
    );
    let run = |d: &PixelShaderDef, word: u32, tc1: [f32; 3], tc2: [f32; 3]| {
        let byte = |s: u32| ((word >> s) & 0xFF) as f32 / 255.0;
        let mut inp = Inputs::default();
        // Passthru hands the "texel" through: (r, g, b, a).
        inp.t[0] = [byte(16), byte(8), byte(0), byte(24)];
        inp.t[1] = [tc1[0], tc1[1], tc1[2], 0.0];
        inp.t[2] = [tc2[0], tc2[1], tc2[2], 0.0];
        let t = psh::translate(d, &Default::default());
        assert!(t.warnings.is_empty(), "{:?}", t.warnings);
        common::check(&t.module());
        let out = interp::run(
            d,
            &Default::default(),
            &inp,
            &PixelUniforms::default(),
            &Echo,
        );
        let o = out.unwrap();
        [o[0], o[1]]
    };
    let close = |a: [f32; 2], b: [f64; 2]| {
        for k in 0..2 {
            assert!((a[k] as f64 - b[k]).abs() < 2e-6, "{a:?} != {b:?}");
        }
    };

    // HILO_1 on a D24S8 word (depth 0x123456, stencil 0x78) seen as colour:
    // (hi/65535, lo/65535, 1). The texture coordinates of the menu's depth
    // fog style reconstruct depth / 2^24 (plus stencil / 2^32).
    d.dot_mapping = dot_mapping(DOT_HILO_1, DOT_HILO_1, 0);
    let word = 0x1234_5678;
    let k = 65535.0f32;
    close(
        run(
            &d,
            word,
            [1.0, 0.0, 0.25],
            [k / 65536.0, k / 4_294_967_296.0, 0.0],
        ),
        [
            0x1234 as f64 / 65535.0 + 0.25,
            0x123456 as f64 / 16_777_216.0 + 0x78 as f64 / 4_294_967_296.0,
        ],
    );
    close(
        run(&d, word, [0.0, 1.0, 0.0], [0.0, 0.0, 0.5]),
        [0x5678 as f64 / 65535.0, 0.5],
    );

    // Hemisphere: signed halves, z = sqrt(1 - hi^2 - lo^2).
    let hemi = |hi: f64, lo: f64| (1.0 - hi * hi - lo * lo).max(0.0).sqrt();
    // Two's complement over 32767: 0x4000 -> 0.50002, 0xC000 -> -0.50002.
    d.dot_mapping = dot_mapping(DOT_HILO_HEMISPHERE, DOT_HILO_HEMISPHERE, 0);
    let (h, l) = (16384.0 / 32767.0, -16384.0 / 32767.0);
    close(
        run(&d, 0x4000_C000, [1.0, 0.0, 0.0], [0.0, -1.0, 0.0]),
        [h, -l],
    );
    close(
        run(&d, 0x4000_C000, [0.0, 0.0, 1.0], [0.0, 0.0, 0.0]),
        [hemi(h, l), 0.0],
    );
    // D3D: excess 32768 over 32767 (0xC000 -> 0.50002, 0x4000 -> -0.50002).
    d.dot_mapping = dot_mapping(DOT_HILO_HEMISPHERE_D3D, DOT_HILO_HEMISPHERE_D3D, 0);
    close(
        run(&d, 0xC000_4000, [1.0, 0.0, 0.0], [0.0, -1.0, 0.0]),
        [h, -l],
    );
    close(
        run(&d, 0xC000_4000, [0.0, 0.0, 1.0], [0.0, 0.0, 0.0]),
        [hemi(h, l), 0.0],
    );
    // GL: two's complement, (2s + 1) / 65535.
    d.dot_mapping = dot_mapping(DOT_HILO_HEMISPHERE_GL, DOT_HILO_HEMISPHERE_GL, 0);
    let (h, l) = (32769.0 / 65535.0, -32767.0 / 65535.0);
    close(
        run(&d, 0x4000_C000, [1.0, 0.0, 0.0], [0.0, -1.0, 0.0]),
        [h, -l],
    );
    close(
        run(&d, 0x4000_C000, [0.0, 0.0, 1.0], [0.0, 0.0, 0.0]),
        [hemi(h, l), 0.0],
    );
    // Saturated halves leave no room for z.
    d.dot_mapping = dot_mapping(DOT_HILO_HEMISPHERE, DOT_HILO_HEMISPHERE, 0);
    close(
        run(&d, 0x7FFF_7FFF, [0.0, 0.0, 1.0], [0.0, 0.0, 0.0]),
        [0.0, 0.0],
    );
}

#[test]
fn implicit_samples_use_each_stage_lod_bias() {
    let d = def(&[], [TEX_PROJECT2D, TEX_PROJECT2D, 0, 0]);
    let shader = psh::translate(&d, &PixelOptions::default());
    assert!(shader.body.contains("textureSampleBias(xps_tex0, xps_samp0"));
    assert!(shader.body.contains("clamp(xps.lod_bias[0], -16.0, 15.99)"));
    assert!(shader.body.contains("clamp(xps.lod_bias[1], -16.0, 15.99)"));
    let u = PixelUniforms { lod_bias: [-1.0, 2.0, 0.0, 0.0], ..Default::default() };
    let bytes = u.to_bytes();
    // lod_bias sits before the stipple (128 bytes), depth and depth_range.
    assert_eq!(&bytes[bytes.len() - 176..bytes.len() - 172], &(-1.0f32).to_le_bytes());
    assert_eq!(&bytes[bytes.len() - 172..bytes.len() - 168], &(2.0f32).to_le_bytes());
    let explicit = psh::translate(&d, &PixelOptions { explicit_lod: true, ..Default::default() });
    assert!(explicit.body.contains("textureSampleLevel(xps_tex0"));
    assert!(!explicit.body.contains("textureSampleBias"));
}

/// D3D's linear fog parameters (`0x00400068`): scale `-1/(end - start)`
/// (8192 when they are equal), bias `1 + end/(end - start)`.
fn linear_fog(start: f32, end: f32) -> psh::fog::Fog {
    let k = if start == end {
        8192.0
    } else {
        1.0 / (end as f64 - start as f64)
    };
    psh::fog::Fog {
        enable: true,
        mode: psh::fog::FogMode::Linear,
        bias: (1.0 + end as f64 * k) as f32,
        scale: -k as f32,
    }
}

/// D3D's exp and exp2 parameters (`0x004000ad`): bias 1.5, scale
/// `density * -1/(16 ln 2)` or `density * -1/sqrt(32 ln 2)`.
fn exp_fog(mode: psh::fog::FogMode, density: f32) -> psh::fog::Fog {
    let ln2 = std::f64::consts::LN_2;
    let k = match mode {
        psh::fog::FogMode::Exp | psh::fog::FogMode::ExpAbs => -1.0 / (16.0 * ln2),
        _ => -1.0 / (32.0 * ln2).sqrt(),
    };
    psh::fog::Fog {
        enable: true,
        mode,
        bias: 1.5,
        scale: (density as f64 * k as f32 as f64) as f32,
    }
}

/// A one-stage program whose output is `fog` (rgb = fog.rgb, alpha =
/// fog.a), so the fog register can be read back directly.
fn fog_reader() -> PixelShaderDef {
    def(
        &[(
            inputs(REG_FOG, ONE, 0, 0),
            outputs(REG_R0, 0, 0, 0),
            inputs(REG_FOG | CHAN_ALPHA, ONE, 0, 0),
            outputs(REG_R0, 0, 0, 0),
        )],
        [0, 0, 0, 0],
    )
}

/// Every NV2A fog mode with the parameters D3D derives gives D3D's fog
/// functions, clamped to [0, 1]; fog off reads 1; the ABS modes use the
/// coordinate's magnitude.
#[test]
fn fog_unit_modes_give_d3d_fog_functions() {
    use psh::fog::{Fog, FogMode};
    let close = |a: f32, b: f32, what: &str| assert!((a - b).abs() < 2e-4, "{what}: {a} vs {b}");
    let lin = linear_fog(10.0, 30.0);
    for d in [0.0f32, 10.0, 12.5, 20.0, 29.0, 30.0, 45.0, -5.0] {
        close(lin.factor(d), ((30.0 - d) / 20.0).clamp(0.0, 1.0), "linear");
    }
    for density in [0.0f32, 0.01, 0.5, 2.0] {
        let e = exp_fog(FogMode::Exp, density);
        let e2 = exp_fog(FogMode::Exp2, density);
        for d in [0.0f32, 0.5, 1.0, 3.0, 100.0] {
            close(e.factor(d), (-density * d).exp(), "exp");
            close(e2.factor(d), (-(density * d) * (density * d)).exp(), "exp2");
        }
    }
    // Negative coordinates: exp grows past 1 and clamps; ABS folds them.
    let e = exp_fog(FogMode::Exp, 0.5);
    close(e.factor(-2.0), 1.0, "exp of a negative coordinate clamps");
    let ea = Fog {
        mode: FogMode::ExpAbs,
        ..e
    };
    close(ea.factor(-2.0), (-1.0f32).exp(), "exp abs");
    let la = Fog {
        mode: FogMode::LinearAbs,
        ..lin
    };
    close(la.factor(-20.0), 0.5, "linear abs");
    let e2a = Fog {
        mode: FogMode::Exp2Abs,
        ..exp_fog(FogMode::Exp2, 0.5)
    };
    close(e2a.factor(-2.0), (-1.0f32).exp(), "exp2 abs");
    // Vertex fog: the coordinate is the factor, clamped.
    for (d, want) in [(0.25f32, 0.25f32), (-0.5, 0.0), (1.5, 1.0)] {
        close(Fog::FROM_VERTEX.factor(d), want, "vertex");
    }
    // Off: 1 whatever the coordinate.
    for d in [-1e9f32, 0.0, 0.5, 1e9] {
        assert_eq!(Fog::OFF.factor(d), 1.0);
    }
    // Method values round-trip.
    for m in [
        FogMode::Linear,
        FogMode::Exp,
        FogMode::Exp2,
        FogMode::LinearAbs,
        FogMode::ExpAbs,
        FogMode::Exp2Abs,
    ] {
        assert_eq!(FogMode::from_method(m.method()), Some(m));
    }
    assert_eq!(FogMode::from_method(0x0805), None);
}

/// The fog register reads (fog colour rgb, factor): the colour's own alpha
/// never shows, in a general combiner as in the final one.
#[test]
fn fog_register_alpha_is_the_factor() {
    let d = fog_reader();
    let mut u = PixelUniforms::default();
    u.fog_color = [0.2, 0.4, 0.6, 0.9];
    u.fog = linear_fog(0.0, 100.0).uniform();
    let mut inp = Inputs::default();
    inp.fog = 25.0;
    let out = interp::run(&d, &Default::default(), &inp, &u, &interp::NoTextures).unwrap();
    approx(out, [0.2, 0.4, 0.6, 0.75]);
    u.fog = psh::fog::Fog::OFF.uniform();
    let out = interp::run(&d, &Default::default(), &inp, &u, &interp::NoTextures).unwrap();
    approx(out, [0.2, 0.4, 0.6, 1.0]);
}

/// An infinite fog coordinate (D3D's pass-through `rcp` of `rhw = 0`)
/// gives xemu's fixed factor per mode, whatever the bias, scale or sign:
/// 1 for LINEAR, LINEAR_ABS and EXP, 0 for EXP_ABS, EXP2 and EXP2_ABS, and
/// 1 with fog off. NaN coordinates (and NaN results) give the same.
#[test]
fn infinite_fog_coordinate_gives_the_mode_constant() {
    use psh::fog::{Fog, FogMode};
    let with = |f: Fog, mode: FogMode| Fog { mode, ..f };
    let cases = [
        (linear_fog(10.0, 30.0), 1.0, "linear"),
        (linear_fog(-10.0, 10.0), 1.0, "linear -10..10"),
        (Fog::FROM_VERTEX, 1.0, "vertex fog"),
        (
            with(linear_fog(-10.0, 10.0), FogMode::LinearAbs),
            1.0,
            "linear abs",
        ),
        (exp_fog(FogMode::Exp, 0.5), 1.0, "exp"),
        (exp_fog(FogMode::Exp, 0.0), 1.0, "exp, density 0"),
        (exp_fog(FogMode::ExpAbs, 0.5), 0.0, "exp abs"),
        (exp_fog(FogMode::Exp2, 0.5), 0.0, "exp2"),
        (exp_fog(FogMode::Exp2, 0.0), 0.0, "exp2, density 0"),
        (exp_fog(FogMode::Exp2Abs, 0.5), 0.0, "exp2 abs"),
        (Fog::OFF, 1.0, "off"),
    ];
    for (fog, want, what) in cases {
        for d in [f32::INFINITY, f32::NEG_INFINITY, f32::NAN] {
            assert_eq!(fog.factor(d), want, "{what} at {d}");
        }
    }
    // A NaN result from finite inputs: exp with density 0 and a huge
    // coordinate is 2^(0 * x), finite; linear with scale inf at 0 is NaN.
    let nan_scale = Fog {
        scale: f32::INFINITY,
        ..linear_fog(0.0, 1.0)
    };
    assert_eq!(nan_scale.factor(0.0), 1.0, "NaN linear result");
    let nan_exp2 = Fog {
        scale: f32::INFINITY,
        ..exp_fog(FogMode::Exp2, 0.5)
    };
    assert_eq!(nan_exp2.factor(0.0), 0.0, "NaN exp2 result");
}

/// `fs_main` has a `discard` only when the program can kill: on Apple GPUs
/// even a `discard` that is never taken changes the implicit-LOD samples at
/// a draw's triangle edges when it shares a render pass with earlier draws,
/// and the d3d backend gives draws that can discard a render pass of their
/// own (`PixelShader::discards`).
#[test]
fn discard_only_when_the_program_can_kill() {
    let plain = def(
        &[(inputs(REG_V0, ONE, 0, 0), outputs(REG_R0, 0, 0, 0), 0, 0)],
        [0; 4],
    );
    let textured = def(
        &[(inputs(REG_T0, ONE, 0, 0), outputs(REG_R0, 0, 0, 0), 0, 0)],
        [TEX_PROJECT2D, 0, 0, 0],
    );
    let clip = def(
        &[(inputs(REG_V0, ONE, 0, 0), outputs(REG_R0, 0, 0, 0), 0, 0)],
        [TEX_CLIPPLANE, 0, 0, 0],
    );
    let cases: [(&str, &PixelShaderDef, PixelOptions, bool); 7] = [
        ("nothing kills", &plain, PixelOptions::default(), false),
        ("textured, no alpha kill", &textured, PixelOptions::default(), false),
        (
            "alpha test always passes",
            &plain,
            PixelOptions { alpha_test: Some(psh::AlphaFunc::Always), ..Default::default() },
            false,
        ),
        (
            "alpha test",
            &plain,
            PixelOptions { alpha_test: Some(psh::AlphaFunc::Greater), ..Default::default() },
            true,
        ),
        (
            "alpha kill",
            &textured,
            PixelOptions { alpha_kill: [true, false, false, false], ..Default::default() },
            true,
        ),
        ("clip plane", &clip, PixelOptions::default(), true),
        ("stipple", &plain, PixelOptions { stipple: true, ..Default::default() }, true),
    ];
    for (what, d, opts, discards) in cases {
        let t = psh::translate(d, &opts);
        assert_eq!(t.discards, discards, "{what}");
        assert_eq!(t.body.contains("discard;"), discards, "{what}");
        shaders::validate(&t.module()).unwrap_or_else(|e| panic!("{what}: {e}"));
    }
}

/// W-buffering writes the depth from the fragment stage (`frag_depth`)
/// without adding a `discard`, and the module still validates and lowers
/// to MSL.
#[test]
fn w_buffer_writes_frag_depth_without_discard() {
    let d = PixelShaderDef::default();
    let on = psh::translate(&d, &PixelOptions { w_buffer: true, ..Default::default() });
    let off = psh::translate(&d, &PixelOptions::default());
    assert!(on.body.contains("@builtin(frag_depth)"));
    assert!(!off.body.contains("frag_depth"));
    assert!(!on.discards);
    common::check(&on.module());
}

/// The level loading screen's sweep (`0x2a8d0` with its multiply flag, the
/// combiners built at `0x2ac13`): stage 0 writes `t0 + blue * c0[0].a`
/// back into `t0`, stage 1 tints it (`r0`) and takes the texel alpha
/// (`r1`), and the final combiner scales `r0 * r1` by the final constant's
/// alpha. On an `L8` texel the output is grey plus blue, never yellow.
#[test]
fn loading_screen_sweep_adds_blue_to_a_grey_texel() {
    let mut d = PixelShaderDef::default();
    d.texture_modes = texture_modes([TEX_PROJECT2D, 0, 0, 0]);
    d.combiner_count = 2 | COUNT_UNIQUE_C0 | COUNT_UNIQUE_C1;
    // Stage 0: sum t0 * 1 + c1.rgb * c0.a into t0.
    d.rgb_inputs[0] = 0x0820_0211;
    d.rgb_outputs[0] = 0x800;
    d.constant0[0] = 0x4000_0000; // c0[0].a = 64/255
    d.constant1[0] = 0x0000_00FF; // blue
    // Stage 1: r0 = t0.rgb * c0.rgb, r1 = t0.a * c0.a (white tint, alpha 1).
    d.rgb_inputs[1] = 0x0801_1811;
    d.rgb_outputs[1] = 0xCD;
    d.constant0[1] = 0xFFFF_FFFF;
    // Final: rgb = (r0 * r1) * fc0.a, alpha = r1.a.
    d.final_inputs_abcd = 0x0F11_0000;
    d.final_inputs_efg = 0x0C0D_1D00;
    d.final_constant0 = 0x8000_0000; // fc0.a = 128/255
    let (c0, c1) = d.embedded_constants();
    let mut u = PixelUniforms::default();
    u.c0 = c0;
    u.c1 = c1;
    let l = 160.0 / 255.0;
    let s = Flat([[l, l, l, 1.0], [0.0; 4], [0.0; 4], [0.0; 4]]);
    let out = interp::run(&d, &PixelOptions::default(), &Inputs::default(), &u, &s).unwrap();
    let m = 128.0 / 255.0;
    approx(out, [l * m, l * m, (l + 64.0 / 255.0) * m, 0.0]);
    assert_eq!(
        psh::disassemble(&d),
        "combiners 2 mux=lsb c0=per-stage c1=per-stage\ntex t0 project2d\n  stage0.rgb: t0.rgb = (t0.rgb * 1) + (c1.rgb * c0.a)\n  stage0.a  : (no writes)\n  stage1.rgb: r0.rgb = t0.rgb * c0.rgb\n  stage1.rgb: r1.rgb = t0.a * c0.a\n  stage1.a  : (no writes)\nfinal: rgb = prod.rgb*c0.a + (1-prod.rgb)*0 + 0; a = r1.a; E=r0.rgb F=r1.rgb\n"
    );
}

/// The glyph filter (`PixelOptions::glyph`) replaces a 2D projective fetch
/// on its stages only; with no stage marked, or under `explicit_lod`, the
/// program is the one without the option, byte for byte. The module
/// validates and lowers to MSL.
#[test]
fn glyph_filter_changes_only_its_stages() {
    let d = def(
        &[(
            inputs(REG_T0, REG_T1, 0, 0),
            outputs(REG_R0, 0, 0, 0),
            inputs(REG_T0 | CHAN_ALPHA, REG_T1 | CHAN_ALPHA, 0, 0),
            outputs(REG_R0, 0, 0, 0),
        )],
        [TEX_PROJECT2D, TEX_PROJECT2D, 0, 0],
    );
    let off = psh::translate(&d, &PixelOptions::default());
    let none = psh::translate(
        &d,
        &PixelOptions {
            glyph: 0,
            ..Default::default()
        },
    );
    assert_eq!(off.body, none.body);
    assert!(!off.body.contains("xps_glyph"));
    let lod = PixelOptions {
        glyph: 1,
        explicit_lod: true,
        ..Default::default()
    };
    let lod_off = PixelOptions {
        explicit_lod: true,
        ..Default::default()
    };
    assert_eq!(
        psh::translate(&d, &lod).body,
        psh::translate(&d, &lod_off).body
    );

    let on = psh::translate(
        &d,
        &PixelOptions {
            glyph: 1,
            ..Default::default()
        },
    );
    assert!(on.warnings.is_empty(), "{:?}", on.warnings);
    assert!(on.body.contains("t0 = xps_glyph(xps_tex0, guest, uv, k);"));
    assert!(
        !on.body.contains("xps_glyph(xps_tex1"),
        "stage 1 is not a glyph stage"
    );
    assert!(on.body.contains("t1 = textureSampleBias(xps_tex1"));
    common::check(&on.module());

    // A COLORSIGN or shader-bordered stage keeps the guest's fetch.
    let signed = PixelOptions {
        glyph: 1,
        color_sign: [SIGN_A, 0, 0, 0],
        ..Default::default()
    };
    assert!(
        !psh::translate(&d, &signed)
            .body
            .contains("xps_glyph(xps_tex0")
    );
    let bordered = PixelOptions {
        glyph: 1,
        border: [3, 0, 0, 0],
        ..Default::default()
    };
    assert!(
        !psh::translate(&d, &bordered)
            .body
            .contains("xps_glyph(xps_tex0")
    );

    // Options without the field (older pipeline-cache records) read as 0,
    // and 0 is not written, so existing records keep their hash.
    let json = serde_json::to_string(&PixelOptions::default()).unwrap();
    assert!(!json.contains("glyph"));
    let back: PixelOptions = serde_json::from_str(&json).unwrap();
    assert_eq!(back.glyph, 0);
}
