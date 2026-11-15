//! [`ShaderProvider`] over `crates/shaders`: every draw gfx's built-in
//! shader cannot reproduce exactly (see [`builtin_equivalent`]) renders
//! through a translated NV2A vertex program and register-combiner pixel
//! program.
//!
//! **Vertex side.**
//! * A vertex program (`SelectVertexShaderDirect`/`LoadVertexShaderProgram`)
//!   is decoded from the microcode the device captured and translated by
//!   `shaders::vsh`. Its constants are the device's 192 registers; vertex
//!   registers the layout does not supply read the `SetVertexData*` values.
//! * Fixed-function pre-transformed vertices (`XYZRHW` FVFs) run a small
//!   synthesized program (see [`pretransformed_program`]): `oPos` is the
//!   window position plus half a pixel with `w = 1/rhw` (`rhw = 0` counts as
//!   1), exactly what gfx's built-in shader does; `oD0`/`oD1` are diffuse
//!   and specular; `oTn` is the texture coordinate set
//!   `D3DTSS_TEXCOORDINDEX` names; `oFog` is the specular alpha.
//! * Untransformed fixed-function vertices (`XYZ` FVFs) need the transform
//!   and lighting pipeline. Halo never draws them; they are skipped.
//!
//! **Pixel side.**
//! * With a pixel shader, the combiner program is the `D3DPIXELSHADERDEF`
//!   as the guest's render state holds it: slots 0..57 (`SetPixelShader`
//!   copies the definition there and later constant or combiner writes land
//!   there too), `PSTEXTUREMODES` from slot 136, and the three trailing
//!   dwords from the definition.
//!   A definition without a final combiner gets the one D3D programs from
//!   `FOGENABLE` and `SPECULARENABLE` (`psh::d3d_default_final_combiner`).
//! * Without one, the texture-stage states are turned into a combiner
//!   program the way D3D's fixed-function path does it on the NV2A (see
//!   [`fixed_function_def`]): one general combiner stage per enabled texture
//!   stage, plus the same final combiner for specular add and fog.
//!
//! Per-pipeline state that changes the generated code (texture dimensions,
//! alpha kill, `COLORSIGN`, alpha test function, two-sided lighting, flat
//! shading) is
//! part of the cache key, as is everything the synthesized programs are
//! built from (texture-stage ops and arguments, `SPECULARENABLE`,
//! `FOGENABLE`, texture-coordinate indices); the alpha reference, combiner
//! constants (`TFACTOR` included), fog mode and range (factor 1 with
//! `FOGENABLE` off), bump-environment matrices and texture scale are
//! uniforms.
//!
//! Translations are cached by (vertex program, combiner program with its
//! constants cleared, options) and registered with the renderer once; the
//! renderer caches pipelines per program and translated state. Failures are
//! cached too, so a draw that cannot be translated costs a lookup the next
//! time.

use std::collections::HashMap;

use gfx::pipeline::ShaderId;
use gfx::program::{ProgramBindings, StageDims};
use gfx::state::{
    CompareFunc, ShadeMode, TextureAddress, TextureArg, TextureArgSource, TextureOp,
    TextureStageState,
};
use gfx::texture::TextureKind;
use gfx::translate::SamplerKey;
use gfx::xdk::RenderStateTable;
use shaders::psh::bits::*;
use shaders::psh::{
    self, AlphaFunc, PixelOptions, PixelShaderDef, PixelUniforms, TextureDim, argb,
};
use shaders::vsh::encode::{Inst, Operand, program};
use shaders::vsh::{self, IluOp, MacOp, OutReg, ProgramKind, VertexOptions, VertexUniforms};

use crate::backend::ResolvedDraw;
use crate::shader::{PreparedShader, ShaderProvider, VertexProcessing};
use crate::state::slot;

/// Shader ids this provider registers start here (the built-in is 0).
pub const SHADER_ID_BASE: u32 = 0x4E56_0000;

/// Canonical render-state indices (identical to Halo 2's guest slots below
/// 154) the translation reads from the raw words.
mod rs {
    pub const STIPPLEENABLE: u32 = 83;
    pub const FOGTABLEMODE: u32 = 93;
    pub const FOGSTART: u32 = 94;
    pub const FOGEND: u32 = 95;
    pub const FOGDENSITY: u32 = 96;
    pub const SPECULARENABLE: u32 = 103;
    pub const TWOSIDEDLIGHTING: u32 = 141;
}

/// `D3DTSS_ALPHAKILL` value that enables the kill.
const ALPHAKILL_ENABLE: u32 = 4;

/// Hardware constant registers the pre-transformed program reads.
const FF_HALF_PIXEL: usize = 0;
const FF_ZERO: usize = 1;

/// Counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Nv2aStats {
    /// Programs translated and registered.
    pub translated: u64,
    /// Programs that failed to translate (each counted once).
    pub failed: u64,
    /// Draws prepared.
    pub draws: u64,
    /// Draws whose bound definition has a final combiner but whose pushed
    /// final-combiner inputs are both zero, so it runs as an all-zero
    /// final combiner (black, alpha 0; [`PixelShaderDef::explicit_final`]).
    pub zero_final_draws: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct PretransformedKey {
    /// Vertex register read for `oT0..oT3` (`9 + D3DTSS_TEXCOORDINDEX`).
    texcoords: [u8; 4],
    /// Write `oFog.x` from the specular alpha.
    fog: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum VertexSource {
    Microcode(Vec<u32>),
    Pretransformed(PretransformedKey),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ProgramKey {
    vertex: VertexSource,
    vertex_options: VertexOptions,
    /// The combiner program with its constants cleared (they are uniforms).
    pixel: PixelShaderDef,
    pixel_options: PixelOptions,
}

#[derive(Clone, Debug)]
struct Compiled {
    id: ShaderId,
    dims: StageDims,
    warnings: Vec<String>,
}

/// Translates draws with `crates/shaders`; install it with
/// [`GfxBackend::with_shaders`](crate::GfxBackend::with_shaders) (the
/// default for [`GfxBackend::new`](crate::GfxBackend::new)).
pub struct Nv2aShaders {
    bindings: Option<ProgramBindings>,
    programs: HashMap<ProgramKey, Result<Compiled, String>>,
    table: RenderStateTable,
    next: u32,
    stats: Nv2aStats,
}

impl Default for Nv2aShaders {
    fn default() -> Self {
        Self::new()
    }
}

impl Nv2aShaders {
    /// An empty provider.
    pub fn new() -> Self {
        Self {
            bindings: None,
            programs: HashMap::new(),
            table: RenderStateTable::default(),
            next: 0,
            stats: Nv2aStats::default(),
        }
    }

    /// Counters so far.
    pub fn stats(&self) -> Nv2aStats {
        self.stats
    }

    /// Approximations the translator reported, per translated program.
    pub fn warnings(&self) -> Vec<String> {
        self.programs
            .values()
            .filter_map(|p| p.as_ref().ok())
            .flat_map(|c| c.warnings.iter().cloned())
            .collect()
    }
}

/// A raw render-state word by canonical index.
fn raw(table: &RenderStateTable, draw: &ResolvedDraw, canonical: u32) -> u32 {
    table
        .canonical_to_guest(canonical)
        .and_then(|g| draw.raw.render.get(g as usize).copied())
        .unwrap_or(0)
}

fn texture_dim(kind: TextureKind) -> TextureDim {
    match kind {
        TextureKind::D2 => TextureDim::D2,
        TextureKind::Cube => TextureDim::Cube,
        TextureKind::Volume => TextureDim::D3,
    }
}

fn view_dim(d: TextureDim) -> wgpu::TextureViewDimension {
    match d {
        TextureDim::D2 => wgpu::TextureViewDimension::D2,
        TextureDim::D3 => wgpu::TextureViewDimension::D3,
        TextureDim::Cube => wgpu::TextureViewDimension::Cube,
    }
}

fn alpha_func(f: CompareFunc) -> AlphaFunc {
    match f {
        CompareFunc::Never => AlphaFunc::Never,
        CompareFunc::Less => AlphaFunc::Less,
        CompareFunc::Equal => AlphaFunc::Equal,
        CompareFunc::LessEqual => AlphaFunc::LessEqual,
        CompareFunc::Greater => AlphaFunc::Greater,
        CompareFunc::NotEqual => AlphaFunc::NotEqual,
        CompareFunc::GreaterEqual => AlphaFunc::GreaterEqual,
        CompareFunc::Always => AlphaFunc::Always,
    }
}

/// The guest's combiner program: render-state slots 0..57, texture modes
/// from slot 136, the last three dwords from the definition.
///
/// Whether there is a final combiner comes from the definition itself, as
/// D3D latches it at `SetPixelShader` (`0x3fa452`, device `+0x788`) and the
/// lazy commit reads the latch, not the definition: `final_latch` is that
/// value ([`ShaderInputs::final_combiner_latch`]). A definition bound with
/// a final combiner keeps it even when zero is later pushed to both
/// final-combiner methods ([`PixelShaderDef::explicit_final`]), and editing
/// the definition's buffer without rebinding changes neither way.
///
/// [`ShaderInputs::final_combiner_latch`]: crate::shader::ShaderInputs::final_combiner_latch
pub fn guest_def(
    psdef: &[u32; 60],
    combiner_state: &[u32; 57],
    texture_modes: u32,
    final_latch: bool,
) -> PixelShaderDef {
    let mut words = *psdef;
    words[..57].copy_from_slice(combiner_state);
    words[54] = texture_modes;
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let mut def = PixelShaderDef::from_bytes(&bytes).expect("60 dwords");
    def.explicit_final = final_latch;
    def
}

/// The vertex program for pre-transformed (`XYZRHW`) fixed-function
/// vertices. Reads `c[0]` = (0.5, 0.5, 0, 0) and `c[1]` = 0.
pub fn pretransformed_program(texcoords: [u8; 4], fog: bool) -> vsh::Program {
    let (x, y, z, w) = (8u8, 4u8, 2u8, 1u8);
    let half = FF_HALF_PIXEL as u8;
    let zero = FF_ZERO as u8;
    let mut insts = vec![
        // r2.x = rhw >= 0, r2.y = -rhw >= 0; their product is rhw == 0.
        Inst::mac(
            MacOp::Sge,
            Some(Operand::v(0).swz("w")),
            Some(Operand::c(zero).swz("x")),
            None,
        )
        .to_temp(2, x),
        Inst::mac(
            MacOp::Sge,
            Some(Operand::v(0).swz("w").negated()),
            Some(Operand::c(zero).swz("x")),
            None,
        )
        .to_temp(2, y),
        Inst::mac(
            MacOp::Mul,
            Some(Operand::r(2).swz("x")),
            Some(Operand::r(2).swz("y")),
            None,
        )
        .to_temp(2, x),
        // rhw + (rhw == 0): a zero rhw behaves as 1.
        Inst::mac(
            MacOp::Add,
            Some(Operand::r(2).swz("x")),
            None,
            Some(Operand::v(0).swz("w")),
        )
        .to_temp(2, x),
        Inst::ilu(IluOp::Rcp, Operand::r(2).swz("x")).ilu_to_temp(1, x),
        // Window position plus half a pixel (as gfx's built-in shader).
        Inst::mac(
            MacOp::Add,
            Some(Operand::v(0)),
            None,
            Some(Operand::c(half)),
        )
        .to_out(false, OutReg::Pos, x | y | z),
        Inst::mac(MacOp::Mov, Some(Operand::r(1).swz("x")), None, None).to_out(
            false,
            OutReg::Pos,
            w,
        ),
        Inst::mac(MacOp::Mov, Some(Operand::v(3)), None, None).to_out(false, OutReg::D0, 0xF),
        Inst::mac(MacOp::Mov, Some(Operand::v(4)), None, None).to_out(false, OutReg::D1, 0xF),
    ];
    for (n, &reg) in texcoords.iter().enumerate() {
        insts.push(
            Inst::mac(MacOp::Mov, Some(Operand::v(reg)), None, None).to_out(
                false,
                OutReg::T(n as u8),
                0xF,
            ),
        );
    }
    if fog {
        insts.push(
            Inst::mac(MacOp::Mov, Some(Operand::v(4).swz("w")), None, None).to_out(
                false,
                OutReg::Fog,
                x,
            ),
        );
    }
    let last = insts.pop().expect("non-empty").last();
    insts.push(last);
    vsh::Program::parse(&program(&insts)).expect("well-formed")
}

/// A combiner input under construction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct In {
    reg: u8,
    alpha: bool,
    map: u8,
}

const ONE: In = In {
    reg: REG_ZERO,
    alpha: false,
    map: MAP_UNSIGNED_INVERT,
};
const ZERO: In = In {
    reg: REG_ZERO,
    alpha: false,
    map: MAP_UNSIGNED_IDENTITY,
};
/// `2 * 0 - 1`.
const MINUS_ONE: In = In {
    reg: REG_ZERO,
    alpha: false,
    map: MAP_EXPAND_NORMAL,
};

impl In {
    fn reg(reg: u8) -> Self {
        Self {
            reg,
            alpha: false,
            map: MAP_UNSIGNED_IDENTITY,
        }
    }
    fn a(self) -> Self {
        Self {
            alpha: true,
            ..self
        }
    }
    fn inv(self) -> Result<Self, String> {
        let map = match self.map {
            MAP_UNSIGNED_IDENTITY => MAP_UNSIGNED_INVERT,
            MAP_UNSIGNED_INVERT => MAP_UNSIGNED_IDENTITY,
            _ => return Err("complement of a signed combiner input".into()),
        };
        Ok(Self { map, ..self })
    }
    fn with_map(self, map: u8) -> Result<Self, String> {
        if self.map != MAP_UNSIGNED_IDENTITY {
            return Err("signed mapping of a complemented argument".into());
        }
        Ok(Self { map, ..self })
    }
    fn encode(self, alpha_portion: bool) -> u8 {
        let chan = if alpha_portion || self.alpha {
            CHAN_ALPHA
        } else {
            0
        };
        self.reg | chan | self.map
    }
}

/// Inputs and output of one combiner portion.
struct Portion {
    abcd: [In; 4],
    flags: u32,
    /// Result from the sum (else from AB).
    sum: bool,
}

/// What a texture-stage argument reads at stage `n`; `current` is where
/// `D3DTA_CURRENT` lives (diffuse until a stage has written `r0`).
fn stage_arg(a: TextureArg, n: usize, textured: bool, current: In) -> Result<In, String> {
    let base = match a.source {
        TextureArgSource::Diffuse => In::reg(REG_V0),
        TextureArgSource::Current => current,
        // An unbound texture reads as one, so MODULATE with no texture
        // leaves diffuse (as gfx's built-in shader).
        TextureArgSource::Texture if !textured => ONE,
        TextureArgSource::Texture => In::reg(REG_T0 + n as u8),
        TextureArgSource::TFactor => In::reg(REG_C0),
        TextureArgSource::Specular => In::reg(REG_V1),
        TextureArgSource::Temp => In::reg(REG_R1),
    };
    let mut i = if a.complement { base.inv()? } else { base };
    if a.alpha_replicate {
        i = i.a();
    }
    Ok(i)
}

/// A stage portion as it runs: with no texture bound, an operation that
/// reads `D3DTA_TEXTURE` as one of its operands passes `CURRENT` through
/// instead (D3D's behaviour, as Wine's fixed-function path also documents),
/// so an untextured default stage 0 outputs diffuse colour and alpha.
fn effective(op: TextureOp, args: [TextureArg; 3], textured: bool) -> (TextureOp, [TextureArg; 3]) {
    use TextureOp as T;
    let tex = |a: TextureArg| a.source == TextureArgSource::Texture;
    let reads_texture = (tex(args[1]) && op != T::SelectArg2)
        || (tex(args[2]) && op != T::SelectArg1)
        || (tex(args[0]) && matches!(op, T::MultiplyAdd | T::Lerp));
    if textured || op == T::Disable || !reads_texture {
        return (op, args);
    }
    let current = TextureArg::plain(TextureArgSource::Current);
    (T::SelectArg1, [current, current, current])
}

/// One `D3DTEXTUREOP` as a combiner portion.
///
/// D3D saturates every stage result to [0, 1]. The combiner registers
/// clamp to [-1, 1], and every unsigned input mapping clamps a negative
/// value to 0, so a negative intermediate is only visible through a signed
/// mapping: no operand here reads a register through one (SUBTRACT negates
/// with a `-1` operand instead of `SIGNED_NEGATE`; DOTPRODUCT3's
/// `EXPAND_NORMAL` clamps first), and the final output is clamped.
fn stage_op(
    op: TextureOp,
    args: [In; 3],
    n: usize,
    textured: bool,
    alpha_portion: bool,
    current: In,
) -> Result<Portion, String> {
    use TextureOp as T;
    let [a0, a1, a2] = args;
    let texture = if textured {
        In::reg(REG_T0 + n as u8)
    } else {
        ONE
    };
    let ab = |a: In, b: In, flags: u32| Portion {
        abcd: [a, b, ZERO, ZERO],
        flags,
        sum: false,
    };
    let sum = |abcd: [In; 4], flags: u32| Portion {
        abcd,
        flags,
        sum: true,
    };
    let blend = |alpha: In| -> Result<Portion, String> {
        Ok(sum([a1, alpha.a(), a2, alpha.a().inv()?], OUT_IDENTITY))
    };
    let rgb_only = |what: &str| -> Result<(), String> {
        if alpha_portion {
            Err(format!("{what} as an alpha operation"))
        } else {
            Ok(())
        }
    };
    Ok(match op {
        T::SelectArg1 => ab(a1, ONE, OUT_IDENTITY),
        T::SelectArg2 => ab(a2, ONE, OUT_IDENTITY),
        T::Modulate => ab(a1, a2, OUT_IDENTITY),
        T::Modulate2x => ab(a1, a2, OUT_SHIFTLEFT_1),
        T::Modulate4x => ab(a1, a2, OUT_SHIFTLEFT_2),
        T::Add => sum([a1, ONE, a2, ONE], OUT_IDENTITY),
        T::AddSigned => sum([a1, ONE, a2, ONE], OUT_BIAS),
        T::AddSigned2x => sum([a1, ONE, a2, ONE], OUT_SHIFTLEFT_1_BIAS),
        T::Subtract => sum([a1, ONE, a2, MINUS_ONE], OUT_IDENTITY),
        T::AddSmooth => sum([a2, a1.inv()?, a1, ONE], OUT_IDENTITY),
        T::BlendDiffuseAlpha => blend(In::reg(REG_V0))?,
        T::BlendTextureAlpha => blend(texture)?,
        T::BlendCurrentAlpha => blend(current)?,
        T::BlendFactorAlpha => blend(In::reg(REG_C0))?,
        T::BlendTextureAlphaPm => sum([a1, ONE, a2, texture.a().inv()?], OUT_IDENTITY),
        T::ModulateAlphaAddColor => {
            rgb_only("MODULATEALPHA_ADDCOLOR")?;
            sum([a1, ONE, a1.a(), a2], OUT_IDENTITY)
        }
        T::ModulateColorAddAlpha => {
            rgb_only("MODULATECOLOR_ADDALPHA")?;
            sum([a1, a2, a1.a(), ONE], OUT_IDENTITY)
        }
        T::ModulateInvAlphaAddColor => {
            rgb_only("MODULATEINVALPHA_ADDCOLOR")?;
            sum([a1.a().inv()?, a2, a1, ONE], OUT_IDENTITY)
        }
        T::ModulateInvColorAddAlpha => {
            rgb_only("MODULATEINVCOLOR_ADDALPHA")?;
            sum([a1.inv()?, a2, a1.a(), ONE], OUT_IDENTITY)
        }
        T::DotProduct3 => {
            rgb_only("DOTPRODUCT3")?;
            ab(
                a1.with_map(MAP_EXPAND_NORMAL)?,
                a2.with_map(MAP_EXPAND_NORMAL)?,
                OUT_AB_DOT_PRODUCT,
            )
        }
        // arg0 + arg1 * arg2.
        T::MultiplyAdd => sum([a1, a2, a0, ONE], OUT_IDENTITY),
        T::Lerp => sum([a0, a1, a0.inv()?, a2], OUT_IDENTITY),
        T::Disable => ab(current, ONE, OUT_IDENTITY),
        T::BumpEnvMap | T::BumpEnvMapLuminance | T::PreModulate => {
            return Err(format!(
                "fixed-function texture op {op:?} (needs a texture-mode change)"
            ));
        }
    })
}

/// Fixed-function inputs to [`fixed_function_def`].
#[derive(Clone, Copy, Debug)]
pub struct FixedFunction<'a> {
    /// Texture-stage states.
    pub stages: &'a [TextureStageState; 4],
    /// Kind of the texture bound to each stage.
    pub bound: [Option<TextureKind>; 4],
    /// `D3DRS_TEXTUREFACTOR`.
    pub texture_factor: u32,
    /// `D3DRS_SPECULARENABLE`.
    pub specular: bool,
    /// `D3DRS_FOGENABLE`.
    pub fog: bool,
}

