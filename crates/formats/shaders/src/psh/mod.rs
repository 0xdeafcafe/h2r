//! NV2A pixel pipeline: texture-stage modes plus register combiners, as
//! described by the Xbox SDK's `D3DPIXELSHADERDEF`.
//!
//! The structure is 60 little-endian dwords (0xF0 bytes) and mirrors the
//! NV2A's own register file, which is why D3D can copy it almost verbatim
//! into the push buffer. Field order (from Cxbx-Reloaded's
//! `X_D3DPIXELSHADERDEF`):
//!
//! ```text
//! 0x00 PSAlphaInputs[8]      0x88 PSRGBInputs[8]       0xD4 PSCombinerCount
//! 0x20 PSFinalCombinerInputsABCD                       0xD8 PSTextureModes
//! 0x24 PSFinalCombinerInputsEFG                        0xDC PSDotMapping
//! 0x28 PSConstant0[8]        0xA8 PSCompareMode        0xE0 PSInputTexture
//! 0x48 PSConstant1[8]        0xAC PSFinalCombinerConstant0  0xE4 PSC0Mapping
//! 0x68 PSAlphaOutputs[8]     0xB0 PSFinalCombinerConstant1  0xE8 PSC1Mapping
//!                            0xB4 PSRGBOutputs[8]      0xEC PSFinalCombinerConstants
//! ```

pub mod bits;
mod disasm;
pub mod fog;
pub mod interp;
pub mod shadow;
mod wgsl;

pub use disasm::disassemble;
pub use interp::Uniforms as PixelUniforms;
pub use wgsl::{AlphaFunc, COUNT_DEPTH_ENTRY, PixelOptions, PixelShader, translate, translate_with_default_final_uniform};

/// Size of a `D3DPIXELSHADERDEF`.
pub const PSDEF_SIZE: usize = 0xF0;

/// Too few bytes for a `D3DPIXELSHADERDEF`.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("pixel shader def needs {PSDEF_SIZE} bytes, have {0}")]
pub struct Truncated(pub usize);

/// The raw `D3DPIXELSHADERDEF`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[allow(missing_docs)]
pub struct PixelShaderDef {
    pub alpha_inputs: [u32; 8],
    pub final_inputs_abcd: u32,
    pub final_inputs_efg: u32,
    pub constant0: [u32; 8],
    pub constant1: [u32; 8],
    pub alpha_outputs: [u32; 8],
    pub rgb_inputs: [u32; 8],
    pub compare_mode: u32,
    pub final_constant0: u32,
    pub final_constant1: u32,
    pub rgb_outputs: [u32; 8],
    pub combiner_count: u32,
    pub texture_modes: u32,
    pub dot_mapping: u32,
    pub input_texture: u32,
    pub c0_mapping: u32,
    pub c1_mapping: u32,
    pub final_constants: u32,
    /// The final combiner runs even if both of its input dwords are zero.
    /// Not part of the 60-dword layout: D3D latches whether the bound
    /// definition has a final combiner when `SetPixelShader` runs (device
    /// `+0x788`), and later pushes of zero to the final-combiner methods do
    /// not clear that latch, so the hardware then runs an all-zero final
    /// combiner (black, alpha 0) rather than D3D's default one.
    pub explicit_final: bool,
}

