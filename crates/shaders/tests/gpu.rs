//! Differential tests: run the generated WGSL on the GPU (through a compute
//! harness that calls `xvs_program` / `xps_program` directly) and compare
//! with the reference interpreters, over randomly generated programs.
//!
//! Skips (passing) when no adapter is available or `SHADERS_NO_GPU` is set.

mod common;

use common::{Rng, close};
use shaders::psh::bits::*;
use shaders::psh::{self, PixelOptions, PixelShaderDef, PixelUniforms};
use shaders::vsh::encode::{Inst, Operand, program};
use shaders::vsh::{self, IluOp, MacOp, OutReg, SrcReg, VertexUniforms, interp};

const VS_HARNESS: &str = r#"
@group(1) @binding(8) var<storage, read> hin: array<vec4f>;
@group(1) @binding(9) var<storage, read_write> hout: array<vec4f>;
@compute @workgroup_size(1)
fn harness(@builtin(global_invocation_id) id: vec3u) {
    let b = id.x * 16u;
    let o = xvs_program(ARGS);
    let ob = id.x * 12u;
    hout[ob + 0u] = o.pos; hout[ob + 1u] = o.d0; hout[ob + 2u] = o.d1; hout[ob + 3u] = o.fog;
    hout[ob + 4u] = o.pts; hout[ob + 5u] = o.b0; hout[ob + 6u] = o.b1; hout[ob + 7u] = o.t0;
    hout[ob + 8u] = o.t1; hout[ob + 9u] = o.t2; hout[ob + 10u] = o.t3;
    hout[ob + 11u] = xvs_fixup(o.pos);
}
"#;

const PS_HARNESS: &str = r#"
@group(1) @binding(8) var<storage, read> hin: array<vec4f>;
@group(1) @binding(9) var<storage, read_write> hout: array<vec4f>;
@compute @workgroup_size(1)
fn harness(@builtin(global_invocation_id) id: vec3u) {
    let b = id.x * 9u;
    let m = hin[b + 8u];
    let r = xps_program(XpsInputs(hin[b], hin[b + 1u], hin[b + 2u], hin[b + 3u],
        hin[b + 4u], hin[b + 5u], hin[b + 6u], hin[b + 7u], m.x, m.y > 0.5));
    hout[id.x * 2u] = r.color;
    hout[id.x * 2u + 1u] = vec4f(select(0.0, 1.0, r.kill), 0.0, 0.0, 0.0);
}
"#;

const VERTS: usize = 32;

fn run_vertex(
    gpu: &common::Gpu,
    p: &vsh::Program,
    u: &VertexUniforms,
    inputs: &[[[f32; 4]; 16]],
) -> Vec<[[f32; 4]; 12]> {
    let t = vsh::translate(p, &Default::default()).unwrap();
    let reads = p.inputs_read();
    let args: Vec<String> = (0..16)
        .filter(|n| reads & (1 << n) != 0)
        .map(|n| format!("hin[b + {n}u]"))
        .collect();
    let src = format!(
        "{}{}",
        t.module(),
        VS_HARNESS.replace("ARGS", &args.join(", "))
    );
    common::check(&src);
    let flat: Vec<f32> = inputs.iter().flatten().flatten().copied().collect();
    let out = gpu.run(
        &src,
        "harness",
        0,
        &u.to_bytes(),
        &flat,
        inputs.len() * 48,
        inputs.len() as u32,
    );
    out.as_chunks::<48>()
        .0
        .iter()
        .map(|c| std::array::from_fn(|r| std::array::from_fn(|k| c[r * 4 + k])))
        .collect()
}

