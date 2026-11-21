//! Vertex microcode to WGSL.
//!
//! Each instruction becomes a block that first evaluates every operand into
//! `let`s and then performs the writes, which gives the hardware's paired
//! MAC/ILU semantics (both units read the pre-instruction state) without
//! special cases. The program body is the function `xvs_program`, which
//! returns the raw output registers; `vs_main` feeds it vertex attributes,
//! applies the viewport fixup to `oPos` and fills [`crate::VARYINGS_STRUCT`].

use super::*;
use crate::common;
use std::fmt::Write;

/// Per-pipeline choices that change the generated vertex code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct VertexOptions {
    /// Bit `n` set when the vertex layout supplies `v<n>`. Registers the
    /// program reads that are not supplied come from `xvs.defaults[n]`
    /// (the D3D `SetVertexData4f` value, normally `(0, 0, 0, 1)`).
    pub provided_inputs: u16,
    /// Flat (`D3DSHADE_FLAT`) colour interpolation.
    pub flat_shading: bool,
    /// How each supplied `v<n>` arrives from the vertex buffer. All
    /// [`InputFetch::Float`] when the renderer converts every attribute to
    /// floats on the CPU; the other kinds let it bind guest bytes as they
    /// are (`gfx::repack`, "GPU fetch").
    #[serde(default)]
    pub input_fetch: [InputFetch; INPUT_COUNT],
}

impl Default for VertexOptions {
    fn default() -> Self {
        Self {
            provided_inputs: 0xFFFF,
            flat_shading: false,
            input_fetch: [InputFetch::Float; INPUT_COUNT],
        }
    }
}

/// What a vertex input location holds and how the entry point turns it
/// into the `vec4f` register the program reads. Missing components of a
/// narrower vertex format already read as `(0, 0, 0, 1)`.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum InputFetch {
    /// A float (or normalised) format, used as is.
    #[default]
    Float,
    /// A four-component float format holding three: `(x, y, z, 1)`
    /// (`NORMSHORT3` as `snorm16x4`, `PBYTE3` as `unorm8x4`).
    Xyz1,
    /// A signed integer format (`SHORT1/2/4`), converted to float.
    Int,
    /// A four-component signed integer format holding three (`SHORT3`
    /// as `sint16x4`): `(x, y, z, 1)`.
    IntXyz1,
    /// `NORMPACKED3` as one `u32`: signed-normalised 11:11:10, x in the
    /// low bits, `max(v / (2^(bits-1) - 1), -1)`, w = 1.
    NormPacked3,
    /// `FLOAT2H` as three floats `(x, y, w)`: `(x, y, 0, w)`.
    Float2H,
}

impl InputFetch {
    /// The WGSL type of the entry-point parameter.
    fn wgsl_type(self) -> &'static str {
        match self {
            Self::Float | Self::Xyz1 | Self::Float2H => "vec4f",
            Self::Int | Self::IntXyz1 => "vec4i",
            Self::NormPacked3 => "u32",
        }
    }

    /// The `vec4f` register value from parameter `v`.
    fn wgsl_value(self, v: &str) -> String {
        match self {
            Self::Float => v.to_string(),
            Self::Xyz1 => format!("vec4f({v}.xyz, 1.0)"),
            Self::Int => format!("vec4f({v})"),
            Self::IntXyz1 => format!("vec4f(vec3f({v}.xyz), 1.0)"),
            Self::NormPacked3 => format!("xvs_normpacked3({v})"),
            Self::Float2H => format!("vec4f({v}.xy, 0.0, {v}.z)"),
        }
    }
}

/// A translated vertex program.
#[derive(Clone, Debug)]
pub struct VertexShader {
    /// Declarations and functions without the shared varyings struct; see
    /// [`crate::link`] and [`VertexShader::module`].
    pub body: String,
    /// Shader locations the entry point consumes (bit n = `@location(n)`).
    pub locations: u16,
    /// Whether [`VertexOptions::flat_shading`] was set.
    pub flat_shading: bool,
}

impl VertexShader {
    /// A standalone module (varyings struct included).
    pub fn module(&self) -> String {
        format!(
            "{}{}",
            common::varyings_struct(self.flat_shading),
            self.body
        )
    }
}