impl PixelShaderDef {
    /// Reads the 0xF0-byte structure (little endian). Extra bytes are ignored.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Truncated> {
        if bytes.len() < PSDEF_SIZE {
            return Err(Truncated(bytes.len()));
        }
        let mut w = [0u32; PSDEF_SIZE / 4];
        for (w, c) in w.iter_mut().zip(bytes[..PSDEF_SIZE].as_chunks::<4>().0) {
            *w = u32::from_le_bytes(*c);
        }
        Ok(Self::from_words(&w))
    }

    /// The structure from its 60 dwords, as [`from_bytes`](Self::from_bytes)
    /// reads them (without the byte round trip or an allocation: the guest
    /// judge builds one per draw).
    pub fn from_words(w: &[u32; PSDEF_SIZE / 4]) -> Self {
        let arr = |at: usize| -> [u32; 8] { std::array::from_fn(|i| w[at + i]) };
        Self {
            alpha_inputs: arr(0),
            final_inputs_abcd: w[8],
            final_inputs_efg: w[9],
            constant0: arr(10),
            constant1: arr(18),
            alpha_outputs: arr(26),
            rgb_inputs: arr(34),
            compare_mode: w[42],
            final_constant0: w[43],
            final_constant1: w[44],
            rgb_outputs: arr(45),
            combiner_count: w[53],
            texture_modes: w[54],
            explicit_final: false,
            dot_mapping: w[55],
            input_texture: w[56],
            c0_mapping: w[57],
            c1_mapping: w[58],
            final_constants: w[59],
        }
    }

    /// Serialises back to the 0xF0-byte layout.
    pub fn to_bytes(&self) -> [u8; PSDEF_SIZE] {
        let mut words = Vec::with_capacity(60);
        words.extend_from_slice(&self.alpha_inputs);
        words.extend([self.final_inputs_abcd, self.final_inputs_efg]);
        words.extend_from_slice(&self.constant0);
        words.extend_from_slice(&self.constant1);
        words.extend_from_slice(&self.alpha_outputs);
        words.extend_from_slice(&self.rgb_inputs);
        words.extend([
            self.compare_mode,
            self.final_constant0,
            self.final_constant1,
        ]);
        words.extend_from_slice(&self.rgb_outputs);
        words.extend([
            self.combiner_count,
            self.texture_modes,
            self.dot_mapping,
            self.input_texture,
            self.c0_mapping,
            self.c1_mapping,
            self.final_constants,
        ]);
        let mut out = [0u8; PSDEF_SIZE];
        for (i, w) in words.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
        }
        out
    }

    /// The constants embedded in the definition (from `def` statements) as
    /// RGBA floats: `[stage 0..8]`, index 8 being the final combiner. The
    /// game normally overrides these with `SetPixelShaderConstant`.
    pub fn embedded_constants(&self) -> ([[f32; 4]; 9], [[f32; 4]; 9]) {
        let mut c0 = [[0.0; 4]; 9];
        let mut c1 = [[0.0; 4]; 9];
        for i in 0..8 {
            c0[i] = argb(self.constant0[i]);
            c1[i] = argb(self.constant1[i]);
        }
        c0[8] = argb(self.final_constant0);
        c1[8] = argb(self.final_constant1);
        (c0, c1)
    }

    /// Whether the definition programs the final combiner: either inputs
    /// dword non-zero, or [`Self::explicit_final`]. D3D keeps this in
    /// device `+0x788`.
    pub fn has_final_combiner(&self) -> bool {
        self.explicit_final || self.final_inputs_abcd != 0 || self.final_inputs_efg != 0
    }

    /// The definition as the NV2A runs it under the given render states.
    ///
    /// When the definition has no final combiner, `SetPixelShader` does not
    /// push one. It marks the specular/fog state dirty instead, and the
    /// lazy commit (`0x00400000`, after the fog methods) programs the
    /// final combiner D3D also uses for the fixed-function path; see
    /// [`d3d_default_final_combiner`]. With a final combiner the
    /// definition is returned unchanged.
    pub fn with_default_final_combiner(&self, fog: bool, specular: bool) -> Self {
        let mut def = self.clone();
        if !def.has_final_combiner() {
            (def.final_inputs_abcd, def.final_inputs_efg) =
                d3d_default_final_combiner(fog, specular);
        }
        def
    }

    /// Structured view.
    pub fn decode(&self) -> Combiners {
        let count = (self.combiner_count & 0xFF).clamp(0, 8) as usize;
        let flags = self.combiner_count >> 8;
        let stages = (0..count)
            .map(|i| Stage {
                rgb: Portion {
                    inputs: inputs(self.rgb_inputs[i]),
                    output: output(self.rgb_outputs[i]),
                },
                alpha: Portion {
                    inputs: inputs(self.alpha_inputs[i]),
                    output: output(self.alpha_outputs[i]),
                },
            })
            .collect();
        let final_combiner = self.has_final_combiner().then(|| {
            let [a, b, c, d] = inputs(self.final_inputs_abcd);
            let [e, f, g, _] = inputs(self.final_inputs_efg);
            let fl = self.final_inputs_efg & 0xFF;
            FinalCombiner {
                a,
                b,
                c,
                d,
                e,
                f,
                g,
                clamp_sum: fl & 0x80 != 0,
                complement_v1: fl & 0x40 != 0,
                complement_r0: fl & 0x20 != 0,
            }
        });
        Combiners {
            stages,
            mux_msb: flags & 0x001 != 0,
            unique_c0: flags & 0x010 != 0,
            unique_c1: flags & 0x100 != 0,
            final_combiner,
            texture_modes: std::array::from_fn(|i| {
                TexMode::from_bits((self.texture_modes >> (5 * i)) & 0x1F)
            }),
            dot_mapping: [
                DotMap::Unused,
                DotMap::from_bits(self.dot_mapping & 0xF),
                DotMap::from_bits((self.dot_mapping >> 4) & 0xF),
                DotMap::from_bits((self.dot_mapping >> 8) & 0xF),
            ],
            input_texture: [
                0,
                0,
                ((self.input_texture >> 16) & 0xF) as u8,
                ((self.input_texture >> 20) & 0xF) as u8,
            ],
            compare_mode: std::array::from_fn(|i| ((self.compare_mode >> (4 * i)) & 0xF) as u8),
        }
    }
}

