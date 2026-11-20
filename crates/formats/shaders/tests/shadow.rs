//! Shadow maps: PROJECT2D/PROJECT3D on depth-format textures compare the
//! stored depth with the reference under `D3DRS_SHADOWFUNC`, per texel,
//! then filter. Synthetic depth textures in the renderer's raw-bit layout
//! (`gfx::convert::depth_channels`), every comparison function, every
//! format, on the interpreter and (when there is an adapter) the GPU. The
//! expected values come from the ordering of the stored values against the
//! reference, not from either implementation.

mod common;

use shaders::psh::bits::*;
use shaders::psh::interp::{self, Inputs, NoTextures, Sampler};
use shaders::psh::shadow::{DepthFormat, Shadow, ShadowAddress};
use shaders::psh::{self, AlphaFunc, PixelOptions, PixelShaderDef, PixelUniforms, TextureDim};

const FUNCS: [AlphaFunc; 8] = [
    AlphaFunc::Never,
    AlphaFunc::Less,
    AlphaFunc::Equal,
    AlphaFunc::LessEqual,
    AlphaFunc::Greater,
    AlphaFunc::NotEqual,
    AlphaFunc::GreaterEqual,
    AlphaFunc::Always,
];

const FORMATS: [DepthFormat; 4] = [
    DepthFormat::D24,
    DepthFormat::D16,
    DepthFormat::F24,
    DepthFormat::F16,
];

/// Stage 0 in `mode`, its result straight to the output (final combiner
/// `D = t0`, `G = t0.a`).
fn def(mode: u32) -> PixelShaderDef {
    PixelShaderDef {
        texture_modes: texture_modes([mode, TEX_NONE, TEX_NONE, TEX_NONE]),
        final_inputs_abcd: inputs(0, 0, 0, REG_T0),
        final_inputs_efg: final_efg(0, 0, REG_T0 | CHAN_ALPHA, 0),
        ..Default::default()
    }
}

/// A stored word as the renderer uploads it: 32-bit formats give the depth
/// bytes high to low in r, g, b and the stencil in a; 16-bit formats give
/// them in r and g.
fn texel(format: DepthFormat, bits: u32) -> [u8; 4] {
    match format {
        DepthFormat::D24 | DepthFormat::F24 => {
            [(bits >> 16) as u8, (bits >> 8) as u8, bits as u8, 0xA5]
        }
        DepthFormat::D16 | DepthFormat::F16 => [(bits >> 8) as u8, bits as u8, 0, 0],
    }
}

/// A 2D depth texture of `w` by `h` raw-bit texels.
struct DepthTexture {
    w: u32,
    h: u32,
    texels: Vec<[u8; 4]>,
}

impl DepthTexture {
    fn new(format: DepthFormat, w: u32, h: u32, bits: &[u32]) -> Self {
        assert_eq!(bits.len(), (w * h) as usize);
        Self {
            w,
            h,
            texels: bits.iter().map(|&b| texel(format, b)).collect(),
        }
    }
}

impl Sampler for DepthTexture {
    fn sample(&self, stage: usize, _: TextureDim, _: [f32; 3]) -> [f32; 4] {
        panic!("stage {stage}: a shadow map was sampled as colour")
    }
    fn load(&self, stage: usize, x: u32, y: u32) -> [f32; 4] {
        assert_eq!(stage, 0);
        assert!(x < self.w && y < self.h, "texel ({x}, {y}) outside");
        self.texels[(y * self.w + x) as usize].map(|b| b as f32 / 255.0)
    }
    fn size(&self, _: usize) -> [u32; 2] {
        [self.w, self.h]
    }
}

