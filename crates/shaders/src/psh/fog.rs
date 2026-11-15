//! The NV2A fog unit: how the interpolated fog coordinate becomes the
//! factor the combiners read as `fog.a`.
//!
//! The vertex stage writes a fog *coordinate* (`oFog.x`). With a vertex
//! program the program computes it; the fog generation mode
//! (`NV097_SET_FOG_GEN_MODE`: specular alpha, radial, planar, absolute
//! planar, fog X) only selects the coordinate of the fixed-function
//! transform, which these programs never use (xemu's `glsl/vsh.c` takes
//! `oFog.x` as the distance whenever a program is bound). The fog unit then
//! applies `NV097_SET_FOG_MODE` with the two `NV097_SET_FOG_PARAMS` values
//! D3D computes (bias, scale) to that coordinate:
//!
//! | mode | method value | factor |
//! |---|---|---|
//! | linear | `0x2601` | `bias + scale·d − 1` |
//! | exp | `0x0800` | `bias + 2^(16·scale·d) − 1.5` |
//! | exp2 | `0x0801` | `bias + 2^(−32·(scale·d)²) − 1.5` |
//! | exp abs, exp2 abs, linear abs | `0x0802`, `0x0803`, `0x0804` | the same with `|d|` |
//!
//! The factor is clamped to [0, 1] before the combiners see it. With fog
//! disabled (`NV097_SET_FOG_ENABLE` 0) the factor reads 1, so D3D's final
//! combiner (`fog.a·r0 + (1 − fog.a)·fog.rgb`) leaves the colour alone.
//! The fog register's rgb is `NV097_SET_FOG_COLOR`; its alpha is always the
//! factor, never the colour's alpha.
//!
//! The formulas are the ones xemu's `glsl/vsh.c` evaluates and match the
//! parameters D3D derives (see `d3d::nv2a::d3d_fog`), which give D3D's
//! `(end − d)/(end − start)`, `e^(−density·d)` and `e^(−(density·d)²)`.
//! Two choices here are not settled by hardware evidence in this
//! repository:
//!
//! * the factor is evaluated per fragment from the interpolated coordinate
//!   (xemu evaluates it per vertex and marks that as a guess). The two
//!   agree for linear fog, and differ inside a triangle for the exp modes;
//! * the `ABS` modes take the absolute value of the coordinate (the
//!   OpenGL eye-plane-absolute meaning; xemu takes the absolute value of
//!   the factor instead). Xbox D3D never selects them.

/// `NV097_SET_FOG_MODE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FogMode {
    /// `0x2601`.
    Linear,
    /// `0x0800`.
    Exp,
    /// `0x0801`.
    Exp2,
    /// `0x0804`.
    LinearAbs,
    /// `0x0802`.
    ExpAbs,
    /// `0x0803`.
    Exp2Abs,
}

impl FogMode {
    /// From the method value, or `None` for a value the NV2A does not
    /// define.
    pub fn from_method(value: u32) -> Option<Self> {
        Some(match value {
            0x2601 => Self::Linear,
            0x0800 => Self::Exp,
            0x0801 => Self::Exp2,
            0x0802 => Self::ExpAbs,
            0x0803 => Self::Exp2Abs,
            0x0804 => Self::LinearAbs,
            _ => return None,
        })
    }

    /// The method value.
    pub fn method(self) -> u32 {
        match self {
            Self::Linear => 0x2601,
            Self::Exp => 0x0800,
            Self::Exp2 => 0x0801,
            Self::ExpAbs => 0x0802,
            Self::Exp2Abs => 0x0803,
            Self::LinearAbs => 0x0804,
        }
    }

    /// The code in the uniform's x (`xps_fog_factor` in the WGSL).
    fn code(self) -> f32 {
        match self {
            Self::Linear => 1.0,
            Self::Exp => 2.0,
            Self::Exp2 => 3.0,
            Self::LinearAbs => 4.0,
            Self::ExpAbs => 5.0,
            Self::Exp2Abs => 6.0,
        }
    }
}

/// The fog unit's state: enable, mode and the bias and scale of
/// `NV097_SET_FOG_PARAMS` (its third value is always 0 from D3D and is not
/// used).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fog {
    /// `NV097_SET_FOG_ENABLE`.
    pub enable: bool,
    /// `NV097_SET_FOG_MODE`.
    pub mode: FogMode,
    /// `NV097_SET_FOG_PARAMS[0]`.
    pub bias: f32,
    /// `NV097_SET_FOG_PARAMS[1]`.
    pub scale: f32,
}

