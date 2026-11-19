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
//!   `D3DTSS_TEXCOORDINDEX` names; `oFog` is the coordinate D3D's own
//!   pass-through program writes ([`PassthroughFog`]): the specular alpha
//!   without a fog table, else the vertex z (affine projection) or w.
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
//! * A depth-format texture a stage samples is a shadow map: PROJECT2D and
//!   PROJECT3D compare its stored depth under `D3DRS_SHADOWFUNC` (see
//!   [`shadow_stages`] and `shaders::psh::shadow`).
//!
//! Per-pipeline state that changes the generated code (texture dimensions,
//! alpha kill, `COLORSIGN`, alpha test function, two-sided lighting, flat
//! shading, the shadow-map format, function, filter and addressing) is
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

// Program keys are large (microcode plus the combiner definition) and are
// hashed on every draw: Fx instead of SipHash. Nothing here is keyed by
// untrusted input.
use rustc_hash::{FxHashMap, FxHashSet};
use std::path::PathBuf;
use std::sync::mpsc;

use gfx::pipeline::ShaderId;
use gfx::program::{ProgramBindings, StageDims};
use gfx::state::{
    CompareFunc, ShadeMode, TextureAddress, TextureArg, TextureArgSource, TextureFilter,
    TextureOp,
    TextureStageState,
};
use gfx::texture::TextureKind;
use gfx::translate::SamplerKey;
use gfx::xdk::RenderStateTable;
use shaders::psh::bits::*;
use shaders::psh::shadow::{DepthFormat, Shadow, ShadowAddress};
use shaders::psh::{
    self, AlphaFunc, PixelOptions, PixelShaderDef, PixelUniforms, TextureDim, argb,
};
use shaders::vsh::encode::{Inst, Operand, program};
use gfx::repack::{AttributeFetch, FetchMode, RepackPlan};
use shaders::vsh::{
    self, IluOp, InputFetch, MacOp, OutReg, ProgramKind, VertexOptions, VertexUniforms,
};

use crate::backend::ResolvedDraw;
use crate::pipeline_store::{PipelineStore, StoredPipeline, program_id};
use crate::shader::{PreparedShader, ShaderProvider, VertexProcessing};
use crate::state::slot;

#[path = "nv2a_hot.rs"]
mod hot;

#[path = "nv2a_draw.rs"]
mod draw;
pub use draw::{NativeDrawKey, NativeDrawProgram, NativeDrawShader};

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
    pub const RANGEFOGENABLE: u32 = 97;
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
    /// Programs translated ahead of time from the pipeline cache (not
    /// counted in `translated`).
    pub prewarmed: u64,
    /// Pipelines queued for building ahead of time from the pipeline cache.
    pub prewarmed_pipelines: u64,
    /// Draws whose bound definition has a final combiner but whose pushed
    /// final-combiner inputs are both zero, so it runs as an all-zero
    /// final combiner (black, alpha 0; [`PixelShaderDef::explicit_final`]).
    pub zero_final_draws: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
struct PretransformedKey {
    /// Vertex register read for `oT0..oT3` (`9 + D3DTSS_TEXCOORDINDEX`).
    texcoords: [u8; 4],
    /// What `oFog.x` is.
    fog: PassthroughFog,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
enum VertexSource {
    Microcode(crate::shader::Microcode),
    Pretransformed(PretransformedKey),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct ProgramKey {
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
    discards: bool,
    warnings: Vec<String>,
}

/// Translates draws with `crates/shaders`; install it with
/// [`GfxBackend::with_shaders`](crate::GfxBackend::with_shaders) (the
/// default for [`GfxBackend::new`](crate::GfxBackend::new)).
pub struct Nv2aShaders {
    bindings: Option<ProgramBindings>,
    vertex_bytes: [u8; VertexUniforms::SIZE],
    pixel_bytes: [u8; PixelUniforms::SIZE],
    programs: FxHashMap<ProgramKey, Result<Compiled, String>>,
    table: RenderStateTable,
    next: u32,
    stats: Nv2aStats,
    /// The pipeline cache to open at the first [`maintain`](ShaderProvider::maintain).
    cache_path: Option<PathBuf>,
    persist: Option<Persist>,
    warm: Option<Warm>,
    /// [`ShaderProvider::set_render_scale`].
    render_scale: u32,
    /// [`ShaderProvider::set_hard_shadows`].
    hard_shadows: bool,
    /// WGSL overrides and dumps (`nv2a_hot.rs`).
    hot: hot::HotState,
}

/// Recording to the pipeline cache.
struct Persist {
    store: PipelineStore,
    /// The program each registered shader id runs, and its cache id.
    ids: HashMap<ShaderId, (u64, ProgramKey)>,
}

/// A translated program with its module (the source is dropped).
type Translated = Result<(wgpu::ShaderModule, Translation), String>;

/// Programs from the pipeline cache being translated on the prewarm thread.
struct Warm {
    rx: mpsc::Receiver<(ProgramKey, Translated)>,
    /// Sent to the thread and not installed yet.
    waiting: FxHashSet<ProgramKey>,
    /// Recorded pipelines by program cache id, built once it is installed.
    pipelines: HashMap<u64, Vec<StoredPipeline>>,
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
            vertex_bytes: [0; VertexUniforms::SIZE],
            pixel_bytes: [0; PixelUniforms::SIZE],
            programs: FxHashMap::default(),
            table: RenderStateTable::default(),
            next: 0,
            stats: Nv2aStats::default(),
            cache_path: None,
            persist: None,
            warm: None,
            render_scale: 1,
            hard_shadows: false,
            hot: hot::HotState::new(hot_render::global()),
        }
    }

