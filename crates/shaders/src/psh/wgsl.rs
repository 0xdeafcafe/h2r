//! Pixel shader definition to WGSL.
//!
//! Order of evaluation, as on the NV2A: texture stages 0..3 (later stages can
//! read earlier results), then the general combiners in order, then the
//! final combiner, then alpha test. The body lives in `xps_program`, which
//! returns a colour and a kill flag; `fs_main` discards on the flag, so all
//! texture sampling happens in uniform control flow.

use super::*;
use crate::common;
use std::fmt::Write;

/// `D3DCMPFUNC` for the alpha test, which wgpu does not have in fixed
/// function.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[allow(missing_docs)]
pub enum AlphaFunc {
    Never,
    Less,
    Equal,
    LessEqual,
    Greater,
    NotEqual,
    GreaterEqual,
    Always,
}

impl AlphaFunc {
    pub(crate) fn op(self) -> Option<&'static str> {
        Some(match self {
            Self::Less => "<",
            Self::Equal => "==",
            Self::LessEqual => "<=",
            Self::Greater => ">",
            Self::NotEqual => "!=",
            Self::GreaterEqual => ">=",
            Self::Never | Self::Always => return None,
        })
    }
    /// Evaluates the comparison the way the generated code does.
    pub fn passes(self, alpha_255: f32, reference: f32) -> bool {
        match self {
            Self::Never => false,
            Self::Always => true,
            Self::Less => alpha_255 < reference,
            Self::Equal => alpha_255 == reference,
            Self::LessEqual => alpha_255 <= reference,
            Self::Greater => alpha_255 > reference,
            Self::NotEqual => alpha_255 != reference,
            Self::GreaterEqual => alpha_255 >= reference,
        }
    }
}

/// Per-pipeline state outside the `D3DPIXELSHADERDEF` that changes the code.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct PixelOptions {
    /// Dimension of the texture bound to each stage, when known. D3D's
    /// `PS_GLOBALFLAGS_TEXMODE_ADJUST` turns PROJECT2D/PROJECT3D/CUBEMAP
    /// into whichever matches the bound texture; set this to get the same.
    pub texture_dims: [Option<TextureDim>; 4],
    /// `D3DTSS_ALPHAKILL` per stage: kill when the fetched alpha is 0.
    pub alpha_kill: [bool; 4],
    /// `D3DTSS_COLORSIGN` per stage as [`bits::color_sign`] masks
    /// ([`bits::SIGN_R`] ...): the channels the texture unit reads as
    /// signed bytes. The bound texture must store such a channel's
    /// two's-complement byte `s` biased, as the unsigned byte `s + 128`
    /// (`gfx::texture::DecodeOptions::signed`), so that filtering
    /// interpolates signed values; the shader maps the filtered value `v`
    /// to `max(2v - 256/255, -1)`, i.e. `2s/255`, which is what Xbox
    /// captures show (byte 127 reads as 254/255, not 1).
    pub color_sign: [u8; 4],
    /// Per stage, the axes (bit 0 u, 1 v, 2 w) on which the shader, not the
    /// sampler, supplies the border: a 2D or 3D sample whose coordinate is
    /// outside `[0, 1]` on such an axis reads `border[n]` of the uniforms
    /// instead. Used for signed or arbitrary-colour borders and when the
    /// host lacks border addressing; the sampler then clamps to the edge.
    /// Linear filtering across the boundary is approximated. Cube maps are
    /// never bordered.
    pub border: [u8; 4],
    /// Alpha test; `None` disables it.
    pub alpha_test: Option<AlphaFunc>,
    /// Two-sided lighting: back faces read `oB0`/`oB1` as `v0`/`v1`.
    pub two_sided_lighting: bool,
    /// Flat colour interpolation (must match the vertex side).
    pub flat_shading: bool,
    /// Sample mip level 0 with `textureSampleLevel` instead of
    /// `textureSample`, so `xps_program` can run where there are no
    /// derivatives (compute; the GPU tests use this). Textures are then
    /// never minified.
    pub explicit_lod: bool,
    /// Apply the 32 by 32 polygon stipple mask in the fragment shader.
    pub stipple: bool,
    /// Stages whose texture is a shadow map (a depth format): PROJECT2D
    /// and PROJECT3D on a 2D texture compare instead of sampling colour
    /// (see [`super::shadow`]).
    pub shadow: [Option<super::shadow::Shadow>; 4],
    /// W-buffering (`SET_CONTROL0` z-perspective, `D3DZB_USEW`): the
    /// fragment writes its depth as the interpolated w plus the polygon
    /// offset, over the depth full scale (`Uniforms::depth`), as the NV2A
    /// stores it. The vertex stage must clip w (`VertexUniforms::w_clip`).
    #[serde(default)]
    pub w_buffer: bool,
    /// Stages (bit `n` for stage `n`) whose texture is a coverage-
    /// antialiased glyph atlas being magnified (a font cache drawn at a
    /// render scale above 1). A 2D projective fetch on such a stage
    /// rebuilds the edges at the host's resolution ([`GLYPH_WGSL`]); the
    /// guest's filter is ignored there. Needs derivatives, so it is
    /// ignored with [`explicit_lod`](Self::explicit_lod). Zero (the
    /// default) leaves the program as it was, byte for byte.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub glyph: u8,
    /// Hard shadows (E3 lighting, docs/graphics/e3-lighting.md phase 1):
    /// per shadow stage, the stages that sample the same shadow map, as a
    /// mask (bit `n` for stage `n`). A shadow stage with a non-zero mask
    /// compares once, at the nearest texel, at the mean of those stages'
    /// coordinates: no percentage-closer filter, and taps spread over
    /// stages of one map fall on one point ([`super::shadow`]). Only the
    /// render-scale view sets it. All zero (the default) is retail's
    /// lookup, byte for byte, and is left out of persisted keys.
    #[serde(default, skip_serializing_if = "no_hard_shadow")]
    pub hard_shadow: [u8; 4],
}