/// The register-combiner program D3D's fixed-function texture stages
/// describe: stage `n` (until the first `COLOROP = DISABLE`) is general
/// combiner `n` writing `r0` (or `r1` for `D3DTA_TEMP` results), with
/// `TFACTOR` in `c0`; `CURRENT` is diffuse until a stage writes it; an
/// operation reading an unbound texture passes `CURRENT` through (see
/// `effective`). Stage results saturate to [0, 1] (see
/// `stage_op`). Specular is added and fog applied in the final combiner.
pub fn fixed_function_def(ff: &FixedFunction<'_>) -> Result<PixelShaderDef, String> {
    let mut def = PixelShaderDef::default();
    let mut count = 0usize;
    let mut modes = [TEX_NONE; 4];
    // D3DTA_CURRENT: diffuse until a stage writes r0.
    let mut current = In::reg(REG_V0);
    for (n, s) in ff.stages.iter().enumerate() {
        if s.color_op == TextureOp::Disable {
            break;
        }
        let textured = ff.bound[n].is_some();
        if let Some(kind) = ff.bound[n] {
            modes[n] = match kind {
                TextureKind::D2 => TEX_PROJECT2D,
                TextureKind::Cube => TEX_CUBEMAP,
                TextureKind::Volume => TEX_PROJECT3D,
            };
        }
        let dst = if s.result_arg.source == TextureArgSource::Temp {
            REG_R1
        } else {
            REG_R0
        };
        for alpha in [false, true] {
            let (op, args) = if alpha {
                (s.alpha_op, [s.alpha_arg0, s.alpha_arg1, s.alpha_arg2])
            } else {
                (s.color_op, [s.color_arg0, s.color_arg1, s.color_arg2])
            };
            let (op, args) = effective(op, args, textured);
            let args = [
                stage_arg(args[0], n, textured, current)?,
                stage_arg(args[1], n, textured, current)?,
                stage_arg(args[2], n, textured, current)?,
            ];
            let p = stage_op(op, args, n, textured, alpha, current)?;
            let [a, b, c, d] = p.abcd.map(|i| i.encode(alpha));
            let out = if p.sum {
                outputs(0, 0, dst, p.flags)
            } else {
                outputs(dst, 0, 0, p.flags)
            };
            if alpha {
                def.alpha_inputs[n] = inputs(a, b, c, d);
                def.alpha_outputs[n] = out;
            } else {
                def.rgb_inputs[n] = inputs(a, b, c, d);
                def.rgb_outputs[n] = out;
            }
        }
        def.constant0[n] = ff.texture_factor;
        if dst == REG_R0 {
            current = In::reg(REG_R0);
        }
        count += 1;
    }
    if current.reg != REG_R0 {
        // No stage wrote CURRENT (stage 0 disabled, or every result went to
        // TEMP): the output is diffuse. At most four stages ran, so there
        // is room for one more.
        def.rgb_inputs[count] = inputs(REG_V0, ONE.encode(false), 0, 0);
        def.rgb_outputs[count] = outputs(REG_R0, 0, 0, 0);
        def.alpha_inputs[count] = inputs(REG_V0 | CHAN_ALPHA, ONE.encode(true), 0, 0);
        def.alpha_outputs[count] = outputs(REG_R0, 0, 0, 0);
        count += 1;
    }
    def.combiner_count = count as u32 | COUNT_MUX_MSB | COUNT_UNIQUE_C0;
    def.texture_modes = texture_modes(modes);
    def.final_constant0 = ff.texture_factor;
    // The final combiner D3D programs for specular add and fog.
    (def.final_inputs_abcd, def.final_inputs_efg) =
        psh::d3d_default_final_combiner(ff.fog, ff.specular);
    Ok(def)
}