impl Combiners {
    /// The earlier stage whose result stage `i` reads as its source (bump,
    /// dependent and dot modes), or `None` when the selector names stage `i`
    /// itself or a later one. Stage 2 may read t0..t1 and stage 3 t0..t2.
    pub fn input_stage(&self, i: usize) -> Option<usize> {
        let s = *self.input_texture.get(i)? as usize;
        (s < i).then_some(s)
    }
}

impl TexMode {
    /// Modes that read an earlier stage's result (`PSInputTexture`).
    pub fn reads_input_texture(self) -> bool {
        self.is_dot()
            || matches!(
                self,
                TexMode::BumpEnvMap
                    | TexMode::BumpEnvMapLum
                    | TexMode::DependentAr
                    | TexMode::DependentGb
            )
    }
    /// Modes that dot the texture coordinate with the mapped source.
    pub fn is_dot(self) -> bool {
        use TexMode::*;
        matches!(
            self,
            DotProduct
                | DotSt
                | DotZw
                | DotRflctDiff
                | DotRflctSpec
                | DotRflctSpecConst
                | DotStr3d
                | DotStrCube
        )
    }
}

/// The final combiner (`PSFinalCombinerInputsABCD`, `...EFG`) Xbox D3D
/// programs when the pixel shader has none, or for the fixed-function
/// path, from `D3DRS_FOGENABLE` and `D3DRS_SPECULARENABLE`. Read from
/// Halo 2's D3D (`0x00400146` and `0x004001b8` push
/// `NV097_SET_COMBINER_SPECULAR_FOG_CW0/1`):
///
/// * fog: `A = fog.a, B = r0 (v1+r0 with specular), C = fog.rgb, D = 0`,
///   so `rgb = fog.a * color + (1 - fog.a) * fog.rgb`;
/// * no fog: `D = r0` (or `v1+r0`), `A = B = C = 0`;
/// * always `G = r0.a` with the clamp-sum flag.
pub fn d3d_default_final_combiner(fog: bool, specular: bool) -> (u32, u32) {
    let color = if specular {
        bits::REG_V1R0_SUM
    } else {
        bits::REG_R0
    };
    let abcd = if fog {
        bits::inputs(
            bits::REG_FOG | bits::CHAN_ALPHA,
            color,
            bits::REG_FOG,
            bits::REG_ZERO,
        )
    } else {
        bits::inputs(0, 0, 0, color)
    };
    let efg = bits::final_efg(0, 0, bits::REG_R0 | bits::CHAN_ALPHA, bits::FINAL_CLAMP_SUM);
    (abcd, efg)
}