    /// A provider that records every pipeline it needs to the cache file at
    /// `path` and, when it starts, builds the ones already in it ahead of
    /// time (see [`crate::pipeline_store`]).
    pub fn with_pipeline_cache(path: PathBuf) -> Self {
        Self {
            cache_path: Some(path),
            ..Self::new()
        }
    }

    /// Opens the cache file, queues the built-in shader's pipelines and
    /// starts translating the recorded programs.
    fn open_cache(&mut self, ctx: &gfx::GfxContext, renderer: &mut gfx::Renderer, path: PathBuf) {
        let (store, loaded) = PipelineStore::open::<ProgramKey>(&path);
        let mut pipelines: HashMap<u64, Vec<StoredPipeline>> = HashMap::new();
        for p in loaded.pipelines {
            match p.program {
                None => {
                    let key = gfx::pipeline::PipelineKey {
                        shader: gfx::renderer::SHADER_XYZRHW_DIFFUSE,
                        ..p.key
                    };
                    if renderer.prewarm(ctx, &key, p.count) {
                        self.stats.prewarmed_pipelines += 1;
                    }
                }
                Some(id) => pipelines.entry(id).or_default().push(p),
            }
        }
        let keys: Vec<ProgramKey> = loaded.programs.into_iter().map(|(_, k)| k).collect();
        let waiting: FxHashSet<ProgramKey> = keys.iter().cloned().collect();
        let (tx, rx) = mpsc::channel();
        let device = ctx.device.clone();
        let spawned = std::thread::Builder::new()
            .name("nv2a prewarm".into())
            .spawn(move || {
                for key in keys {
                    let r = translate(&key).map(|mut t| {
                        (module(&device, std::mem::take(&mut t.source)), t)
                    });
                    if tx.send((key, r)).is_err() {
                        break;
                    }
                }
            });
        if spawned.is_ok() {
            self.warm = Some(Warm { rx, waiting, pipelines });
        }
        self.persist = Some(Persist { store, ids: HashMap::new() });
    }