/// The fragment entry a W-buffered program ([`PixelOptions::w_buffer`])
/// also has for a pixel-count pass (`gfx::occlusion`): it writes the depth
/// `fs_main` writes and a colour of 1 in red, so the count tests the depth
/// the draws stored (the interpolated w), not the rasterized z.
pub const COUNT_DEPTH_ENTRY: &str = "fs_count_depth";

fn is_zero(v: &u8) -> bool {
    *v == 0
}

fn no_hard_shadow(v: &[u8; 4]) -> bool {
    *v == [0; 4]
}

/// The glyph filter for [`PixelOptions::glyph`].
///
/// A glyph cache holds text rasterised once at the guest's size, with the
/// edges antialiased into a few alpha levels. Magnified with the guest's
/// own filter it is soft (bilinear) or stair-stepped (point). Here the
/// alpha is read as coverage, which across an edge rises about linearly
/// with the distance to it, crossing one half on the edge:
///
/// - `a`, a cubic B-spline through the 4x4 texels around the sample: a
///   smooth surface that reproduces such ramps exactly, so straight edges
///   stay where they were, and that smooths the 3-bit steps of the
///   coverage instead of following them (an interpolating cubic wobbles);
/// - `m`, the same spline through each texel's brightest neighbour (3x3):
///   the full alpha of the stroke the sample is near, 1 for solid text,
///   lower for a thin stroke that never covered a whole texel;
/// - the edge is put back where `a` crosses `m / 2`, with a ramp `k` times
///   steeper than the texel grid (`k` = host pixels per texel, from the
///   derivatives), so it is about one host pixel wide, and drawn at alpha
///   `m`. Faint strokes keep their weight instead of vanishing under a
///   fixed threshold, and the faint texels along a solid edge are not
///   mistaken for strokes of their own.
///
/// Colour is the alpha-weighted bilinear colour of the four nearest texels
/// (button icons carry per-texel colour; the transparent texels around a
/// glyph hold white). Where a texel covers at most a host pixel and a
/// quarter (`k <= 1.25`, text drawn at or below its size) the guest's own
/// sample is returned unchanged. 36 texel loads per fragment, on text only.
pub const GLYPH_WGSL: &str = r#"
fn xps_glyph_bspline(t: f32) -> vec4f {
    let u = 1.0 - t;
    let t2 = t * t;
    let t3 = t2 * t;
    return vec4f(u * u * u, 3.0 * t3 - 6.0 * t2 + 4.0, -3.0 * t3 + 3.0 * t2 + 3.0 * t + 1.0, t3) / 6.0;
}
fn xps_glyph(tex: texture_2d<f32>, guest: vec4f, uv: vec2f, k: f32) -> vec4f {
    let size = vec2i(textureDimensions(tex, 0));
    let p = uv * vec2f(size) - 0.5;
    let base = vec2i(floor(p));
    let f = p - floor(p);
    let wx = xps_glyph_bspline(f.x);
    let wy = xps_glyph_bspline(f.y);
    // Alpha of the 6x6 texels around the 4x4 the spline reads.
    var t: array<f32, 36>;
    var rgb = vec3f(0.0);
    var wa = 0.0;
    for (var y = 0; y < 6; y++) {
        for (var x = 0; x < 6; x++) {
            let c = clamp(base + vec2i(x - 2, y - 2), vec2i(0), size - 1);
            let s = textureLoad(tex, c, 0);
            t[y * 6 + x] = s.a;
            if (x == 2 || x == 3) && (y == 2 || y == 3) {
                let wb = select(1.0 - f.x, f.x, x == 3) * select(1.0 - f.y, f.y, y == 3) * s.a;
                rgb += wb * s.rgb;
                wa += wb;
            }
        }
    }
    // a: the coverage, smoothed. m: the same spline through each texel's
    // brightest neighbour (3x3), the local stroke's full alpha.
    var a = 0.0;
    var m = 0.0;
    for (var y = 0; y < 4; y++) {
        for (var x = 0; x < 4; x++) {
            let w = wx[x] * wy[y];
            let i = (y + 1) * 6 + x + 1;
            a += w * t[i];
            let top = max(max(t[i - 7], t[i - 6]), t[i - 5]);
            let mid = max(max(t[i - 1], t[i]), t[i + 1]);
            let bottom = max(max(t[i + 5], t[i + 6]), t[i + 7]);
            m += w * max(max(top, mid), bottom);
        }
    }
    let n = clamp(a / max(m, 1e-4), 0.0, 1.0);
    let alpha = m * clamp((n - 0.5) * k + 0.5, 0.0, 1.0);
    let color = select(guest.rgb, rgb / wa, wa > 1e-4);
    return select(guest, vec4f(color, alpha), k > 1.25);
}
"#;