/// `D3DCOLOR` (ARGB) to RGBA floats.
pub fn argb(c: u32) -> [f32; 4] {
    let ch = |s: u32| ((c >> s) & 0xFF) as f32 / 255.0;
    [ch(16), ch(8), ch(0), ch(24)]
}

/// Combiner register numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum Reg {
    /// Zero on read, discard on write.
    Zero,
    C0,
    C1,
    /// Fog colour (rgb) and fog factor (alpha).
    Fog,
    /// Diffuse (oD0 / oB0).
    V0,
    /// Specular (oD1 / oB1).
    V1,
    T(u8),
    R0,
    R1,
    /// Final combiner only: `v1 + r0` (rgb), alpha zero.
    V1R0Sum,
    /// Final combiner only: `E * F` (rgb), alpha zero.
    EfProd,
    /// Register numbers 6 and 7.
    Invalid(u8),
}

impl Reg {
    fn from_bits(b: u32) -> Self {
        match b & 0xF {
            0 => Self::Zero,
            1 => Self::C0,
            2 => Self::C1,
            3 => Self::Fog,
            4 => Self::V0,
            5 => Self::V1,
            n @ 8..=11 => Self::T(n as u8 - 8),
            12 => Self::R0,
            13 => Self::R1,
            14 => Self::V1R0Sum,
            15 => Self::EfProd,
            n => Self::Invalid(n as u8),
        }
    }
    /// Short name, as in the D3D assembler.
    pub fn name(self) -> String {
        match self {
            Self::Zero => "zero".into(),
            Self::C0 => "c0".into(),
            Self::C1 => "c1".into(),
            Self::Fog => "fog".into(),
            Self::V0 => "v0".into(),
            Self::V1 => "v1".into(),
            Self::T(n) => format!("t{n}"),
            Self::R0 => "r0".into(),
            Self::R1 => "r1".into(),
            Self::V1R0Sum => "sum".into(),
            Self::EfProd => "prod".into(),
            Self::Invalid(n) => format!("invalid{n}"),
        }
    }
}

/// Input mapping applied when a combiner reads a register.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum InputMap {
    /// `max(0, x)`
    UnsignedIdentity,
    /// `1 - clamp(x, 0, 1)`
    UnsignedInvert,
    /// `2 * max(0, x) - 1`
    ExpandNormal,
    /// `1 - 2 * max(0, x)`
    ExpandNegate,
    /// `max(0, x) - 0.5`
    HalfBiasNormal,
    /// `0.5 - max(0, x)`
    HalfBiasNegate,
    /// `x`
    SignedIdentity,
    /// `-x`
    SignedNegate,
}

/// One of a combiner's four inputs (A, B, C, D).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Input {
    /// Register.
    pub reg: Reg,
    /// Alpha channel (`.a`); otherwise RGB (in the RGB portion) or blue (in
    /// the alpha portion).
    pub alpha: bool,
    /// Mapping.
    pub map: InputMap,
}

fn input(b: u32) -> Input {
    let map = match (b >> 5) & 7 {
        0 => InputMap::UnsignedIdentity,
        1 => InputMap::UnsignedInvert,
        2 => InputMap::ExpandNormal,
        3 => InputMap::ExpandNegate,
        4 => InputMap::HalfBiasNormal,
        5 => InputMap::HalfBiasNegate,
        6 => InputMap::SignedIdentity,
        _ => InputMap::SignedNegate,
    };
    Input {
        reg: Reg::from_bits(b),
        alpha: b & 0x10 != 0,
        map,
    }
}

/// A, B, C, D from an inputs dword (A in the top byte).
fn inputs(w: u32) -> [Input; 4] {
    [input(w >> 24), input(w >> 16), input(w >> 8), input(w)]
}

/// Output scale/bias.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum OutMap {
    Identity,
    /// `x - 0.5`
    Bias,
    /// `x * 2`
    Shl1,
    /// `(x - 0.5) * 2`
    Shl1Bias,
    /// `x * 4`
    Shl2,
    /// `(x - 0.5) * 4` (not a documented D3D value)
    Shl2Bias,
    /// `x / 2`
    Shr1,
    /// `(x - 0.5) / 2` (not a documented D3D value)
    Shr1Bias,
}