    /// Installs programs the prewarm thread has translated and queues their
    /// pipelines; with `until`, waits until that program is in.
    fn install_warm(
        &mut self,
        ctx: &gfx::GfxContext,
        renderer: &mut gfx::Renderer,
        until: Option<&ProgramKey>,
    ) {
        loop {
            let Some(warm) = self.warm.as_mut() else { return };
            let wait = until.is_some_and(|k| warm.waiting.contains(k));
            let msg = if wait {
                let _t = timeline::span(timeline::Cat::Gfx, timeline::Track::Host, "prewarm wait");
                warm.rx.recv().ok()
            } else {
                warm.rx.try_recv().ok()
            };
            let Some((key, r)) = msg else {
                if wait || warm.waiting.is_empty() {
                    // The thread is gone (or done): translate the rest on demand.
                    self.warm = None;
                }
                return;
            };
            warm.waiting.remove(&key);
            let stored = warm.pipelines.remove(&program_id(&key)).unwrap_or_default();
            if self.programs.contains_key(&key) {
                continue;
            }
            let r = r.map(|(module, t)| {
                let Translation { dims, discards, warnings, .. } = t;
                let id = ShaderId(SHADER_ID_BASE + self.next);
                self.next += 1;
                report_warnings(id.0, &warnings);
                let bindings = self
                    .bindings
                    .get_or_insert_with(|| ProgramBindings::new(&ctx.device));
                renderer.register_shader(
                    id,
                    registered_program(bindings, &ctx.device, module, dims, &key),
                );
                self.stats.prewarmed += 1;
                if let Some(p) = self.persist.as_mut() {
                    p.ids.insert(id, (program_id(&key), key.clone()));
                }
                for s in stored {
                    let k = gfx::pipeline::PipelineKey { shader: id, ..s.key };
                    if renderer.prewarm(ctx, &k, s.count) {
                        self.stats.prewarmed_pipelines += 1;
                    }
                }
                Compiled { id, dims, discards, warnings }
            });
            self.programs.insert(key, r);
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
    // Straight from the words: the judge builds one per draw on the
    // guest's thread.
    let mut def = PixelShaderDef::from_words(&words);
    def.explicit_final = final_latch;
    def
}

/// The fog coordinate of pre-transformed (`XYZRHW`) vertices: what the
/// pass-through program D3D loads writes to `oFog`. `0x3f6f40` picks one
/// of three programs in the image (`GuestLayout::passthrough_programs`):
/// with `D3DRS_FOGTABLEMODE` = `D3DFOG_NONE` it writes `mov oFog, v4.w`
/// (the specular alpha, D3D's vertex fog); with a fog table it writes
/// `mov oFog, v0.z` when the projection is affine (device flag 2) and
/// `rcp oFog, v0.w` (the vertex w) otherwise. The fog unit then applies the
/// table mode to that coordinate ([`d3d_fog`]). D3D does this whatever
/// `D3DRS_FOGENABLE` says; with fog off the factor is 1 anyway.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum PassthroughFog {
    /// `v4.w`: specular alpha (no fog table).
    SpecularAlpha,
    /// `v0.z`: window z (fog table, affine projection).
    Z,
    /// `1/v0.w`: w (fog table, perspective projection).
    W,
}

impl PassthroughFog {
    /// `0x3f6f40`'s choice from `D3DRS_FOGTABLEMODE` and device flag 2
    /// (affine projection).
    pub fn select(fog_table_mode: u32, affine_projection: bool) -> Self {
        if fog_table_mode == 0 {
            Self::SpecularAlpha
        } else if affine_projection {
            Self::Z
        } else {
            Self::W
        }
    }
}

/// The vertex program for pre-transformed (`XYZRHW`) fixed-function
/// vertices. Reads `c[0]` = (0.5, 0.5, 0, 0) and `c[1]` = 0.
///
/// `oFog.x` follows `fog` ([`PassthroughFog`]). For [`PassthroughFog::W`]
/// it is `rcp` of the vertex's own rhw, as in the guest program, so
/// `rhw = 0` gives infinity (the fog unit's limit, see `psh::fog`) even
/// though the position treats `rhw = 0` as 1.
pub fn pretransformed_program(texcoords: [u8; 4], fog: PassthroughFog) -> vsh::Program {
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
    let fog_source = match fog {
        PassthroughFog::SpecularAlpha => Operand::v(4).swz("w"),
        PassthroughFog::Z => Operand::v(0).swz("z"),
        PassthroughFog::W => Operand::v(0).swz("w"),
    };
    insts.push(match fog {
        PassthroughFog::W => Inst::ilu(IluOp::Rcp, fog_source).to_out(true, OutReg::Fog, x),
        _ => Inst::mac(MacOp::Mov, Some(fog_source), None, None).to_out(false, OutReg::Fog, x),
    });
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
    fn set_render_scale(&mut self, scale: u32) {
        self.render_scale = scale.max(1);
    }

    fn set_hard_shadows(&mut self, on: bool) {
        self.hard_shadows = on;
    }

    fn prepare(
        &mut self,
        ctx: &gfx::GfxContext,
        renderer: &mut gfx::Renderer,
        draw: &ResolvedDraw,
        textures: [Option<&wgpu::TextureView>; 4],
    ) -> Result<PreparedShader, String> {
        let present = textures.map(|t| t.is_some());
        let KeyParts {
            key,
            def,
            signs,
            border,
            bound,
        } = program_key(&self.table, ctx.caps, draw, present, renderer.vertex_fetch())?;
        // Text from the glyph cache, magnified: the glyph filter
        // (`crate::glyph`). Never at 1x, so the reference is untouched.
        let mut key = key;
        if self.render_scale > 1 {
            key.pixel_options.glyph = draw.glyph_cache & 0xF;
        }
        // E3 lighting's one-tap shadow lookup (view only, like the glyph
        // filter): the reference never sets it.
        if self.hard_shadows {
            key.pixel_options.hard_shadow = hard_shadow_groups(draw, &key.pixel_options.shadow);
        }
        if def.explicit_final && def.final_inputs_abcd == 0 && def.final_inputs_efg == 0 {
            self.stats.zero_final_draws += 1;
        }
        // One lookup on the common path (the program is known): the key
        // holds the whole microcode, so each hash and compare is not free.
        let compiled = match self.programs.get(&key) {
            Some(r) => r.clone()?,
            None => {
                // Being translated ahead of time: take that one.
                self.install_warm(ctx, renderer, Some(&key));
                if !self.programs.contains_key(&key) {
                    let bindings = self
                        .bindings
                        .get_or_insert_with(|| ProgramBindings::new(&ctx.device));
                    let r = compile(ctx, renderer, bindings, &key, SHADER_ID_BASE + self.next);
                    match &r {
                        Ok(c) => {
                            self.next += 1;
                            self.stats.translated += 1;
                            if let Some(p) = self.persist.as_mut() {
                                p.ids.insert(c.id, (program_id(&key), key.clone()));
                            }
                        }
                        Err(_) => self.stats.failed += 1,
                    }
                    self.programs.insert(key.clone(), r);
                }
                self.programs[&key].clone()?
            }
        };
        let bindings = self
            .bindings
            .get_or_insert_with(|| ProgramBindings::new(&ctx.device));
        self.stats.draws += 1;

        let (vu, pu) = draw::uniforms(&self.table, draw, &key, &def, signs, self.render_scale);
        let w_buffer = key.pixel_options.w_buffer;
        vu.write_bytes(&mut self.vertex_bytes);
        pu.write_bytes(&mut self.pixel_bytes);
        let group0 = bindings.uniforms(
            &ctx.device,
            &ctx.queue,
            &self.vertex_bytes,
            &self.pixel_bytes,
        );

        // Textures: a stand-in where the bound view has another dimension.
        let views: [Option<&wgpu::TextureView>; 4] = std::array::from_fn(|n| {
            let want = compiled.dims[n]?;
            let have = bound[n].map(|k| view_dim(texture_dim(k)))?;
            (want == have).then_some(textures[n]).flatten()
        });
        let samplers = draw::samplers(draw, border, ctx.caps);
        if let VertexSource::Microcode(words) = &key.vertex {
            crate::uniform_compare::observe(
                words,
                &vu,
                &pu,
                &key.vertex_options,
                (&def, &key.pixel_options),
                (
                    &gfx::translate::primitive_state(&draw.state.render, draw.primitive.host_topology(), ctx.caps),
                    gfx::translate::depth_stencil_state(
                        &draw.state.render,
                        Some(wgpu::TextureFormat::Depth24PlusStencil8),
                        draw.primitive.host_topology(),
                    )
                    .as_ref(),
                ),
                &samplers,
                &draw.texture_info,
            );
        }
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
            discards: compiled.discards,
            writes_depth: w_buffer,
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

    fn maintain(
        &mut self,
        ctx: &gfx::GfxContext,
        renderer: &mut gfx::Renderer,
        new_pipelines: &[(gfx::pipeline::PipelineKey, bool)],
    ) {
        if let Some(path) = self.cache_path.take() {
            self.open_cache(ctx, renderer, path);
        }
        self.install_warm(ctx, renderer, None);
        self.hot_maintain(ctx, renderer);
        let Some(p) = self.persist.as_mut() else { return };
        for (key, count) in new_pipelines {
            if key.shader == gfx::renderer::SHADER_XYZRHW_DIFFUSE {
                p.store.record::<ProgramKey>(None, *count, key);
            } else if let Some((id, program)) = p.ids.get(&key.shader) {
                p.store.record(Some((*id, program)), *count, key);
            }
        }
    }
    fn judge(&self, caps: gfx::translate::Capabilities) -> Option<Box<dyn crate::shader::ShaderJudge>> {
        Some(Box::new(Nv2aJudge {
            table: self.table.clone(),
            caps,
            programs: FxHashMap::default(),
        }))
    }
}

/// [`Nv2aShaders`]' verdicts without a GPU: the same program key, and the
/// same translation (cached by key, as `prepare` caches compiles), up to
/// WGSL validation.
pub struct Nv2aJudge {
    table: RenderStateTable,
    caps: gfx::translate::Capabilities,
    programs: FxHashMap<ProgramKey, Result<(), String>>,
}

impl crate::shader::ShaderJudge for Nv2aJudge {
    fn check(&mut self, draw: &ResolvedDraw, present: [bool; 4]) -> Result<(), String> {
        // Fetch only changes the entry point's parameter types, never
        // whether translation succeeds.
        let key = program_key(&self.table, self.caps, draw, present, FetchMode::Cpu)?.key;
        if let Some(v) = self.programs.get(&key) {
            return v.clone();
        }
        let v = translate(&key).map(|_| ());
        self.programs.insert(key, v.clone());
        v
    }
}

/// How the vertex program reads each register for `draw` in `mode`: the
/// renderer's plan for the same layout and strides (`gfx::repack`), so the
/// entry point's parameter types match the pipeline's vertex formats.
pub(crate) fn input_fetch(draw: &ResolvedDraw, mode: FetchMode) -> [InputFetch; 16] {
    if mode == FetchMode::Cpu {
        return [InputFetch::Float; 16];
    }
    let strides: Vec<u32> = draw.streams.iter().map(|s| s.stride).collect();
    RepackPlan::with_mode(&draw.layout, &strides, mode)
        .input_fetch()
        .map(|f| match f {
            AttributeFetch::Float => InputFetch::Float,
            AttributeFetch::Xyz1 => InputFetch::Xyz1,
            AttributeFetch::Int => InputFetch::Int,
            AttributeFetch::IntXyz1 => InputFetch::IntXyz1,
            AttributeFetch::NormPacked3 => InputFetch::NormPacked3,
            AttributeFetch::Float2H => InputFetch::Float2H,
        })
}

/// What [`program_key`] derives from a draw.
struct KeyParts {
    key: ProgramKey,
    def: PixelShaderDef,
    signs: [u8; 4],
    border: [u8; 4],
    bound: [Option<TextureKind>; 4],
}

/// The program a draw needs, from the draw and which stages have a
/// sampleable texture view (`present`). Pure: [`Nv2aJudge`] runs it on
/// the device's side to know a draw's outcome before the render thread
/// prepares it. An error is the reason the draw is skipped.
fn program_key(
    table: &RenderStateTable,
    caps: gfx::translate::Capabilities,
    draw: &ResolvedDraw,
    present: [bool; 4],
    fetch: FetchMode,
) -> Result<KeyParts, String> {
        let render = &draw.state.render;
        let flat = render.shade_mode == ShadeMode::Flat;
        let bound: [Option<TextureKind>; 4] =
            std::array::from_fn(|n| present[n].then_some(()).and(draw.texture_info[n]).map(|i| i.kind));
        let fog_enable = render.fog_enable;
        let sampled = crate::device::sampled_stages(&draw.shader, &draw.raw, &draw.state);

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
                    fog: PassthroughFog::select(
                        raw(table, draw, rs::FOGTABLEMODE),
                        draw.shader.affine_projection,
                    ),
                })
            }
        };

        // Pixel side.
        let specular = raw(table, draw, rs::SPECULARENABLE) != 0;
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
        let shadow = shadow_stages(draw, &def, sampled, present)?;
        let mut code = def.clone();
        code.constant0 = [0; 8];
        code.constant1 = [0; 8];
        code.final_constant0 = 0;
        code.final_constant1 = 0;

        let vertex_options = VertexOptions {
            provided_inputs: draw.shader.provided_inputs,
            flat_shading: flat,
            input_fetch: input_fetch(draw, fetch),
        };
        let signs: [u8; 4] = std::array::from_fn(|n| color_sign(draw.extras[n].color_sign));
        let border: [u8; 4] = std::array::from_fn(|n| {
            shader_border(&draw.state.stages[n], signs[n], caps.clamp_to_border)
        });
        let pixel_options = PixelOptions {
            texture_dims: bound.map(|k| k.map(texture_dim)),
            alpha_kill: std::array::from_fn(|n| draw.extras[n].alpha_kill & ALPHAKILL_ENABLE != 0),
            color_sign: signs,
            border,
            alpha_test: render
                .alpha_test_enable
                .then(|| alpha_func(render.alpha_func)),
            two_sided_lighting: raw(table, draw, rs::TWOSIDEDLIGHTING) != 0,
            flat_shading: flat,
            explicit_lod: false,
            stipple: polygon_stipple(table, draw),
            shadow,
            // The depth write needs a depth attachment: a W-buffered draw
            // to a colour-only target still clips w but writes no depth
            // (a pipeline whose fragment writes depth without one fails
            // validation).
            // A draw that neither tests nor writes depth gets no depth
            // write either (it would only cost the early depth test).
            w_buffer: w_clipped(&vertex, draw)
                && draw.target.depth_format.is_some()
                && uses_depth(render),
            // Depends on the target's scale: set by `Nv2aShaders::prepare`.
            glyph: 0,
            // E3 lighting, on the view only: set by `Nv2aShaders::prepare`.
            hard_shadow: [0; 4],
        };
        let key = ProgramKey {
            vertex,
            vertex_options,
            pixel: code,
            pixel_options,
        };
        Ok(KeyParts {
            key,
            def,
            signs,
            border,
            bound,
        })
}