/// A translated pixel pipeline.
#[derive(Clone, Debug)]
pub struct PixelShader {
    /// Declarations and functions without the shared varyings struct.
    pub body: String,
    /// Texture bound at group 1, binding `2n` (sampler at `2n + 1`).
    pub textures: [Option<TextureDim>; 4],
    /// Whether [`PixelOptions::flat_shading`] was set.
    pub flat_shading: bool,
    /// Whether `fs_main` can discard: a kill source (clip-plane texture
    /// mode, alpha kill on a bound stage, an alpha test that can fail) or
    /// the polygon stipple. A program with nothing to kill has no
    /// `discard` at all.
    pub discards: bool,
    /// Things that were approximated or dropped (unsupported modes, invalid
    /// registers). Empty for a faithful translation.
    pub warnings: Vec<String>,
}

impl PixelShader {
    /// A standalone module (varyings struct included).
    pub fn module(&self) -> String {
        format!(
            "{}{}",
            common::varyings_struct(self.flat_shading),
            self.body
        )
    }
}

pub(crate) const PRELUDE: &str = r#"
struct XpsUniforms {
    // Combiner constants per stage; index 8 is the final combiner's.
    c0: array<vec4f, 9>,
    c1: array<vec4f, 9>,
    fog_color: vec4f,
    // The NV2A fog unit (psh::fog): x: 0 = off, 1 linear, 2 exp, 3 exp2,
    // 4..6 the same on |coordinate|; y: bias, z: scale (SET_FOG_PARAMS).
    fog: vec4f,
    // D3DTSS_BUMPENVMAT00, 01, 10, 11 per stage.
    bump_mat: array<vec4f, 4>,
    // x: D3DTSS_BUMPENVLSCALE, y: D3DTSS_BUMPENVLOFFSET per stage.
    bump_lum: array<vec4f, 4>,
    // xy multiplies 2D coordinates (texel-addressed linear textures).
    tex_scale: array<vec4f, 4>,
    // Eye vector for DOT_RFLCT_SPEC_CONST.
    eye: vec4f,
    // x: alpha reference, 0..255.
    alpha_ref: vec4f,
    // The texture unit's border value per stage (PixelOptions::border).
    border: array<vec4f, 4>,
    lod_bias: vec4f,
    stipple: array<vec4u, 8>,
    // W-buffering (PixelOptions::w_buffer): x the depth full scale, y the
    // polygon offset in depth units, z its slope factor, w 1 for an
    // integer depth format (the stored w is truncated, as the NV2A does).
    depth: vec4f,
    // x, y: the range the stored w is clamped to (CLIP_MIN, CLIP_MAX).
    depth_range: vec4f,
}
@group(0) @binding(1) var<uniform> xps: XpsUniforms;

struct XpsInputs {
    d0: vec4f, d1: vec4f, b0: vec4f, b1: vec4f,
    t0: vec4f, t1: vec4f, t2: vec4f, t3: vec4f,
    fog: f32,
    front: bool,
}
struct XpsResult { color: vec4f, kill: bool, }

// Dot-product input mappings, for unsigned 8-bit texture data.
fn xps_sign_d3d(x: vec3f) -> vec3f { return (x * 255.0 - 128.0) / 127.0; }
fn xps_sign_gl(x: vec3f) -> vec3f {
    let b = x * 255.0;
    return select((b + 0.5) / 127.5, (b - 255.5) / 127.5, b >= vec3f(128.0));
}
fn xps_sign_nv(x: vec3f) -> vec3f {
    let b = x * 255.0;
    return select(b / 127.0, (b - 256.0) / 127.0, b >= vec3f(128.0));
}
// COLORSIGN channels: the host texture holds the signed byte s as s + 128
// (so filtering is linear in s); the texture unit delivers 2s/255.
fn xps_signed(v: vec4f, m: vec4<bool>) -> vec4f {
    return select(v, max(2.0 * v - 256.0 / 255.0, vec4f(-1.0)), m);
}
// Outside [0, 1] on one of the axes in m (PixelOptions::border).
fn xps_outside(c: vec3f, m: vec3<bool>) -> bool {
    return any(m & ((c < vec3f(0.0)) | (c > vec3f(1.0))));
}
fn xps_mux_lsb(a: f32) -> bool { return (u32(a * 255.0) & 1u) == 1u; }
"#;

/// HILO dot mappings. The source texel is read as one 32-bit word, laid out
/// the way an `A8R8G8B8` texture samples it (a = bits 31..24, r = 23..16,
/// g = 15..8, b = 7..0, so xemu's `dotmap_hilo_1` split): hi = bits 31..16,
/// lo = bits 15..0. The hemisphere variants read the halves as signed, by
/// analogy with the 8-bit mappings, and complete the vector to unit length.
pub(crate) const HILO_PRELUDE: &str = r#"
fn xps_hilo(c: vec4f) -> vec2f {
    let b = round(clamp(c, vec4f(0.0), vec4f(1.0)) * 255.0);
    return vec2f(b.a * 256.0 + b.r, b.g * 256.0 + b.b);
}
fn xps_hilo_twos(h: vec2f) -> vec2f { return select(h, h - 65536.0, h >= vec2f(32768.0)); }
fn xps_hemisphere(v: vec2f) -> vec3f { return vec3f(v, sqrt(max(1.0 - dot(v, v), 0.0))); }
fn xps_hilo_1(c: vec4f) -> vec3f { return vec3f(xps_hilo(c) / 65535.0, 1.0); }
fn xps_hilo_hemi_d3d(c: vec4f) -> vec3f { return xps_hemisphere((xps_hilo(c) - 32768.0) / 32767.0); }
fn xps_hilo_hemi_gl(c: vec4f) -> vec3f { return xps_hemisphere((2.0 * xps_hilo_twos(xps_hilo(c)) + 1.0) / 65535.0); }
fn xps_hilo_hemi_nv(c: vec4f) -> vec3f { return xps_hemisphere(xps_hilo_twos(xps_hilo(c)) / 32767.0); }
"#;