/// A combiner portion's output configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Output {
    /// Destination of `A*B` (or `A.B`).
    pub ab: Reg,
    /// Destination of `C*D` (or `C.D`).
    pub cd: Reg,
    /// Destination of `AB + CD` or `mux(AB, CD)`.
    pub sum: Reg,
    /// `A.B` instead of `A*B` (RGB only).
    pub ab_dot: bool,
    /// `C.D` instead of `C*D` (RGB only).
    pub cd_dot: bool,
    /// Third output is a mux on `r0.a` instead of a sum.
    pub mux: bool,
    /// Scale/bias.
    pub map: OutMap,
    /// Copy the AB blue result into the AB register's alpha (RGB only).
    pub ab_blue_to_alpha: bool,
    /// Copy the CD blue result into the CD register's alpha (RGB only).
    pub cd_blue_to_alpha: bool,
}

fn output(w: u32) -> Output {
    let map = match (w >> 15) & 7 {
        0 => OutMap::Identity,
        1 => OutMap::Bias,
        2 => OutMap::Shl1,
        3 => OutMap::Shl1Bias,
        4 => OutMap::Shl2,
        5 => OutMap::Shl2Bias,
        6 => OutMap::Shr1,
        _ => OutMap::Shr1Bias,
    };
    Output {
        cd: Reg::from_bits(w),
        ab: Reg::from_bits(w >> 4),
        sum: Reg::from_bits(w >> 8),
        cd_dot: w & 0x1000 != 0,
        ab_dot: w & 0x2000 != 0,
        mux: w & 0x4000 != 0,
        map,
        cd_blue_to_alpha: w & 0x40000 != 0,
        ab_blue_to_alpha: w & 0x80000 != 0,
    }
}

/// Inputs and outputs of one portion (RGB or alpha) of a stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Portion {
    /// A, B, C, D.
    pub inputs: [Input; 4],
    /// Outputs.
    pub output: Output,
}

/// One general combiner stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Stage {
    /// RGB portion.
    pub rgb: Portion,
    /// Alpha portion.
    pub alpha: Portion,
}

/// The final combiner: `rgb = A*B + (1-A)*C + D`, `alpha = G`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub struct FinalCombiner {
    pub a: Input,
    pub b: Input,
    pub c: Input,
    pub d: Input,
    pub e: Input,
    pub f: Input,
    pub g: Input,
    /// Clamp `v1 + r0` to `[0, 1]`.
    pub clamp_sum: bool,
    /// Use `1 - v1` in the sum.
    pub complement_v1: bool,
    /// Use `1 - r0` in the sum.
    pub complement_r0: bool,
}

/// `PS_TEXTUREMODES_*`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum TexMode {
    None,
    Project2d,
    Project3d,
    Cubemap,
    Passthru,
    ClipPlane,
    BumpEnvMap,
    BumpEnvMapLum,
    Brdf,
    DotSt,
    DotZw,
    DotRflctDiff,
    DotRflctSpec,
    DotStr3d,
    DotStrCube,
    DependentAr,
    DependentGb,
    DotProduct,
    DotRflctSpecConst,
    Reserved(u8),
}