impl Fog {
    /// Fog off: the factor reads 1.
    pub const OFF: Self = Self {
        enable: false,
        mode: FogMode::Linear,
        bias: 0.0,
        scale: 0.0,
    };

    /// What D3D programs with `D3DFOG_NONE` (vertex fog): linear with bias
    /// and scale 1, so the factor is the coordinate itself.
    pub const FROM_VERTEX: Self = Self {
        enable: true,
        mode: FogMode::Linear,
        bias: 1.0,
        scale: 1.0,
    };

    /// The `XpsUniforms.fog` value: `[code, bias, scale, 0]`, code 0 when
    /// disabled.
    pub fn uniform(&self) -> [f32; 4] {
        let code = if self.enable { self.mode.code() } else { 0.0 };
        [code, self.bias, self.scale, 0.0]
    }

    /// The clamped factor for fog coordinate `d`, as the generated WGSL
    /// computes it (see [`factor_from_uniform`]).
    pub fn factor(&self, d: f32) -> f32 {
        factor_from_uniform(self.uniform(), d)
    }
}

/// The factor from a `XpsUniforms.fog` value (what the interpreter and the
/// WGSL `xps_fog_factor` both evaluate).
///
/// An infinite coordinate (D3D's perspective pass-through program writes
/// `rcp oFog, v0.w`, infinity for `rhw = 0`) follows xemu's `glsl/vsh.c`:
/// `LINEAR`, `LINEAR_ABS` and `EXP` treat it as 0, and the other modes take
/// the limit of their formula (`EXP2`: the exponential term is 0, so the
/// factor is `bias − 1.5`, which clamps to 0 with D3D's bias). The limits
/// are spelled out rather than computed, because the GPU's fast math does
/// not keep IEEE infinities.
pub fn factor_from_uniform(u: [f32; 4], d: f32) -> f32 {
    let code = u[0] as u32;
    let (bias, scale) = (u[1], u[2]);
    let f = if d.is_infinite() {
        match code {
            0 => 1.0,
            1 | 4 => bias - 1.0,
            2 => bias + 1.0 - 1.5,
            5 if scale < 0.0 => bias - 1.5,
            5 => 1.0,
            3 | 6 if scale != 0.0 => bias - 1.5,
            3 | 6 => bias + 1.0 - 1.5,
            _ => 1.0,
        }
    } else {
        let d = if code >= 4 { d.abs() } else { d };
        match code {
            0 => 1.0,
            1 | 4 => bias + scale * d - 1.0,
            2 | 5 => bias + (16.0 * scale * d).exp2() - 1.5,
            3 | 6 => {
                let s = scale * d;
                bias + (-32.0 * s * s).exp2() - 1.5
            }
            _ => 1.0,
        }
    };
    f.clamp(0.0, 1.0)
}

/// The WGSL counterpart of [`factor_from_uniform`], reading
/// `xps.fog` = `[code, bias, scale, 0]`. Infinity is recognised by its
/// bits (fast math).
pub(crate) const WGSL: &str = r#"
fn xps_fog_factor(coord: f32) -> f32 {
    let code = u32(xps.fog.x);
    let bias = xps.fog.y;
    let scale = xps.fog.z;
    var f = 1.0;
    if ((bitcast<u32>(coord) & 0x7fffffffu) == 0x7f800000u) {
        if (code == 1u || code == 4u) { f = bias - 1.0; }
        if (code == 2u) { f = bias - 0.5; }
        if (code == 5u) { f = select(1.0, bias - 1.5, scale < 0.0); }
        if (code == 3u || code == 6u) { f = select(bias - 0.5, bias - 1.5, scale != 0.0); }
        return clamp(f, 0.0, 1.0);
    }
    let d = select(coord, abs(coord), code >= 4u);
    if (code == 1u || code == 4u) { f = bias + scale * d - 1.0; }
    if (code == 2u || code == 5u) { f = bias + exp2(16.0 * scale * d) - 1.5; }
    if (code == 3u || code == 6u) { let s = scale * d; f = bias + exp2(-32.0 * s * s) - 1.5; }
    return clamp(f, 0.0, 1.0);
}
"#;