fn ty(alpha_portion: bool) -> &'static str {
    if alpha_portion { "f32" } else { "vec3f" }
}

struct Gen<'a> {
    c: &'a Combiners,
    warnings: Vec<String>,
}

impl Gen<'_> {
    fn reg(&mut self, r: Reg, stage: Option<usize>) -> String {
        let const_index = |unique: bool| match stage {
            None => 8,
            Some(s) if unique => s,
            Some(_) => 0,
        };
        match r {
            Reg::Zero => "vec4f(0.0)".into(),
            Reg::C0 => format!("xps.c0[{}]", const_index(self.c.unique_c0)),
            Reg::C1 => format!("xps.c1[{}]", const_index(self.c.unique_c1)),
            Reg::Fog => "fogr".into(),
            Reg::V0 => "v0".into(),
            Reg::V1 => "v1".into(),
            Reg::T(n) => format!("t{n}"),
            Reg::R0 => "r0".into(),
            Reg::R1 => "r1".into(),
            Reg::V1R0Sum | Reg::EfProd if stage.is_none() => {
                if r == Reg::V1R0Sum {
                    "fc_sum".into()
                } else {
                    "fc_prod".into()
                }
            }
            other => {
                self.warnings.push(format!(
                    "register {} read where it is not valid; reads zero",
                    other.name()
                ));
                "vec4f(0.0)".into()
            }
        }
    }

    /// An input expression of type vec3f (RGB portion) or f32 (alpha).
    fn input(&mut self, i: &Input, alpha_portion: bool, stage: Option<usize>) -> String {
        let base = self.reg(i.reg, stage);
        let ch = match (alpha_portion, i.alpha) {
            (false, false) => format!("{base}.rgb"),
            (false, true) => format!("vec3f({base}.a)"),
            (true, false) => format!("{base}.b"),
            (true, true) => format!("{base}.a"),
        };
        let t = ty(alpha_portion);
        match i.map {
            InputMap::UnsignedIdentity => format!("max({ch}, {t}(0.0))"),
            InputMap::UnsignedInvert => format!("(1.0 - clamp({ch}, {t}(0.0), {t}(1.0)))"),
            InputMap::ExpandNormal => format!("(2.0 * max({ch}, {t}(0.0)) - 1.0)"),
            InputMap::ExpandNegate => format!("(1.0 - 2.0 * max({ch}, {t}(0.0)))"),
            InputMap::HalfBiasNormal => format!("(max({ch}, {t}(0.0)) - 0.5)"),
            InputMap::HalfBiasNegate => format!("(0.5 - max({ch}, {t}(0.0)))"),
            InputMap::SignedIdentity => ch,
            InputMap::SignedNegate => format!("(-{ch})"),
        }
    }

    fn dest(&mut self, r: Reg) -> Option<&'static str> {
        match r {
            Reg::Zero => None,
            Reg::V0 => Some("v0"),
            Reg::V1 => Some("v1"),
            Reg::T(0) => Some("t0"),
            Reg::T(1) => Some("t1"),
            Reg::T(2) => Some("t2"),
            Reg::T(3) => Some("t3"),
            Reg::R0 => Some("r0"),
            Reg::R1 => Some("r1"),
            other => {
                self.warnings.push(format!(
                    "write to read-only register {}; dropped",
                    other.name()
                ));
                None
            }
        }
    }
}

fn out_map(e: &str, m: OutMap) -> String {
    match m {
        OutMap::Identity => e.to_string(),
        OutMap::Bias => format!("({e} - 0.5)"),
        OutMap::Shl1 => format!("({e} * 2.0)"),
        OutMap::Shl1Bias => format!("(({e} - 0.5) * 2.0)"),
        OutMap::Shl2 => format!("({e} * 4.0)"),
        OutMap::Shl2Bias => format!("(({e} - 0.5) * 4.0)"),
        OutMap::Shr1 => format!("({e} * 0.5)"),
        OutMap::Shr1Bias => format!("(({e} - 0.5) * 0.5)"),
    }
}

fn dot_map_fn(m: DotMap, warnings: &mut Vec<String>, stage: usize) -> &'static str {
    match m {
        DotMap::ZeroToOne | DotMap::Unused => "",
        DotMap::Minus1To1D3d => "xps_sign_d3d",
        DotMap::Minus1To1Gl => "xps_sign_gl",
        DotMap::Minus1To1 => "xps_sign_nv",
        DotMap::Hilo1 => "xps_hilo_1",
        DotMap::HiloHemisphereD3d => "xps_hilo_hemi_d3d",
        DotMap::HiloHemisphereGl => "xps_hilo_hemi_gl",
        DotMap::HiloHemisphere => "xps_hilo_hemi_nv",
        other => {
            warnings.push(format!(
                "stage {stage}: dot mapping {other:?} unsupported; using zero_to_one"
            ));
            ""
        }
    }
}

/// The register stage `i` reads as its source; an out-of-range selector
/// reads zero (with a warning).
fn source_texture(c: &Combiners, i: usize, warnings: &mut Vec<String>) -> String {
    match c.input_stage(i) {
        Some(s) => format!("t{s}"),
        None => {
            warnings.push(format!(
                "stage {i}: input texture {} is not an earlier stage; reads zero",
                c.input_texture[i]
            ));
            "vec4f(0.0)".into()
        }
    }
}