/// Seven stored values against a reference: three below, equal, three
/// above (as in nxdk_pgraph_tests' raw-value boxes), as raw bits, plus the
/// reference as the value the texture coordinate carries.
fn ladder(format: DepthFormat) -> ([u32; 7], f32) {
    match format {
        DepthFormat::D24 => (
            [0, 1, 0x7F_FFFE, 0x7F_FFFF, 0x80_0000, 0xFF_FFFE, 0xFF_FFFF],
            0x7F_FFFF as f32,
        ),
        DepthFormat::D16 => (
            [0, 1, 0x7FFE, 0x7FFF, 0x8000, 0xFFFE, 0xFFFF],
            0x7FFF as f32,
        ),
        // 1.0 is 0x7F0000 (the top 24 bits of 0x3F800000 without the sign).
        DepthFormat::F24 => (
            [
                0, 0x01_0000, 0x7E_FFFF, 0x7F_0000, 0x7F_0001, 0xFE_FFFE, 0xFE_FFFF,
            ],
            1.0,
        ),
        // 1.0 is exponent 7, mantissa 0.
        DepthFormat::F16 => ([0, 1, 0x6FFF, 0x7000, 0x7001, 0xFFFE, 0xFFFF], 1.0),
    }
}

/// `stored OP reference` from the ordering alone (`cmp` is stored against
/// the reference).
fn expect(func: AlphaFunc, cmp: std::cmp::Ordering) -> f32 {
    use std::cmp::Ordering::*;
    let pass = match func {
        AlphaFunc::Never => false,
        AlphaFunc::Less => cmp == Less,
        AlphaFunc::Equal => cmp == Equal,
        AlphaFunc::LessEqual => cmp != Greater,
        AlphaFunc::Greater => cmp == Greater,
        AlphaFunc::NotEqual => cmp != Equal,
        AlphaFunc::GreaterEqual => cmp != Less,
        AlphaFunc::Always => true,
    };
    if pass { 1.0 } else { 0.0 }
}

fn shadow(format: DepthFormat, func: AlphaFunc, linear: bool) -> Shadow {
    Shadow {
        format,
        func,
        linear,
        address: [ShadowAddress::Clamp; 2],
    }
}

fn options(sh: Shadow) -> PixelOptions {
    let mut shadow = [None; 4];
    shadow[0] = Some(sh);
    PixelOptions {
        texture_dims: [Some(TextureDim::D2), None, None, None],
        shadow,
        ..Default::default()
    }
}

/// One fragment: the stage-0 texture coordinate and the value every
/// channel must hold.
#[derive(Clone, Copy, Debug)]
struct Frag {
    t0: [f32; 4],
    want: f32,
}

/// A draw: one program, one texture, its fragments.
struct Case {
    name: String,
    mode: u32,
    sh: Shadow,
    tex: DepthTexture,
    frags: Vec<Frag>,
}

/// Point-sampled ladders: every function, every format, both modes. The
/// coordinate is projective (q = 2) and sits on texel centres of a 7x1
/// texture.
fn ladder_cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for format in FORMATS {
        let (bits, reference) = ladder(format);
        for func in FUNCS {
            for mode in [TEX_PROJECT2D, TEX_PROJECT3D] {
                let q = 2.0;
                let frags = (0..7)
                    .map(|i| {
                        let u = (i as f32 + 0.5) / 7.0;
                        let (r, cmp) = if mode == TEX_PROJECT3D {
                            (reference, i.cmp(&3))
                        } else {
                            // PROJECT2D compares with 0: only the stored 0
                            // is equal.
                            (reference, i.cmp(&0))
                        };
                        Frag {
                            t0: [u * q, 0.5 * q, r * q, q],
                            want: expect(func, cmp),
                        }
                    })
                    .collect();
                cases.push(Case {
                    name: format!("{format:?} {func:?} mode {mode}"),
                    mode,
                    sh: shadow(format, func, false),
                    tex: DepthTexture::new(format, 7, 1, &bits),
                    frags,
                });
            }
        }
    }
    cases
}