/// `SET_CONTROL0` bit 16 (z-perspective), which D3D's control fragment
/// (`0x3f6390`) sets when `D3DRS_ZENABLE` is `D3DZB_USEW`: the NV2A then
/// depth-tests and stores the interpolated w instead of z, and clips w to
/// `[CLIP_MIN, CLIP_MAX]`. Halo turns it on at boot (`ZEnable(2)` at
/// `0x1267a`). Before any `SET_CONTROL0` has been seen, the render state
/// decides.
pub const CONTROL0_Z_PERSPECTIVE: u32 = 0x1_0000;

/// Whether `draw` is W-buffered (see [`CONTROL0_Z_PERSPECTIVE`]).
pub fn w_buffered(draw: &ResolvedDraw) -> bool {
    match draw.hw.control0 {
        Some(c) => c & CONTROL0_Z_PERSPECTIVE != 0,
        None => draw.state.render.depth_mode == gfx::state::DepthBufferMode::W,
    }
}

/// Whether the vertex stage clips w instead of z: a W-buffered draw of a
/// vertex program.
fn w_clipped(vertex: &VertexSource, draw: &ResolvedDraw) -> bool {
    matches!(vertex, VertexSource::Microcode(_)) && w_buffered(draw)
}

/// Whether the depth buffer matters to a draw: tested or written.
fn uses_depth(render: &gfx::state::RenderState) -> bool {
    render.depth_mode != gfx::state::DepthBufferMode::Disabled
        && (render.depth_write || render.depth_func != CompareFunc::Always)
}

