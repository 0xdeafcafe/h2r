//! Shadow-map sampling: PROJECT2D and PROJECT3D on a depth-format texture.
//!
//! On the NV2A a texture in a depth format (`D24S8`, `D16` and the float
//! variants `F24S8`, `F16`, swizzled or linear) is always a shadow map.
//! Sampling compares the stored depth with a reference under `SHADOWCTL`'s
//! function (D3D's `D3DRS_SHADOWFUNC`) and gives 1 or 0 in every channel.
//! The behaviour follows xemu's `psh_append_shadowmap` (read for facts
//! only, see docs/graphics/shaders.md):
//!
//! * the comparison is `stored OP reference`, with `OP` the function's
//!   value (`0x204`, `GREATER`, passes where the stored depth is greater);
//! * PROJECT2D compares with 0;
//! * PROJECT3D compares with `r/q`, clamped to the format's depth range
//!   (16777215 for `D24`, 65535 for `D16`; for the float formats the
//!   largest finite value, gfx's `D3DZ_MAX` for them, where xemu clamps
//!   any 24-bit format to 16777215 and F16 to 511.9375);
//! * NEVER and ALWAYS give 0 and 1 without sampling.
//!
//! abaire's `nxdk_pgraph_tests` (`texture_shadow_comparator_tests.cpp`)
//! and their hardware results agree once its names are read as the
//! register values: its `GREATER` is register value 1, xemu's (and D3D's)
//! `LESS`, and on hardware passes where the stored depth is below the
//! reference.
//!
//! Filtering. NVIDIA's GeForce3-class shadow mapping (the NV2A is an NV20
//! derivative) compares each texel and filters the results: with linear
//! filtering the four neighbouring texels are compared and the 1/0
//! results bilinearly weighted (2x2 percentage-closer filtering; NVIDIA,
//! "Hardware Shadow Mapping", Everitt, Rege and Cebenoyan). xemu instead
//! filters the depth values and compares once, which this does not copy.
//! Here the stage's texels are fetched with `textureLoad` (no host
//! filtering of the raw bits), decoded exactly, compared, and weighted in
//! the shader: one texel for point filtering, four for linear. Addressing
//! (wrap, mirror, clamp) is applied to the texel indices; `BORDER` is
//! treated as clamp. Only mip level 0 is read.
//!
//! **Hard shadows** (E3 lighting, docs/graphics/e3-lighting.md, "The edge"):
//! [`super::PixelOptions::hard_shadow`] makes a stage compare once, at the
//! nearest texel, with no percentage-closer filter. When several stages of
//! the program sample the same shadow map (taps a pass spreads over
//! stages and averages in the combiners), each compares at the mean of
//! their coordinates, so the average is one tap too. Only the
//! render-scale view sets it; the backend the guest reads never does.
//!
//! **Sharp shadows** average four bilinear comparisons offset by half a
//! texel along each axis. The resulting 3x3 filter gives continuous coverage
//! across texel boundaries. A ramp centred at half coverage uses the screen
//! gradient to keep the transition about one pixel wide.
//!
//! The renderer hands depth textures over as their raw bits in RGBA8
//! (`gfx::convert::depth_channels`): 32-bit formats give the depth bytes,
//! most significant first, in red, green and blue; 16-bit formats give
//! them in red and green. `round(c * 255)` recovers each byte.

use super::AlphaFunc;
use super::interp::{Sampler, Vec4};
use std::fmt::Write;

/// The depth comparison, as `D3DCMPFUNC` (`stored OP reference`).
pub type CompareFunc = AlphaFunc;

/// The stored depth format of a shadow map.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum DepthFormat {
    /// 24-bit integer depth (`D24S8`, `LIN_D24S8`).
    D24,
    /// 16-bit integer depth (`D16`, `LIN_D16`).
    D16,
    /// 24-bit float depth (`F24S8`, `LIN_F24S8`): 8-bit exponent (bias
    /// 127) and 16-bit mantissa, no sign; the top 24 bits of an IEEE
    /// single without its sign bit (xemu's `convert_f24_to_float` in
    /// `pgraph/util.h` reads it the same way). A zero exponent gives an
    /// IEEE denormal, which the host GPU may flush to zero.
    F24,
    /// 16-bit float depth (`F16`, `LIN_F16`): 4-bit exponent (bias 7) and
    /// 12-bit mantissa, no sign, no denormals and no infinities. Every
    /// word but 0 has the implicit leading one, exponent 0 included
    /// (`0x0800` is `1.5 * 2^-7`); only the all-zero word is 0. This
    /// follows xemu's `convert_f16_to_float` (`pgraph/util.h`).
    F16,
}