impl ShaderProvider for Nv2aShaders {
    fn prepare(
        &mut self,
        ctx: &gfx::GfxContext,
        renderer: &mut gfx::Renderer,
        draw: &ResolvedDraw,
        textures: [Option<&wgpu::TextureView>; 4],
    ) -> Result<PreparedShader, String> {
        let render = &draw.state.render;
        let flat = render.shade_mode == ShadeMode::Flat;
        let bound: [Option<TextureKind>; 4] =
            std::array::from_fn(|n| textures[n].and(draw.texture_info[n]).map(|i| i.kind));
        let fog_enable = render.fog_enable;

        // Vertex side.
        let vertex = match &draw.shader.vertex {
            VertexProcessing::Program { microcode, .. } => {
                VertexSource::Microcode(microcode.clone())
            }
            VertexProcessing::FixedFunction { fvf } => {
                if !draw.layout.pretransformed {
                    return Err(format!(
                        "untransformed fixed-function vertices (FVF {fvf:#x}) need the transform and lighting pipeline"
                    ));
                }
                let mut texcoords = [9u8; 4];
                for (n, s) in draw.state.stages.iter().enumerate() {
                    if s.tex_coord_index >= 4 {
                        return Err(format!(
                            "stage {n}: D3DTSS_TEXCOORDINDEX {:#x} (texture coordinate generation)",
                            s.tex_coord_index
                        ));
                    }
                    texcoords[n] = 9 + s.tex_coord_index as u8;
                }
                VertexSource::Pretransformed(PretransformedKey {
                    texcoords,
                    fog: fog_enable,
                })
            }
        };

        // Pixel side.
        let specular = raw(&self.table, draw, rs::SPECULARENABLE) != 0;
        let def = match &draw.shader.pixel_shader {
            // A definition without a final combiner gets the one D3D's
            // lazy commit programs from FOGENABLE and SPECULARENABLE.
            Some(ps) => guest_def(
                ps,
                &draw.shader.combiner_state,
                draw.raw.render[slot::PS_TEXTURE_MODES as usize],
                draw.shader.final_combiner_latch,
            )
            .with_default_final_combiner(fog_enable, specular),
            None => fixed_function_def(&FixedFunction {
                stages: &draw.state.stages,
                bound,
                texture_factor: render.texture_factor,
                specular,
                fog: fog_enable,
            })?,
        };
        if def.explicit_final && def.final_inputs_abcd == 0 && def.final_inputs_efg == 0 {
            self.stats.zero_final_draws += 1;
        }
        let (c0, c1) = def.embedded_constants();
        let mut code = def.clone();
        code.constant0 = [0; 8];
        code.constant1 = [0; 8];
        code.final_constant0 = 0;
        code.final_constant1 = 0;

        let vertex_options = VertexOptions {
            provided_inputs: draw.shader.provided_inputs,
            flat_shading: flat,
        };
        let signs: [u8; 4] = std::array::from_fn(|n| color_sign(draw.extras[n].color_sign));
        let border: [u8; 4] = std::array::from_fn(|n| {
            shader_border(&draw.state.stages[n], signs[n], ctx.caps.clamp_to_border)
        });
        let pixel_options = PixelOptions {
            texture_dims: bound.map(|k| k.map(texture_dim)),
            alpha_kill: std::array::from_fn(|n| draw.extras[n].alpha_kill & ALPHAKILL_ENABLE != 0),
            color_sign: signs,
            border,
            alpha_test: render
                .alpha_test_enable
                .then(|| alpha_func(render.alpha_func)),
            two_sided_lighting: raw(&self.table, draw, rs::TWOSIDEDLIGHTING) != 0,
            flat_shading: flat,
            explicit_lod: false,
            stipple: polygon_stipple(&self.table, draw),
        };
        let key = ProgramKey {
            vertex,
            vertex_options,
            pixel: code,
            pixel_options,
        };
        let bindings = self
            .bindings
            .get_or_insert_with(|| ProgramBindings::new(&ctx.device));
        if !self.programs.contains_key(&key) {
            let r = compile(ctx, renderer, bindings, &key, SHADER_ID_BASE + self.next);
            match &r {
                Ok(_) => {
                    self.next += 1;
                    self.stats.translated += 1;
                }
                Err(_) => self.stats.failed += 1,
            }
            self.programs.insert(key.clone(), r);
        }
        let compiled = self.programs[&key].clone()?;
        self.stats.draws += 1;

        // Uniforms.
        let mut vu = VertexUniforms {
            surface: [draw.target.width as f32, draw.target.height as f32],
            // Same full scale as gfx's built-in shader.
            z_max: gfx::renderer::d3dz_max(draw.target.depth_format),
            ..VertexUniforms::default()
        };
        match key.vertex {
            VertexSource::Microcode(_) => {
                vu.constants = *draw.shader.constants;
                vu.defaults = draw.shader.vertex_defaults;
            }
            VertexSource::Pretransformed(_) => {
                vu.constants[FF_HALF_PIXEL] = [0.5, 0.5, 0.0, 0.0];
                vu.constants[FF_ZERO] = [0.0; 4];
                // Missing diffuse is white; missing specular black with no fog.
                vu.defaults[3] = [1.0; 4];
                vu.defaults[4] = [0.0, 0.0, 0.0, 1.0];
            }
        }
        let mut pu = PixelUniforms {
            c0,
            c1,
            fog_color: argb(render.fog_color),
            fog: fog_uniform(&self.table, draw),
            alpha_ref: [render.alpha_ref as f32, 0.0, 0.0, 0.0],
            eye: eye_uniform(&draw.hw),
            ..PixelUniforms::default()
        };
        pu.stipple = draw.hw.stipple_pattern;
        pu.lod_bias = std::array::from_fn(|n| {
            let bias = draw.state.stages[n].mip_lod_bias;
            if bias.is_finite() { bias } else { 0.0 }
        });
        pu.border =
            std::array::from_fn(|n| border_value(draw.state.stages[n].border_color, signs[n]));
        for n in 0..4 {
            let e = &draw.extras[n];
            pu.bump_mat[n] = e.bump_env_mat;
            pu.bump_lum[n] = [e.bump_env_lscale, e.bump_env_loffset, 0.0, 0.0];
            if let Some(info) = draw.texture_info[n]
                && info.format.layout() == gfx::format::Layout::Linear
            {
                // Linear textures are addressed in texels.
                let (w, h) = (info.width.max(1) as f32, info.height.max(1) as f32);
                pu.tex_scale[n] = [1.0 / w, 1.0 / h, 1.0, 1.0];
            }
        }
        let group0 = bindings.uniforms(&ctx.device, &ctx.queue, &vu.to_bytes(), &pu.to_bytes());

        // Textures: a stand-in where the bound view has another dimension.
        let views: [Option<&wgpu::TextureView>; 4] = std::array::from_fn(|n| {
            let want = compiled.dims[n]?;
            let have = bound[n].map(|k| view_dim(texture_dim(k)))?;
            (want == have).then_some(textures[n]).flatten()
        });
        let samplers: [SamplerKey; 4] = std::array::from_fn(|n| {
            let mut k = SamplerKey::from_stage(&draw.state.stages[n], ctx.caps);
            if border[n] != 0 {
                // The shader supplies the border; the sampler clamps.
                for (axis, mode) in [&mut k.address_u, &mut k.address_v, &mut k.address_w]
                    .into_iter()
                    .enumerate()
                {
                    if border[n] >> axis & 1 != 0 {
                        *mode = wgpu::AddressMode::ClampToEdge;
                    }
                }
                k.border = None;
            }
            k
        });
        // Stand-ins carry the stage's signed channels too.
        let signed =
            signs.map(|m| [m & SIGN_R, m & SIGN_G, m & SIGN_B, m & SIGN_A].map(|b| b != 0));
        let mut bind_groups = vec![group0];
        if let Some(g) = bindings.textures(
            &ctx.device,
            &ctx.queue,
            compiled.dims,
            views,
            samplers,
            signed,
        ) {
            bind_groups.push(gfx::BoundGroup::new(1, g));
        }
        Ok(PreparedShader {
            id: compiled.id,
            bind_groups,
        })
    }