/// `src.rgb` through the stage's dot mapping.
fn mapped(src: &str, m: DotMap, warnings: &mut Vec<String>, stage: usize) -> String {
    let f = dot_map_fn(m, warnings, stage);
    if f.is_empty() {
        format!("{src}.rgb")
    } else if f.starts_with("xps_hilo") {
        // HILO reads the whole texel (both halves of the word).
        format!("{f}({src})")
    } else {
        format!("{f}({src}.rgb)")
    }
}

/// Translates a pixel shader definition.
pub fn translate(def: &PixelShaderDef, options: &PixelOptions) -> PixelShader {
    let c = def.decode();
    let mut g = Gen {
        c: &c,
        warnings: Vec::new(),
    };
    let mut s = String::new();
    s.push_str(PRELUDE);
    s.push_str(super::fog::WGSL);
    s.push_str(HILO_PRELUDE);
    if options.shadow.iter().any(Option::is_some) {
        s.push_str(super::shadow::WGSL_HELPERS);
    }
    let glyph = if options.explicit_lod { 0 } else { options.glyph & 0xF };
    if glyph != 0 {
        s.push_str(GLYPH_WGSL);
    }

    // Texture bindings.
    let mut textures = [None; 4];
    // `i` is the stage number: it indexes several per-stage arrays and names.
    #[allow(clippy::needless_range_loop)]
    for i in 0..4 {
        let mode = c.texture_modes[i];
        let Some(default) = mode.texture_dim() else {
            continue;
        };
        let dim = match mode {
            TexMode::Project2d | TexMode::Project3d | TexMode::Cubemap => {
                options.texture_dims[i].unwrap_or(default)
            }
            _ => {
                if let Some(d) = options.texture_dims[i].filter(|d| *d != default) {
                    g.warnings.push(format!(
                        "stage {i}: {} needs a {default:?} texture, bound {d:?}",
                        mode.name()
                    ));
                }
                default
            }
        };
        textures[i] = Some(dim);
        let t = match dim {
            TextureDim::D2 => "texture_2d<f32>",
            TextureDim::D3 => "texture_3d<f32>",
            TextureDim::Cube => "texture_cube<f32>",
        };
        let _ = writeln!(s, "@group(1) @binding({}) var xps_tex{i}: {t};", 2 * i);
        let _ = writeln!(
            s,
            "@group(1) @binding({}) var xps_samp{i}: sampler;",
            2 * i + 1
        );
    }

    s.push_str("\nfn xps_program(i: XpsInputs) -> XpsResult {\n    var kill = false;\n");
    let mut kills = false;
    if options.two_sided_lighting {
        s.push_str("    var v0 = clamp(select(i.b0, i.d0, i.front), vec4f(0.0), vec4f(1.0));\n");
        s.push_str("    var v1 = clamp(select(i.b1, i.d1, i.front), vec4f(0.0), vec4f(1.0));\n");
    } else {
        s.push_str("    var v0 = clamp(i.d0, vec4f(0.0), vec4f(1.0));\n");
        s.push_str("    var v1 = clamp(i.d1, vec4f(0.0), vec4f(1.0));\n");
    }
    s.push_str("    let fogr = vec4f(xps.fog_color.rgb, xps_fog_factor(i.fog));\n");
    s.push_str("    var t0 = vec4f(0.0, 0.0, 0.0, 1.0);\n    var t1 = t0;\n    var t2 = t0;\n    var t3 = t0;\n");
    s.push_str("    var dot1 = 0.0;\n    var dot2 = 0.0;\n    var dot3 = 0.0;\n");

    // Texture stages (`i` indexes several per-stage arrays and names).
    #[allow(clippy::needless_range_loop)]
    for i in 0..4 {
        let mode = c.texture_modes[i];
        let tc = format!("i.t{i}");
        let (func, lod) = if options.explicit_lod {
            ("textureSampleLevel", ", 0.0".to_string())
        } else {
            (
                "textureSampleBias",
                format!(", clamp(xps.lod_bias[{i}], -16.0, 15.99)"),
            )
        };
        // Signed channels are converted as the texel leaves the texture
        // unit, before anything else in the stage uses it. The host
        // texture stores them biased, so this runs after filtering.
        let sign = options.color_sign[i] & 0xF;
        let signed = |e: String| {
            if sign == 0 || textures[i].is_none() {
                return e;
            }
            let b = |m: u8| sign & m != 0;
            format!(
                "xps_signed({e}, vec4<bool>({}, {}, {}, {}))",
                b(bits::SIGN_R),
                b(bits::SIGN_G),
                b(bits::SIGN_B),
                b(bits::SIGN_A)
            )
        };
        // Shader-supplied border (PixelOptions::border).
        let axes = options.border[i] & 7;
        let bordered = |e: String, c: String| {
            if axes == 0 || textures[i].is_none() {
                return e;
            }
            let b = |m: u8| axes & m != 0;
            format!(
                "select({e}, xps.border[{i}], xps_outside({c}, vec3<bool>({}, {}, {})))",
                b(1),
                b(2),
                b(4)
            )
        };
        let sample2 = |uv: &str| {
            let c = format!("({uv}) * xps.tex_scale[{i}].xy");
            bordered(
                signed(format!("{func}(xps_tex{i}, xps_samp{i}, {c}{lod})")),
                format!("vec3f({c}, 0.5)"),
            )
        };
        let sample = |coord: &str| {
            let e = signed(format!("{func}(xps_tex{i}, xps_samp{i}, {coord}{lod})"));
            match textures[i] {
                Some(TextureDim::D3) => bordered(e, format!("vec3f({coord})")),
                _ => e,
            }
        };
        let _ = writeln!(s, "    // texture stage {i}: {}", mode.name());
        let valid_stage = match mode {
            TexMode::BumpEnvMap
            | TexMode::BumpEnvMapLum
            | TexMode::DependentAr
            | TexMode::DependentGb => i >= 1,
            TexMode::DotProduct => i == 1 || i == 2,
            TexMode::DotSt | TexMode::DotZw | TexMode::Brdf => i >= 2,
            TexMode::DotRflctDiff => i == 2,
            TexMode::DotRflctSpec
            | TexMode::DotStr3d
            | TexMode::DotStrCube
            | TexMode::DotRflctSpecConst => i == 3,
            _ => true,
        };
        if !valid_stage {
            g.warnings.push(format!(
                "stage {i}: {} is not valid in this stage; skipped",
                mode.name()
            ));
            continue;
        }
        // The source stage and its dot mapping only matter to modes that
        // read them (an unused selector or mapping may hold anything).
        let src = if mode.reads_input_texture() {
            source_texture(&c, i, &mut g.warnings)
        } else {
            String::new()
        };
        let mapped_src = if mode.is_dot() {
            mapped(&src, c.dot_mapping[i], &mut g.warnings, i)
        } else {
            String::new()
        };
        let line = match mode {
            TexMode::None => String::new(),
            TexMode::Project2d | TexMode::Project3d
                if options.shadow[i].is_some() && textures[i] == Some(TextureDim::D2) =>
            {
                super::shadow::wgsl_stage(
                    i,
                    &options.shadow[i].unwrap(),
                    mode == TexMode::Project3d,
                    options.hard_shadow[i],
                )
            }
            TexMode::Project2d | TexMode::Project3d | TexMode::Cubemap => {
                if options.shadow[i].is_some() {
                    g.warnings.push(format!(
                        "stage {i}: shadow map bound as {:?}; sampled as colour",
                        textures[i].unwrap()
                    ));
                }
                let glyph_stage = glyph & (1 << i) != 0 && sign == 0 && axes == 0;
                match textures[i].unwrap() {
                    TextureDim::D2 if glyph_stage && mode == TexMode::Project2d => format!(
                        "{{\n        let guest = {};\n        let uv = ({tc}.xy / {tc}.w) * xps.tex_scale[{i}].xy;\n        let px = uv * vec2f(textureDimensions(xps_tex{i}, 0));\n        let k = 1.0 / max(max(length(dpdx(px)), length(dpdy(px))), 1e-6);\n        t{i} = xps_glyph(xps_tex{i}, guest, uv, k);\n    }}",
                        sample2(&format!("{tc}.xy / {tc}.w"))
                    ),
                    TextureDim::D2 => format!("t{i} = {};", sample2(&format!("{tc}.xy / {tc}.w"))),
                    TextureDim::D3 => format!("t{i} = {};", sample(&format!("{tc}.xyz / {tc}.w"))),
                    TextureDim::Cube => format!("t{i} = {};", sample(&format!("{tc}.xyz"))),
                }
            }
            TexMode::Passthru => format!("t{i} = clamp({tc}, vec4f(0.0), vec4f(1.0));"),
            TexMode::ClipPlane => {
                let mut l = format!("t{i} = vec4f(0.0);");
                for (j, comp) in "xyzw".chars().enumerate() {
                    let op = if c.compare_mode[i] & (1 << j) != 0 {
                        ">="
                    } else {
                        "<"
                    };
                    let _ = write!(l, "\n    kill = kill || ({tc}.{comp} {op} 0.0);");
                    kills = true;
                }
                l
            }
            TexMode::BumpEnvMap | TexMode::BumpEnvMapLum => {
                let mut l = format!(
                    "{{\n        let m = xps.bump_mat[{i}];\n        let d = {src}.rg;\n        let uv = {tc}.xy + vec2f(m.x * d.x + m.z * d.y, m.y * d.x + m.w * d.y);\n        t{i} = {};",
                    sample2("uv")
                );
                if mode == TexMode::BumpEnvMapLum {
                    let _ = write!(
                        l,
                        "\n        t{i} = t{i} * clamp(xps.bump_lum[{i}].x * {src}.b + xps.bump_lum[{i}].y, 0.0, 1.0);"
                    );
                }
                l.push_str("\n    }");
                l
            }
            TexMode::DotProduct => {
                format!("dot{i} = dot({tc}.xyz, {mapped_src});\n    t{i} = vec4f(0.0);")
            }
            TexMode::DotSt => format!(
                "dot{i} = dot({tc}.xyz, {mapped_src});\n    t{i} = {};",
                sample2(&format!("vec2f(dot{}, dot{i})", i - 1))
            ),
            TexMode::DotZw => {
                g.warnings.push(format!(
                    "stage {i}: dot_zw depth replacement unsupported; depth unchanged"
                ));
                format!("dot{i} = dot({tc}.xyz, {mapped_src});\n    t{i} = vec4f(0.0);")
            }
            TexMode::DotRflctDiff => {
                // Stage 2 also needs stage 3's dot product for the normal.
                let src3 = source_texture(&c, 3, &mut g.warnings);
                let m3 = mapped(&src3, c.dot_mapping[3], &mut g.warnings, 3);
                format!(
                    "dot{i} = dot({tc}.xyz, {mapped_src});\n    t{i} = {};",
                    sample(&format!("vec3f(dot1, dot2, dot(i.t3.xyz, {m3}))"))
                )
            }
            TexMode::DotRflctSpec | TexMode::DotRflctSpecConst => {
                let eye = if mode == TexMode::DotRflctSpec {
                    "vec3f(i.t1.w, i.t2.w, i.t3.w)"
                } else {
                    "xps.eye.xyz"
                };
                format!(
                    "dot{i} = dot({tc}.xyz, {mapped_src});\n    {{\n        let n = vec3f(dot1, dot2, dot3);\n        let e = {eye};\n        let r = 2.0 * n * dot(n, e) / dot(n, n) - e;\n        t{i} = {};\n    }}",
                    sample("r")
                )
            }
            TexMode::DotStr3d | TexMode::DotStrCube => {
                format!(
                    "dot{i} = dot({tc}.xyz, {mapped_src});\n    t{i} = {};",
                    sample("vec3f(dot1, dot2, dot3)")
                )
            }
            TexMode::DependentAr => format!("t{i} = {};", sample2(&format!("{src}.ar"))),
            TexMode::DependentGb => format!("t{i} = {};", sample2(&format!("{src}.gb"))),
            TexMode::Brdf => {
                g.warnings
                    .push(format!("stage {i}: brdf unsupported; reads zero"));
                format!("t{i} = vec4f(0.0);")
            }
            TexMode::Reserved(n) => {
                g.warnings.push(format!(
                    "stage {i}: reserved texture mode {n:#x}; reads zero"
                ));
                format!("t{i} = vec4f(0.0);")
            }
        };
        if options.shadow[i].is_some()
            && !matches!(
                mode,
                TexMode::Project2d | TexMode::Project3d | TexMode::Cubemap
            )
            && textures[i].is_some()
        {
            g.warnings.push(format!(
                "stage {i}: shadow map sampled by {}; no comparison, sampled as colour",
                mode.name()
            ));
        }
        if !line.is_empty() {
            let _ = writeln!(s, "    {line}");
        }
        if options.alpha_kill[i] && textures[i].is_some() {
            let _ = writeln!(s, "    kill = kill || (t{i}.a == 0.0);");
            kills = true;
        }
    }

    // r0.a starts as texture 0's alpha.
    if c.texture_modes[0] == TexMode::None {
        s.push_str("    var r0 = vec4f(0.0, 0.0, 0.0, 1.0);\n");
    } else {
        s.push_str("    var r0 = vec4f(0.0, 0.0, 0.0, t0.a);\n");
    }
    s.push_str("    var r1 = vec4f(0.0);\n");

    for (n, st) in c.stages.iter().enumerate() {
        let _ = writeln!(s, "    // combiner stage {n}\n    {{");
        if c.mux_msb {
            s.push_str("        let mux = r0.a >= 0.5;\n");
        } else {
            s.push_str("        let mux = xps_mux_lsb(r0.a);\n");
        }
        let mut writes = String::new();
        for (alpha, portion) in [(false, &st.rgb), (true, &st.alpha)] {
            let p = if alpha { "al" } else { "rgb" };
            let t = ty(alpha);
            let [a, b, cc, d] = portion.inputs;
            let ea = g.input(&a, alpha, Some(n));
            let eb = g.input(&b, alpha, Some(n));
            let ec = g.input(&cc, alpha, Some(n));
            let ed = g.input(&d, alpha, Some(n));
            let o = &portion.output;
            if alpha && (o.ab_dot || o.cd_dot || o.ab_blue_to_alpha || o.cd_blue_to_alpha) {
                g.warnings.push(format!(
                    "stage {n}: RGB-only output flags on the alpha portion; ignored"
                ));
            }
            let ab = if o.ab_dot && !alpha {
                format!("vec3f(dot({ea}, {eb}))")
            } else {
                format!("{ea} * {eb}")
            };
            let cd = if o.cd_dot && !alpha {
                format!("vec3f(dot({ec}, {ed}))")
            } else {
                format!("{ec} * {ed}")
            };
            let _ = writeln!(s, "        let {p}_ab: {t} = {ab};");
            let _ = writeln!(s, "        let {p}_cd: {t} = {cd};");
            if o.mux {
                let _ = writeln!(s, "        let {p}_s: {t} = select({p}_ab, {p}_cd, mux);");
            } else {
                let _ = writeln!(s, "        let {p}_s: {t} = {p}_ab + {p}_cd;");
            }
            for (which, reg) in [("ab", o.ab), ("cd", o.cd), ("s", o.sum)] {
                let Some(dst) = g.dest(reg) else { continue };
                let val = format!(
                    "clamp({}, {t}(-1.0), {t}(1.0))",
                    out_map(&format!("{p}_{which}"), o.map)
                );
                if alpha {
                    let _ = writeln!(writes, "        {dst}.a = {val};");
                } else {
                    let _ = writeln!(writes, "        {dst} = vec4f({val}, {dst}.a);");
                    let bta = match which {
                        "ab" => o.ab_blue_to_alpha,
                        "cd" => o.cd_blue_to_alpha,
                        _ => false,
                    };
                    if bta {
                        let _ = writeln!(writes, "        {dst}.a = {dst}.b;");
                    }
                }
            }
        }
        s.push_str(&writes);
        s.push_str("    }\n");
    }

    if let Some(f) = &c.final_combiner {
        s.push_str("    // final combiner\n    var color: vec4f;\n    {\n");
        // Each operand is unsigned-mapped (clamped to [0, 1], inverted when
        // complemented) before the add: NV_register_combiners, and the
        // V1R0_SUM cases in nxdk_pgraph_tests (see docs/graphics/shaders.md).
        let un = |reg: &str, invert: bool| {
            let x = format!("clamp({reg}.rgb, vec3f(0.0), vec3f(1.0))");
            if invert { format!("(1.0 - {x})") } else { x }
        };
        let v1 = un("v1", f.complement_v1);
        let r0 = un("r0", f.complement_r0);
        let sum = if f.clamp_sum {
            format!("clamp({v1} + {r0}, vec3f(0.0), vec3f(1.0))")
        } else {
            format!("{v1} + {r0}")
        };
        let _ = writeln!(s, "        let fc_sum = vec4f({sum}, 0.0);");
        let e = g.input(&f.e, false, None);
        let ff = g.input(&f.f, false, None);
        let _ = writeln!(s, "        let fc_prod = vec4f({e} * {ff}, 0.0);");
        let a = g.input(&f.a, false, None);
        let b = g.input(&f.b, false, None);
        let cc = g.input(&f.c, false, None);
        let d = g.input(&f.d, false, None);
        let gg = g.input(&f.g, true, None);
        let _ = writeln!(
            s,
            "        let a = {a};\n        let b = {b};\n        let c = {cc};\n        let d = {d};"
        );
        let _ = writeln!(
            s,
            "        color = vec4f(clamp(a * b + (1.0 - a) * c + d, vec3f(0.0), vec3f(1.0)), clamp({gg}, 0.0, 1.0));"
        );
        s.push_str("    }\n");
    } else {
        s.push_str("    let color = clamp(r0, vec4f(0.0), vec4f(1.0));\n");
    }

    match options.alpha_test {
        Some(AlphaFunc::Never) => {
            s.push_str("    kill = true;\n");
            kills = true;
        }
        Some(f) => {
            if let Some(op) = f.op() {
                kills = true;
                let _ = writeln!(
                    s,
                    "    kill = kill || !(round(color.a * 255.0) {op} xps.alpha_ref.x);"
                );
            }
        }
        None => {}
    }
    s.push_str("    return XpsResult(color, kill);\n}\n");

    if options.w_buffer {
        // position.w is 1/w interpolated linearly in screen space, so its
        // reciprocal is the perspective-correct w the NV2A stores
        // (xemu's z-perspective path in glsl/psh.c does the same). The
        // slope comes before any discard, in uniform control flow.
        //
        // `fs_count_depth` (COUNT_DEPTH_ENTRY) is the pixel-count pass's
        // fragment for this program: the same depth, and 1 per fragment.
        // Without it the count pass tests the rasterized z, which under
        // W-buffering is not what the draws stored.
        s.push_str(
            r#"
struct XpsOut {
    @location(0) color: vec4f,
    @builtin(frag_depth) depth: f32,
}

fn xps_w_depth(position: vec4f) -> f32 {
    let zw = 1.0 / position.w;
    let slope = max(abs(dpdx(zw)), abs(dpdy(zw)));
    let zc = clamp(zw + xps.depth.y + xps.depth.z * slope, xps.depth_range.x, xps.depth_range.y);
    let zq = select(zc, floor(zc), xps.depth.w != 0.0);
    return clamp(zq / xps.depth.x, 0.0, 1.0);
}

@fragment
fn fs_count_depth(@builtin(position) position: vec4f) -> XpsOut {
    return XpsOut(vec4f(1.0, 0.0, 0.0, 0.0), xps_w_depth(position));
}

@fragment
fn fs_main(v: XboxVaryings, @builtin(front_facing) front: bool) -> XpsOut {
    let depth = xps_w_depth(v.position);
    let r = xps_program(XpsInputs(v.d0, v.d1, v.b0, v.b1, v.t0, v.t1, v.t2, v.t3, v.fog, front));
"#,
        );
    } else {
        s.push_str(
            r#"
@fragment
fn fs_main(v: XboxVaryings, @builtin(front_facing) front: bool) -> @location(0) vec4f {
    let r = xps_program(XpsInputs(v.d0, v.d1, v.b0, v.b1, v.t0, v.t1, v.t2, v.t3, v.fog, front));
"#,
        );
    }
    if options.stipple {
        s.push_str(
            r#"    let xy = vec2u(v.position.xy) & vec2u(31u);
    let y = 31u - xy.y;
    let row = xps.stipple[y / 4u][y % 4u];
    if ((row & (1u << (xy.x ^ 7u))) == 0u) { discard; }
"#,
        );
    }
    // A `discard` the program can never take still changes how Apple GPUs
    // shade the draw's edges when it shares a render pass with earlier
    // draws (docs/graphics/graphics.md, "Merged passes"), so none is emitted.
    if kills {
        s.push_str("    if (r.kill) { discard; }\n");
    }
    if options.w_buffer {
        s.push_str("    return XpsOut(r.color, depth);\n}\n");
    } else {
        s.push_str("    return r.color;\n}\n");
    }
    let mut warnings = g.warnings;
    warnings.dedup();
    PixelShader {
        body: s,
        textures,
        flat_shading: options.flat_shading,
        discards: kills || options.stipple,
        warnings,
    }
}