#[test]
fn vertex_transform_on_gpu() {
    let Some(gpu) = common::gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let o = Operand::v(0);
    let bytes = program(&[
        Inst::mac(MacOp::Dp4, Some(o), Some(Operand::c(96)), None).to_out(false, OutReg::Pos, 8),
        Inst::mac(MacOp::Dp4, Some(o), Some(Operand::c(97)), None).to_out(false, OutReg::Pos, 4),
        Inst::mac(MacOp::Dp4, Some(o), Some(Operand::c(98)), None).to_out(false, OutReg::Pos, 2),
        Inst::mac(MacOp::Dp4, Some(o), Some(Operand::c(99)), None).to_out(false, OutReg::Pos, 1),
        Inst::mac(MacOp::Mul, Some(Operand::r(12)), Some(Operand::c(58)), None)
            .to_out(false, OutReg::Pos, 14)
            .with_ilu(IluOp::Rcc, Operand::r(12).swz("w"))
            .ilu_to_temp(1, 8),
        Inst::mac(
            MacOp::Mad,
            Some(Operand::r(12)),
            Some(Operand::r(1).swz("x")),
            Some(Operand::c(59)),
        )
        .to_out(false, OutReg::Pos, 14),
    ]);
    let p = vsh::Program::parse(&bytes).unwrap();
    let mut u = VertexUniforms::default();
    // A perspective-ish matrix: w = z.
    u.constants[96] = [1.0, 0.0, 0.0, 0.0];
    u.constants[97] = [0.0, 1.0, 0.0, 0.0];
    u.constants[98] = [0.0, 0.0, 0.5, 0.0];
    u.constants[99] = [0.0, 0.0, 1.0, 0.0];
    u.constants[58] = [320.0, -240.0, u.z_max, 0.0];
    u.constants[59] = [320.0, 240.0, 0.0, 0.0];
    let mut inputs = vec![[[0.0; 4]; 16]; 4];
    for (i, v) in inputs.iter_mut().enumerate() {
        v[0] = [0.25 * i as f32 - 0.3, 0.1 * i as f32, 1.0 + i as f32, 1.0];
    }
    let gpu_out = run_vertex(&gpu, &p, &u, &inputs);
    for (inp, g) in inputs.iter().zip(&gpu_out) {
        let cpu = interp::run(&p, inp, &mut u.constants.clone());
        let fix = interp::fixup(cpu.get(OutReg::Pos), u.surface, u.z_max);
        for k in 0..4 {
            assert!(close(g[0][k], cpu.regs[0][k], 1e-5), "pos {g:?} vs {cpu:?}");
            assert!(
                close(g[11][k], fix[k], 1e-5),
                "fixup {:?} vs {fix:?}",
                g[11]
            );
        }
        // Clip-space x/w recovers the NDC of the screen position, up to the
        // rasterizer's 1/16-pixel snap.
        let ndc_x = fix[0] / fix[3];
        assert!((ndc_x - (2.0 * cpu.regs[0][0] / 640.0 - 1.0)).abs() <= 2.0 / 16.0 / 640.0 + 1e-6);
    }
}