    fn flush_uploads(&mut self, ctx: &gfx::GfxContext) {
        if let Some(b) = &mut self.bindings {
            b.flush_uploads(&ctx.queue);
        }
    }

    fn submitted(&mut self) {
        if let Some(b) = &mut self.bindings {
            b.submitted();
        }
    }

    fn end_frame(&mut self) {
        if let Some(b) = &mut self.bindings {
            b.end_frame();
        }
    }
}

/// [`PixelOptions::border`] for a stage: the `BORDER` axes when the stage
/// reads signed channels, uses a non-preset colour, or the host cannot
/// clamp to border. The shader path approximates linear filtering at edges.
fn shader_border(s: &TextureStageState, sign: u8, clamp_to_border: bool) -> u8 {
    if sign == 0 && clamp_to_border && matches!(s.border_color, 0 | 0xff00_0000 | 0xffff_ffff) {
        return 0;
    }
    [s.address_u, s.address_v, s.address_w]
        .iter()
        .enumerate()
        .fold(0, |m, (axis, &a)| {
            m | u8::from(a == TextureAddress::Border) << axis
        })
}

/// The texture unit's value for a stage's `D3DTSS_BORDERCOLOR`
/// (`D3DCOLOR`, ARGB): signed channels read the border byte the way they
/// read texel bytes (`2s/255`, clamped at -1), the others `b/255`.
fn border_value(argb_color: u32, sign: u8) -> [f32; 4] {
    let mut v = argb(argb_color);
    for (c, bit) in [SIGN_R, SIGN_G, SIGN_B, SIGN_A].into_iter().enumerate() {
        if sign & bit != 0 {
            let s = ((v[c] * 255.0).round() as u8) as i8;
            v[c] = (2.0 * s as f32 / 255.0).max(-1.0);
        }
    }
    v
}