/// The values of `XvsUniforms` (group 0, binding 0).
#[derive(Clone, Debug, PartialEq)]
pub struct VertexUniforms {
    /// `c[0..192]`, hardware numbering.
    pub constants: [[f32; 4]; CONSTANT_COUNT],
    /// Values for unsupplied vertex registers.
    pub defaults: [[f32; 4]; INPUT_COUNT],
    /// Guest render-target width and height, and depth full scale.
    pub surface: [f32; 2],
    /// Depth buffer full scale: 65535 (D16), 16777215 (D24), or 1.0 for float depth.
    pub z_max: f32,
    /// W-buffering (`SET_CONTROL0` z-perspective): the depth clip range
    /// `[SET_CLIP_MIN, SET_CLIP_MAX]` that the vertex's w is held to, in
    /// depth units. `None` clips z to `[0, z_max]` as usual. With it the
    /// fragment stage must write the depth (`PixelOptions::w_buffer`).
    pub w_clip: Option<[f32; 2]>,
    /// With `w_clip`: `SET_ZMIN_MAX_CONTROL` clamps instead of culling
    /// (`D3DDCC_DEPTHCLAMP`). Nothing is clipped by w beyond w > 0; the
    /// fragment stage clamps the stored w to the range instead.
    pub w_clamp: bool,
    /// Guest pixels subtracted from x and y after the subpixel truncation:
    /// 0 at the guest's size. A target rendered at `s` times the guest's
    /// size takes `(2 s - 1) / (4 s)` (`gfx::renderer::window_shift`), so
    /// an edge on a guest pixel centre covers whole blocks of host pixels,
    /// as it covers whole pixels at 1x (docs/graphics/high-resolution.md,
    /// "Half-pixel and texel offsets"). Above 0 it also turns the 1/16
    /// truncation off, so seams between draws stay closed at scale
    /// ("Subpixel snapping and cracks").
    pub shift: [f32; 2],
    /// UI window scale (xy) and translation (zw), before raster correction.
    pub ui_transform: [f32; 4],
}

impl Default for VertexUniforms {
    fn default() -> Self {
        Self {
            constants: [[0.0; 4]; CONSTANT_COUNT],
            defaults: [[0.0, 0.0, 0.0, 1.0]; INPUT_COUNT],
            surface: [640.0, 480.0],
            z_max: 16_777_215.0,
            w_clip: None,
            w_clamp: false,
            shift: [0.0; 2],
            ui_transform: [1.0, 1.0, 0.0, 0.0],
        }
    }
}

impl VertexUniforms {
    /// Size of the uniform buffer in bytes.
    pub const SIZE: usize = (CONSTANT_COUNT + INPUT_COUNT + 3) * 16;

    /// Buffer contents matching the WGSL struct layout.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = vec![0; Self::SIZE];
        self.write_bytes(&mut out);
        out
    }

    /// Writes the WGSL uniform layout into an existing buffer of [`Self::SIZE`]
    /// bytes, preserving every float's bits (including NaNs and signed zero).
    pub fn write_bytes(&self, out: &mut [u8]) {
        assert_eq!(out.len(), Self::SIZE);
        let viewport = [self.surface[0], self.surface[1], self.z_max, self.shift[0]];
        let clip = match self.w_clip {
            Some([lo, hi]) => [lo, hi, if self.w_clamp { 2.0 } else { 1.0 }, self.shift[1]],
            None => [0.0, 0.0, 0.0, self.shift[1]],
        };
        let out = common::uniform_vectors(out, &self.constants);
        let out = common::uniform_vectors(out, &self.defaults);
        common::uniform_vectors(out, &[viewport, clip, self.ui_transform]);
    }
}

pub(crate) const PRELUDE: &str = r#"
struct XvsUniforms {
    // c[0..192], hardware numbering (D3D c-96 is c[0]).
    c: array<vec4f, 192>,
    // Values for vertex registers the layout does not supply.
    defaults: array<vec4f, 16>,
    // x, y: guest render-target size in the units the program's viewport
    // constants use; z: depth full scale (65535 for D16, 16777215 for D24);
    // w: guest pixels taken off x (0 unless the target is scaled).
    viewport: vec4f,
    // x, y: depth clip range. z: 0 off, 1 cull, 2 clamp.
    // w: guest pixels taken off y.
    clip: vec4f,
    ui_transform: vec4f,
}
@group(0) @binding(0) var<uniform> xvs: XvsUniforms;

struct XvsRegs {
    pos: vec4f, d0: vec4f, d1: vec4f, fog: vec4f, pts: vec4f,
    b0: vec4f, b1: vec4f, t0: vec4f, t1: vec4f, t2: vec4f, t3: vec4f,
}