/// `D3DRS_DEPTHCLIPCONTROL` (canonical 82), which D3D and Halo's inlined
/// copies push to `NV097_SET_ZMIN_MAX_CONTROL` (`0x1D78`): bits 7:4 = 1
/// (`D3DDCC_DEPTHCLAMP`) clamp out-of-range depth instead of culling it
/// (xemu's `ZCLAMP_EN`, `pgraph.c` and `glsl/psh.c`).
pub const RS_DEPTHCLIPCONTROL: u32 = 82;

/// Whether out-of-range depth is clamped, not culled.
fn depth_clamp(table: &RenderStateTable, draw: &ResolvedDraw) -> bool {
    clamps_depth(raw(table, draw, RS_DEPTHCLIPCONTROL))
}

/// Whether `draw` clamps out-of-range depth, read with the default
/// render-state table (the one [`Nv2aShaders`] starts with): for
/// diagnostics that hold a draw but no provider (`render.frame`).
pub fn draw_clamps_depth(draw: &ResolvedDraw) -> bool {
    static TABLE: std::sync::LazyLock<RenderStateTable> =
        std::sync::LazyLock::new(RenderStateTable::default);
    depth_clamp(&TABLE, draw)
}

/// Whether a `D3DRS_DEPTHCLIPCONTROL` value clamps out-of-range depth
/// (bits 7:4 = 1, `D3DDCC_DEPTHCLAMP`).
pub fn clamps_depth(depth_clip_control: u32) -> bool {
    gfx::state::clamps_depth(depth_clip_control)
}

/// The w clip range of a W-buffered draw: the pushed `[CLIP_MIN,
/// CLIP_MAX]`, or `[0, z_max]` when that is empty or not finite (nothing
/// pushed yet).
pub fn w_clip_range(hw: &crate::hw::HwState, z_max: f32) -> [f32; 2] {
    let (lo, hi) = (hw.clip_min, hw.clip_max);
    if lo.is_finite() && hi.is_finite() && lo >= 0.0 && hi > lo {
        [lo, hi]
    } else {
        [0.0, z_max]
    }
}

/// The shadow-map stages of a draw: a depth-format texture (`D24S8`,
/// `D16`, `F24S8`, `F16`, swizzled or linear) that a stage samples is
/// compared, not read as colour (`shaders::psh::shadow`). The function is
/// `D3DRS_SHADOWFUNC` (guest slot 156): `SetRenderState_ShadowFunc`
/// (`0x3f5330`) pushes `Value - 0x200` to `NV097_SET_SHADOW_DEPTH_FUNC`
/// (`0x1E6C`), the register xemu's `psh_append_shadowmap` reads, so the
/// `D3DCMP_*` value is the comparison `stored OP reference`. A linear (or
/// anisotropic, quincunx, gaussian) minification or magnification filter
/// gives 2x2 percentage-closer filtering; `BORDER` addressing is treated
/// as clamp.
///
/// Only PROJECT2D and PROJECT3D compare (xemu has no other mode either);
/// a depth texture sampled by another mode, or a `SHADOWFUNC` that is not
/// a `D3DCMP_*` value, skips the draw with the reason.
fn shadow_stages(
    draw: &ResolvedDraw,
    def: &PixelShaderDef,
    sampled: [bool; 4],
    present: [bool; 4],
) -> Result<[Option<Shadow>; 4], String> {
    let mut out = [None; 4];
    let func_raw = draw.raw.render[slot::SHADOWFUNC as usize];
    for n in 0..4 {
        let Some(info) = draw.texture_info[n] else {
            continue;
        };
        if !sampled[n] || !present[n] || !info.format.is_depth() {
            continue;
        }
        use gfx::D3DFormat as F;
        let format = match info.format {
            F::D24S8 | F::LinD24S8 => DepthFormat::D24,
            F::D16 | F::LinD16 => DepthFormat::D16,
            F::F24S8 | F::LinF24S8 => DepthFormat::F24,
            _ => DepthFormat::F16,
        };
        let mode = (def.texture_modes >> (5 * n)) & 0x1F;
        if mode != TEX_PROJECT2D && mode != TEX_PROJECT3D {
            return Err(format!(
                "stage {n} samples {:?} texture {:#010x} as a shadow map in texture mode {mode:#x}: only PROJECT2D and PROJECT3D compare",
                info.format, info.header
            ));
        }
        let Some(func) = CompareFunc::from_xbox(func_raw) else {
            return Err(format!(
                "stage {n} samples {:?} texture {:#010x} as a shadow map with D3DRS_SHADOWFUNC {func_raw:#x}, not a D3DCMP value",
                info.format, info.header
            ));
        };
        let stage = &draw.state.stages[n];
        let point = |f: TextureFilter| matches!(f, TextureFilter::None | TextureFilter::Point);
        let address = |a: TextureAddress| match a {
            TextureAddress::Wrap => ShadowAddress::Wrap,
            TextureAddress::Mirror => ShadowAddress::Mirror,
            _ => ShadowAddress::Clamp,
        };
        out[n] = Some(Shadow {
            format,
            func: alpha_func(func),
            linear: !point(stage.min_filter) || !point(stage.mag_filter),
            address: [address(stage.address_u), address(stage.address_v)],
        });
    }
    Ok(out)
}