/// PROJECT3D clamps `r/q` to `[0, max]`: a reference past the format's
/// range equals the largest stored value, a negative one equals 0.
fn clamp_cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for (format, top, beyond) in [
        (DepthFormat::D24, 0xFF_FFFF, 2.0e7),
        (DepthFormat::D16, 0xFFFF, 7.0e4),
        (DepthFormat::F16, 0xFFFF, 600.0),
    ] {
        let frags = vec![
            Frag {
                t0: [0.25, 0.5, beyond, 1.0],
                want: 1.0,
            },
            Frag {
                t0: [0.75, 0.5, -3.0, 1.0],
                want: 1.0,
            },
        ];
        cases.push(Case {
            name: format!("{format:?} clamp"),
            mode: TEX_PROJECT3D,
            sh: shadow(format, AlphaFunc::Equal, false),
            tex: DepthTexture::new(format, 2, 1, &[top, 0]),
            frags,
        });
    }
    cases
}

/// F16 words with a zero exponent are not denormals: 0x0800 is
/// 0.01171875, above a reference of 0.01, and 0x0400 (0.009765625) below.
fn f16_low_exponent_cases() -> Vec<Case> {
    vec![Case {
        name: "F16 zero exponent".into(),
        mode: TEX_PROJECT3D,
        sh: shadow(DepthFormat::F16, AlphaFunc::Greater, false),
        tex: DepthTexture::new(DepthFormat::F16, 2, 1, &[0x0800, 0x0400]),
        frags: vec![
            Frag {
                t0: [0.25, 0.5, 0.01, 1.0],
                want: 1.0,
            },
            Frag {
                t0: [0.75, 0.5, 0.01, 1.0],
                want: 0.0,
            },
        ],
    }]
}

/// Linear filtering is 2x2 percentage-closer filtering: each texel is
/// compared and the results weighted. On a 2x2 texture the coordinate
/// (0.375, 0.625) sits a quarter of the way across and three quarters of
/// the way down from texel (0, 0)'s centre.
fn pcf_cases() -> Vec<Case> {
    let mut cases = Vec::new();
    // Stored 0xABCDEF beside 0xABCDF0 against reference 0xABCDEF: any
    // filtering of the depth values before comparing would blur these.
    let lo = 0xAB_CDEF;
    let hi = 0xAB_CDF0;
    let bits = [lo, hi, hi, lo];
    let r = lo as f32;
    for (linear, want) in [
        // s00 = 0, s10 = 1, s01 = 1, s11 = 0:
        // mix(mix(0, 1, 0.25), mix(1, 0, 0.25), 0.75) = 0.625.
        (true, 0.625),
        // Point: texel floor((0.75, 1.25)) = (0, 1), stored hi.
        (false, 1.0),
    ] {
        cases.push(Case {
            name: format!("pcf linear={linear}"),
            mode: TEX_PROJECT3D,
            sh: shadow(DepthFormat::D24, AlphaFunc::Greater, linear),
            tex: DepthTexture::new(DepthFormat::D24, 2, 2, &bits),
            frags: vec![
                Frag {
                    t0: [0.375, 0.625, r, 1.0],
                    want,
                },
                // On a texel centre linear filtering reads one texel.
                Frag {
                    t0: [0.75, 0.25, r, 1.0],
                    want: 1.0,
                },
            ],
        });
    }
    // Addressing applies to texel indices. At (1/16, 1/4), p = (-0.375,
    // 0): x0 is texel -1 with weight 0.375 and x1 texel 0 with weight
    // 0.625, both on row 0 (lo, hi), where GREATER passes on hi only.
    // Wrapping makes -1 texel 1 (hi); clamping and mirroring make it 0.
    for (address, want) in [
        (ShadowAddress::Wrap, 0.375),
        (ShadowAddress::Clamp, 0.0),
        (ShadowAddress::Mirror, 0.0),
    ] {
        let mut sh = shadow(DepthFormat::D24, AlphaFunc::Greater, true);
        sh.address = [address, ShadowAddress::Clamp];
        cases.push(Case {
            name: format!("pcf address {address:?}"),
            mode: TEX_PROJECT3D,
            sh,
            tex: DepthTexture::new(DepthFormat::D24, 2, 2, &[lo, hi, lo, lo]),
            frags: vec![Frag {
                t0: [0.0625, 0.25, r, 1.0],
                want,
            }],
        });
    }
    cases
}