// Zero times anything (inf and NaN included) is zero on the NV2A.
fn xvs_mul(a: vec4f, b: vec4f) -> vec4f {
    return select(a * b, vec4f(0.0), (a == vec4f(0.0)) | (b == vec4f(0.0)));
}
fn xvs_dp3(a: vec4f, b: vec4f) -> vec4f { let p = xvs_mul(a, b); return vec4f(p.x + p.y + p.z); }
fn xvs_dph(a: vec4f, b: vec4f) -> vec4f { let p = xvs_mul(a, b); return vec4f(p.x + p.y + p.z + b.w); }
fn xvs_dp4(a: vec4f, b: vec4f) -> vec4f { let p = xvs_mul(a, b); return vec4f(p.x + p.y + p.z + p.w); }
fn xvs_dst(a: vec4f, b: vec4f) -> vec4f { return vec4f(1.0, xvs_mul(a, b).y, a.z, b.w); }
fn xvs_slt(a: vec4f, b: vec4f) -> vec4f { return select(vec4f(0.0), vec4f(1.0), a < b); }
fn xvs_sge(a: vec4f, b: vec4f) -> vec4f { return select(vec4f(0.0), vec4f(1.0), a >= b); }

// Clamp away from zero and infinity, keeping the sign; +0 counts as positive.
// The sign comes from the bit pattern: Metal's fast-math does not keep
// signed-zero or infinity comparisons reliable.
fn xvs_neg(t: f32) -> bool { return (bitcast<u32>(t) & 0x80000000u) != 0u; }
fn xvs_clamp_away(t: f32) -> f32 {
    let r = clamp(abs(t), 0x1p-64f, 0x1p64f);
    return select(r, -r, xvs_neg(t));
}
// NORMPACKED3: 11:11:10 signed fields (x lowest), each max(v / (2^(b-1) - 1), -1).
fn xvs_normpacked3(p: u32) -> vec4f {
    let x = f32(bitcast<i32>(p << 21u) >> 21u) / 1023.0;
    let y = f32(bitcast<i32>(p << 10u) >> 21u) / 1023.0;
    let z = f32(bitcast<i32>(p) >> 22u) / 511.0;
    return vec4f(max(vec3f(x, y, z), vec3f(-1.0)), 1.0);
}
fn xvs_rcp(x: f32) -> vec4f { return vec4f(1.0 / x); }
// Saturate on |x| before dividing: 1/0 = inf then clamped gives -2^-64 on
// Metal (fast-math), so no infinity may reach the clamp.
fn xvs_rcc(x: f32) -> vec4f {
    let t = abs(x);
    var r = 1.0 / t;
    if (t < 0x1p-64f) { r = 0x1p64f; } else if (t > 0x1p64f) { r = 0x1p-64f; }
    return vec4f(select(r, -r, xvs_neg(x)));
}
fn xvs_rsq(x: f32) -> vec4f { return vec4f(1.0 / sqrt(abs(x))); }
// Infinities are recognised by their bits for the same reason.
fn xvs_exp(x: f32) -> vec4f {
    let bits = bitcast<u32>(x);
    if ((bits & 0x7fffffffu) == 0x7f800000u) {
        let e = select(bitcast<f32>(0x7f800000u), 0.0, xvs_neg(x));
        return vec4f(e, 0.0, e, 1.0);
    }
    let f = floor(x);
    return vec4f(exp2(f), x - f, exp2(x), 1.0);
}
// Exponent and mantissa straight from the IEEE fields (floor(log2(t))
// rounds up just below powers of two). Zero and denormals give
// (-inf, 1, -inf, 1), infinity (inf, 1, inf, 1).
fn xvs_log(x: f32) -> vec4f {
    let bits = bitcast<u32>(x) & 0x7fffffffu;
    let e = bits >> 23u;
    let inf = bitcast<f32>(0x7f800000u);
    if (e == 0u) { return vec4f(-inf, 1.0, -inf, 1.0); }
    if (bits == 0x7f800000u) { return vec4f(inf, 1.0, inf, 1.0); }
    let m = bitcast<f32>((bits & 0x007fffffu) | 0x3f800000u);
    return vec4f(f32(i32(e) - 127), m, log2(bitcast<f32>(bits)), 1.0);
}
// a0.x = floor(x + 0.001), held to +-1024 so a0 + index cannot overflow
// (anything past that saturates the 0..191 clamp anyway); NaN gives 0.
fn xvs_arl(x: f32) -> i32 {
    let f = floor(x + 0.001);
    let nan = (bitcast<u32>(f) & 0x7fffffffu) > 0x7f800000u;
    return select(i32(clamp(f, -1024.0, 1024.0)), 0, nan);
}
fn xvs_lit(s: vec4f) -> vec4f {
    let x = max(s.x, 0.0);
    let y = max(s.y, 0.0);
    let p = clamp(s.w, -(128.0 - 1.0 / 256.0), 128.0 - 1.0 / 256.0);
    var z = 0.0;
    if (x > 0.0) { z = exp2(p * log2(y)); }
    return vec4f(1.0, x, z, 1.0);
}