/// [`PixelOptions::hard_shadow`] for a draw's shadow stages `shadow`: per
/// shadow stage, the shadow stages bound to the same texture header
/// (itself included), so taps a pass spreads over stages of one map
/// collapse onto one. Zero for every other stage.
pub fn hard_shadow_groups(draw: &ResolvedDraw, shadow: &[Option<Shadow>; 4]) -> [u8; 4] {
    let header = |n: usize| shadow[n].and(draw.texture_info[n]).map(|info| info.header);
    std::array::from_fn(|n| {
        let Some(h) = header(n) else { return 0 };
        (0..4)
            .filter(|&m| header(m) == Some(h))
            .fold(0u8, |mask, m| mask | 1 << m)
    })
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

/// The exp fog scale per unit density, `−1/(16 ln 2)`, so that
/// `2^(16·scale·d)` is `e^(−density·d)`. D3D multiplies by its own
/// constant (`0x45E378`), which differs from this by about 4e-6 relative;
/// the ignored `tests/fog_xbe.rs` reads it from `$H2_XBE` and checks the
/// tolerance.
pub fn exp_fog_scale() -> f32 {
    (-1.0 / (16.0 * std::f64::consts::LN_2)) as f32
}

/// The exp2 fog scale per unit density, `−1/√(32 ln 2)`, so that
/// `2^(−32·(scale·d)²)` is `e^(−(density·d)²)`. D3D's own constant
/// (`0x45E374`) differs from this by a similar few parts per million.
pub fn exp2_fog_scale() -> f32 {
    (-1.0 / (32.0 * std::f64::consts::LN_2).sqrt()) as f32
}

/// `NV097_SET_FOG_GEN_MODE` values D3D pushes.
pub mod fog_gen {
    /// `FOGGEN_SPEC_ALPHA`: the specular alpha (no fog table).
    pub const SPEC_ALPHA: u32 = 0;
    /// `FOGGEN_RADIAL`: the eye-space distance (`D3DRS_RANGEFOGENABLE`).
    pub const RADIAL: u32 = 1;
    /// `FOGGEN_PLANAR`: the eye-space depth.
    pub const PLANAR: u32 = 2;
}

/// The fog state D3D's lazy commit pushes (`0x00400000`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct D3dFog {
    /// `NV097_SET_FOG_GEN_MODE` ([`fog_gen`]). It selects the coordinate
    /// of the fixed-function transform only: vertex programs, D3D's
    /// pass-through programs among them, supply the coordinate in `oFog`, so no
    /// draw this provider translates reads it.
    pub gen_mode: u32,
    /// `NV097_SET_FOG_ENABLE`, `NV097_SET_FOG_MODE` and the bias and scale
    /// of `NV097_SET_FOG_PARAMS`.
    pub fog: psh::fog::Fog,
}

/// D3D's fog commit (`0x00400000`), from `D3DRS_FOGENABLE`,
/// `D3DRS_FOGTABLEMODE`, `D3DRS_FOGSTART`, `D3DRS_FOGEND`,
/// `D3DRS_FOGDENSITY` (the float states as their raw words) and
/// `D3DRS_RANGEFOGENABLE`:
///
/// * fog off: only `SET_FOG_ENABLE` 0 is pushed. The other registers keep
///   their old values, which no longer matter (the factor reads 1); this
///   returns [`psh::fog::Fog::OFF`] and `SPEC_ALPHA`;
/// * `D3DFOG_NONE`: `LINEAR` with bias 1 and scale 1, so the factor is the
///   vertex's `oFog.x`, and generation mode `SPEC_ALPHA`;
/// * `D3DFOG_LINEAR` (3): scale `−1/(end − start)` and bias
///   `1 + end/(end − start)`, or scale `−8192` (`0x45DFF4`) when start and
///   end compare equal (the x87 compare is unordered for NaN, which
///   divides);
/// * `D3DFOG_EXP` (1): bias 1.5 and scale `density ·` [`exp_fog_scale`];
///   any other mode, `D3DFOG_EXP2` (2) included: bias 1.5 and scale
///   `density ·` [`exp2_fog_scale`] (D3D's own constants differ from these
///   closed forms by about 4e-6 relative);
/// * with a table mode the generation mode is `RADIAL` when
///   `D3DRS_RANGEFOGENABLE` is set and `PLANAR` otherwise.
///
/// The x87 code keeps intermediates in extended precision and rounds when
/// it stores each parameter; here intermediates are `f64`.
pub fn d3d_fog(
    enable: bool,
    table_mode: u32,
    start: u32,
    end: u32,
    density: u32,
    range_fog: bool,
) -> D3dFog {
    use psh::fog::{Fog, FogMode};
    let f = |w: u32| f32::from_bits(w) as f64;
    let table_gen = if range_fog {
        fog_gen::RADIAL
    } else {
        fog_gen::PLANAR
    };
    let (gen_mode, fog) = match table_mode {
        _ if !enable => (fog_gen::SPEC_ALPHA, Fog::OFF),
        0 => (fog_gen::SPEC_ALPHA, Fog::FROM_VERTEX),
        3 => {
            let (start, end) = (f(start), f(end));
            let k = if start == end {
                8192.0
            } else {
                1.0 / (end - start)
            };
            let fog = Fog {
                enable: true,
                mode: FogMode::Linear,
                bias: (end * k + 1.0) as f32,
                scale: -k as f32,
            };
            (table_gen, fog)
        }
        m => {
            let (mode, k) = if m == 1 {
                (FogMode::Exp, exp_fog_scale())
            } else {
                (FogMode::Exp2, exp2_fog_scale())
            };
            let fog = Fog {
                enable: true,
                mode,
                bias: 1.5,
                scale: (f(density) * k as f64) as f32,
            };
            (table_gen, fog)
        }
    };
    D3dFog { gen_mode, fog }
}