/// The constant eye vector of `DOT_RFLCT_SPEC_CONST` (`SET_EYE_VECTOR`).
fn eye_uniform(hw: &crate::hw::HwState) -> [f32; 4] {
    let [x, y, z] = hw.eye_vector;
    [x, y, z, 0.0]
}

/// The fog uniform: `[mode, start, end, density]`. With `D3DRS_FOGENABLE`
/// off the factor is 1 whatever the program reads (exp fog with density 0:
/// a combiner program that reads the fog register sees no fog).
fn fog_uniform(table: &RenderStateTable, draw: &ResolvedDraw) -> [f32; 4] {
    if !draw.state.render.fog_enable {
        return [2.0, 0.0, 0.0, 0.0];
    }
    let mode = match raw(table, draw, rs::FOGTABLEMODE) {
        1 => 2.0, // D3DFOG_EXP
        2 => 3.0, // D3DFOG_EXP2
        3 => 1.0, // D3DFOG_LINEAR
        _ => 0.0, // factor from the vertex
    };
    let f = |c: u32| f32::from_bits(raw(table, draw, c));
    [mode, f(rs::FOGSTART), f(rs::FOGEND), f(rs::FOGDENSITY)]
}

fn polygon_stipple(table: &RenderStateTable, draw: &ResolvedDraw) -> bool {
    use gfx::vertex::PrimitiveType;
    !matches!(
        draw.primitive,
        PrimitiveType::PointList
            | PrimitiveType::LineList
            | PrimitiveType::LineLoop
            | PrimitiveType::LineStrip
    ) && draw
        .hw
        .stipple_control
        .unwrap_or_else(|| raw(table, draw, rs::STIPPLEENABLE))
        != 0
}

/// Whether gfx's built-in `XYZRHW | DIFFUSE` shader draws `draw` exactly as
/// this provider would: no pixel shader, no texture, no fog, no specular,
/// no flat shading or two-sided lighting, and the texture stages output
/// diffuse colour and alpha unchanged (stage 0 disabled, or stage 0 alone
/// selecting the diffuse, an unbound texture passing it through). Anything
/// else needs the provider (TFACTOR arguments, complements, ...).
pub fn builtin_equivalent(draw: &ResolvedDraw, textured: bool) -> bool {
    static TABLE: std::sync::LazyLock<RenderStateTable> =
        std::sync::LazyLock::new(RenderStateTable::default);
    let render = &draw.state.render;
    if polygon_stipple(&TABLE, draw)
        || textured
        || draw.shader.pixel_shader.is_some()
        || render.fog_enable
        || render.shade_mode == ShadeMode::Flat
        || raw(&TABLE, draw, rs::SPECULARENABLE) != 0
        || raw(&TABLE, draw, rs::TWOSIDEDLIGHTING) != 0
    {
        return false;
    }
    let [s0, s1, ..] = &draw.state.stages;
    if s0.color_op == TextureOp::Disable {
        return true;
    }
    s1.color_op == TextureOp::Disable
        && s0.result_arg.source != TextureArgSource::Temp
        && passes_diffuse(
            s0.color_op,
            [s0.color_arg0, s0.color_arg1, s0.color_arg2],
            false,
        )
        && passes_diffuse(
            s0.alpha_op,
            [s0.alpha_arg0, s0.alpha_arg1, s0.alpha_arg2],
            true,
        )
}

/// Whether a stage-0 portion with no texture bound outputs diffuse.
fn passes_diffuse(op: TextureOp, args: [TextureArg; 3], alpha: bool) -> bool {
    let (op, [_, a1, a2]) = effective(op, args, false);
    let diffuse = |a: TextureArg| {
        matches!(
            a.source,
            TextureArgSource::Diffuse | TextureArgSource::Current
        ) && !a.complement
            && (alpha || !a.alpha_replicate)
    };
    match op {
        TextureOp::SelectArg1 => diffuse(a1),
        TextureOp::SelectArg2 => diffuse(a2),
        _ => false,
    }
}