// NV2A window coordinates (pixels, depth in [0, z full scale], w kept for
// perspective) to wgpu clip space. The rasterizer keeps 4 fractional bits
// and truncates. The NV2A's depth clip range is inclusive, so z exactly at
// the depth maximum is inside; z / Zmax can round above 1 (Metal divides
// through a reciprocal), so it is held to 1 there. Anything past Zmax must
// still clip, but the division can also round down to 1 there, so such z is
// pushed to at least the float after 1. The comparison is on the bits
// (Zmax > 0, so for non-NaN z, int order is float order above it): a float
// compare lets fast math fold the select into a bare min(zn, 1).
//
// At a render scale (`viewport.w`, the shift, above 0) a position past the
// last pixel centre is held at least on the surface's far edge: the shift
// would otherwise pull a quad ending on that edge (a 1:1 texel mapping, the
// usual full-screen pass) off the last host columns and rows. It is off at
// 1x (no shift), where such a position covers the last pixel as the edge
// does: holding it there changes no coverage the guest would see.
//
// Scaled, x and y are not truncated: the host rasterizer's own sub-pixel
// grid (much finer than a host pixel) applies. On the guest's 1/16 grid a
// vertex that two draws compute a few ulps apart, or a T-junction vertex,
// lands up to 1/16 of a guest pixel off the neighbouring draw's edge: at
// 1x that sliver covers few pixel centres, at 4x it is a quarter of a host
// pixel wide and shows as a see-through line along the seam
// (docs/graphics/high-resolution.md, "Subpixel snapping and cracks").
fn xvs_fixup(p: vec4f) -> vec4f {
    let shift = vec2f(xvs.viewport.w, xvs.clip.w);
    let window = p.xy * xvs.ui_transform.xy + xvs.ui_transform.zw;
    let t = select(trunc(window * 16.0) / 16.0, window, shift > vec2f(0.0));
    let size = xvs.viewport.xy;
    let far = (shift > vec2f(0.0)) & (t > size - 0.5);
    let xy = select(t - shift, max(t - shift, size), far);
    let w = xvs_clamp_away(p.w);
    let zn = p.z / xvs.viewport.z;
    let over = bitcast<i32>(p.z) > bitcast<i32>(xvs.viewport.z);
    let z = select(min(zn, 1.0), max(zn, bitcast<f32>(0x3f800001u)), over);
    let n = vec3f(2.0 * xy.x / xvs.viewport.x - 1.0, 1.0 - 2.0 * xy.y / xvs.viewport.y, z);
    if (xvs.clip.z != 0.0) {
        // W-buffered: the depth is w (written by the fragment stage), and
        // the clip range applies to w. Clip z = a (w - lo) with
        // a = hi / (hi - lo) is 0 at w = lo and equals w at w = hi, and it
        // is linear in the clip coordinates, so the rasterizer's
        // 0 <= z <= w clip is exactly lo <= w <= hi.
        // Clamped instead: z = w / 2 is always inside for w > 0. The
        // stored depth is w, from the fragment stage, so z only decides
        // clipping. Such a draw's pipeline also leaves depth unclipped
        // (gfx::translate::primitive_state, with DEPTH_CLIP_CONTROL), as
        // the NV2A keeps what lies outside the range; z then decides
        // nothing there.
        let lo = xvs.clip.x;
        let hi = xvs.clip.y;
        let z = select(hi / (hi - lo) * (w - lo), 0.5 * w, xvs.clip.z == 2.0);
        return vec4f(n.xy * w, z, w);
    }
    return vec4f(n * w, w);
}
"#;