/// The fog uniform (`psh::fog`): the fog unit as D3D programs it for the
/// draw's render state ([`d3d_fog`]).
fn fog_uniform(table: &RenderStateTable, draw: &ResolvedDraw) -> [f32; 4] {
    let r = |c: u32| raw(table, draw, c);
    d3d_fog(
        draw.state.render.fog_enable,
        r(rs::FOGTABLEMODE),
        r(rs::FOGSTART),
        r(rs::FOGEND),
        r(rs::FOGDENSITY),
        r(rs::RANGEFOGENABLE) != 0,
    )
    .fog
    .uniform()
}

/// Whether the polygon offset applies: the enable for the fill mode in
/// effect, on triangles (gfx's `depth_stencil_state` rule).
fn polygon_offset(render: &gfx::state::RenderState, primitive: gfx::vertex::PrimitiveType) -> bool {
    use gfx::state::FillMode;
    use gfx::vertex::PrimitiveType as P;
    let enabled = match render.fill_mode {
        FillMode::Point => render.point_offset_enable,
        FillMode::Wireframe => render.wireframe_offset_enable,
        FillMode::Solid => render.solid_offset_enable,
    };
    enabled
        && !matches!(
            primitive,
            P::PointList | P::LineList | P::LineLoop | P::LineStrip
        )
}

fn stipple_eligible(primitive: gfx::vertex::PrimitiveType, fill: gfx::state::FillMode) -> bool {
    use gfx::vertex::PrimitiveType;
    fill == gfx::state::FillMode::Solid
        && !matches!(
            primitive,
            PrimitiveType::PointList
                | PrimitiveType::LineList
                | PrimitiveType::LineLoop
                | PrimitiveType::LineStrip
        )
}

fn polygon_stipple(table: &RenderStateTable, draw: &ResolvedDraw) -> bool {
    stipple_eligible(draw.primitive, draw.state.render.fill_mode)
        && draw
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
    timeline::phase::count(timeline::phase::Counter::Shaders, 1);
    let Translation { source, dims, discards, warnings } = translate(key)?;
    report_warnings(id, &warnings);
    let module = module(&ctx.device, source);
    let id = ShaderId(id);
    renderer.register_shader(
        id,
        registered_program(bindings, &ctx.device, module, dims, key),
    );
    Ok(Compiled { id, dims, discards, warnings })
}

/// The registered program for `key`'s module. A W-buffered program writes
/// its depth from the fragment stage, so its pixel-count pass must too
/// (`shaders::psh::COUNT_DEPTH_ENTRY`): counted with the rasterized z, a
/// visibility test compares a value the depth buffer does not hold. That
/// was every lens flare in front of level geometry counting nothing and
/// never showing (docs/graphics/d3d.md, "Visibility tests").
fn registered_program(
    bindings: &mut ProgramBindings,
    device: &wgpu::Device,
    module: wgpu::ShaderModule,
    dims: StageDims,
    key: &ProgramKey,
) -> gfx::pipeline::ShaderProgram {
    let mut p = bindings.program(device, module, dims);
    if key.pixel_options.w_buffer {
        p.count_entry = Some(shaders::psh::COUNT_DEPTH_ENTRY.into());
    }
    p
}

/// The WGSL module of a translated program.
fn module(device: &wgpu::Device, source: String) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("nv2a program"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    })
}

/// A translated program, before it has a module.
struct Translation {
    source: String,
    dims: StageDims,
    /// The pixel side may discard (see [`Compiled`]).
    discards: bool,
    warnings: Vec<String>,
}

/// Translates and validates a program: its WGSL, the texture dimensions it
/// samples, whether it discards and the translator's warnings. Needs no GPU.
fn translate(key: &ProgramKey) -> Result<Translation, String> {
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
    let source = shaders::link(&vs, &ps).map_err(|e| e.to_string())?;
    shaders::validate(&source).map_err(|e| format!("generated WGSL: {e}"))?;
    let dims: StageDims = ps.textures.map(|d| d.map(view_dim));
    Ok(Translation { source, dims, discards: ps.discards, warnings: ps.warnings })
}