impl DepthFormat {
    /// The largest stored value: PROJECT3D clamps `r/q` to `[0, max]`.
    /// These are the `D3DZ_MAX_*` values gfx uses for window z.
    pub fn max(self) -> f32 {
        match self {
            Self::D24 => 16_777_215.0,
            Self::D16 => 65_535.0,
            // (2 - 2^-16) * 2^127
            Self::F24 => 3.402_797_7e38,
            // (2 - 2^-12) * 2^8
            Self::F16 => 511.9375,
        }
    }

    /// The stored depth of a texel given as its raw-bit channels.
    pub fn decode(self, texel: Vec4) -> f32 {
        let b = texel.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u32);
        let d24 = (b[0] << 16) | (b[1] << 8) | b[2];
        let d16 = (b[0] << 8) | b[1];
        match self {
            Self::D24 => d24 as f32,
            Self::D16 => d16 as f32,
            Self::F24 => f32::from_bits(d24 << 7),
            Self::F16 if d16 == 0 => 0.0,
            Self::F16 => {
                // Rebias 7 to IEEE's 127 and widen the mantissa to 23 bits.
                let (e, m) = (d16 >> 12, d16 & 0xFFF);
                f32::from_bits(((e + 120) << 23) | (m << 11))
            }
        }
    }

    fn wgsl_decoder(self) -> &'static str {
        match self {
            Self::D24 => "xps_shadow_d24",
            Self::D16 => "xps_shadow_d16",
            Self::F24 => "xps_shadow_f24",
            Self::F16 => "xps_shadow_f16",
        }
    }
}

/// Texel addressing for one axis of a shadow map.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ShadowAddress {
    /// `D3DTADDRESS_WRAP`.
    #[default]
    Wrap,
    /// `D3DTADDRESS_MIRROR`.
    Mirror,
    /// `D3DTADDRESS_CLAMP`, `CLAMPTOEDGE` and (approximated) `BORDER`.
    Clamp,
}

impl ShadowAddress {
    fn code(self) -> u32 {
        match self {
            Self::Wrap => 0,
            Self::Mirror => 1,
            Self::Clamp => 2,
        }
    }

    /// The texel index `i` lands on in a row of `n` texels.
    pub fn apply(self, i: i32, n: i32) -> i32 {
        match self {
            Self::Wrap => ((i % n) + n) % n,
            Self::Mirror => {
                let p = 2 * n;
                let m = ((i % p) + p) % p;
                if m >= n { p - 1 - m } else { m }
            }
            Self::Clamp => i.clamp(0, n - 1),
        }
    }
}

/// A stage that samples a shadow map (part of [`super::PixelOptions`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Shadow {
    /// Stored depth format.
    pub format: DepthFormat,
    /// `D3DRS_SHADOWFUNC`.
    pub func: CompareFunc,
    /// Bilinear percentage-closer filtering (the stage filters linearly);
    /// otherwise the nearest texel.
    pub linear: bool,
    /// Addressing in u and v.
    pub address: [ShadowAddress; 2],
}

/// WGSL helpers the shadow stages call; emitted once per module.
pub(crate) const WGSL_HELPERS: &str = r#"
// Shadow maps: raw-bit RGBA8 texels back to the stored depth, and texel
// addressing (0 wrap, 1 mirror, 2 clamp).
fn xps_shadow_bytes(c: vec4f) -> vec4u { return vec4u(round(clamp(c, vec4f(0.0), vec4f(1.0)) * 255.0)); }
fn xps_shadow_d24(c: vec4f) -> f32 { let b = xps_shadow_bytes(c); return f32((b.x << 16u) | (b.y << 8u) | b.z); }
fn xps_shadow_d16(c: vec4f) -> f32 { let b = xps_shadow_bytes(c); return f32((b.x << 8u) | b.y); }
fn xps_shadow_f24(c: vec4f) -> f32 {
    let b = xps_shadow_bytes(c);
    return bitcast<f32>(((b.x << 16u) | (b.y << 8u) | b.z) << 7u);
}
fn xps_shadow_f16(c: vec4f) -> f32 {
    let b = xps_shadow_bytes(c);
    let d = (b.x << 8u) | b.y;
    let e = d >> 12u;
    let m = d & 0xFFFu;
    if (d == 0u) { return 0.0; }
    return bitcast<f32>(((e + 120u) << 23u) | (m << 11u));
}
fn xps_shadow_addr(i: i32, n: i32, mode: u32) -> i32 {
    if (mode == 0u) { return ((i % n) + n) % n; }
    if (mode == 1u) {
        let p = 2 * n;
        let m = ((i % p) + p) % p;
        return select(m, p - 1 - m, m >= n);
    }
    return clamp(i, 0, n - 1);
}
"#;