fn run_interp(c: &Case) -> Vec<[f32; 4]> {
    let d = def(c.mode);
    let opts = options(c.sh);
    c.frags
        .iter()
        .map(|f| {
            let mut t = [[0.0; 4]; 4];
            t[0] = f.t0;
            let i = Inputs {
                t,
                front: true,
                ..Default::default()
            };
            interp::run(&d, &opts, &i, &PixelUniforms::default(), &c.tex).expect("not killed")
        })
        .collect()
}

fn all_cases() -> Vec<Case> {
    let mut v = ladder_cases();
    v.extend(clamp_cases());
    v.extend(pcf_cases());
    v.extend(f16_low_exponent_cases());
    v
}

#[test]
fn addressing_of_texel_indices() {
    use ShadowAddress::*;
    let n = 4;
    let got = |a: ShadowAddress| (-5..9).map(|i| a.apply(i, n)).collect::<Vec<_>>();
    assert_eq!(got(Wrap), [3, 0, 1, 2, 3, 0, 1, 2, 3, 0, 1, 2, 3, 0]);
    assert_eq!(got(Mirror), [3, 3, 2, 1, 0, 0, 1, 2, 3, 3, 2, 1, 0, 0]);
    assert_eq!(got(Clamp), [0, 0, 0, 0, 0, 0, 1, 2, 3, 3, 3, 3, 3, 3]);
    // No overflow at the ends of i32.
    assert_eq!(Wrap.apply(i32::MIN, 3), 1);
    assert_eq!(Mirror.apply(i32::MAX, 3), Mirror.apply(i32::MAX % 6, 3));
}

#[test]
fn raw_bits_decode_exactly() {
    let f = |c: [u8; 4]| c.map(|b| b as f32 / 255.0);
    assert_eq!(
        DepthFormat::D24.decode(f([0xAB, 0xCD, 0xEF, 0x12])),
        0xAB_CDEF as f32
    );
    assert_eq!(
        DepthFormat::D24.decode(f([0xFF, 0xFF, 0xFF, 0])),
        16_777_215.0
    );
    assert_eq!(
        DepthFormat::D16.decode(f([0xBE, 0xEF, 0, 0])),
        0xBEEF as f32
    );
    // F24: the top 24 bits of an IEEE single without its sign.
    assert_eq!(DepthFormat::F24.decode(f([0x7F, 0x00, 0x00, 0])), 1.0);
    assert_eq!(DepthFormat::F24.decode(f([0x80, 0x80, 0x00, 0])), 3.0);
    assert_eq!(
        DepthFormat::F24.decode(f([0xFE, 0xFF, 0xFF, 0])),
        DepthFormat::F24.max()
    );
    // F16: 4-bit exponent (bias 7), 12-bit mantissa, no denormals: every
    // word but 0 has the implicit one (xemu's convert_f16_to_float).
    let f16 = |bits: u32| DepthFormat::F16.decode(f(texel(DepthFormat::F16, bits)));
    assert_eq!(f16(0x7000), 1.0);
    assert_eq!(f16(0x7800), 1.5);
    assert_eq!(f16(0x0000), 0.0);
    assert_eq!(f16(0x0001), (1.0 + 2f32.powi(-12)) * 2f32.powi(-7));
    assert_eq!(f16(0x0800), 0.011_718_75);
    assert_eq!(f16(0x0FFF), (2.0 - 2f32.powi(-12)) * 2f32.powi(-7));
    assert_eq!(f16(0x1000), 2f32.powi(-6));
    // F24 keeps IEEE's reading: the zero word is 0.
    assert_eq!(DepthFormat::F24.decode(f([0, 0, 0, 0])), 0.0);
    assert_eq!(DepthFormat::F16.decode(f([0xFF, 0xFF, 0, 0])), 511.9375);
    assert_eq!(DepthFormat::F16.max(), 511.9375);
    // Every 16-bit pattern is monotonic in its value.
    let mut last = -1.0;
    for bits in 0..=0xFFFFu32 {
        let v = DepthFormat::F16.decode(f(texel(DepthFormat::F16, bits)));
        assert!(v > last, "{bits:#x}");
        last = v;
    }
}