/// Prints what the translator approximated, once per translated program
/// (`compile` runs once per cache key; failures are cached too).
fn report_warnings(id: u32, warnings: &[String]) {
    for w in warnings {
        eprintln!("nv2a: pixel shader {id:#x}: {w}");
    }
}

fn compile(
    ctx: &gfx::GfxContext,
    renderer: &mut gfx::Renderer,
    bindings: &mut ProgramBindings,
    key: &ProgramKey,
    id: u32,
) -> Result<Compiled, String> {
    let program = match &key.vertex {
        VertexSource::Microcode(words) => vsh::Program::from_words(ProgramKind::Normal, words)
            .map_err(|e| format!("vertex program: {e}"))?,
        VertexSource::Pretransformed(p) => pretransformed_program(p.texcoords, p.fog),
    };
    if program.writes_constants() {
        return Err(
            "vertex program writes constants (read/write programs are not translated)".into(),
        );
    }
    let vs = vsh::translate(&program, &key.vertex_options)
        .map_err(|e| format!("vertex program: {e}"))?;
    let ps = psh::translate(&key.pixel, &key.pixel_options);
    report_warnings(id, &ps.warnings);
    let source = shaders::link(&vs, &ps).map_err(|e| e.to_string())?;
    shaders::validate(&source).map_err(|e| format!("generated WGSL: {e}"))?;
    let module = ctx
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("nv2a program"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
    let dims: StageDims = ps.textures.map(|d| d.map(view_dim));
    let id = ShaderId(id);
    renderer.register_shader(id, bindings.program(&ctx.device, module, dims));
    Ok(Compiled {
        id,
        dims,
        warnings: ps.warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaders::psh::interp::{self, Inputs, NoTextures};

    fn stages() -> [TextureStageState; 4] {
        std::array::from_fn(|n| TextureStageState::default_for_stage(n as u32))
    }

    fn run(def: &PixelShaderDef, inputs: &Inputs) -> [f32; 4] {
        let (c0, c1) = def.embedded_constants();
        let u = PixelUniforms {
            c0,
            c1,
            ..PixelUniforms::default()
        };
        interp::run(def, &PixelOptions::default(), inputs, &u, &NoTextures).expect("not killed")
    }

    #[test]
    fn disabled_stage_zero_outputs_diffuse() {
        let mut s = stages();
        s[0].color_op = TextureOp::Disable;
        let def = fixed_function_def(&FixedFunction {
            stages: &s,
            bound: [None; 4],
            texture_factor: 0,
            specular: false,
            fog: false,
        })
        .unwrap();
        let i = Inputs {
            d0: [0.25, 0.5, 0.75, 0.5],
            ..Inputs::default()
        };
        assert_eq!(run(&def, &i), [0.25, 0.5, 0.75, 0.5]);
    }

    #[test]
    fn default_stage_without_texture_is_diffuse() {
        let def = fixed_function_def(&FixedFunction {
            stages: &stages(),
            bound: [None; 4],
            texture_factor: 0,
            specular: false,
            fog: false,
        })
        .unwrap();
        assert_eq!(def.texture_modes, 0);
        let i = Inputs {
            d0: [0.25, 0.5, 0.75, 0.5],
            ..Inputs::default()
        };
        // MODULATE(texture, diffuse) and SELECTARG1(texture) with no
        // texture both pass CURRENT (diffuse) through.
        assert_eq!(run(&def, &i), [0.25, 0.5, 0.75, 0.5]);
    }

    #[test]
    fn tfactor_blend_and_specular() {
        let mut s = stages();
        s[0].color_op = TextureOp::BlendFactorAlpha;
        s[0].color_arg1 = TextureArg::plain(TextureArgSource::TFactor);
        s[0].color_arg2 = TextureArg::plain(TextureArgSource::Diffuse);
        s[0].alpha_op = TextureOp::SelectArg1;
        s[0].alpha_arg1 = TextureArg::plain(TextureArgSource::Diffuse);
        let tfactor = 0x40FF_0000; // alpha 0x40, red
        let def = fixed_function_def(&FixedFunction {
            stages: &s,
            bound: [None; 4],
            texture_factor: tfactor,
            specular: true,
            fog: false,
        })
        .unwrap();
        let i = Inputs {
            d0: [0.0, 0.0, 1.0, 0.75],
            d1: [0.0, 0.5, 0.0, 0.0],
            ..Inputs::default()
        };
        let c = run(&def, &i);
        let a = 0x40 as f32 / 255.0;
        let want = [a, 0.5, 1.0 - a, 0.75];
        for (g, w) in c.iter().zip(want) {
            assert!((g - w).abs() < 1e-5, "{c:?} vs {want:?}");
        }
    }

    #[test]
    fn complement_and_subtract() {
        let mut s = stages();
        s[0].color_op = TextureOp::Subtract;
        s[0].color_arg1 = TextureArg::plain(TextureArgSource::Diffuse);
        s[0].color_arg2 = TextureArg::plain(TextureArgSource::Specular);
        s[0].alpha_op = TextureOp::SelectArg1;
        s[0].alpha_arg1 = TextureArg {
            source: TextureArgSource::Diffuse,
            complement: true,
            alpha_replicate: false,
        };
        let def = fixed_function_def(&FixedFunction {
            stages: &s,
            bound: [None; 4],
            texture_factor: 0,
            specular: false,
            fog: false,
        })
        .unwrap();
        let i = Inputs {
            d0: [0.5, 0.5, 0.25, 0.25],
            d1: [0.25, 0.75, 0.0, 0.0],
            ..Inputs::default()
        };
        assert_eq!(run(&def, &i), [0.25, 0.0, 0.25, 0.75]);
    }

    #[test]
    fn bump_ops_are_refused() {
        let mut s = stages();
        s[0].color_op = TextureOp::BumpEnvMap;
        assert!(
            fixed_function_def(&FixedFunction {
                stages: &s,
                bound: [Some(TextureKind::D2), None, None, None],
                texture_factor: 0,
                specular: false,
                fog: false,
            })
            .is_err()
        );
    }

    fn ff(s: &[TextureStageState; 4], texture_factor: u32) -> PixelShaderDef {
        fixed_function_def(&FixedFunction {
            stages: s,
            bound: [None; 4],
            texture_factor,
            specular: false,
            fog: false,
        })
        .unwrap()
    }

    fn near(got: [f32; 4], want: [f32; 4]) {
        for (g, w) in got.iter().zip(want) {
            assert!((g - w).abs() < 1e-5, "{got:?} vs {want:?}");
        }
    }

    fn plain(source: TextureArgSource) -> TextureArg {
        TextureArg::plain(source)
    }

    #[test]
    fn multiply_add_is_arg0_plus_arg1_times_arg2() {
        let mut s = stages();
        s[0].color_op = TextureOp::MultiplyAdd;
        s[0].color_arg0 = plain(TextureArgSource::Specular);
        s[0].color_arg1 = plain(TextureArgSource::Diffuse);
        s[0].color_arg2 = plain(TextureArgSource::TFactor);
        s[0].alpha_op = TextureOp::MultiplyAdd;
        s[0].alpha_arg0 = plain(TextureArgSource::Specular);
        s[0].alpha_arg1 = plain(TextureArgSource::Diffuse);
        s[0].alpha_arg2 = plain(TextureArgSource::TFactor);
        // TFACTOR 0.5 everywhere (0x80 / 255).
        let def = ff(&s, 0x8080_8080);
        let i = Inputs {
            d0: [0.5, 0.25, 1.0, 0.5],
            d1: [0.125, 0.25, 0.0, 0.25],
            ..Inputs::default()
        };
        let h = 128.0 / 255.0;
        near(
            run(&def, &i),
            [0.125 + 0.5 * h, 0.25 + 0.25 * h, h, 0.25 + 0.5 * h],
        );
    }

    #[test]
    fn stage_zero_writing_temp_keeps_current_diffuse() {
        let mut s = stages();
        // Stage 0: TEMP = TFACTOR. Stage 1: CURRENT * TEMP.
        s[0].color_op = TextureOp::SelectArg1;
        s[0].color_arg1 = plain(TextureArgSource::TFactor);
        s[0].alpha_op = TextureOp::SelectArg1;
        s[0].alpha_arg1 = plain(TextureArgSource::TFactor);
        s[0].result_arg = plain(TextureArgSource::Temp);
        s[1].color_op = TextureOp::Modulate;
        s[1].color_arg1 = plain(TextureArgSource::Current);
        s[1].color_arg2 = plain(TextureArgSource::Temp);
        s[1].alpha_op = TextureOp::SelectArg1;
        s[1].alpha_arg1 = plain(TextureArgSource::Current);
        let def = ff(&s, 0xFF80_8080);
        let i = Inputs {
            d0: [1.0, 0.5, 0.25, 0.75],
            ..Inputs::default()
        };
        let h = 128.0 / 255.0;
        near(run(&def, &i), [h, 0.5 * h, 0.25 * h, 0.75]);

        // Every stage writing TEMP: the output is diffuse.
        s[1].color_op = TextureOp::Disable;
        near(run(&ff(&s, 0xFF80_8080), &i), i.d0);
    }

    #[test]
    fn stage_results_saturate_between_stages() {
        let mut s = stages();
        // Stage 0: diffuse - specular < 0 saturates to 0; stage 1 subtracts
        // it from TFACTOR, leaving TFACTOR (not TFACTOR + |negative|).
        s[0].color_op = TextureOp::Subtract;
        s[0].color_arg1 = plain(TextureArgSource::Diffuse);
        s[0].color_arg2 = plain(TextureArgSource::Specular);
        s[0].alpha_op = TextureOp::Subtract;
        s[0].alpha_arg1 = plain(TextureArgSource::Diffuse);
        s[0].alpha_arg2 = plain(TextureArgSource::Specular);
        s[1].color_op = TextureOp::Subtract;
        s[1].color_arg1 = plain(TextureArgSource::TFactor);
        s[1].color_arg2 = plain(TextureArgSource::Current);
        s[1].alpha_op = TextureOp::Subtract;
        s[1].alpha_arg1 = plain(TextureArgSource::TFactor);
        s[1].alpha_arg2 = plain(TextureArgSource::Current);
        let def = ff(&s, 0x8080_8080);
        let i = Inputs {
            d0: [0.0, 0.25, 0.0, 0.0],
            d1: [0.5, 0.0, 0.25, 0.5],
            ..Inputs::default()
        };
        let h = 128.0 / 255.0;
        near(run(&def, &i), [h, h - 0.25, h, h]);

        // Above one: ADD saturates to 1 before stage 1 subtracts diffuse.
        s[0].color_op = TextureOp::Add;
        s[0].color_arg2 = plain(TextureArgSource::Diffuse);
        s[1].color_arg1 = plain(TextureArgSource::Current);
        s[1].color_arg2 = plain(TextureArgSource::Diffuse);
        let i = Inputs {
            d0: [0.75, 0.25, 0.0, 0.0],
            ..Inputs::default()
        };
        let c = run(&ff(&s, 0x8080_8080), &i);
        near([c[0], c[1], c[2], 0.0], [0.25, 0.25, 0.0, 0.0]);
    }

    #[test]
    fn complemented_subtrahend() {
        let mut s = stages();
        s[0].color_op = TextureOp::Subtract;
        s[0].color_arg1 = plain(TextureArgSource::Diffuse);
        s[0].color_arg2 = TextureArg {
            source: TextureArgSource::Specular,
            complement: true,
            alpha_replicate: false,
        };
        s[0].alpha_op = TextureOp::SelectArg1;
        s[0].alpha_arg1 = plain(TextureArgSource::Diffuse);
        let i = Inputs {
            d0: [1.0, 0.5, 0.25, 1.0],
            d1: [0.5, 0.75, 1.0, 0.0],
            ..Inputs::default()
        };
        near(run(&ff(&s, 0), &i), [0.5, 0.25, 0.25, 1.0]);
    }

    #[test]
    fn pretransformed_program_maps_rhw() {
        let p = pretransformed_program([9, 10, 11, 12], true);
        let mut c = [[0.0f32; 4]; 192];
        c[FF_HALF_PIXEL] = [0.5, 0.5, 0.0, 0.0];
        let mut run = |rhw: f32| {
            let mut v = [[0.0f32, 0.0, 0.0, 1.0]; 16];
            v[0] = [10.0, 20.0, 30.0, rhw];
            v[4] = [0.0, 0.0, 0.0, 0.5];
            v[10] = [0.25, 0.75, 0.0, 1.0];
            vsh::interp::run(&p, &v, &mut c)
        };
        let o = run(0.5);
        assert_eq!(o.get(OutReg::Pos), [10.5, 20.5, 30.0, 2.0]);
        assert_eq!(o.get(OutReg::T(1)), [0.25, 0.75, 0.0, 1.0]);
        assert_eq!(o.get(OutReg::Fog)[0], 0.5);
        assert_eq!(run(0.0).get(OutReg::Pos)[3], 1.0, "rhw 0 behaves as 1");
    }
}