/// Why a program could not be translated.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TranslateError {
    /// State programs run without vertices and have no place in a pipeline.
    #[error("state programs (0x7378) are not vertex shaders")]
    StateProgram,
}

fn src_expr(s: &Source, consts: &str) -> String {
    let base = match s.reg {
        SrcReg::Temp(12) => "o.pos".to_string(),
        SrcReg::Temp(n) => format!("r{n}"),
        SrcReg::Input(n) => format!("v{n}"),
        SrcReg::Const {
            index,
            relative: false,
        } => format!("{consts}[{index}]"),
        SrcReg::Const {
            index,
            relative: true,
        } => format!("{consts}[clamp(a0 + {index}, 0, 191)]"),
        SrcReg::Unused => "vec4f(0.0)".to_string(),
    };
    let names = b"xyzw";
    let swz = if s.swizzle == [0, 1, 2, 3] {
        base
    } else {
        let sw: String = s
            .swizzle
            .iter()
            .map(|&c| names[c as usize] as char)
            .collect();
        format!("{base}.{sw}")
    };
    if s.neg { format!("-{swz}") } else { swz }
}

fn mask_bools(m: Mask) -> String {
    let c = m.components();
    format!("vec4<bool>({}, {}, {}, {})", c[0], c[1], c[2], c[3])
}

fn masked_write(out: &mut String, dst: &str, val: &str, m: Mask) {
    if m == Mask::XYZW {
        let _ = writeln!(out, "        {dst} = {val};");
    } else {
        let _ = writeln!(
            out,
            "        {dst} = select({dst}, {val}, {});",
            mask_bools(m)
        );
    }
}

fn out_field(r: OutReg) -> String {
    match r {
        OutReg::Pos => "o.pos".into(),
        OutReg::D0 => "o.d0".into(),
        OutReg::D1 => "o.d1".into(),
        OutReg::Fog => "o.fog".into(),
        OutReg::Pts => "o.pts".into(),
        OutReg::B0 => "o.b0".into(),
        OutReg::B1 => "o.b1".into(),
        OutReg::T(n) => format!("o.t{n}"),
    }
}

fn temp_name(r: u8) -> String {
    if r == 12 {
        "o.pos".into()
    } else {
        format!("r{r}")
    }
}