#[cfg(test)]
mod tests {
    #[test]
    fn stipple_only_applies_to_filled_polygon_primitives() {
        use super::stipple_eligible;
        use gfx::state::FillMode;
        use gfx::vertex::PrimitiveType;
        for p in [
            PrimitiveType::TriangleList,
            PrimitiveType::TriangleStrip,
            PrimitiveType::TriangleFan,
            PrimitiveType::QuadList,
            PrimitiveType::QuadStrip,
            PrimitiveType::Polygon,
        ] {
            assert!(stipple_eligible(p, FillMode::Solid));
            assert!(!stipple_eligible(p, FillMode::Wireframe));
            assert!(!stipple_eligible(p, FillMode::Point));
        }
        for p in [
            PrimitiveType::PointList,
            PrimitiveType::LineList,
            PrimitiveType::LineLoop,
            PrimitiveType::LineStrip,
        ] {
            assert!(!stipple_eligible(p, FillMode::Solid));
        }
    }

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
        let p = pretransformed_program([9, 10, 11, 12], PassthroughFog::SpecularAlpha);
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

    /// The pass-through fog coordinate: specular alpha without a fog
    /// table, z with an affine projection, w (the same w as the position)
    /// otherwise.
    #[test]
    fn pretransformed_program_fog_sources() {
        let mut c = [[0.0f32; 4]; 192];
        c[FF_HALF_PIXEL] = [0.5, 0.5, 0.0, 0.0];
        let mut v = [[0.0f32, 0.0, 0.0, 1.0]; 16];
        v[0] = [10.0, 20.0, 1234.5, 0.25];
        v[4] = [0.0, 0.0, 0.0, 0.5];
        let fog = |f: PassthroughFog, v: &[[f32; 4]; 16]| {
            vsh::interp::run(&pretransformed_program([9; 4], f), v, &mut c.clone()).get(OutReg::Fog)
                [0]
        };
        assert_eq!(fog(PassthroughFog::SpecularAlpha, &v), 0.5);
        assert_eq!(fog(PassthroughFog::Z, &v), 1234.5);
        assert_eq!(fog(PassthroughFog::W, &v), 4.0);
        v[0][3] = 0.0;
        // The position treats rhw 0 as 1; the fog coordinate is the
        // guest's rcp of it, infinity.
        assert_eq!(fog(PassthroughFog::W, &v), f32::INFINITY);
        // 0x3f6f40's choice.
        assert_eq!(
            PassthroughFog::select(0, true),
            PassthroughFog::SpecularAlpha
        );
        assert_eq!(
            PassthroughFog::select(0, false),
            PassthroughFog::SpecularAlpha
        );
        assert_eq!(PassthroughFog::select(1, true), PassthroughFog::Z);
        assert_eq!(PassthroughFog::select(3, false), PassthroughFog::W);
        assert_eq!(PassthroughFog::select(7, false), PassthroughFog::W);
    }

    /// What D3D's fog commit (`0x00400000`) pushes for each table mode.
    #[test]
    fn d3d_fog_commit_registers() {
        use psh::fog::{Fog, FogMode};
        let b = |x: f32| x.to_bits();
        // Off: factor 1, whatever else is set.
        let off = d3d_fog(false, 3, b(0.0), b(100.0), b(1.0), true);
        assert_eq!(off.fog, Fog::OFF);
        assert_eq!(off.fog.factor(50.0), 1.0);
        // NONE: linear 1/1 and the specular-alpha generation mode, with or
        // without range fog.
        for range in [false, true] {
            let v = d3d_fog(true, 0, b(10.0), b(20.0), b(0.5), range);
            assert_eq!(v.fog, Fog::FROM_VERTEX);
            assert_eq!(v.gen_mode, fog_gen::SPEC_ALPHA);
        }
        // LINEAR: bias 1 + end/(end - start), scale -1/(end - start).
        let l = d3d_fog(true, 3, b(10.0), b(30.0), 0, false);
        assert_eq!(
            l.fog,
            Fog {
                enable: true,
                mode: FogMode::Linear,
                bias: 2.5,
                scale: -0.05,
            }
        );
        assert_eq!(l.gen_mode, fog_gen::PLANAR);
        assert_eq!(l.fog.factor(25.0), 0.25);
        // Range fog only changes the generation mode.
        let r = d3d_fog(true, 3, b(10.0), b(30.0), 0, true);
        assert_eq!((r.gen_mode, r.fog), (fog_gen::RADIAL, l.fog));
        // start == end: scale -8192.
        let e = d3d_fog(true, 3, b(50.0), b(50.0), 0, false);
        assert_eq!((e.fog.bias, e.fog.scale), (1.0 + 50.0 * 8192.0, -8192.0));
        assert_eq!(e.fog.factor(49.0), 1.0);
        assert_eq!(e.fog.factor(51.0), 0.0);
        // EXP and EXP2: bias 1.5, scale density times the image's constant.
        let x = d3d_fog(true, 1, 0, 0, b(0.5), false);
        assert_eq!(x.fog.mode, FogMode::Exp);
        assert_eq!((x.fog.bias, x.fog.scale), (1.5, 0.5 * exp_fog_scale()));
        assert!((x.fog.factor(2.0) - (-1.0f32).exp()).abs() < 1e-5);
        let x2 = d3d_fog(true, 2, 0, 0, b(0.5), true);
        assert_eq!(x2.fog.mode, FogMode::Exp2);
        assert_eq!(x2.gen_mode, fog_gen::RADIAL);
        assert_eq!(x2.fog.scale, 0.5 * exp2_fog_scale());
        assert!((x2.fog.factor(2.0) - (-1.0f32).exp()).abs() < 1e-5);
        // Any other table mode falls through to EXP2.
        assert_eq!(d3d_fog(true, 9, 0, 0, b(0.5), false).fog, x2.fog);
        // Unordered start/end (NaN) divides rather than taking 8192.
        let n = d3d_fog(true, 3, b(f32::NAN), b(1.0), 0, false);
        assert!(n.fog.scale.is_nan());
    }
}