/// How a stage filters its depth comparisons.
#[derive(Clone, Copy)]
pub(super) enum Filter {
    Retail,
    Hard(u8),
    Sharp { derivatives: bool },
}

impl Filter {
    /// The stages a hard stage takes the mean coordinate of.
    fn hard_stages(self, i: usize) -> Vec<usize> {
        let Self::Hard(group) = self else {
            return Vec::new();
        };
        let group = group | 1 << i;
        (0..4).filter(|j| group >> j & 1 != 0).collect()
    }
}

/// The statement block that sets `t{i}` for a shadow-map stage.
/// `project3d` selects the `r/q` reference; otherwise it is 0.
pub(super) fn wgsl_stage(i: usize, sh: &Shadow, project3d: bool, filter: Filter) -> String {
    let op = match sh.func {
        AlphaFunc::Never => return format!("t{i} = vec4f(0.0);"),
        AlphaFunc::Always => return format!("t{i} = vec4f(1.0);"),
        f => f.op().expect("comparison"),
    };
    let tc = format!("i.t{i}");
    let mut s = String::from("{\n");
    let group = filter.hard_stages(i);
    if group.len() > 1 {
        let taps: Vec<String> = group
            .iter()
            .map(|j| format!("(i.t{j}.xy / i.t{j}.w) * xps.tex_scale[{j}].xy"))
            .collect();
        let _ = writeln!(
            s,
            "        let uv = ({}) / {:?};",
            taps.join(" + "),
            group.len() as f32
        );
    } else {
        let _ = writeln!(
            s,
            "        let uv = ({tc}.xy / {tc}.w) * xps.tex_scale[{i}].xy;"
        );
    }
    if project3d {
        let _ = writeln!(
            s,
            "        let r = clamp({tc}.z / {tc}.w, 0.0, {:?});",
            sh.format.max()
        );
    } else {
        s.push_str("        let r = 0.0;\n");
    }
    let _ = writeln!(
        s,
        "        let size = vec2i(textureDimensions(xps_tex{i}, 0));"
    );
    let [au, av] = sh.address.map(ShadowAddress::code);
    let dec = sh.format.wgsl_decoder();
    let test = |x: &str, y: &str| {
        format!("select(0.0, 1.0, {dec}(textureLoad(xps_tex{i}, vec2i({x}, {y}), 0)) {op} r)")
    };
    if let Filter::Sharp { derivatives } = filter {
        // The two bilinear taps on either side of an axis share the middle
        // texel. Fold them together before sampling (nine loads).
        s.push_str("        let p = uv * vec2f(size);\n");
        s.push_str("        let base = vec2i(floor(p));\n");
        s.push_str("        let f = fract(p);\n");
        s.push_str("        let wx = vec3f(1.0 - f.x, 1.0, f.x) * 0.5;\n");
        s.push_str("        let wy = vec3f(1.0 - f.y, 1.0, f.y) * 0.5;\n");
        s.push_str("        var coverage = 0.0;\n");
        s.push_str("        for (var row = 0; row < 3; row++) {\n");
        let _ = writeln!(s, "            let y = xps_shadow_addr(base.y + row - 1, size.y, {av}u);");
        s.push_str("            for (var col = 0; col < 3; col++) {\n");
        let _ = writeln!(s, "                let x = xps_shadow_addr(base.x + col - 1, size.x, {au}u);");
        let _ = writeln!(s, "                coverage += wx[col] * wy[row] * {};", test("x", "y"));
        s.push_str("            }\n        }\n");
        if derivatives {
            s.push_str("        let width = max(length(vec2f(dpdxFine(coverage), dpdyFine(coverage))), 1e-6);\n");
        } else {
            s.push_str("        let width = 1e-6;\n");
        }
        let _ = writeln!(s, "        t{i} = vec4f(clamp(0.5 + (coverage - 0.5) / width, 0.0, 1.0));");
    } else if sh.linear && group.is_empty() {
        s.push_str("        let p = uv * vec2f(size) - 0.5;\n");
        s.push_str("        let base = vec2i(floor(p));\n");
        s.push_str("        let f = p - floor(p);\n");
        let _ = writeln!(
            s,
            "        let x0 = xps_shadow_addr(base.x, size.x, {au}u);\n        let x1 = xps_shadow_addr(base.x + 1, size.x, {au}u);"
        );
        let _ = writeln!(
            s,
            "        let y0 = xps_shadow_addr(base.y, size.y, {av}u);\n        let y1 = xps_shadow_addr(base.y + 1, size.y, {av}u);"
        );
        let _ = writeln!(s, "        let s00 = {};", test("x0", "y0"));
        let _ = writeln!(s, "        let s10 = {};", test("x1", "y0"));
        let _ = writeln!(s, "        let s01 = {};", test("x0", "y1"));
        let _ = writeln!(s, "        let s11 = {};", test("x1", "y1"));
        let _ = writeln!(
            s,
            "        t{i} = vec4f(mix(mix(s00, s10, f.x), mix(s01, s11, f.x), f.y));"
        );
    } else {
        s.push_str("        let texel = vec2i(floor(uv * vec2f(size)));\n");
        let _ = writeln!(
            s,
            "        let x = xps_shadow_addr(texel.x, size.x, {au}u);\n        let y = xps_shadow_addr(texel.y, size.y, {av}u);"
        );
        let _ = writeln!(s, "        t{i} = vec4f({});", test("x", "y"));
    }
    s.push_str("    }");
    s
}