/// Translates a program to WGSL.
pub fn translate(
    program: &Program,
    options: &VertexOptions,
) -> Result<VertexShader, TranslateError> {
    if program.kind == ProgramKind::State {
        return Err(TranslateError::StateProgram);
    }
    let reads = program.inputs_read();
    let local_consts = program.writes_constants();
    let consts = if local_consts { "cw" } else { "xvs.c" };

    let mut s = String::new();
    s.push_str(PRELUDE);

    // The program function.
    let params: Vec<String> = (0..16)
        .filter(|n| reads & (1 << n) != 0)
        .map(|n| format!("v{n}: vec4f"))
        .collect();
    let _ = writeln!(s, "\nfn xvs_program({}) -> XvsRegs {{", params.join(", "));
    s.push_str("    var o: XvsRegs;\n");
    for f in [
        "pos", "d0", "d1", "fog", "pts", "b0", "b1", "t0", "t1", "t2", "t3",
    ] {
        let _ = writeln!(s, "    o.{f} = vec4f(0.0, 0.0, 0.0, 1.0);");
    }
    for n in 0..TEMP_COUNT {
        let _ = writeln!(s, "    var r{n} = vec4f(0.0);");
    }
    s.push_str("    var a0: i32 = 0;\n");
    if local_consts {
        // Constant writes stay within this vertex; see docs/graphics/shaders.md.
        s.push_str("    var cw = xvs.c;\n");
    }
    for (i, inst) in program.instructions.iter().enumerate() {
        let _ = writeln!(
            s,
            "    // {i:03}: {inst}\n    // {:08x} {:08x} {:08x} {:08x}",
            inst.words[0], inst.words[1], inst.words[2], inst.words[3]
        );
        s.push_str("    {\n");
        let (ra, rb, rc) = inst.mac_reads();
        let rc = rc || inst.ilu != IluOp::Nop;
        if ra {
            let _ = writeln!(s, "        let a = {};", src_expr(&inst.a, consts));
        }
        if rb {
            let _ = writeln!(s, "        let b = {};", src_expr(&inst.b, consts));
        }
        if rc {
            let _ = writeln!(s, "        let c = {};", src_expr(&inst.c, consts));
        }
        let mac = match inst.mac {
            MacOp::Nop => None,
            MacOp::Mov | MacOp::Arl => Some("a".to_string()),
            MacOp::Mul => Some("xvs_mul(a, b)".into()),
            MacOp::Add => Some("a + c".into()),
            MacOp::Mad => Some("xvs_mul(a, b) + c".into()),
            MacOp::Dp3 => Some("xvs_dp3(a, b)".into()),
            MacOp::Dph => Some("xvs_dph(a, b)".into()),
            MacOp::Dp4 => Some("xvs_dp4(a, b)".into()),
            MacOp::Dst => Some("xvs_dst(a, b)".into()),
            MacOp::Min => Some("min(a, b)".into()),
            MacOp::Max => Some("max(a, b)".into()),
            MacOp::Slt => Some("xvs_slt(a, b)".into()),
            MacOp::Sge => Some("xvs_sge(a, b)".into()),
        };
        if let Some(m) = mac {
            let _ = writeln!(s, "        let mac = {m};");
        }
        let ilu = match inst.ilu {
            IluOp::Nop => None,
            IluOp::Mov => Some("c".to_string()),
            IluOp::Rcp => Some("xvs_rcp(c.x)".into()),
            IluOp::Rcc => Some("xvs_rcc(c.x)".into()),
            IluOp::Rsq => Some("xvs_rsq(c.x)".into()),
            IluOp::Exp => Some("xvs_exp(c.x)".into()),
            IluOp::Log => Some("xvs_log(c.x)".into()),
            IluOp::Lit => Some("xvs_lit(c)".into()),
        };
        if let Some(l) = ilu {
            let _ = writeln!(s, "        let ilu = {l};");
        }
        if inst.mac == MacOp::Arl {
            s.push_str("        a0 = xvs_arl(mac.x);\n");
        }
        if inst.mac_writes_temp() {
            masked_write(&mut s, &temp_name(inst.mac_temp), "mac", inst.mac_mask);
        }
        if inst.ilu_writes_temp() {
            masked_write(&mut s, &temp_name(inst.ilu_temp), "ilu", inst.ilu_mask);
        }
        if let Some(o) = inst.out {
            let val = if o.unit == Unit::Mac { "mac" } else { "ilu" };
            match o.dest {
                OutDest::Output(OutReg::Fog) => {
                    let _ = writeln!(s, "        o.fog.x = {val}.x;");
                }
                OutDest::Output(r) => masked_write(&mut s, &out_field(r), val, o.mask),
                OutDest::Const(i) if (i as usize) < CONSTANT_COUNT => {
                    masked_write(&mut s, &format!("cw[{i}]"), val, o.mask)
                }
                OutDest::Const(_) => {}
            }
        }
        s.push_str("    }\n");
    }
    s.push_str("    return o;\n}\n");

    // Entry point.
    let provided = reads & options.provided_inputs;
    let entry_params: Vec<String> = (0..16)
        .filter(|n| provided & (1 << n) != 0)
        .map(|n| {
            format!(
                "@location({n}) v{n}: {}",
                options.input_fetch[n].wgsl_type()
            )
        })
        .collect();
    let args: Vec<String> = (0..16)
        .filter(|n| reads & (1 << n) != 0)
        .map(|n| {
            if provided & (1 << n) != 0 {
                options.input_fetch[n].wgsl_value(&format!("v{n}"))
            } else {
                format!("xvs.defaults[{n}]")
            }
        })
        .collect();
    let _ = writeln!(
        s,
        r#"
@vertex
fn vs_main({}) -> XboxVaryings {{
    let o = xvs_program({});
    var out: XboxVaryings;
    out.position = xvs_fixup(o.pos);
    out.d0 = clamp(o.d0, vec4f(0.0), vec4f(1.0));
    out.d1 = clamp(o.d1, vec4f(0.0), vec4f(1.0));
    out.b0 = clamp(o.b0, vec4f(0.0), vec4f(1.0));
    out.b1 = clamp(o.b1, vec4f(0.0), vec4f(1.0));
    out.t0 = o.t0;
    out.t1 = o.t1;
    out.t2 = o.t2;
    out.t3 = o.t3;
    out.fog = o.fog.x;
    return out;
}}"#,
        entry_params.join(", "),
        args.join(", ")
    );
    Ok(VertexShader {
        body: s,
        locations: provided,
        flat_shading: options.flat_shading,
    })
}