#[test]
fn ladder_values_order_as_intended() {
    for format in FORMATS {
        let (bits, reference) = ladder(format);
        let values: Vec<f32> = bits
            .iter()
            .map(|&b| format.decode(texel(format, b).map(|x| x as f32 / 255.0)))
            .collect();
        assert_eq!(values[3], reference, "{format:?}");
        assert!(
            values.windows(2).all(|w| w[0] < w[1]),
            "{format:?} {values:?}"
        );
        assert_eq!(values[0], 0.0);
    }
}

#[test]
fn interpreter_compares_every_function_and_format() {
    for c in all_cases() {
        let got = run_interp(&c);
        for (f, g) in c.frags.iter().zip(&got) {
            assert_eq!(*g, [f.want; 4], "{}: {f:?}", c.name);
        }
    }
}

#[test]
fn never_and_always_do_not_sample() {
    for (func, want) in [(AlphaFunc::Never, 0.0), (AlphaFunc::Always, 1.0)] {
        for mode in [TEX_PROJECT2D, TEX_PROJECT3D] {
            let opts = options(shadow(DepthFormat::D24, func, true));
            let mut t = [[0.0; 4]; 4];
            t[0] = [0.5, 0.5, 0.5, 1.0];
            let i = Inputs {
                t,
                ..Default::default()
            };
            // NoTextures panics on any lookup.
            let c = interp::run(
                &def(mode),
                &opts,
                &i,
                &PixelUniforms::default(),
                &NoTextures,
            );
            assert_eq!(c, Some([want; 4]));
            let ps = psh::translate(&def(mode), &opts);
            assert!(!ps.body.contains("textureLoad"), "{func:?}");
            assert!(ps.body.contains(&format!("t0 = vec4f({want:?});")));
        }
    }
}

#[test]
fn translations_validate_without_warnings() {
    for c in all_cases() {
        let ps = psh::translate(&def(c.mode), &options(c.sh));
        assert!(ps.warnings.is_empty(), "{}: {:?}", c.name, ps.warnings);
        assert_eq!(ps.textures[0], Some(TextureDim::D2));
        common::check(&ps.module());
    }
}

#[test]
fn colour_textures_and_other_modes_are_unchanged() {
    // No shadow option: the usual sample.
    let plain = psh::translate(&def(TEX_PROJECT3D), &PixelOptions::default());
    assert!(!plain.body.contains("xps_shadow"));
    // A shadow option on a cube map has nothing to compare: sampled as
    // colour, with a warning.
    let mut opts = options(shadow(DepthFormat::D24, AlphaFunc::Greater, false));
    opts.texture_dims[0] = Some(TextureDim::Cube);
    let ps = psh::translate(&def(TEX_CUBEMAP), &opts);
    assert!(
        ps.warnings.iter().any(|w| w.contains("shadow map")),
        "{:?}",
        ps.warnings
    );
    assert!(!ps.body.contains("textureLoad"));
}

const HARNESS: &str = r#"
@group(1) @binding(8) var<storage, read> hin: array<vec4f>;
@group(1) @binding(9) var<storage, read_write> hout: array<vec4f>;
@compute @workgroup_size(1)
fn harness(@builtin(global_invocation_id) id: vec3u) {
    let z = vec4f(0.0);
    let r = xps_program(XpsInputs(z, z, z, z, hin[id.x], z, z, z, 0.0, true));
    hout[id.x] = r.color;
}
"#;