/// The reference evaluation of [`wgsl_stage`]: `tc` are the stages'
/// texture coordinates and `scales` their `tex_scale`. A single fragment
/// has no screen derivatives, so Sharp uses a zero-width footprint.
pub(super) fn sample(
    stage: usize,
    sh: &Shadow,
    project3d: bool,
    filter: Filter,
    tc: &[Vec4; 4],
    scales: &[Vec4; 4],
    sampler: &dyn Sampler,
) -> Vec4 {
    match sh.func {
        AlphaFunc::Never => return [0.0; 4],
        AlphaFunc::Always => return [1.0; 4],
        _ => {}
    }
    let t = tc[stage];
    let uv_of = |j: usize| {
        let (t, s) = (tc[j], scales[j]);
        [(t[0] / t[3]) * s[0], (t[1] / t[3]) * s[1]]
    };
    let group = filter.hard_stages(stage);
    let uv = if group.len() > 1 {
        // Summed in stage order, then divided, as the translation does.
        let mut sum = uv_of(group[0]);
        for &j in &group[1..] {
            let p = uv_of(j);
            sum = [sum[0] + p[0], sum[1] + p[1]];
        }
        let n = group.len() as f32;
        [sum[0] / n, sum[1] / n]
    } else {
        uv_of(stage)
    };
    let r = if project3d {
        (t[2] / t[3]).clamp(0.0, sh.format.max())
    } else {
        0.0
    };
    let [w, h] = sampler.size(stage).map(|n| n as i32);
    let test = |x: i32, y: i32| {
        let x = sh.address[0].apply(x, w);
        let y = sh.address[1].apply(y, h);
        let d = sh.format.decode(sampler.load(stage, x as u32, y as u32));
        if sh.func.passes(d, r) { 1.0f32 } else { 0.0 }
    };
    let v = if let Filter::Sharp { .. } = filter {
        let p = [uv[0] * w as f32, uv[1] * h as f32];
        let base = p.map(|v| v.floor() as i32);
        let [wx, wy] = p.map(|v| {
            let f = v - v.floor();
            [0.5 * (1.0 - f), 0.5, 0.5 * f]
        });
        let mut coverage = 0.0;
        for (row, wy) in wy.into_iter().enumerate() {
            for (col, wx) in wx.into_iter().enumerate() {
                let x = base[0].wrapping_add(col as i32 - 1);
                let y = base[1].wrapping_add(row as i32 - 1);
                coverage += wx * wy * test(x, y);
            }
        }
        (0.5 + (coverage - 0.5) / 1e-6).clamp(0.0, 1.0)
    } else if sh.linear && group.is_empty() {
        let p = [uv[0] * w as f32 - 0.5, uv[1] * h as f32 - 0.5];
        let base = [p[0].floor() as i32, p[1].floor() as i32];
        let f = [p[0] - p[0].floor(), p[1] - p[1].floor()];
        let (x1, y1) = (base[0].wrapping_add(1), base[1].wrapping_add(1));
        let mix = |a: f32, b: f32, t: f32| a * (1.0 - t) + b * t;
        mix(
            mix(test(base[0], base[1]), test(x1, base[1]), f[0]),
            mix(test(base[0], y1), test(x1, y1), f[0]),
            f[1],
        )
    } else {
        test(
            (uv[0] * w as f32).floor() as i32,
            (uv[1] * h as f32).floor() as i32,
        )
    };
    [v; 4]
}