/// Spec-derived RCC/EXP/LOG boundaries on the GPU. Regression for RCC(+0),
/// which gave -5.42e-20 on Metal when the clamp ran after `1.0 / x`, and
/// for LOG/EXP at infinities and just below powers of two.
#[test]
fn ilu_boundaries_on_gpu() {
    let Some(gpu) = common::gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let p = common::ilu_boundary_program();
    let cases = common::ilu_boundary_cases();
    let inputs: Vec<[[f32; 4]; 16]> = cases
        .iter()
        .map(|c| {
            let mut v = [[0.0; 4]; 16];
            v[0] = [c.x, 0.0, 0.0, 0.0];
            v
        })
        .collect();
    let got = run_vertex(&gpu, &p, &VertexUniforms::default(), &inputs);
    let mut failures = Vec::new();
    for (c, g) in cases.iter().zip(&got) {
        // Slots 7, 8, 9 are oT0, oT1, oT2.
        let (rcc, exp, log) = (g[7], g[8], g[9]);
        for k in 0..4 {
            let ok = close(rcc[k], c.rcc, 1e-6)
                && rcc[k].is_sign_negative() == c.rcc.is_sign_negative()
                && close(exp[k], c.exp[k], 1e-5)
                && close(log[k], c.log[k], 1e-5);
            if !ok {
                failures.push(format!(
                    "x={:e} ({:#010x}): rcc {rcc:?} want {:e}; exp {exp:?} want {:?}; log {log:?} want {:?}",
                    c.x,
                    c.x.to_bits(),
                    c.rcc,
                    c.exp,
                    c.log
                ));
                break;
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// `arl` with huge and non-finite inputs on the GPU (review P1: the
/// relative index must not overflow).
#[test]
fn arl_saturates_on_gpu() {
    let Some(gpu) = common::gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let bytes = program(&[
        Inst::mac(MacOp::Arl, Some(Operand::v(0).swz("x")), None, None),
        Inst::mac(MacOp::Mov, Some(Operand::c_rel(5)), None, None).to_out(false, OutReg::D0, 15),
    ]);
    let p = vsh::Program::parse(&bytes).unwrap();
    let mut u = VertexUniforms::default();
    for (i, c) in u.constants.iter_mut().enumerate() {
        *c = [i as f32; 4];
    }
    let xs = [
        2_147_483_648.0f32,
        f32::MAX,
        f32::INFINITY,
        -2_147_483_648.0,
        f32::NEG_INFINITY,
        186.0,
        -5.0,
        (17.0f32 / 255.0) * 255.0,
        f32::NAN,
    ];
    let want = [191.0, 191.0, 191.0, 0.0, 0.0, 191.0, 0.0, 22.0, 5.0];
    let inputs: Vec<[[f32; 4]; 16]> = xs
        .iter()
        .map(|&x| {
            let mut v = [[0.0; 4]; 16];
            v[0] = [x, 0.0, 0.0, 0.0];
            v
        })
        .collect();
    let got = run_vertex(&gpu, &p, &u, &inputs);
    for ((x, w), g) in xs.iter().zip(want).zip(&got) {
        assert_eq!(g[1], [w; 4], "arl {x}");
    }
}

fn random_vertex_program(rng: &mut Rng) -> Vec<u8> {
    use MacOp::*;
    let macs = [
        Nop, Mov, Mul, Add, Mad, Dp3, Dph, Dp4, Dst, Min, Max, Slt, Sge, Arl,
    ];
    let ilus = [
        IluOp::Nop,
        IluOp::Nop,
        IluOp::Mov,
        IluOp::Rcp,
        IluOp::Rcc,
        IluOp::Rsq,
        IluOp::Exp,
        IluOp::Log,
        IluOp::Lit,
    ];
    let outs = [
        OutReg::Pos,
        OutReg::D0,
        OutReg::D1,
        OutReg::Fog,
        OutReg::Pts,
        OutReg::B0,
        OutReg::B1,
        OutReg::T(0),
        OutReg::T(1),
        OutReg::T(2),
        OutReg::T(3),
    ];
    let n = 3 + rng.below(12) as usize;
    let mut insts = Vec::new();
    for _ in 0..n {
        let v = rng.below(4) as u8;
        let rel = rng.below(4) == 0;
        let cidx = 10 + rng.below(20) as u8;
        let operand = |rng: &mut Rng| -> Operand {
            let mut o = match rng.below(3) {
                0 => Operand::r(rng.pick(&[0, 1, 2, 3, 12])),
                1 => Operand::v(v),
                _ => {
                    if rel {
                        Operand::c_rel(cidx)
                    } else {
                        Operand::c(cidx)
                    }
                }
            };
            let s: String = (0..4).map(|_| rng.pick(&['x', 'y', 'z', 'w'])).collect();
            o = o.swz(&s);
            if rng.below(3) == 0 {
                o = o.negated();
            }
            o
        };
        let mac = rng.pick(&macs);
        let ilu = rng.pick(&ilus);
        if mac == Nop && ilu == IluOp::Nop {
            continue;
        }
        let a = Some(operand(rng));
        let b = Some(operand(rng));
        let c = operand(rng);
        let mut inst = Inst::mac(mac, a, b, Some(c));
        if mac != Nop && mac != Arl && rng.below(4) != 0 {
            inst = inst.to_temp(rng.below(4) as u8, 1 + rng.below(15) as u8);
        }
        if ilu != IluOp::Nop {
            inst = inst.with_ilu(ilu, c);
            if rng.below(3) != 0 {
                inst = inst.ilu_to_temp(rng.below(4) as u8, 1 + rng.below(15) as u8);
            }
        }
        if rng.below(2) == 0 {
            let from_ilu = ilu != IluOp::Nop && (mac == Nop || mac == Arl || rng.below(2) == 0);
            inst = inst.to_out(from_ilu, rng.pick(&outs), 1 + rng.below(15) as u8);
        }
        insts.push(inst);
    }
    // Make sure something is observable.
    insts.push(
        Inst::mac(MacOp::Mov, Some(Operand::r(rng.below(4) as u8)), None, None).to_out(
            false,
            OutReg::T(3),
            15,
        ),
    );
    insts.push(
        Inst::mac(MacOp::Add, Some(Operand::r(0)), None, Some(Operand::r(1))).to_out(
            false,
            OutReg::T(2),
            15,
        ),
    );
    program(&insts)
}

#[test]
fn random_vertex_programs_match_interpreter() {
    let Some(gpu) = common::gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut rng = Rng(0x5eed_1234_abcd_0001);
    let mut compared = 0usize;
    let (mut large, mut nonfinite) = (0usize, 0usize);
    let mut nonfinite_bad = Vec::new();
    for case in 0..120 {
        let bytes = random_vertex_program(&mut rng);
        let p = vsh::Program::parse(&bytes).unwrap();
        let mut u = VertexUniforms::default();
        for c in u.constants.iter_mut() {
            *c = std::array::from_fn(|_| rng.f(-2.0, 2.0));
        }
        let inputs: Vec<[[f32; 4]; 16]> = (0..VERTS)
            .map(|_| std::array::from_fn(|_| std::array::from_fn(|_| rng.f(-2.0, 2.0))))
            .collect();
        let got = run_vertex(&gpu, &p, &u, &inputs);
        for (vi, (inp, g)) in inputs.iter().zip(&got).enumerate() {
            let cpu = interp::run(&p, inp, &mut u.constants.clone());
            // `r` and `k` index the GPU and CPU outputs in step.
            #[allow(clippy::needless_range_loop)]
            for r in 0..11 {
                for k in 0..4 {
                    let (a, b) = (g[r][k], cpu.regs[r][k]);
                    if b.is_finite() {
                        // Every finite value, however large, at a relative
                        // tolerance.
                        compared += 1;
                        if b.abs() > 1e4 {
                            large += 1;
                        }
                        assert!(
                            close(a, b, 1e-3),
                            "case {case} vertex {vi} reg {r}.{k}: gpu {a} cpu {b}\n{}",
                            vsh::disassemble(&p)
                        );
                    } else {
                        // Metal compiles with fast math, so arithmetic on an
                        // infinity (an ADD fed by LOG(0), say) is not IEEE
                        // on the GPU. Infinities straight out of the ILU are
                        // exact (see ilu_boundaries_on_gpu); the rest are
                        // tallied, not skipped silently.
                        nonfinite += 1;
                        if !close(a, b, 1e-3) {
                            nonfinite_bad.push(format!("case {case} reg {r}.{k}: gpu {a} cpu {b}"));
                        }
                    }
                }
            }
        }
    }
    eprintln!(
        "compared {compared} finite values ({large} above 1e4); {} of {nonfinite} non-finite values differ",
        nonfinite_bad.len()
    );
    assert!(compared > 100_000, "only {compared} comparisons");
    assert!(large > 100, "only {large} large values");
    assert!(
        nonfinite_bad.len() * 4 < nonfinite,
        "too many non-finite mismatches:\n{}",
        nonfinite_bad.join("\n")
    );
}

fn random_pixel_def(rng: &mut Rng) -> PixelShaderDef {
    let regs = [
        REG_ZERO, REG_C0, REG_C1, REG_FOG, REG_V0, REG_V1, REG_T0, REG_T1, REG_T2, REG_T3, REG_R0,
        REG_R1,
    ];
    let dests = [
        REG_ZERO, REG_ZERO, REG_R0, REG_R1, REG_T0, REG_T1, REG_T2, REG_T3, REG_V0, REG_V1,
    ];
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
    let out_maps = [
        OUT_IDENTITY,
        OUT_BIAS,
        OUT_SHIFTLEFT_1,
        OUT_SHIFTLEFT_1_BIAS,
        OUT_SHIFTLEFT_2,
        OUT_SHIFTRIGHT_1,
    ];
    let mut d = PixelShaderDef::default();
    let inp = |rng: &mut Rng, regs: &[u8]| {
        rng.pick(regs) | rng.pick(&maps) | if rng.below(3) == 0 { CHAN_ALPHA } else { 0 }
    };
    let n = rng.below(9) as usize;
    for s in 0..n {
        d.rgb_inputs[s] = inputs(
            inp(rng, &regs),
            inp(rng, &regs),
            inp(rng, &regs),
            inp(rng, &regs),
        );
        d.alpha_inputs[s] = inputs(
            inp(rng, &regs),
            inp(rng, &regs),
            inp(rng, &regs),
            inp(rng, &regs),
        );
        let mut f = rng.pick(&out_maps);
        for (bit, p) in [
            (OUT_AB_DOT_PRODUCT, 3),
            (OUT_CD_DOT_PRODUCT, 3),
            (OUT_AB_CD_MUX, 3),
            (OUT_AB_BLUE_TO_ALPHA, 4),
            (OUT_CD_BLUE_TO_ALPHA, 4),
        ] {
            if rng.below(p) == 0 {
                f |= bit;
            }
        }
        d.rgb_outputs[s] = outputs(rng.pick(&dests), rng.pick(&dests), rng.pick(&dests), f);
        let fa = rng.pick(&out_maps) | if rng.below(3) == 0 { OUT_AB_CD_MUX } else { 0 };
        d.alpha_outputs[s] = outputs(rng.pick(&dests), rng.pick(&dests), rng.pick(&dests), fa);
    }
    d.combiner_count = n as u32;
    for (bit, _) in [
        (COUNT_MUX_MSB, 0),
        (COUNT_UNIQUE_C0, 0),
        (COUNT_UNIQUE_C1, 0),
    ] {
        if rng.below(2) == 0 {
            d.combiner_count |= bit;
        }
    }
    // Texture stages: every mode the translator supports, legal per stage,
    // and a third of the time a full dot-product/reflection chain.
    let base = [
        TEX_NONE,
        TEX_PROJECT2D,
        TEX_PROJECT3D,
        TEX_CUBEMAP,
        TEX_PASSTHRU,
        TEX_CLIPPLANE,
    ];
    let dependent = [
        TEX_BUMPENVMAP,
        TEX_BUMPENVMAP_LUM,
        TEX_DPNDNT_AR,
        TEX_DPNDNT_GB,
        TEX_DOTPRODUCT,
    ];
    let s1: Vec<u32> = base.iter().chain(&dependent).copied().collect();
    let mut s2 = s1.clone();
    s2.extend([TEX_DOT_ST, TEX_DOT_RFLCT_DIFF]);
    let mut s3: Vec<u32> = s1
        .iter()
        .copied()
        .filter(|&m| m != TEX_DOTPRODUCT)
        .collect();
    s3.extend([
        TEX_DOT_ST,
        TEX_DOT_RFLCT_SPEC,
        TEX_DOT_RFLCT_SPEC_CONST,
        TEX_DOT_STR_3D,
        TEX_DOT_STR_CUBE,
    ]);
    let modes = if rng.below(3) == 0 {
        [
            rng.pick(&[TEX_PROJECT2D, TEX_CUBEMAP, TEX_PASSTHRU]),
            TEX_DOTPRODUCT,
            rng.pick(&[TEX_DOTPRODUCT, TEX_DOT_RFLCT_DIFF]),
            rng.pick(&[
                TEX_DOT_RFLCT_SPEC,
                TEX_DOT_RFLCT_SPEC_CONST,
                TEX_DOT_STR_3D,
                TEX_DOT_STR_CUBE,
            ]),
        ]
    } else {
        [rng.pick(&base), rng.pick(&s1), rng.pick(&s2), rng.pick(&s3)]
    };
    d.texture_modes = texture_modes(modes);
    let maps = [
        DOT_ZERO_TO_ONE,
        DOT_MINUS1_TO_1_D3D,
        DOT_MINUS1_TO_1_GL,
        DOT_MINUS1_TO_1,
        DOT_HILO_1,
        DOT_HILO_HEMISPHERE_D3D,
        DOT_HILO_HEMISPHERE_GL,
        DOT_HILO_HEMISPHERE,
    ];
    d.dot_mapping = dot_mapping(rng.pick(&maps), rng.pick(&maps), rng.pick(&maps));
    d.input_texture = input_texture(rng.below(2) as u32, rng.below(3) as u32);
    d.compare_mode = rng.below(1 << 16) as u32;
    if rng.below(4) != 0 {
        let mut fregs = regs.to_vec();
        fregs.extend([REG_V1R0_SUM, REG_EF_PROD]);
        d.final_inputs_abcd = inputs(
            inp(rng, &fregs),
            inp(rng, &fregs),
            inp(rng, &fregs),
            inp(rng, &fregs),
        );
        let mut fl = 0;
        for bit in [FINAL_CLAMP_SUM, FINAL_COMPLEMENT_V1, FINAL_COMPLEMENT_R0] {
            if rng.below(2) == 0 {
                fl |= bit;
            }
        }
        d.final_inputs_efg = final_efg(inp(rng, &regs), inp(rng, &regs), inp(rng, &regs), fl) | 1;
    }
    d
}

#[test]
fn random_combiners_match_interpreter() {
    let Some(gpu) = common::gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut rng = Rng(0xc0de_cafe_f00d_0002);
    let funcs = [
        None,
        None,
        Some(psh::AlphaFunc::Less),
        Some(psh::AlphaFunc::GreaterEqual),
        Some(psh::AlphaFunc::NotEqual),
    ];
    let (mut compared, mut killed, mut ambiguous) = (0usize, 0usize, 0usize);
    let mut sampled = std::collections::BTreeMap::new();
    let mut mapped = std::collections::BTreeMap::new();
    for case in 0..200 {
        let d = random_pixel_def(&mut rng);
        let opts = PixelOptions {
            alpha_test: rng.pick(&funcs),
            two_sided_lighting: rng.below(2) == 0,
            alpha_kill: std::array::from_fn(|_| rng.below(4) == 0),
            color_sign: std::array::from_fn(|_| {
                if rng.below(3) == 0 {
                    rng.below(16) as u8
                } else {
                    0
                }
            }),
            explicit_lod: true,
            ..Default::default()
        };
        let t = psh::translate(&d, &opts);
        assert!(t.warnings.is_empty(), "{:?}", t.warnings);
        let src = format!("{}{}", t.module(), PS_HARNESS);
        common::check(&src);
        let sampler = TestTextures::random(&mut rng, t.textures);
        let modes = d.decode().texture_modes;
        let dot_maps = d.decode().dot_mapping;
        let mut u = PixelUniforms::default();
        for c in u.c0.iter_mut().chain(u.c1.iter_mut()) {
            *c = std::array::from_fn(|_| rng.f(0.0, 1.0));
        }
        u.fog_color = std::array::from_fn(|_| rng.f(0.0, 1.0));
        u.alpha_ref = [rng.below(256) as f32, 0.0, 0.0, 0.0];
        // Fog: from the vertex, linear (also with start == end), exp, exp2.
        u.fog = match rng.below(5) {
            0 => [1.0, 0.25, 0.75, 0.0],
            1 => [1.0, 0.5, 0.5, 0.0],
            2 => [2.0, 0.0, 0.0, rng.f(0.0, 2.0)],
            3 => [3.0, 0.0, 0.0, rng.f(0.0, 2.0)],
            _ => [0.0; 4],
        };
        for s in 0..4 {
            u.bump_mat[s] = std::array::from_fn(|_| rng.f(-1.0, 1.0));
            u.bump_lum[s] = [rng.f(0.0, 2.0), rng.f(-0.5, 0.5), 0.0, 0.0];
            u.tex_scale[s] = [rng.f(0.5, 2.0), rng.f(0.5, 2.0), 1.0, 1.0];
        }
        u.eye = [rng.f(-1.0, 1.0), rng.f(-1.0, 1.0), rng.f(-1.0, 1.0), 0.0];
        let frags: Vec<psh::interp::Inputs> = (0..VERTS)
            .map(|_| {
                let mut v = || std::array::from_fn(|_| rng.f(-0.2, 1.2));
                let (d0, d1, b0, b1) = (v(), v(), v(), v());
                let t = std::array::from_fn(|_| std::array::from_fn(|_| rng.f(-1.0, 1.5)));
                psh::interp::Inputs {
                    d0,
                    d1,
                    b0,
                    b1,
                    t,
                    fog: rng.f(-0.2, 1.2),
                    front: rng.below(2) == 0,
                }
            })
            .collect();
        let mut flat = Vec::new();
        for f in &frags {
            for v in [f.d0, f.d1, f.b0, f.b1, f.t[0], f.t[1], f.t[2], f.t[3]] {
                flat.extend(v);
            }
            flat.extend([f.fog, if f.front { 1.0 } else { 0.0 }, 0.0, 0.0]);
        }
        let out = gpu.run_with_textures(
            &src,
            "harness",
            1,
            &u.to_bytes(),
            &flat,
            frags.len() * 8,
            frags.len() as u32,
            &sampler.upload(&gpu),
        );
        for (i, f) in frags.iter().enumerate() {
            sampler.ambiguous.set(false);
            let cpu = psh::interp::run(&d, &opts, f, &u, &sampler);
            if sampler.ambiguous.get() {
                // A lookup within rounding of a texel or face boundary:
                // GPU and CPU may legitimately pick different texels.
                ambiguous += 1;
                continue;
            }
            for m in modes {
                *sampled.entry(m.name()).or_insert(0usize) += 1;
            }
            for (st, m) in modes.iter().enumerate() {
                if m.is_dot() {
                    *mapped
                        .entry(format!("{:?}", dot_maps[st]))
                        .or_insert(0usize) += 1;
                }
            }
            let gk = out[i * 8 + 4] != 0.0;
            assert_eq!(
                gk,
                cpu.is_none(),
                "case {case} frag {i}: kill differs\n{}",
                psh::disassemble(&d)
            );
            if let Some(c) = cpu {
                for k in 0..4 {
                    compared += 1;
                    assert!(
                        (out[i * 8 + k] - c[k]).abs() < 1e-4,
                        "case {case} frag {i}.{k}: gpu {:?} cpu {c:?}\n{}",
                        &out[i * 8..i * 8 + 4],
                        psh::disassemble(&d)
                    );
                }
            } else {
                killed += 1;
            }
        }
    }
    eprintln!(
        "compared {compared}, killed {killed}, ambiguous {ambiguous}; fragments per mode {sampled:?}; dot stages per mapping {mapped:?}"
    );
    assert!(
        compared > 5_000 && killed > 0,
        "compared {compared}, killed {killed}"
    );
    // Mostly zero-length cube directions and near-singular projections.
    assert!(
        ambiguous * 2 < compared / 4 + killed,
        "{ambiguous} ambiguous fragments"
    );
    // Every dot mapping, HILO included, fed a compared dot product.
    for m in [
        "ZeroToOne",
        "Minus1To1D3d",
        "Minus1To1Gl",
        "Minus1To1",
        "Hilo1",
        "HiloHemisphereD3d",
        "HiloHemisphereGl",
        "HiloHemisphere",
    ] {
        let n = mapped.get(m).copied().unwrap_or(0);
        assert!(
            n >= 32,
            "dot mapping {m} compared on only {n} fragments: {mapped:?}"
        );
    }
    // Every supported texture mode was exercised and compared.
    for m in [
        "project2d",
        "project3d",
        "cubemap",
        "bumpenvmap",
        "bumpenvmap_lum",
        "dpndnt_ar",
        "dpndnt_gb",
        "dotproduct",
        "dot_st",
        "dot_rflct_diff",
        "dot_rflct_spec",
        "dot_rflct_spec_const",
        "dot_str_3d",
        "dot_str_cube",
    ] {
        let n = sampled.get(m).copied().unwrap_or(0);
        assert!(
            n >= 32,
            "mode {m} compared on only {n} fragments: {sampled:?}"
        );
    }
}

/// CPU twin of the textures the GPU test binds: RGBA8, nearest filtering,
/// repeat addressing (2D 4x4, 3D 2x2x2, cube 2x2 per face). Flags lookups
/// too close to a texel or face boundary to call.
struct TestTextures {
    stages: [Option<(psh::TextureDim, Vec<[u8; 4]>)>; 4],
    ambiguous: std::cell::Cell<bool>,
}

impl TestTextures {
    fn size(dim: psh::TextureDim) -> [u32; 3] {
        match dim {
            psh::TextureDim::D2 => [4, 4, 1],
            psh::TextureDim::D3 => [2, 2, 2],
            psh::TextureDim::Cube => [2, 2, 6],
        }
    }

    fn random(rng: &mut Rng, dims: [Option<psh::TextureDim>; 4]) -> Self {
        let stages = dims.map(|d| {
            d.map(|dim| {
                let [w, h, l] = Self::size(dim);
                let texels = (0..w * h * l)
                    .map(|_| {
                        let mut t: [u8; 4] = std::array::from_fn(|_| rng.below(256) as u8);
                        if rng.below(4) == 0 {
                            t[3] = 0;
                        }
                        t
                    })
                    .collect();
                (dim, texels)
            })
        });
        Self {
            stages,
            ambiguous: std::cell::Cell::new(false),
        }
    }

    fn upload(&self, gpu: &common::Gpu) -> Vec<common::GpuTexture> {
        self.stages
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                let (dim, texels) = s.as_ref()?;
                let vd = match dim {
                    psh::TextureDim::D2 => wgpu::TextureViewDimension::D2,
                    psh::TextureDim::D3 => wgpu::TextureViewDimension::D3,
                    psh::TextureDim::Cube => wgpu::TextureViewDimension::Cube,
                };
                Some(common::GpuTexture {
                    binding: 2 * i as u32,
                    view: gpu.texture(vd, Self::size(*dim), texels),
                    dim: vd,
                })
            })
            .collect()
    }

    /// Nearest texel index for a repeating coordinate.
    fn wrap(&self, x: f32, n: u32) -> usize {
        let p = x * n as f32;
        // An exact integer (typically 0, from a zero source) is computed
        // identically on both sides; one merely close to it is not.
        let near = p != p.round() && (p - p.round()).abs() < 1e-3;
        if !p.is_finite() || p.abs() > 1e4 || near {
            self.ambiguous.set(true);
            return 0;
        }
        p.floor().rem_euclid(n as f32) as usize
    }

    /// Cube face (wgpu layer order +X -X +Y -Y +Z -Z) and face coordinates,
    /// per the WebGPU / Vulkan face-selection table.
    fn cube(&self, v: [f32; 3]) -> (usize, f32, f32) {
        let a = v.map(f32::abs);
        let mut order = [0, 1, 2];
        order.sort_by(|&i, &j| a[j].total_cmp(&a[i]));
        let (m, second) = (a[order[0]], a[order[1]]);
        if !m.is_finite() || m < 1e-6 || m - second < 1e-3 * m {
            self.ambiguous.set(true);
            return (0, 0.5, 0.5);
        }
        let [x, y, z] = v;
        let (face, sc, tc) = match (order[0], v[order[0]] >= 0.0) {
            (0, true) => (0, -z, -y),
            (0, false) => (1, z, -y),
            (1, true) => (2, x, z),
            (1, false) => (3, x, -z),
            (2, true) => (4, x, -y),
            _ => (5, -x, -y),
        };
        (face, 0.5 * (sc / m + 1.0), 0.5 * (tc / m + 1.0))
    }
}

impl psh::interp::Sampler for TestTextures {
    fn sample(&self, stage: usize, dim: psh::TextureDim, c: [f32; 3]) -> [f32; 4] {
        let (bound, texels) = self.stages[stage].as_ref().expect("stage has a texture");
        assert_eq!(*bound, dim, "stage {stage} sampled as {dim:?}");
        let [w, h, _] = Self::size(dim);
        let index = match dim {
            psh::TextureDim::D2 => self.wrap(c[1], h) * w as usize + self.wrap(c[0], w),
            psh::TextureDim::D3 => {
                (self.wrap(c[2], 2) * 2 + self.wrap(c[1], 2)) * 2 + self.wrap(c[0], 2)
            }
            psh::TextureDim::Cube => {
                let (face, s, t) = self.cube(c);
                // Clamped within the face; only the middle line is a boundary.
                let texel = |u: f32| {
                    if (u - 0.5).abs() < 1e-3 {
                        self.ambiguous.set(true);
                    }
                    usize::from(u >= 0.5)
                };
                face * 4 + texel(t) * 2 + texel(s)
            }
        };
        texels[index].map(|b| b as f32 / 255.0)
    }
}

/// Regression (review P1): V1R0_SUM on the GPU. `r0.rgb = -0.5`,
/// `v1.rgb = 0.75`, `A = B = C = 0, D = V1R0_SUM` gives 0.75 (each operand
/// is unsigned-mapped before the add), not 0.25.
#[test]
fn v1r0_sum_on_gpu() {
    let Some(gpu) = common::gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut d = PixelShaderDef::default();
    d.rgb_inputs[0] = inputs(REG_C0 | MAP_SIGNED_NEGATE, ONE, 0, 0);
    d.rgb_outputs[0] = outputs(REG_R0, 0, 0, 0);
    d.combiner_count = 1;
    d.final_inputs_abcd = inputs(0, 0, 0, REG_V1R0_SUM);
    d.final_inputs_efg = 1;
    let t = psh::translate(&d, &Default::default());
    let src = format!("{}{}", t.module(), PS_HARNESS);
    common::check(&src);
    let mut u = PixelUniforms::default();
    u.c0[0] = [0.5, 0.5, 0.5, 0.0];
    let mut input = vec![0.0f32; 36];
    input[4..8].copy_from_slice(&[0.75, 0.75, 0.75, 0.0]); // d1
    input[33] = 1.0; // front
    let out = gpu.run(&src, "harness", 1, &u.to_bytes(), &input, 8, 1);
    let want = [0.75, 0.75, 0.75, 0.0];
    for k in 0..4 {
        assert!((out[k] - want[k]).abs() < 1e-6, "{:?}", &out[..4]);
    }
}

#[test]
fn harness_programs_use_the_expected_registers() {
    // Guard for the random generator: inputs v0..v3 and constants 10..30 only.
    let mut rng = Rng(7);
    for _ in 0..50 {
        let p = vsh::Program::parse(&random_vertex_program(&mut rng)).unwrap();
        for i in &p.instructions {
            for s in i.sources() {
                match s.reg {
                    SrcReg::Input(n) => assert!(n < 4),
                    SrcReg::Const { index, .. } => assert!((10..30).contains(&index)),
                    _ => {}
                }
            }
        }
    }
}