#[test]
fn gpu_compares_every_function_and_format() {
    let Some(gpu) = common::gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut compared = 0;
    for c in all_cases() {
        let ps = psh::translate(&def(c.mode), &options(c.sh));
        let src = format!("{}{}", ps.module(), HARNESS);
        common::check(&src);
        let tex = common::GpuTexture {
            binding: 0,
            view: gpu.texture(
                wgpu::TextureViewDimension::D2,
                [c.tex.w, c.tex.h, 1],
                &c.tex.texels,
            ),
            dim: wgpu::TextureViewDimension::D2,
        };
        let input: Vec<f32> = c.frags.iter().flat_map(|f| f.t0).collect();
        let out = gpu.run_with_textures(
            &src,
            "harness",
            1,
            &PixelUniforms::default().to_bytes(),
            &input,
            c.frags.len() * 4,
            c.frags.len() as u32,
            &[tex],
        );
        let cpu = run_interp(&c);
        for (k, f) in c.frags.iter().enumerate() {
            let g = &out[k * 4..k * 4 + 4];
            assert_eq!(g, [f.want; 4], "{}: fragment {k} {f:?}", c.name);
            assert_eq!(g, cpu[k], "{}: fragment {k} interpreter", c.name);
            compared += 1;
        }
    }
    eprintln!("compared {compared} fragments");
    assert!(compared > 400);
}

// --- Hard shadows (E3 lighting, docs/graphics/e3-lighting.md, "The edge") ---

/// The programs the golden files hold: retail's lookups as the game's
/// first level uses them (linear PROJECT3D `GREATER` on D24) and a point
/// PROJECT2D one, and a pass sampling one map on two stages.
fn golden_programs(hard: bool) -> Vec<(String, PixelShaderDef, PixelOptions)> {
    let mut out = Vec::new();
    for (name, mode, sh) in [
        (
            "project3d linear d24 greater",
            TEX_PROJECT3D,
            shadow(DepthFormat::D24, AlphaFunc::Greater, true),
        ),
        (
            "project2d point d16 less",
            TEX_PROJECT2D,
            shadow(DepthFormat::D16, AlphaFunc::Less, false),
        ),
    ] {
        let mut opts = options(sh);
        if hard {
            opts.hard_shadow[0] = 1;
        }
        out.push((name.to_string(), def(mode), opts));
    }
    let sh = shadow(DepthFormat::D24, AlphaFunc::Greater, true);
    let mut opts = two_stage_options(sh);
    if hard {
        opts.hard_shadow = [0b11, 0b11, 0, 0];
    }
    out.push(("two stages, one map".into(), two_stage_def(REG_T1), opts));
    out
}

/// Every golden program's module, each under a header line.
fn golden_text(hard: bool) -> String {
    let mut s = String::new();
    for (name, d, opts) in golden_programs(hard) {
        let ps = psh::translate(&d, &opts);
        assert!(ps.warnings.is_empty(), "{name}: {:?}", ps.warnings);
        common::check(&ps.module());
        s.push_str(&format!("// ---- {name}\n{}\n", ps.module()));
    }
    s
}