impl TexMode {
    /// The mode of one stage from its 5-bit field of `PSTEXTUREMODES`.
    pub fn from_bits(b: u32) -> Self {
        use TexMode::*;
        match b {
            0x00 => None,
            0x01 => Project2d,
            0x02 => Project3d,
            0x03 => Cubemap,
            0x04 => Passthru,
            0x05 => ClipPlane,
            0x06 => BumpEnvMap,
            0x07 => BumpEnvMapLum,
            0x08 => Brdf,
            0x09 => DotSt,
            0x0a => DotZw,
            0x0b => DotRflctDiff,
            0x0c => DotRflctSpec,
            0x0d => DotStr3d,
            0x0e => DotStrCube,
            0x0f => DependentAr,
            0x10 => DependentGb,
            0x11 => DotProduct,
            0x12 => DotRflctSpecConst,
            n => Reserved(n as u8),
        }
    }
    /// The texture this mode samples, if any.
    pub fn texture_dim(self) -> Option<TextureDim> {
        use TexMode::*;
        match self {
            Project2d | BumpEnvMap | BumpEnvMapLum | DotSt | DependentAr | DependentGb => {
                Some(TextureDim::D2)
            }
            Project3d | DotStr3d => Some(TextureDim::D3),
            Cubemap | DotRflctDiff | DotRflctSpec | DotStrCube | DotRflctSpecConst => {
                Some(TextureDim::Cube)
            }
            None | Passthru | ClipPlane | Brdf | DotZw | DotProduct | Reserved(_) => Option::None,
        }
    }
    /// Assembler-ish name.
    pub fn name(self) -> String {
        use TexMode::*;
        match self {
            None => "none".into(),
            Project2d => "project2d".into(),
            Project3d => "project3d".into(),
            Cubemap => "cubemap".into(),
            Passthru => "passthru".into(),
            ClipPlane => "clipplane".into(),
            BumpEnvMap => "bumpenvmap".into(),
            BumpEnvMapLum => "bumpenvmap_lum".into(),
            Brdf => "brdf".into(),
            DotSt => "dot_st".into(),
            DotZw => "dot_zw".into(),
            DotRflctDiff => "dot_rflct_diff".into(),
            DotRflctSpec => "dot_rflct_spec".into(),
            DotStr3d => "dot_str_3d".into(),
            DotStrCube => "dot_str_cube".into(),
            DependentAr => "dpndnt_ar".into(),
            DependentGb => "dpndnt_gb".into(),
            DotProduct => "dotproduct".into(),
            DotRflctSpecConst => "dot_rflct_spec_const".into(),
            Reserved(n) => format!("reserved{n:#x}"),
        }
    }
}

/// Texture dimensionality a stage samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum TextureDim {
    /// `texture_2d<f32>`.
    D2,
    /// `texture_3d<f32>`.
    D3,
    /// `texture_cube<f32>`.
    Cube,
}

/// `PS_DOTMAPPING_*`: how a dot-product stage reads its input texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum DotMap {
    /// Stage 0 has no mapping.
    Unused,
    ZeroToOne,
    Minus1To1D3d,
    Minus1To1Gl,
    Minus1To1,
    Hilo1,
    HiloHemisphereD3d,
    HiloHemisphereGl,
    HiloHemisphere,
    Reserved(u8),
}

impl DotMap {
    fn from_bits(b: u32) -> Self {
        match b {
            0 => Self::ZeroToOne,
            1 => Self::Minus1To1D3d,
            2 => Self::Minus1To1Gl,
            3 => Self::Minus1To1,
            4 => Self::Hilo1,
            5 => Self::HiloHemisphereD3d,
            6 => Self::HiloHemisphereGl,
            7 => Self::HiloHemisphere,
            n => Self::Reserved(n as u8),
        }
    }
}

/// Decoded pixel pipeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Combiners {
    /// Active general combiner stages (`PSCombinerCount & 0xFF`, at most 8).
    pub stages: Vec<Stage>,
    /// Mux on `r0.a >= 0.5` (MSB); otherwise on the LSB of `r0.a * 255`.
    pub mux_msb: bool,
    /// Each stage has its own `c0`; otherwise stage 0's is used throughout.
    pub unique_c0: bool,
    /// Each stage has its own `c1`.
    pub unique_c1: bool,
    /// The final combiner, if programmed (either inputs dword non-zero).
    /// Without it the fragment colour is `r0`.
    pub final_combiner: Option<FinalCombiner>,
    /// Per texture stage.
    pub texture_modes: [TexMode; 4],
    /// Per texture stage (stage 0 is [`DotMap::Unused`]).
    pub dot_mapping: [DotMap; 4],
    /// Source texture stage for dependent and dot modes, as stored (stage 1
    /// always reads 0). Use [`Combiners::input_stage`], which rejects
    /// selectors that do not name an earlier stage.
    pub input_texture: [u8; 4],
    /// `PSCompareMode` nibble per stage: bit j set means CLIPPLANE kills when
    /// coordinate j (s, t, r, q) is `>= 0`, clear means when it is `< 0`.
    pub compare_mode: [u8; 4],
}