/// Compares `text` with `tests/golden/<name>`; `SHADOW_GOLDEN_BLESS=1`
/// writes it instead.
fn golden(name: &str, text: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os("SHADOW_GOLDEN_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert!(want == text, "{name} differs from the golden file");
}

/// Retail's translation is what it was before hard shadows existed: the
/// golden file was written by the code before the option.
#[test]
fn retail_shadow_lookups_are_unchanged() {
    golden("shadow_retail.wgsl", &golden_text(false));
    // And the option at zero is retail, whatever else is set.
    let mut opts = options(shadow(DepthFormat::D24, AlphaFunc::Greater, true));
    let retail = psh::translate(&def(TEX_PROJECT3D), &opts).module();
    opts.hard_shadow = [0; 4];
    assert_eq!(psh::translate(&def(TEX_PROJECT3D), &opts).module(), retail);
    // Left out of persisted keys at zero, so existing caches still match.
    let json = serde_json::to_string(&opts).unwrap();
    assert!(!json.contains("hard_shadow"), "{json}");
}

/// E3's translation: one `textureLoad` per shadow stage and no filter
/// weights, at the stage's coordinate or the mean of its group's.
#[test]
fn e3_shadow_lookups_are_one_tap() {
    golden("shadow_e3.wgsl", &golden_text(true));
    for (name, d, opts) in golden_programs(true) {
        let ps = psh::translate(&d, &opts);
        for i in 0..2 {
            if opts.shadow[i].is_none() || !ps.body.contains(&format!("t{i} = vec4f(select")) {
                continue;
            }
            let loads = ps.body.matches(&format!("textureLoad(xps_tex{i},")).count();
            assert_eq!(loads, 1, "{name}: stage {i}");
        }
        assert!(!ps.body.contains("mix("), "{name}: no filter weights");
    }
    let (_, d, opts) = golden_programs(true).pop().unwrap();
    let ps = psh::translate(&d, &opts);
    assert!(
        ps.body.contains("let uv = ((i.t0.xy / i.t0.w) * xps.tex_scale[0].xy + (i.t1.xy / i.t1.w) * xps.tex_scale[1].xy) / 2.0;"),
        "{}",
        ps.body
    );
}

/// A hard stage is the point lookup, whatever the stage's filter: the
/// 2x2 case `pcf_cases` filters to 0.625 reads one texel, 1.
#[test]
fn a_hard_stage_compares_once_on_the_interpreter_and_the_gpu() {
    let lo = 0xAB_CDEF;
    let hi = 0xAB_CDF0;
    let tex = DepthTexture::new(DepthFormat::D24, 2, 2, &[lo, hi, hi, lo]);
    let r = lo as f32;
    let frags = [
        ([0.375, 0.625, r, 1.0], 1.0),
        ([0.75, 0.25, r, 1.0], 1.0),
        ([0.25, 0.25, r, 1.0], 0.0),
    ];
    let mut opts = options(shadow(DepthFormat::D24, AlphaFunc::Greater, true));
    opts.hard_shadow[0] = 1;
    let d = def(TEX_PROJECT3D);
    for (t0, want) in frags {
        let mut t = [[0.0; 4]; 4];
        t[0] = t0;
        let i = Inputs {
            t,
            ..Default::default()
        };
        let got = interp::run(&d, &opts, &i, &PixelUniforms::default(), &tex);
        assert_eq!(got, Some([want; 4]), "{t0:?}");
    }
    let Some(gpu) = common::gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let ps = psh::translate(&d, &opts);
    let src = format!("{}{}", ps.module(), HARNESS);
    let input: Vec<f32> = frags.iter().flat_map(|(t, _)| *t).collect();
    let out = gpu.run_with_textures(
        &src,
        "harness",
        1,
        &PixelUniforms::default().to_bytes(),
        &input,
        frags.len() * 4,
        frags.len() as u32,
        &[common::GpuTexture {
            binding: 0,
            view: gpu.texture(wgpu::TextureViewDimension::D2, [2, 2, 1], &tex.texels),
            dim: wgpu::TextureViewDimension::D2,
        }],
    );
    for (k, (_, want)) in frags.iter().enumerate() {
        assert_eq!(&out[k * 4..k * 4 + 4], [*want; 4], "fragment {k}");
    }
}

/// Stages 0 and 1 in PROJECT3D; the output is `reg` (t0 or t1).
fn two_stage_def(reg: u8) -> PixelShaderDef {
    PixelShaderDef {
        texture_modes: texture_modes([TEX_PROJECT3D, TEX_PROJECT3D, TEX_NONE, TEX_NONE]),
        final_inputs_abcd: inputs(0, 0, 0, reg),
        final_inputs_efg: final_efg(0, 0, reg | CHAN_ALPHA, 0),
        ..Default::default()
    }
}

fn two_stage_options(sh: Shadow) -> PixelOptions {
    PixelOptions {
        texture_dims: [Some(TextureDim::D2), Some(TextureDim::D2), None, None],
        shadow: [Some(sh), Some(sh), None, None],
        ..Default::default()
    }
}

/// One texture on every stage.
struct Shared<'a>(&'a DepthTexture);

impl Sampler for Shared<'_> {
    fn sample(&self, stage: usize, _: TextureDim, _: [f32; 3]) -> [f32; 4] {
        panic!("stage {stage}: a shadow map was sampled as colour")
    }
    fn load(&self, _: usize, x: u32, y: u32) -> [f32; 4] {
        self.0.load(0, x, y)
    }
    fn size(&self, _: usize) -> [u32; 2] {
        self.0.size(0)
    }
}

/// Taps a pass spreads over stages of one map collapse onto one: stage 0
/// at texel 0 (passes) and stage 1 at texel 1 (fails) both compare at
/// their mean, texel 1. Retail compares each where it is.
#[test]
fn hard_stages_of_one_map_compare_at_their_mean_coordinate() {
    let lo = 0xAB_CDEF;
    let hi = 0xAB_CDF0;
    let tex = DepthTexture::new(DepthFormat::D24, 2, 1, &[hi, lo]);
    let r = lo as f32;
    let (t0, t1) = ([0.3, 0.5, r, 1.0], [0.8, 0.5, r, 1.0]);
    let mut t = [[0.0; 4]; 4];
    t[0] = t0;
    t[1] = t1;
    let inputs_ = Inputs {
        t,
        ..Default::default()
    };
    let sh = shadow(DepthFormat::D24, AlphaFunc::Greater, false);
    let retail = two_stage_options(sh);
    let mut hard = retail;
    hard.hard_shadow = [0b11, 0b11, 0, 0];
    let run = |reg, opts: &PixelOptions| {
        interp::run(
            &two_stage_def(reg),
            opts,
            &inputs_,
            &PixelUniforms::default(),
            &Shared(&tex),
        )
        .unwrap()[0]
    };
    assert_eq!((run(REG_T0, &retail), run(REG_T1, &retail)), (1.0, 0.0));
    assert_eq!((run(REG_T0, &hard), run(REG_T1, &hard)), (0.0, 0.0));
    // Each stage its own group: its own coordinate, as retail point.
    let mut alone = retail;
    alone.hard_shadow = [0b01, 0b10, 0, 0];
    assert_eq!((run(REG_T0, &alone), run(REG_T1, &alone)), (1.0, 0.0));
    let Some(gpu) = common::gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    const HARNESS2: &str = r#"
@group(1) @binding(8) var<storage, read> hin: array<vec4f>;
@group(1) @binding(9) var<storage, read_write> hout: array<vec4f>;
@compute @workgroup_size(1)
fn harness(@builtin(global_invocation_id) id: vec3u) {
    let z = vec4f(0.0);
    let r = xps_program(XpsInputs(z, z, z, z, hin[2u * id.x], hin[2u * id.x + 1u], z, z, 0.0, true));
    hout[id.x] = r.color;
}
"#;
    for (opts, want) in [
        (&retail, [1.0, 0.0]),
        (&hard, [0.0, 0.0]),
        (&alone, [1.0, 0.0]),
    ] {
        for (k, reg) in [REG_T0, REG_T1].into_iter().enumerate() {
            let ps = psh::translate(&two_stage_def(reg), opts);
            let src = format!("{}{}", ps.module(), HARNESS2);
            common::check(&src);
            let texture = |binding| common::GpuTexture {
                binding,
                view: gpu.texture(wgpu::TextureViewDimension::D2, [2, 1, 1], &tex.texels),
                dim: wgpu::TextureViewDimension::D2,
            };
            let input: Vec<f32> = t0.iter().chain(&t1).copied().collect();
            let out = gpu.run_with_textures(
                &src,
                "harness",
                1,
                &PixelUniforms::default().to_bytes(),
                &input,
                4,
                1,
                &[texture(0), texture(2)],
            );
            assert_eq!(out, [want[k]; 4], "{:?} t{k}", opts.hard_shadow);
        }
    }
}
