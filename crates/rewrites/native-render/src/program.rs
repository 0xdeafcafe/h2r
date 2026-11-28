//! A world material drawn with its own programs: the pass's vertex program
//! and pixel shader definition through `shaders`, the instance constants
//! where the game puts them, and the view's constants from the device
//! each frame (docs/graphics/native-world.md, "What a world pass reads").
//! [`compile`] is pure (WGSL and uniform values); [`ProgramSet`] holds the
//! GPU side for [`crate::gpu::GpuWorld`].

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gfx::compile::PendingPipeline;
use gfx::pipeline::{ShaderId, ShaderProgram};
use gfx::vertex::OwnedVertexBufferLayout;

use shaders::psh::{self, PixelOptions, PixelShaderDef, PixelUniforms, TextureDim};
use shaders::vsh::{self, InputFetch, VertexOptions, VertexUniforms};
use tags::material::Register;
use tags::shader::StageStates;

use crate::gpu::{Image, texture};
use crate::look::{PassLook, Stage, WorldPass};
use crate::pass_state::PassState;
use crate::world::{LightmapKey, ModelRows, Owner, World, WorldDraw, WorldVertex};

/// The per-view vertex constants the device gives the pass
/// (`gfx::external::VIEW_CONSTANTS`).
pub const VIEW_CONSTANTS: usize = 18;

/// The texture-coordinate decompression register the world programs read
/// (`mad r, v1, c[172].xy, c[172].zw`); the BSP's is the identity
/// (`0x0001cf29`).
pub const TEXCOORD_DECOMPRESSION: usize = 172;

/// The model rows the structure draw uploads for a placement
/// (`0x0004d830`, hardware numbering).
pub const MODEL_ROWS: usize = 50;

/// The position decompression a rigid program reads (`v0 · c[170] +
/// c[171]`): scale then offset, the identity for the BSP (`0x0001cdd0`
/// with no compression flags: `c[170]` all `1.0`, `c[171]` zero; uploaded
/// at `c[0xaa]` when the last upload was another, so the registers hold
/// it whenever a structure item draws; the render record's `+4` is `-1`
/// for clusters and placements, which takes that path at `0x0004d7c6`).
pub const POSITION_DECOMPRESSION: usize = 170;

/// The vertex registers a [`WorldVertex`] supplies to a material program,
/// with the field each comes from: `v0` position, `v1` texture coordinate,
/// `v2..v4` the tangent frame (normal, binormal, tangent: the program dots
/// the lighting vector with `v4`, `v3`, `v2` into tangent space), `v5` the
/// lighting vector, `v6` the lightmap coordinate. `v2..v5` are the packed
/// words ([`WorldVertex::packed`]), decoded in the shader
/// ([`WORLD_FETCH`]): the D3D path binds the streams' `NORMPACKED3` words
/// and its translation decodes them so (`xvs_normpacked3`), where the
/// native programs had floats decoded on the CPU (the 2026-11-19 uniform
/// compare: the same constants and inputs, another fetch).
pub const INPUTS: [(u32, u64, wgpu::VertexFormat); 7] = [
    (
        0,
        std::mem::offset_of!(WorldVertex, position) as u64,
        wgpu::VertexFormat::Float32x3,
    ),
    (
        1,
        std::mem::offset_of!(WorldVertex, uv) as u64,
        wgpu::VertexFormat::Float32x2,
    ),
    (
        2,
        std::mem::offset_of!(WorldVertex, packed) as u64,
        wgpu::VertexFormat::Uint32,
    ),
    (
        3,
        std::mem::offset_of!(WorldVertex, packed) as u64 + 4,
        wgpu::VertexFormat::Uint32,
    ),
    (
        4,
        std::mem::offset_of!(WorldVertex, packed) as u64 + 8,
        wgpu::VertexFormat::Uint32,
    ),
    (
        5,
        std::mem::offset_of!(WorldVertex, packed) as u64 + 12,
        wgpu::VertexFormat::Uint32,
    ),
    (
        6,
        std::mem::offset_of!(WorldVertex, lightmap_uv) as u64,
        wgpu::VertexFormat::Float32x2,
    ),
];

/// How a material program's `v` registers arrive ([`INPUTS`]): `v2..v5`
/// as packed words decoded in the shader, the rest as floats.
pub const WORLD_FETCH: [InputFetch; 16] = {
    let mut f = [InputFetch::Float; 16];
    f[2] = InputFetch::NormPacked3;
    f[3] = InputFetch::NormPacked3;
    f[4] = InputFetch::NormPacked3;
    f[5] = InputFetch::NormPacked3;
    f
};

/// Float layout of the placed world's frame and lighting vector.
/// Cluster material and light programs use [`INPUTS`] and [`WORLD_FETCH`].
pub const FLOAT_INPUTS: [(u32, u64, wgpu::VertexFormat); 7] = [
    (
        0,
        std::mem::offset_of!(WorldVertex, position) as u64,
        wgpu::VertexFormat::Float32x3,
    ),
    (
        1,
        std::mem::offset_of!(WorldVertex, uv) as u64,
        wgpu::VertexFormat::Float32x2,
    ),
    (
        2,
        std::mem::offset_of!(WorldVertex, normal) as u64,
        wgpu::VertexFormat::Float32x3,
    ),
    (
        3,
        std::mem::offset_of!(WorldVertex, binormal) as u64,
        wgpu::VertexFormat::Float32x3,
    ),
    (
        4,
        std::mem::offset_of!(WorldVertex, tangent) as u64,
        wgpu::VertexFormat::Float32x3,
    ),
    (
        5,
        std::mem::offset_of!(WorldVertex, lighting) as u64,
        wgpu::VertexFormat::Float32x3,
    ),
    (
        6,
        std::mem::offset_of!(WorldVertex, lightmap_uv) as u64,
        wgpu::VertexFormat::Float32x2,
    ),
];

/// Bit `n` for each supplied `v<n>`.
pub const INPUT_MASK: u16 = 0x7f;

/// Convert the per-vertex lighting classification's input registers into the
/// streams required of a section. The shader still reads v6. Its declaration
/// maps that register to the baked colour from the bucket's second descriptor.
pub fn vertex_lit_inputs(inputs: u16, semantics: &[u16]) -> Result<u16, String> {
    if semantics.len() > 7 || inputs >> semantics.len() != 0 {
        return Err("per-vertex lighting program reads outside its world declaration".into());
    }
    // The geometry binder tests every declared semantic before emitting,
    // including those the vertex microcode happens to leave unused.
    let mut required = 0;
    for (register, &semantic) in semantics.iter().enumerate() {
        // Position, UV, tangent frame, lighting vector and baked colour.
        let mapped = matches!((register, semantic),
            (0, 0) | (1, 3) | (2, 4) | (3, 5) | (4, 6) | (5, 8) | (6, 14));
        if !mapped {
            return Err(format!("per-vertex lighting v{register} has unsupported semantic {semantic}"));
        }
        required |= if register == 6 { crate::world::BAKED_COLOR_INPUT } else { 1 << register };
    }
    Ok(required)
}

fn material_attributes(required: u16) -> [(u32, u64, wgpu::VertexFormat); 7] {
    let mut attributes = INPUTS;
    if required & crate::world::BAKED_COLOR_INPUT != 0 {
        attributes[6] = (6, std::mem::offset_of!(WorldVertex, baked_color) as u64,
            wgpu::VertexFormat::Unorm8x4);
    }
    attributes
}

/// A material's compiled pass.
#[derive(Clone, Debug)]
pub struct MaterialProgram {
    /// The linked module (`vs_main`, `fs_main`).
    pub wgsl: String,
    /// What each stage samples, and its sampler.
    pub stages: [Stage; 4],
    pub samplers: [Option<StageStates>; 4],
    /// The texture each stage's fetch expects (`None`: not sampled).
    pub dims: [Option<TextureDim>; 4],
    /// Per stage, the channels (R, G, B, A) read as signed bytes: the
    /// image is stored biased for them ([`bias_signed`]).
    pub signed: [[bool; 4]; 4],
    /// The vertex uniforms but for the view's constants, surface and depth.
    pub vertex: VertexUniforms,
    pub pixel: PixelUniforms,
    /// Required world streams. Bits 0..6 name [`INPUTS`]; a per-vertex
    /// lighting program that reads v6 requires [`crate::world::BAKED_COLOR_INPUT`].
    pub inputs: u16,
    /// The pixel shader definition and options it was translated with.
    pub def: PixelShaderDef,
    pub pixel_options: PixelOptions,
    /// A world pass's own pipeline state ([`compile_world`]); `None`
    /// draws with the opaque world's (depth test `LessEqual` and write, no
    /// blend, no cull).
    pub state: Option<PassState>,
    /// The pass does not set `CULLMODE` and culls as the device does when
    /// layer 3 is drawn ([`crate::look::WorldPass::inherits_cull`],
    /// [`InheritedCull`]).
    pub inherits_cull: bool,
    /// The pass with its vertex program's rigid classification, which
    /// draws the material's instanced placements ([`compile_rigid`]);
    /// `None` leaves them to the D3D path.
    pub rigid: Option<RigidProgram>,
    /// A world layer-3 pass's animated constants
    /// ([`crate::look::layer3_pass_sampled`]): its uniforms hold them at
    /// clock 0, and each frame's selection samples them at the render
    /// clock ([`crate::gpu::GpuWorld::animate_selection`]). `None` for a
    /// pass that animates nothing and for the other paths' programs,
    /// which keep their animation themselves.
    pub animation: Option<crate::layer_pass::PassAnimation>,
    pub(crate) texaccum_dimensions: Vec<(tags::material::EngineVertexConstant, u8)>,
}

/// A world pass linked with its vertex program's rigid classification
/// (the same pixel shader, stages and pixel uniforms): what the D3D path
/// draws an instanced placement with, each placement's model rows in
/// `c[50..52]` per draw ([`rigid_vertex`]).
#[derive(Clone, Debug)]
pub struct RigidProgram {
    /// The linked module (`vs_main`, `fs_main`).
    pub wgsl: String,
    /// The vertex uniforms but for the view's constants and the rows.
    pub vertex: VertexUniforms,
    /// Required world streams, as in [`MaterialProgram::inputs`].
    pub inputs: u16,
    /// The program's instruction words (four per instruction, no
    /// header): what the D3D path's draw of a placement binds.
    pub words: Vec<u32>,
}

/// How a material's program compiled from an edited shader differs from
/// the one drawing (docs/editor/realtime-edits.md, "The native refresh").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramChange {
    /// Nothing the GPU holds.
    None,
    /// Only uniform values: the vertex and pixel constants, and the rigid
    /// program's vertex constants. The buffers are rewritten in place.
    Constants,
    /// The module, a stage's image or sampler, a texture's shape, the
    /// inputs, the pass state or the rigid program: the material's
    /// programs are built again.
    Program,
}

impl MaterialProgram {
    fn texaccum_stages(&self) -> [bool; 4] {
        std::array::from_fn(|n| self.dims[n].is_some() && self.stages[n] == Stage::Engine(crate::object_program::TEXACCUM_TEXTURE))
    }

    pub(crate) fn needs_texaccum(&self) -> bool {
        self.texaccum_stages().contains(&true) || !self.texaccum_dimensions.is_empty()
    }

    fn apply_texaccum_dimensions(&self, vertex: &mut VertexUniforms, size: (u32, u32)) {
        for &(d, mask) in &self.texaccum_dimensions {
            let row = if d.bank == 1 { &mut vertex.constants[18 + usize::from(d.reg)] }
                else { &mut vertex.defaults[usize::from(d.reg)] };
            let mut value = *row;
            d.eval_size(&mut value, (size.0 as f32, size.1 as f32));
            for (lane, dst) in row.iter_mut().enumerate() {
                if mask & (1 << lane) != 0 { *dst = value[lane]; }
            }
        }
    }

    fn samples_lightmap(&self) -> bool {
        self.stages.iter().zip(&self.dims).any(|(s, dim)| *s == Stage::Lightmap && dim.is_some())
    }

    /// What drawing `edited` instead of `self` takes. The pixel shader
    /// definition is not compared: its embedded constants reach the
    /// program only through the pixel uniforms, and everything else of it
    /// is in the module.
    pub fn change_to(&self, edited: &MaterialProgram) -> ProgramChange {
        let rigid_program = match (&self.rigid, &edited.rigid) {
            (None, None) => true,
            (Some(a), Some(b)) => a.wgsl == b.wgsl && a.inputs == b.inputs && a.words == b.words,
            _ => false,
        };
        let program = self.wgsl == edited.wgsl
            && self.stages == edited.stages
            && self.samplers == edited.samplers
            && self.dims == edited.dims
            && self.texaccum_dimensions == edited.texaccum_dimensions
            && self.signed == edited.signed
            && self.inputs == edited.inputs
            && self.pixel_options == edited.pixel_options
            && self.state == edited.state
            && self.inherits_cull == edited.inherits_cull
            && rigid_program;
        if !program {
            return ProgramChange::Program;
        }
        let rigid_constants = match (&self.rigid, &edited.rigid) {
            (Some(a), Some(b)) => a.vertex == b.vertex,
            _ => true,
        };
        if self.vertex == edited.vertex && self.pixel == edited.pixel && rigid_constants {
            ProgramChange::None
        } else {
            ProgramChange::Constants
        }
    }
}

/// Translates `look`'s pass with its vertex program `microcode` (the
/// world-space classification, [`crate::look::world_program`]), or says
/// why it stays on the D3D path.
pub fn compile(look: &PassLook, microcode: &[u8]) -> Result<MaterialProgram, String> {
    compile_pass(look, microcode, None)
}

/// [`compile`] with an alpha test (a transparent pass's
/// [`crate::pass_state::PassState::alpha_test`]): the fragment is killed
/// unless its alpha (in 1/255) passes `func` against `reference`.
pub fn compile_pass(
    look: &PassLook,
    microcode: &[u8],
    alpha_test: Option<(wgpu::CompareFunction, u8)>,
) -> Result<MaterialProgram, String> {
    compile_with_textures(look, microcode, alpha_test, stage_dims(look))
}

fn stage_dims(look: &PassLook) -> [Option<TextureDim>; 4] {
    look.textures
        .map(|stage| (stage != Stage::Empty).then_some(TextureDim::D2))
}

fn compile_with_textures(
    look: &PassLook,
    microcode: &[u8],
    alpha_test: Option<(wgpu::CompareFunction, u8)>,
    dims: [Option<TextureDim>; 4],
) -> Result<MaterialProgram, String> {
    if look.per_draw_inputs {
        // Resolved with the instance's inputs only; per-draw ones (object
        // functions, lighting, time) are not evaluated here.
        return Err("per-draw engine inputs".into());
    }
    let parts = PassParts {
        microcode,
        pixel_shader: &look.pixel_shader,
        dims,
        // Only a stage's own image is stored biased here.
        biased: look.textures.map(Stage::takes_image),
        stage_states: &look.stage_states,
        vertex_constants: &look.vertex_constants,
        vertex_defaults: &look.vertex_defaults,
        alpha_test,
        provided_inputs: INPUT_MASK,
        input_fetch: WORLD_FETCH,
        w_buffer: true,
        shadow: [None; 4],
    };
    let t = translate_program(&parts, psh::translate, Unsupplied::UnusedDiffuse)?;
    Ok(MaterialProgram {
        wgsl: t.wgsl,
        stages: look.textures,
        samplers: t.samplers,
        dims: t.dims,
        signed: t.signed,
        vertex: t.vertex,
        pixel: t.pixel,
        inputs: t.inputs,
        def: t.def,
        pixel_options: t.pixel_options,
        state: None,
        inherits_cull: false,
        rigid: None,
        animation: None,
        texaccum_dimensions: Vec::new(),
    })
}

/// A world material's layer-3 pass, translated for the native world, or
/// why the material stays on the D3D path. On top of [`compile`]:
///
/// - the pass's own state (alpha test, depth, colour writes, cull;
///   [`crate::look::layer3_pass`]);
/// - every vertex constant the program reads is one the native pass sets:
///   the view's (`c[0..18]`, from the device each frame), the instance's
///   (`c[18..]`), the model rows and the texture-coordinate decompression.
///   Anything else is uploaded per draw by the structure draw (`c[21]`,
///   `c[24]`, `c[31]`, `c[174]`, ...) or by the lights, and would read
///   zero here; relative addressing cannot be bounded;
/// - [`world_parity`]: nothing the D3D path takes from device state the
///   native pass does not have (fog, bump matrices, the eye, borders).
pub fn compile_world(pass: &WorldPass, microcode: &[u8]) -> Result<MaterialProgram, String> {
    compile_section(pass, microcode, stage_dims(&pass.look))
}

/// Compile with the texture shapes of this implementation's actual images.
pub fn compile_section(
    pass: &WorldPass,
    microcode: &[u8],
    dims: [Option<TextureDim>; 4],
) -> Result<MaterialProgram, String> {
    world_parity(&pass.look)?;
    world_constants_read(&pass.look, microcode)?;
    let mut p = compile_with_textures(&pass.look, microcode, pass.state.alpha_test, dims)?;
    p.texaccum_dimensions.clone_from(&pass.texaccum_dimensions);
    // The pass's polygon offset is its fragments' (`frame_pixel` keeps it).
    crate::pass_state::DepthOffset::apply(pass.state.depth_offset, &mut p.pixel);
    p.state = Some(pass.state);
    p.inherits_cull = pass.inherits_cull;
    Ok(p)
}

/// [`compile_world`]'s constants check on its own: every vertex constant
/// the pass's program reads is one the native pass sets (the view's
/// `c[0..18]`, the instance's, the model rows `c[50..52]` and the
/// decompression rows), and none is addressed relative to `a0.x`. Used
/// by the passes that compile through [`compile_pass`] directly (the fog
/// layer's, `crate::fog_gpu`).
pub fn world_constants_read(look: &PassLook, microcode: &[u8]) -> Result<(), String> {
    let program = vsh::Program::parse(microcode).map_err(|e| format!("vertex program: {e:?}"))?;
    let set = set_constants(&look.vertex_constants);
    constants_read(&program)?;
    if let Some(c) = live_unset_read(&program, &set, dead_texcoords(look)) {
        return Err(format!(
            "vertex program reads c[{c}], which the native pass does not set"
        ));
    }
    Ok(())
}

/// Explicit register reads in the active general combiners and final combiner.
/// Implicit inputs, such as the initial r0.a from texture 0, are the caller's.
pub(crate) fn combiner_reads(c: &psh::Combiners, reg: psh::Reg) -> bool {
    c.stages.iter().any(|s| {
        s.rgb.inputs.iter().chain(&s.alpha.inputs).any(|i| i.reg == reg)
    }) || c.final_combiner.as_ref().is_some_and(|f| {
        [f.a, f.b, f.c, f.d, f.e, f.f, f.g].iter().any(|i| i.reg == reg)
    })
}

/// Coordinates ignored by the fragment shader. PASSTHRU copies oTn into tn
/// without sampling. Its result matters only if the fragment program reads it.
pub(crate) fn dead_texcoords(look: &PassLook) -> [bool; 4] {
    let Ok(def) = PixelShaderDef::from_bytes(&look.pixel_shader) else {
        return [false; 4];
    };
    dead_texcoords_of(&def)
}

/// [`dead_texcoords`] of a decoded definition.
fn dead_texcoords_of(def: &PixelShaderDef) -> [bool; 4] {
    use psh::TexMode;
    let c = def.decode();
    std::array::from_fn(|n| {
        // Reflection can read other stages' interpolants directly (the
        // normal's third row and the per-vertex eye), even with no fetch.
        if c.texture_modes[2] == TexMode::DotRflctDiff
            || c.texture_modes[3] == TexMode::DotRflctSpec
        {
            return false;
        }
        match c.texture_modes[n] {
            TexMode::None => true,
            TexMode::Passthru => {
                n != 0 // r0.a starts at t0.a
                    && !combiner_reads(&c, psh::Reg::T(n as u8))
                    && !c.texture_modes.iter().enumerate().any(|(i, mode)| {
                        mode.reads_input_texture() && c.input_stage(i) == Some(n)
                    })
            }
            _ => false,
        }
    })
}

fn live_unset_read(
    program: &vsh::Program,
    set: &impl Fn(u8) -> bool,
    dead: [bool; 4],
) -> Option<u8> {
    live_source_read(
        program,
        |reg| match reg {
            vsh::SrcReg::Const { index, .. } if !set(index) => Some(index),
            _ => None,
        },
        |out| matches!(out, vsh::OutReg::T(k) if dead.get(usize::from(k)) == Some(&true)),
    )
}

/// First missing source whose value may reach a live output, address register
/// or constant write. Track a whole temporary conservatively, including after
/// a partial overwrite. Both units read the temporary bank before either writes.
pub(crate) fn live_source_read(
    program: &vsh::Program,
    missing: impl Fn(vsh::SrcReg) -> Option<u8>,
    dead_output: impl Fn(vsh::OutReg) -> bool,
) -> Option<u8> {
    use vsh::{IluOp, MacOp, OutDest, SrcReg, Unit};
    let mut tainted = [None::<u8>; 16];
    let taint = |s: &vsh::Source, tainted: &[Option<u8>; 16]| match s.reg {
        SrcReg::Temp(n) => tainted[usize::from(n) & 15],
        reg => missing(reg),
    };
    for i in &program.instructions {
        let (ra, rb, rc) = i.mac_reads();
        let first = |picks: [(bool, &vsh::Source); 3]| {
            picks
                .into_iter()
                .filter(|(used, _)| *used)
                .find_map(|(_, s)| taint(s, &tainted))
        };
        let mac = first([(ra, &i.a), (rb, &i.b), (rc, &i.c)]);
        let ilu = if i.ilu != IluOp::Nop { taint(&i.c, &tainted) } else { None };
        if i.mac == MacOp::Arl && mac.is_some() {
            return mac;
        }
        if let Some(o) = i.out {
            let value = match o.unit {
                Unit::Mac => mac,
                Unit::Ilu => ilu,
            };
            if let Some(c) = value {
                match o.dest {
                    OutDest::Output(out) if dead_output(out) => {}
                    _ => return Some(c),
                }
            }
        }
        // A temporary write to r12 writes oPos on both paths.
        if i.mac_writes_temp() && mac.is_some() {
            if i.mac_temp == 12 {
                return mac;
            }
            tainted[usize::from(i.mac_temp) & 15] = mac;
        }
        if i.ilu_writes_temp() && ilu.is_some() {
            if i.ilu_temp == 12 {
                return ilu;
            }
            tainted[usize::from(i.ilu_temp) & 15] = ilu;
        }
    }
    None
}

/// `pass` with its vertex program's rigid classification `microcode`
/// ([`crate::look::rigid_program`]), for the material's instanced
/// placements, or why they stay on the D3D path. As [`compile_world`]
/// (whose checks `pass` has passed), and the program must read the model
/// rows `c[50..52]`: the D3D path places a placement's vertices with them
/// on the GPU, and the native pass uploads the same rows per placement.
pub fn compile_rigid(pass: &WorldPass, microcode: &[u8]) -> Result<RigidProgram, String> {
    compile_pass_rigid(&pass.look, pass.state.alpha_test, microcode)
}

/// [`compile_rigid`] for any pass look (the fog layer's placements,
/// `crate::fog_gpu`): the rigid checks (the model rows `c[50..52]` read,
/// every constant one the native pass sets) and the compile with the
/// pass's own alpha test.
pub fn compile_pass_rigid(
    look: &crate::look::PassLook,
    alpha_test: Option<(wgpu::CompareFunction, u8)>,
    microcode: &[u8],
) -> Result<RigidProgram, String> {
    compile_section_rigid(look, alpha_test, microcode, stage_dims(look))
}

pub fn compile_section_rigid(
    look: &PassLook,
    alpha_test: Option<(wgpu::CompareFunction, u8)>,
    microcode: &[u8],
    dims: [Option<TextureDim>; 4],
) -> Result<RigidProgram, String> {
    let program =
        vsh::Program::parse(microcode).map_err(|e| format!("rigid vertex program: {e:?}"))?;
    let read = constants_read(&program)?;
    if !(MODEL_ROWS..MODEL_ROWS + 3).all(|c| read.contains(&(c as u8))) {
        return Err("the rigid vertex program does not read the model rows c[50..52]".into());
    }
    let set = set_constants(&look.vertex_constants);
    if let Some(c) = live_unset_read(&program, &set, dead_texcoords(look)) {
        return Err(format!(
            "rigid vertex program reads c[{c}], which the native pass does not set"
        ));
    }
    let p = compile_with_textures(look, microcode, alpha_test, dims)?;
    let words = microcode[4..4 + 16 * program.instructions.len()]
        .chunks_exact(4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    Ok(RigidProgram {
        wgsl: p.wgsl,
        vertex: p.vertex,
        inputs: p.inputs,
        words,
    })
}

/// Whether every instanced-placement part of `material` in `world` can be
/// drawn with `rigid`: each supplies the registers it reads, and its
/// placement's rows were read at load ([`World::rows`]).
pub fn rigid_fit(world: &World, material: u16, rigid: &RigidProgram) -> Result<(), String> {
    for d in world.draws.iter().filter(|d| d.material == material) {
        let Owner::Instance(i) = d.owner else {
            continue;
        };
        let missing = rigid.inputs & !d.inputs;
        if missing != 0 {
            return Err(format!(
                "a placement's part has no v{}",
                missing.trailing_zeros()
            ));
        }
        if world.rows(i).is_none() {
            return Err("a placement's rows did not read".into());
        }
    }
    Ok(())
}

/// The rigid program's vertex uniforms for one placement in one frame:
/// [`frame_vertex`], then the placement's rows in `c[50..52]` as the
/// structure draw uploads them before the draw (`0x0004d830`).
pub fn rigid_vertex(base: &VertexUniforms, view: &ViewInputs, rows: &ModelRows) -> VertexUniforms {
    let mut u = frame_vertex(base, view);
    u.constants[MODEL_ROWS..MODEL_ROWS + 3].copy_from_slice(rows);
    u
}

/// Whether the native world sets constant `c` (hardware numbering) for a
/// pass with instance constants `instance`.
pub(crate) fn set_constants(instance: &[Register]) -> impl Fn(u8) -> bool + '_ {
    move |c| {
        let c = usize::from(c);
        c < VIEW_CONSTANTS
            || (MODEL_ROWS..MODEL_ROWS + 3).contains(&c)
            || (POSITION_DECOMPRESSION..POSITION_DECOMPRESSION + 2).contains(&c)
            || c == TEXCOORD_DECOMPRESSION
            || instance
                .iter()
                .any(|r| usize::from(r.index) == c && r.written != 0)
    }
}

/// The constant registers (hardware numbering) `program` reads, sorted;
/// an error when one is addressed relative to `a0.x`.
pub fn constants_read(program: &vsh::Program) -> Result<Vec<u8>, String> {
    let mut read = Vec::new();
    for i in &program.instructions {
        for s in i.sources() {
            if let vsh::SrcReg::Const { index, relative } = s.reg {
                if relative {
                    return Err("vertex program addresses constants relative to a0.x".into());
                }
                read.push(index);
            }
        }
    }
    read.sort_unstable();
    read.dedup();
    Ok(read)
}

/// What the D3D path takes from the device's state at a world draw where
/// the native pass has nothing to match it with; a pass that depends on
/// one stays on the D3D path:
///
/// - **Fog.** Any combiner reading the fog register: its colour is
///   `SET_FOG_COLOR` and its factor the fog unit's, and the pass does not
///   set `FOGENABLE`, so both are whatever the device holds. A definition
///   without a final combiner as well: D3D then programs one from the fog
///   and specular states (`PixelShaderDef::with_default_final_combiner`).
///   The world's layer-3 passes on 03a program their own final combiner
///   and read no fog (the world is fogged by a later layer), so this
///   refuses none of them.
/// - **Texture modes** that read device state the native pass does not
///   have: the bump-environment matrices (`D3DTSS_BUMPENV*`), the eye of
///   the reflection modes, BRDF.
/// - **Border addressing** on a sampled stage (the D3D path may supply
///   the border in the shader).
pub fn world_parity(look: &PassLook) -> Result<(), String> {
    let def = PixelShaderDef::from_bytes(&look.pixel_shader)
        .map_err(|e| format!("pixel shader: {e:?}"))?;
    pixel_parity(
        &def,
        &look.stage_states,
        look.textures.map(|s| s != Stage::Empty),
    )
}

/// Pixel dependencies that neither the world nor the light pass supplies.
pub(crate) fn pixel_parity(
    def: &PixelShaderDef,
    stages: &[StageStates],
    sampled: [bool; 4],
) -> Result<(), String> {
    let c = def.decode();
    let Some(f) = c.final_combiner else {
        return Err("no final combiner (D3D builds one from the fog and specular states)".into());
    };
    let fog = c
        .stages
        .iter()
        .flat_map(|s| s.rgb.inputs.iter().chain(&s.alpha.inputs))
        .chain(&[f.a, f.b, f.c, f.d, f.e, f.f, f.g])
        .any(|i| i.reg == psh::Reg::Fog);
    if fog {
        return Err("reads the fog register".into());
    }
    for (n, m) in c.texture_modes.iter().enumerate() {
        use psh::TexMode as M;
        if matches!(
            m,
            M::BumpEnvMap
                | M::BumpEnvMapLum
                | M::Brdf
                | M::DotRflctDiff
                | M::DotRflctSpec
                | M::DotRflctSpecConst
        ) {
            return Err(format!("stage {n} texture mode {m:?} reads device state"));
        }
    }
    for (n, s) in stages.iter().enumerate().take(4) {
        if sampled[n] && [s.address_u, s.address_v, s.address_w].contains(&ADDRESS_BORDER) {
            return Err(format!("stage {n} addresses the border"));
        }
    }
    Ok(())
}

/// `D3DTADDRESS_BORDER`.
const ADDRESS_BORDER: u8 = 4;

/// Whether every part of `material` in `world` can be drawn with
/// `program`, or why not: each part's section supplies every vertex
/// register the program reads (a missing one would read zero where the
/// D3D path streams the real data or binds another implementation), and
/// where the program samples the lightmap, each part has a page and
/// `page` reports whether it decoded. One part that cannot be drawn keeps
/// the whole material on the D3D path.
pub fn world_fit(
    world: &World,
    material: u16,
    program: &MaterialProgram,
    page: &dyn Fn(LightmapKey) -> Result<(), String>,
) -> Result<(), String> {
    for d in world.draws.iter().filter(|d| d.material == material) {
        part_fit(d, program.inputs, program, page)?;
    }
    Ok(())
}

/// Validate this part against the vertex classification that will draw
/// it. Two sections can use the same material with different streams.
pub fn part_fit(
    d: &WorldDraw,
    inputs: u16,
    program: &MaterialProgram,
    page: &dyn Fn(LightmapKey) -> Result<(), String>,
) -> Result<(), String> {
    const NAMES: [&str; 7] = [
        "positions",
        "texture coordinates",
        "normals",
        "binormals",
        "tangents",
        "lighting vectors",
        "lightmap coordinates",
    ];
    let missing = inputs & !d.inputs;
    if missing == crate::world::BAKED_COLOR_INPUT {
        return Err("a part has no baked vertex colours (v6)".into());
    }
    if missing != 0 {
        let n = missing.trailing_zeros() as usize;
        return Err(format!(
            "a part has no {} (v{n})",
            NAMES.get(n).unwrap_or(&"stream")
        ));
    }
    if program.samples_lightmap() {
        page(d.lightmap.ok_or("a part has no lightmap page")?)?;
    }
    Ok(())
}

/// What [`translate_pass`] translates: one implementation's programs,
/// what its stages hold and its instance constants.
pub(crate) struct PassParts<'a> {
    pub microcode: &'a [u8],
    pub pixel_shader: &'a [u8; tags::shader::PSDEF_SIZE],
    /// The texture each stage has bound (`None`: nothing).
    pub dims: [Option<TextureDim>; 4],
    /// Stages whose texture can be stored biased for signed channels.
    pub biased: [bool; 4],
    pub stage_states: &'a [StageStates],
    pub vertex_constants: &'a [Register],
    pub vertex_defaults: &'a [Register],
    pub alpha_test: Option<(wgpu::CompareFunction, u8)>,
    /// The `v` registers the vertex buffer supplies (bit `n`: `v<n>`);
    /// [`INPUT_MASK`] for a world vertex.
    pub provided_inputs: u16,
    /// How each supplied register arrives ([`WORLD_FETCH`] for a material
    /// program's world vertex).
    pub input_fetch: [InputFetch; 16],
    /// The fragment writes the W-buffered depth (the world's passes); a
    /// colour-only screen pass (`ZENABLE` 0) writes none.
    pub w_buffer: bool,
    /// Stages that compare a frozen depth map instead of sampling colour.
    pub shadow: [Option<psh::shadow::Shadow>; 4],
}

/// A translated pass.
pub(crate) struct Translated {
    pub wgsl: String,
    pub vertex_shader: vsh::wgsl::VertexShader,
    pub samplers: [Option<StageStates>; 4],
    pub dims: [Option<TextureDim>; 4],
    pub signed: [[bool; 4]; 4],
    pub vertex: VertexUniforms,
    pub pixel: PixelUniforms,
    /// `v<n>` read from the vertex stream (within `provided_inputs`).
    pub inputs: u16,
    pub def: PixelShaderDef,
    pub pixel_options: PixelOptions,
}

/// The vertex program and pixel shader through `shaders`, and the uniforms
/// with the instance's constants; or why the pass stays on the D3D path.
pub(crate) fn translate_pass(p: &PassParts<'_>) -> Result<Translated, String> {
    translate_pass_with_pixel(p, psh::translate)
}

pub(crate) fn translate_pass_with_pixel(
    p: &PassParts<'_>,
    translate_pixel: fn(&PixelShaderDef, &PixelOptions) -> psh::PixelShader,
) -> Result<Translated, String> {
    translate_program(p, translate_pixel, Unsupplied::Refused)
}

/// [`translate_pass_with_pixel`] for an object pass: a vertex register the
/// streams do not supply may be read where its value reaches only outputs
/// the pixel shader ignores ([`only_dead_outputs`]).
pub(crate) fn translate_object_pass(
    p: &PassParts<'_>,
    translate_pixel: fn(&PixelShaderDef, &PixelOptions) -> psh::PixelShader,
) -> Result<Translated, String> {
    translate_program(p, translate_pixel, Unsupplied::DeadOutputs)
}

/// Where a vertex program may read a register the streams do not supply.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Unsupplied {
    Refused,
    /// Into a diffuse colour the pixel shader does not read ([`only_unused_diffuse`]).
    UnusedDiffuse,
    /// Into the diffuse colour or texture coordinates the pixel shader
    /// does not read ([`only_dead_outputs`]).
    DeadOutputs,
}

fn translate_program(
    p: &PassParts<'_>,
    translate_pixel: fn(&PixelShaderDef, &PixelOptions) -> psh::PixelShader,
    allowed: Unsupplied,
) -> Result<Translated, String> {
    let program = vsh::Program::parse(p.microcode).map_err(|e| format!("vertex program: {e:?}"))?;
    let unsupplied = program.inputs_read() & !p.provided_inputs & !defaults_mask(p.vertex_defaults);
    let def =
        PixelShaderDef::from_bytes(p.pixel_shader).map_err(|e| format!("pixel shader: {e:?}"))?;
    let dead = match allowed {
        Unsupplied::Refused => false,
        Unsupplied::UnusedDiffuse => only_unused_diffuse(&program, unsupplied, &def),
        Unsupplied::DeadOutputs => only_dead_outputs(&program, unsupplied, &def),
    };
    if unsupplied != 0 && !dead {
        return Err(format!(
            "vertex program reads registers we do not supply ({unsupplied:#06x})"
        ));
    }
    let vs = vsh::translate(
        &program,
        &VertexOptions {
            provided_inputs: p.provided_inputs,
            input_fetch: p.input_fetch,
            ..Default::default()
        },
    )
    .map_err(|e| format!("vertex program: {e:?}"))?;
    let samplers: [Option<StageStates>; 4] =
        std::array::from_fn(|n| p.stage_states.get(n).copied());
    let mut options = PixelOptions {
        texture_dims: p.dims,
        // The game's world is W-buffered (docs/graphics/native-renderer.md).
        w_buffer: p.w_buffer,
        alpha_test: p.alpha_test.map(|(f, _)| alpha_func(f)),
        shadow: p.shadow,
        ..Default::default()
    };
    let mut signed = [[false; 4]; 4];
    for (n, s) in samplers.iter().enumerate() {
        let Some(s) = s else { continue };
        let sign = psh::bits::color_sign(s.color_sign);
        if sign != 0 {
            if !p.biased[n] {
                return Err(format!(
                    "stage {n} reads signed channels of a texture not stored biased"
                ));
            }
            options.color_sign[n] = sign;
            signed[n] = signed_channels(s.color_sign);
        }
        options.alpha_kill[n] = s.alpha_kill != 0;
    }
    let ps = translate_pixel(&def, &options);
    if !ps.warnings.is_empty() {
        return Err(format!("pixel shader: {}", ps.warnings.join("; ")));
    }
    for (n, d) in ps.textures.iter().enumerate() {
        match (d, p.dims[n]) {
            (None, _) => {}
            (Some(_), None) => return Err(format!("stage {n} is sampled with nothing bound")),
            (Some(d), Some(bound)) if *d != bound => {
                return Err(format!("stage {n} is fetched as {d:?}, bound {bound:?}"));
            }
            _ => {}
        }
    }
    let wgsl = shaders::link(&vs, &ps).map_err(|e| e.to_string())?;
    let mut vertex = VertexUniforms::default();
    vertex.constants[TEXCOORD_DECOMPRESSION] = [1.0, 1.0, 0.0, 0.0];
    vertex.constants[POSITION_DECOMPRESSION] = [1.0; 4];
    vertex.constants[POSITION_DECOMPRESSION + 1] = [0.0; 4];
    // The vertices are already placed: the model rows are the identity
    // (clusters never upload them; a program that reads them gets these).
    vertex.constants[MODEL_ROWS..MODEL_ROWS + 3].copy_from_slice(&[
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
    ]);
    for r in p.vertex_constants {
        put(&mut vertex.constants, r);
    }
    for r in p.vertex_defaults {
        put(&mut vertex.defaults, r);
    }
    let (c0, c1) = def.embedded_constants();
    // `D3DTSS_MIPMAPLODBIAS`, applied in the shader as the D3D path does
    // (a wgpu sampler has no bias).
    let mut pixel = PixelUniforms {
        c0,
        c1,
        lod_bias: lod_bias(samplers),
        ..Default::default()
    };
    if let Some((_, reference)) = p.alpha_test {
        // Compared with the alpha rounded to 1/255 (`xps.alpha_ref.x`).
        pixel.alpha_ref = [f32::from(reference), 0.0, 0.0, 0.0];
    }
    let inputs = program.inputs_read() & p.provided_inputs;
    Ok(Translated {
        wgsl,
        vertex_shader: vs,
        samplers,
        dims: ps.textures,
        signed,
        vertex,
        pixel,
        inputs,
        def,
        pixel_options: options,
    })
}

/// The layer-1 vertex program can forward an unstreamed immediate register
/// to oD0 even when the pixel program only samples textures. For ordinary
/// projected fetches the texture units read oT, and only a combiner can
/// consume diffuse. A missing input reaching any other output stays live.
fn only_unused_diffuse(program: &vsh::Program, missing: u16, def: &PixelShaderDef) -> bool {
    diffuse_unread(def)
        && live_source_read(
            program,
            |reg| match reg {
                vsh::SrcReg::Input(n) if missing & (1 << n) != 0 => Some(n),
                _ => None,
            },
            |out| out == vsh::OutReg::D0,
        ).is_none()
}

/// Whether the pixel shader leaves the diffuse colour unread: an explicit
/// final combiner, stages that fetch plainly or not at all, and no
/// combiner input of `v0`.
fn diffuse_unread(def: &PixelShaderDef) -> bool {
    let c = def.decode();
    c.final_combiner.is_some()
        && c.texture_modes.iter().all(|m| {
            matches!(m, psh::TexMode::None | psh::TexMode::Project2d)
        })
        && !combiner_reads(&c, psh::Reg::V0)
}

/// Whether the `missing` registers reach only outputs the pixel shader
/// ignores: the diffuse colour where [`diffuse_unread`], and the
/// coordinates of [`dead_texcoords`]. The stencil and transparent object
/// programs copy the vertex colours `v10`, `v11` into `oD0` whether or not
/// the pass reads it, and the object streams supply neither.
fn only_dead_outputs(program: &vsh::Program, missing: u16, def: &PixelShaderDef) -> bool {
    let diffuse = diffuse_unread(def);
    let dead = dead_texcoords_of(def);
    live_source_read(
        program,
        |reg| match reg {
            vsh::SrcReg::Input(n) if missing & (1 << n) != 0 => Some(n),
            _ => None,
        },
        |out| match out {
            vsh::OutReg::D0 => diffuse,
            vsh::OutReg::T(k) => dead.get(usize::from(k)) == Some(&true),
            _ => false,
        },
    ).is_none()
}

/// Each stage's `D3DTSS_MIPMAPLODBIAS` (f32 bits), zero when not finite,
/// as the guest passes it (the shader clamps it to the NV2A's range).
pub fn lod_bias(samplers: [Option<StageStates>; 4]) -> [f32; 4] {
    samplers.map(|s| {
        let b = s.map_or(0.0, |s| f32::from_bits(s.mip_lod_bias));
        if b.is_finite() { b } else { 0.0 }
    })
}

pub(crate) fn alpha_func(f: wgpu::CompareFunction) -> psh::AlphaFunc {
    use wgpu::CompareFunction as C;
    match f {
        C::Never => psh::AlphaFunc::Never,
        C::Less => psh::AlphaFunc::Less,
        C::Equal => psh::AlphaFunc::Equal,
        C::LessEqual => psh::AlphaFunc::LessEqual,
        C::Greater => psh::AlphaFunc::Greater,
        C::NotEqual => psh::AlphaFunc::NotEqual,
        C::GreaterEqual => psh::AlphaFunc::GreaterEqual,
        C::Always => psh::AlphaFunc::Always,
    }
}

/// The channels (R, G, B, A) a `D3DTSS_COLORSIGN` value reads as signed
/// (`D3DTSIGN_ASIGNED` is bit 28, `RSIGNED` 29, `GSIGNED` 30, `BSIGNED` 31;
/// as `gfx::texture::DecodeOptions::with_color_sign`).
pub fn signed_channels(color_sign: u32) -> [bool; 4] {
    let bit = |n: u32| color_sign >> n & 1 != 0;
    [bit(29), bit(30), bit(31), bit(28)]
}

/// A cull mode and the winding that faces front, as `gfx`'s
/// `primitive_state` gives them for `CULLMODE` and `FRONTFACE`.
pub type Cull = (Option<wgpu::Face>, wgpu::FrontFace);

/// The cull the device holds when the native world pass is asked for (the
/// layer-3 call), for the passes that do not set `CULLMODE` and inherit it
/// ([`MaterialProgram::inherits_cull`]), and which draws take it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct InheritedCull {
    /// `None`: not known; such passes cull nothing.
    pub device: Option<Cull>,
    /// The placements' rigid draws take it (as the D3D path's draws of a
    /// placement do).
    pub placements: bool,
    /// The clusters' draws take it (03a's 02000: 81 cluster pixels on a
    /// curb went with it, nothing else changed).
    pub clusters: bool,
}

impl InheritedCull {
    /// The cull a program's draws of a cluster (`cluster`) or placement
    /// take: the device's where the program inherits it and those draws
    /// take it, otherwise its pass's own (`None`).
    pub fn of(&self, inherits: bool, cluster: bool) -> Option<Cull> {
        let takes = if cluster {
            self.clusters
        } else {
            self.placements
        };
        if inherits && takes { self.device } else { None }
    }
}

/// The primitive and the depth write and compare a program's pipeline
/// draws with ([`described_pipeline`]): its pass's state, or the opaque
/// world's (a triangle list, no cull, `LessEqual` and write), with `cull`
/// over its cull and winding ([`InheritedCull::of`]). No depth bias: the
/// polygon offset is the fragment's (`PixelUniforms::depth`).
pub fn pipeline_state(
    state: Option<&PassState>,
    cull: Option<Cull>,
) -> (wgpu::PrimitiveState, (bool, wgpu::CompareFunction)) {
    let mut primitive = state.map_or(
        wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        |s| s.primitive,
    );
    if let Some((cull_mode, front_face)) = cull {
        primitive.cull_mode = cull_mode;
        primitive.front_face = front_face;
    }
    let depth = state.map_or((true, wgpu::CompareFunction::LessEqual), |s| {
        (s.depth_write, s.depth_compare)
    });
    (primitive, depth)
}

/// `cull` (over `state`'s own, as [`pipeline_state`] takes them) with the
/// winding that faces front reversed, for a draw whose triangles wind the
/// other way round than the native index buffer holds them: an odd-start
/// sub-strip of a visible subpart (`crate::subpart_map`). A pass that
/// culls nothing keeps `cull`.
pub fn reversed_winding(state: Option<&PassState>, cull: Option<Cull>) -> Option<Cull> {
    let (primitive, _) = pipeline_state(state, cull);
    let Some(face) = primitive.cull_mode else { return cull };
    let front = match primitive.front_face {
        wgpu::FrontFace::Ccw => wgpu::FrontFace::Cw,
        wgpu::FrontFace::Cw => wgpu::FrontFace::Ccw,
    };
    Some((Some(face), front))
}

/// What an image is uploaded as on a device that takes BC textures
/// (`bc`) or not ([`crate::gpu::texture`]): format, size, levels and an
/// FNV-1a hash of level 0's bytes.
pub fn describe_image(image: &Image, bc: bool) -> String {
    let (format, level0, levels) = match &image.blocks {
        Some(b) if bc => (format!("{:?}", b.format), &b.levels[0], b.levels.len()),
        _ => ("Rgba8Unorm".to_string(), &image.rgba, 1 + image.mips.len()),
    };
    let hash = level0.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    format!(
        "{format} {}x{} levels {levels} level 0 {hash:016x}",
        image.width, image.height
    )
}

/// Stores the signed channels of an RGBA8 image as `s + 128`, so that
/// filtering interpolates signed values (`shaders::psh::PixelOptions::color_sign`).
pub fn bias_signed(image: &mut Image, signed: [bool; 4]) {
    if signed.iter().any(|&s| s) {
        // BC blocks no longer represent the biased channels.
        image.edit_rgba(|rgba, mips| bias_levels(rgba, mips, signed));
    }
}

fn bias_levels(rgba: &mut [u8], mips: &mut [Vec<u8>], signed: [bool; 4]) {
    let flip = signed.map(|s| if s { 0x80 } else { 0 });
    for level in std::iter::once(rgba).chain(mips.iter_mut().map(Vec::as_mut_slice)) {
        for px in level.chunks_exact_mut(4) {
            for (b, f) in px.iter_mut().zip(flip) {
                *b ^= f;
            }
        }
    }
}

/// Bit `n` for each `v<n>` the pass itself sets before its draws
/// (`SET_VERTEX_DATA4F_M`, `0x00018e3b`, a whole register): the D3D path
/// reads those for any input the vertex layout does not stream. The
/// translated program reads them from `xvs.defaults[n]`, which
/// [`compile_pass`] fills from the same registers; the inputs a
/// [`WorldVertex`] supplies still come from the vertex.
pub fn defaults_mask(defaults: &[Register]) -> u16 {
    defaults
        .iter()
        .filter(|r| r.index < 16 && r.written == 0xf)
        .fold(0, |m, r| m | 1 << r.index)
}

/// Writes the components `r` writes into its register of `regs`.
pub(crate) fn put(regs: &mut [[f32; 4]], r: &Register) {
    let Some(reg) = regs.get_mut(usize::from(r.index)) else {
        return;
    };
    for k in 0..4 {
        if r.written & (1 << k) != 0 {
            reg[k] = r.value[k];
        }
    }
}

/// One frame's view of the target: the constants the device gave the pass
/// and the target's sizes and depth scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewInputs {
    pub constants: [[f32; 4]; VIEW_CONSTANTS],
    /// Guest pixels and this copy's texels.
    pub guest_size: (u32, u32),
    pub size: (u32, u32),
    pub z_scale: f32,
    /// The W-buffer's clip range in depth units and whether out-of-range
    /// depth is clamped rather than culled, as the device holds them when
    /// the pass is issued (`SET_CLIP_MIN`/`MAX`,
    /// `D3DRS_DEPTHCLIPCONTROL`), what the D3D path draws the layer's
    /// draws with. `None`: the whole range, culled.
    pub w_clip: Option<([f32; 2], bool)>,
}

impl ViewInputs {
    /// The view a game-free host (the editor) gives the translated passes
    /// for `camera` drawing a `size` target at the guest's own size, in
    /// the layout the game uploads before a view's draws
    /// (docs/graphics/native-world.md, "What a world pass reads"):
    ///
    /// - `c[0..3]`: the view-projection rows (`dph` with the world
    ///   position), scaled by `zs / far` so the clip `w` is the view depth
    ///   in W-buffer units, as the game's own (docs/graphics/
    ///   occlusion-culling.md, "The W scale"; `zs` D24's full scale). The
    ///   z row is the w row: W-buffered, the depth is `w` and z only
    ///   clips, from `w` (`shaders::vsh` `xvs_fixup`).
    /// - `c[5].w` 0.5, the scale that packs a unit vector into a colour.
    /// - `c[10]`, `c[11]`: the viewport scale and offset from clip x, y
    ///   over w to guest pixels, y down.
    /// - The rest 0 and not given ([`CAMERA_GIVEN`]): `c[4]`, `c[5].xyz`,
    ///   `c[6]` (a world-space vector the game sets per view), `c[7..9]`,
    ///   the `w` of `c[10]` and `c[11]`, and the block at `c[12..17]`
    ///   (`0x00025c70`) are not known well enough to synthesise. A pass
    ///   whose vertex program reads one is refused
    ///   ([`ungiven_view_read`]), not drawn with the zeros.
    ///
    /// The clip range is the game's, `[near / far, 1] · zs`, culled.
    pub fn from_camera(camera: &crate::camera::RenderCamera, size: (u32, u32)) -> Self {
        let zs = crate::camera::D24_SCALE;
        let vp = camera.view_projection();
        let s = zs / camera.far;
        // `vp` is by columns: clip row `i` is `vp[j][i]` over `j`.
        let row = |i: usize| std::array::from_fn(|j| vp[j][i] * s);
        let mut constants = [[0.0; 4]; VIEW_CONSTANTS];
        constants[0] = row(0);
        constants[1] = row(1);
        constants[2] = row(3);
        constants[3] = row(3);
        constants[5] = [0.0, 0.0, 0.0, 0.5];
        let (w, h) = (size.0 as f32, size.1 as f32);
        constants[10] = [w * 0.5, -h * 0.5, 0.0, 0.0];
        constants[11] = [w * 0.5, h * 0.5, 0.0, 0.0];
        let near = (camera.near / camera.far * zs).clamp(0.0, zs);
        Self {
            constants,
            guest_size: size,
            size,
            z_scale: zs,
            w_clip: Some(([near, zs], false)),
        }
    }
}

/// The view constants' components [`ViewInputs::from_camera`] gives, as
/// a mask per register (bit `k` component `k`, x first): all of
/// `c[0..3]`; `c[5].w`; `c[10]` and `c[11]` but `w`. The z lanes there
/// reach only `oPos.z`, which a W-buffered pass does not use
/// (`shaders::vsh` `xvs_fixup`).
pub const CAMERA_GIVEN: [u8; VIEW_CONSTANTS] = {
    let mut given = [0u8; VIEW_CONSTANTS];
    given[0] = 0xf;
    given[1] = 0xf;
    given[2] = 0xf;
    given[3] = 0xf;
    given[5] = 0b1000;
    given[10] = 0b0111;
    given[11] = 0b0111;
    given
};

/// The first view constant component (register, component) `microcode`
/// reads that `given` does not give, as a refusal; `None` when it reads
/// only given ones. Every lane a source's swizzle selects counts as
/// read, whatever the write mask keeps, and a relative read based inside
/// the view's registers reads all of them: a pass is refused rather than
/// drawn from a value nobody gave.
pub fn ungiven_view_read(microcode: &[u8], given: &[u8; VIEW_CONSTANTS]) -> Option<String> {
    let program = match vsh::Program::parse(microcode) {
        Ok(p) => p,
        Err(e) => return Some(format!("vertex program: {e:?}")),
    };
    for i in &program.instructions {
        for src in i.sources() {
            let vsh::SrcReg::Const { index, relative } = src.reg else {
                continue;
            };
            let Some(&mask) = given.get(usize::from(index)) else {
                continue;
            };
            if relative {
                return Some(format!(
                    "vertex program reads c[a0.x + {index}], inside the view's constants"
                ));
            }
            if let Some(&k) = src.swizzle.iter().find(|&&k| mask >> k & 1 == 0) {
                let lane = ["x", "y", "z", "w"][usize::from(k & 3)];
                return Some(format!(
                    "vertex program reads c[{index}].{lane}, which the view does not give"
                ));
            }
        }
    }
    None
}

/// `base` with the view's constants, surface and depth for one frame.
pub fn frame_vertex(base: &VertexUniforms, view: &ViewInputs) -> VertexUniforms {
    let mut u = base.clone();
    u.constants[..VIEW_CONSTANTS].copy_from_slice(&view.constants);
    u.surface = [view.guest_size.0 as f32, view.guest_size.1 as f32];
    u.z_max = view.z_scale;
    let (range, clamp) = view.w_clip.unwrap_or(([0.0, view.z_scale], false));
    u.w_clip = Some(range);
    u.w_clamp = clamp;
    u.shift = target_shift(view.guest_size, view.size);
    u
}

/// Sample offset in guest pixels for each axis of this attachment copy.
pub(crate) fn target_shift(guest: (u32, u32), host: (u32, u32)) -> [f32; 2] {
    [
        gfx::renderer::window_shift(guest.0, host.0),
        gfx::renderer::window_shift(guest.1, host.1),
    ]
}

/// Clip a resolved draw's guest rectangle to this target copy in host pixels.
pub(crate) fn scaled_scissor(
    clip: &gfx::external::PixelRect,
    scale: crate::render_scale::RenderScale,
    extent: (u32, u32),
) -> Option<gfx::ScissorRect> {
    let [x, y, width, height] = scale.rect(clip.x, clip.y, clip.width, clip.height);
    let rect = gfx::ScissorRect {
        x,
        y,
        width,
        height,
    }
    .intersect(gfx::ScissorRect::full(extent.0, extent.1));
    (!rect.is_empty()).then_some(rect)
}

/// `base` with the frame's depth and raster scales.
pub fn frame_pixel(base: &PixelUniforms, view: &ViewInputs) -> PixelUniforms {
    PixelUniforms {
        // y and z: the pass's polygon offset (`pass_state::DepthOffset`).
        depth: [view.z_scale, base.depth[1], base.depth[2], 1.0],
        depth_range: view.w_clip.map_or([0.0, view.z_scale], |(range, _)| range),
        stipple_scale: PixelUniforms::stipple_grid(view.guest_size, view.size),
        ..*base
    }
}

// Byte ranges in the shader uniform ABI. The rest of each block is constant
// for the material (or, for rigid vertices, the material and placement).
const VIEW_PREFIX: usize = VIEW_CONSTANTS * 16;
// The viewport and W-clip vectors follow the two register banks. A
// per-draw UI transform follows them and is independent of the world view.
const VERTEX_VIEW: usize = (vsh::CONSTANT_COUNT + vsh::INPUT_COUNT) * 16;
const PIXEL_TAIL: usize = PixelUniforms::SIZE - 32;

fn view_bytes(view: &ViewInputs) -> [u8; VIEW_PREFIX + 64] {
    let mut bytes = [0; VIEW_PREFIX + 64];
    let mut vertex = [0; VertexUniforms::SIZE];
    frame_vertex(&VertexUniforms::default(), view).write_bytes(&mut vertex);
    let mut pixel = [0; PixelUniforms::SIZE];
    frame_pixel(&PixelUniforms::default(), view).write_bytes(&mut pixel);
    bytes[..VIEW_PREFIX].copy_from_slice(&vertex[..VIEW_PREFIX]);
    bytes[VIEW_PREFIX..VIEW_PREFIX + 32].copy_from_slice(&vertex[VERTEX_VIEW..VERTEX_VIEW + 32]);
    bytes[VIEW_PREFIX + 32..].copy_from_slice(&pixel[PIXEL_TAIL..]);
    bytes
}

/// The wgpu sampler for a stage's states, made as the D3D path makes it
/// (`gfx::translate::SamplerKey::from_stage`: anisotropic filtering,
/// `MAXMIPLEVEL` as the smallest LOD, no mip filter as level 0 only;
/// border addressing clamps to the edge). States `gfx` does not decode
/// fall back to the XDK values read directly (address 1 wrap, 2 mirror,
/// 3 clamp, 4 border, 5 clamp to edge; filter 1 point, else linear; mip
/// filter 0 none, 1 point, 2 linear).
pub fn sampler_descriptor(s: Option<&StageStates>) -> wgpu::SamplerDescriptor<'static> {
    if let Some(k) = s.and_then(sampler_key) {
        let mut d = k.descriptor();
        d.label = Some("native world program");
        return d;
    }
    let address = |a: u8| match a {
        2 => wgpu::AddressMode::MirrorRepeat,
        3..=5 => wgpu::AddressMode::ClampToEdge,
        _ => wgpu::AddressMode::Repeat,
    };
    let filter = |f: u8| {
        if f == 1 {
            wgpu::FilterMode::Nearest
        } else {
            wgpu::FilterMode::Linear
        }
    };
    let Some(s) = s else {
        return wgpu::SamplerDescriptor {
            label: Some("native world program"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        };
    };
    wgpu::SamplerDescriptor {
        label: Some("native world program"),
        address_mode_u: address(s.address_u),
        address_mode_v: address(s.address_v),
        address_mode_w: address(s.address_w),
        mag_filter: filter(s.mag_filter),
        min_filter: filter(s.min_filter),
        mipmap_filter: match s.mip_filter {
            2 => wgpu::MipmapFilterMode::Linear,
            _ => wgpu::MipmapFilterMode::Nearest,
        },
        lod_max_clamp: if s.mip_filter == 0 { 0.0 } else { 32.0 },
        ..Default::default()
    }
}

/// The sampler a stage's states make, as the D3D path keys it
/// (`gfx::translate::SamplerKey::from_stage`), `None` when one does not
/// decode.
pub fn sampler_key(s: &StageStates) -> Option<gfx::translate::SamplerKey> {
    let caps = gfx::translate::Capabilities::from_features(wgpu::Features::empty());
    stage_state(s).map(|t| gfx::translate::SamplerKey::from_stage(&t, caps))
}

/// A stage's sampler states as `gfx` holds them, `None` when one does not
/// decode.
fn stage_state(s: &StageStates) -> Option<gfx::state::TextureStageState> {
    use gfx::state::{TextureAddress as A, TextureFilter as F};
    let mut t = gfx::state::TextureStageState::default_for_stage(0);
    t.address_u = A::from_xbox(u32::from(s.address_u))?;
    t.address_v = A::from_xbox(u32::from(s.address_v))?;
    t.address_w = A::from_xbox(u32::from(s.address_w))?;
    t.mag_filter = F::from_xbox(u32::from(s.mag_filter))?;
    t.min_filter = F::from_xbox(u32::from(s.min_filter))?;
    t.mip_filter = F::from_xbox(u32::from(s.mip_filter))?;
    t.mip_lod_bias = f32::from_bits(s.mip_lod_bias);
    t.max_mip_level = u32::from(s.max_mip_level);
    t.max_anisotropy = u32::from(s.max_anisotropy).max(1);
    t.border_color = s.border_color;
    Some(t)
}

/// One compiled program's GPU side: its module, uniforms, samplers, stage
/// textures and pipelines (by target formats).
pub(crate) struct Gpu {
    pub(crate) program: MaterialProgram,
    pub(crate) pipelines: WorldPipelines,
    textures_layout: wgpu::BindGroupLayout,
    vertex_buffer: wgpu::Buffer,
    pixel_buffer: wgpu::Buffer,
    pub(crate) uniforms: wgpu::BindGroup,
    samplers: [wgpu::Sampler; 4],
    stage_views: [Option<wgpu::TextureView>; 4],
}

/// A world or rigid pipeline's key: target formats, stencil state and the
/// inherited cull it was made with ([`InheritedCull::of`]).
type PipelineKey = (
    wgpu::TextureFormat,
    wgpu::TextureFormat,
    wgpu::StencilState,
    Option<Cull>,
);

/// A direct render (for example the Theater camera) needs its pipeline now.
/// The live guest can draw with D3D while an admission check is pending.
#[derive(Clone, Copy)]
pub(crate) enum PipelineUse {
    Direct,
    Preflight,
}

/// One program and its immutable vertex layout, scoped to this map's GPU resources.
/// The small key avoids rebuilding a vertex descriptor for an already-seen draw.
pub(crate) struct WorldPipelines {
    archive: crate::pipelines::Pipelines,
    shader: ShaderId,
    vertices: OwnedVertexBufferLayout,
    pending: HashMap<PipelineKey, Arc<PendingPipeline>>,
}

impl WorldPipelines {
    pub(crate) fn new(
        device: &wgpu::Device,
        source: &str,
        module: wgpu::ShaderModule,
        layout: wgpu::PipelineLayout,
        shader: ShaderId,
        label: &'static str,
        attributes: Vec<wgpu::VertexAttribute>,
    ) -> Self {
        let program = ShaderProgram {
            module,
            layout,
            vs_entry: "vs_main".into(),
            fs_entry: "fs_main".into(),
            count_entry: None,
        };
        Self {
            archive: crate::pipelines::Pipelines::new(device, source, program, label),
            shader,
            vertices: OwnedVertexBufferLayout {
                stride: std::mem::size_of::<WorldVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes,
            },
            pending: HashMap::new(),
        }
    }

    fn material(
        device: &wgpu::Device,
        source: &str,
        layout: wgpu::PipelineLayout,
        shader: ShaderId,
        required: u16,
    ) -> Option<Self> {
        let module = gfx::program::validated_resource(device, || {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("native world program"),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            })
        })?;
        let attributes = material_attributes(required)
            .into_iter()
            .map(|(shader_location, offset, format)| wgpu::VertexAttribute {
                format,
                offset,
                shader_location,
            })
            .collect();
        Some(Self::new(
            device,
            source,
            module,
            layout,
            shader,
            "native world program",
            attributes,
        ))
    }

    /// The pass state is fixed for this GPU program. Only the attachment formats,
    /// inherited cull and stencil vary between its draws.
    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        color: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
        state: Option<&PassState>,
        cull: Option<Cull>,
        stencil: &wgpu::StencilState,
        usage: PipelineUse,
    ) -> bool {
        let pending = self
            .pending
            .entry((color, depth, stencil.clone(), cull))
            .or_insert_with(|| {
                let mut key =
                    described_pipeline(self.shader, color, depth, state, cull, stencil.clone());
                key.vertex_buffers.push(self.vertices.clone());
                self.archive.request(device, &key)
            });
        match usage {
            PipelineUse::Direct => pending.get().is_some(),
            PipelineUse::Preflight => pending.previous_frame().is_some_and(Option::is_some),
        }
    }

    pub(crate) fn get(
        &self,
        color: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
        stencil: &wgpu::StencilState,
        cull: Option<Cull>,
    ) -> Option<&wgpu::RenderPipeline> {
        self.pending
            .get(&(color, depth, stencil.clone(), cull))?
            .try_get()?
            .as_ref()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MaterialUse {
    pub cluster: bool,
    pub rigid: bool,
    /// Some placement of the material draws faded: with its rigid
    /// program's stipple variant ([`RigidStipple`]).
    pub stippled: bool,
}

/// The materials drawn with their own programs, and each batch's texture
/// group (its lightmap page bound where the pass samples the lightmap).
pub struct ProgramSet {
    materials: HashMap<u16, Gpu>,
    /// The materials whose placements draw with their rigid programs.
    rigid: HashMap<u16, Rigid>,
    /// Per batch of the world: the texture group, when the batch's
    /// material has a program.
    batch_groups: Vec<Option<BatchTextures>>,
    /// Only groups sampling the structure lightmap have alternate bindings.
    /// `None` keeps ordinary loads free of replacement textures and groups.
    relit: Option<Vec<Option<RelitBatch>>>,
    /// The cull the passes that inherit it draw with
    /// ([`ProgramSet::pipelines`]).
    cull: InheritedCull,
    /// The lightmap pages uploaded for the batch groups, by key (`None`
    /// for one that did not decode): what a rebuilt material's groups bind
    /// ([`ProgramSet::refresh`]) and a live edit rewrites
    /// ([`ProgramSet::page_textures`]).
    pages: HashMap<LightmapKey, Option<wgpu::TextureView>>,
}

#[derive(Clone)]
struct RelitBatch {
    on: BatchTextures,
    off: BatchTextures,
    /// This page/palette pair includes texels affected by the chosen lamp.
    changed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BatchTextures {
    Resident(wgpu::BindGroup),
    /// The page (retail or relit) is resident. Target 1 is pinned per draw.
    Accumulation { page: Option<wgpu::TextureView> },
}

impl BatchTextures {
    fn resident(&self) -> Option<&wgpu::BindGroup> {
        match self { Self::Resident(group) => Some(group), Self::Accumulation { .. } => None }
    }
}

/// What [`ProgramSet::refresh`] did, by material, ascending.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Refreshed {
    /// Uniform buffers rewritten in place.
    pub constants: Vec<u16>,
    /// Programs, textures and groups built again.
    pub rebuilt: Vec<u16>,
    /// The edited program draws as the old one did.
    pub unchanged: Vec<u16>,
}

/// One material's refresh, checked and built before anything is swapped.
enum Planned {
    Constants(Box<MaterialProgram>),
    Rebuild(
        Box<Gpu>,
        Option<Rigid>,
        Vec<(usize, Option<BatchTextures>)>,
    ),
}

/// Uploads `program`, each stage's image decoded once per signed variant
/// through `images` (a layered image from `layered`, a two-dimensional
/// one from `stage_image`); `None` when a stage's bitmap does not decode.
fn material_gpu(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    program: MaterialProgram,
    stage_image: &mut dyn FnMut(Stage) -> Option<Image>,
    layered: &HashMap<(map::TagId, u32), crate::light_gpu::LayeredImage>,
    images: &mut HashMap<(map::TagId, u32, [bool; 4]), Option<wgpu::TextureView>>,
) -> Option<Gpu> {
    Gpu::with_images(device, program, &mut |stage, signed| {
        let Stage::Bitmap { bitmap, image } = stage else {
            return None;
        };
        images
            .entry((bitmap, image, signed))
            .or_insert_with(|| {
                if let Some(image) = layered.get(&(bitmap, image)) {
                    let biased;
                    let pixels = if signed.iter().any(|&s| s) {
                        biased = {
                            let mut pixels = image.clone();
                            bias_levels(&mut pixels.rgba, &mut pixels.mips, signed);
                            pixels
                        };
                        &biased
                    } else {
                        image
                    };
                    return Some(crate::light_gpu::layered_texture(
                        device,
                        queue,
                        pixels,
                        "native world stage",
                    ));
                }
                let mut decoded = stage_image(stage)?;
                bias_signed(&mut decoded, signed);
                Some(texture(device, queue, &decoded, "native world stage"))
            })
            .clone()
    })
}

pub(crate) fn uniform_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

impl Gpu {
    /// The textures this program's stages sample as `want`, with the stage
    /// and its signed channels, appended to `out` unless `out` holds them.
    pub(crate) fn held(&self, want: Stage, out: &mut Vec<crate::gpu::Held>) {
        for (n, stage) in self.program.stages.iter().enumerate() {
            let Some(view) = self.stage_views[n].as_ref().filter(|_| *stage == want) else {
                continue;
            };
            let t = view.texture().clone();
            if !out.iter().any(|(o, ..)| *o == t) {
                out.push((t, n, self.program.signed[n]));
            }
        }
    }

    /// Uploads `program`; `None` when a stage's bitmap does not decode.
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        program: MaterialProgram,
        stage_image: &mut dyn FnMut(Stage) -> Option<Image>,
    ) -> Option<Self> {
        Self::with_images(device, program, &mut |stage, signed| {
            let mut image = stage_image(stage)?;
            bias_signed(&mut image, signed);
            Some(texture(device, queue, &image, "native world stage"))
        })
    }

    fn with_images(
        device: &wgpu::Device,
        program: MaterialProgram,
        image: &mut dyn FnMut(Stage, [bool; 4]) -> Option<wgpu::TextureView>,
    ) -> Option<Self> {
        let mut stage_views: [Option<wgpu::TextureView>; 4] = Default::default();
        for (n, &stage) in program.stages.iter().enumerate() {
            if stage.takes_image() && program.dims[n].is_some() {
                stage_views[n] = Some(image(stage, program.signed[n])?);
            }
        }
        let uniforms_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native world program uniforms"),
            entries: &[
                uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
                uniform_entry(1, wgpu::ShaderStages::VERTEX_FRAGMENT),
            ],
        });
        let mut entries = Vec::new();
        for (n, &dim) in program.dims.iter().enumerate() {
            let Some(dim) = dim else { continue };
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: 2 * n as u32,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: crate::light_gpu::view_dimension(dim),
                    multisampled: false,
                },
                count: None,
            });
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: 2 * n as u32 + 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            });
        }
        let textures_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native world program textures"),
            entries: &entries,
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("native world program"),
            bind_group_layouts: &[Some(&uniforms_layout), Some(&textures_layout)],
            immediate_size: 0,
        });
        let buffer = |label, contents: &[u8]| {
            gfx::churn::create_buffer_init(
                device,
                &wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents,
                    // COPY_SRC: a live refresh's test reads the buffers back.
                    usage: wgpu::BufferUsages::UNIFORM
                        | wgpu::BufferUsages::COPY_DST
                        | wgpu::BufferUsages::COPY_SRC,
                },
            )
        };
        let vertex_buffer = buffer("native world program vertex", &program.vertex.to_bytes());
        let pixel_buffer = buffer("native world program pixel", &program.pixel.to_bytes());
        let uniforms = gfx::churn::create_bind_group(
            device,
            &wgpu::BindGroupDescriptor {
                label: Some("native world program uniforms"),
                layout: &uniforms_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: vertex_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: pixel_buffer.as_entire_binding(),
                    },
                ],
            },
        );
        let samplers = std::array::from_fn(|n| {
            device.create_sampler(&sampler_descriptor(program.samplers[n].as_ref()))
        });
        let pipelines =
            WorldPipelines::material(device, &program.wgsl, layout, WORLD_CLUSTER_PROGRAM, program.inputs)?;
        Some(Gpu {
            program,
            pipelines,
            textures_layout,
            vertex_buffer,
            pixel_buffer,
            uniforms,
            samplers,
            stage_views,
        })
    }

    fn batch_binding(&self, device: &wgpu::Device, page: Option<&wgpu::TextureView>) -> Option<BatchTextures> {
        if self.program.texaccum_stages().contains(&true) {
            if self.program.samples_lightmap() && page.is_none() { return None; }
            Some(BatchTextures::Accumulation { page: page.cloned() })
        } else {
            self.textures(device, page).map(BatchTextures::Resident)
        }
    }

    /// The texture group with `page` bound where the program samples the
    /// lightmap; `None` when a sampled stage has nothing to bind.
    pub(crate) fn textures(
        &self,
        device: &wgpu::Device,
        page: Option<&wgpu::TextureView>,
    ) -> Option<wgpu::BindGroup> {
        self.textures_supplied(device, page, &HashMap::new())
    }

    /// [`Gpu::textures`] with `supplied` views at their stages: the
    /// engine-texture stages (the water edge blend's scene copy and
    /// reflections, supplied per view by the reservation), stage number
    /// to view. A supplied stage must not be a bitmap stage.
    pub(crate) fn textures_supplied(
        &self,
        device: &wgpu::Device,
        page: Option<&wgpu::TextureView>,
        supplied: &HashMap<usize, wgpu::TextureView>,
    ) -> Option<wgpu::BindGroup> {
        let mut views = Vec::new();
        for (n, d) in self.program.dims.iter().enumerate() {
            if d.is_none() {
                continue;
            }
            let view = match self.program.stages[n] {
                Stage::Lightmap => page?.clone(),
                Stage::Engine(_) => supplied.get(&n)?.clone(),
                _ => self.stage_views[n].clone()?,
            };
            views.push((n, view));
        }
        let mut entries = Vec::new();
        for (n, view) in &views {
            entries.push(wgpu::BindGroupEntry {
                binding: 2 * *n as u32,
                resource: wgpu::BindingResource::TextureView(view),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: 2 * *n as u32 + 1,
                resource: wgpu::BindingResource::Sampler(&self.samplers[*n]),
            });
        }
        Some(gfx::churn::create_bind_group(
            device,
            &wgpu::BindGroupDescriptor {
                label: Some("native world program textures"),
                layout: &self.textures_layout,
                entries: &entries,
            },
        ))
    }

    /// The frame's vertex and pixel uniforms, written through the submission's
    /// upload arena (the
    /// reservation path records into an isolated encoder, where a queue
    /// write would escape cancellation): for a pass whose pixel state is
    /// resolved per view (the fog layer's `PixelUniforms::fog` and
    /// `fog_color`, `crate::fog_gpu`).
    pub(crate) fn prepare_with(
        &self,
        uploads: &mut gfx::ring::EncoderUploads<'_>,
        view: &ViewInputs,
        vertex: &VertexUniforms,
        pixel: &PixelUniforms,
    ) {
        uploads.write(
            &self.vertex_buffer,
            0,
            &frame_vertex(vertex, view).to_bytes(),
        );
        uploads.write(&self.pixel_buffer, 0, &frame_pixel(pixel, view).to_bytes());
    }

    /// [`Gpu::prepare_with`]'s two blocks as writes ahead of a pass's
    /// packets (`gfx::external::ExternalDraws::writes`).
    pub(crate) fn prepare_writes(
        &self,
        view: &ViewInputs,
        vertex: &VertexUniforms,
        pixel: &PixelUniforms,
    ) -> [(wgpu::Buffer, u64, Vec<u8>); 2] {
        [
            (
                self.vertex_buffer.clone(),
                0,
                frame_vertex(vertex, view).to_bytes(),
            ),
            (
                self.pixel_buffer.clone(),
                0,
                frame_pixel(pixel, view).to_bytes().to_vec(),
            ),
        ]
    }
}

/// Shader ids the world's packets report to the pass they join (for
/// diagnostics; the pipelines are the programs' own).
pub(crate) const WORLD_CLUSTER_PROGRAM: gfx::pipeline::ShaderId =
    gfx::pipeline::ShaderId(u32::MAX - 1);
pub(crate) const WORLD_RIGID_PROGRAM: gfx::pipeline::ShaderId =
    gfx::pipeline::ShaderId(u32::MAX - 2);

/// The state a pipeline reports in a packet: primitive, depth
/// and stencil, and colour target (its write mask splits merged passes).
/// A pass that sets its own stencil ([`PassState::stencil`]) takes it in
/// place of `stencil`, the device's at the layer.
pub(crate) fn described_pipeline(
    shader: gfx::pipeline::ShaderId,
    color: wgpu::TextureFormat,
    depth: wgpu::TextureFormat,
    state: Option<&PassState>,
    cull: Option<Cull>,
    stencil: wgpu::StencilState,
) -> gfx::pipeline::PipelineKey {
    let (primitive, (depth_write, depth_compare)) = pipeline_state(state, cull);
    let stencil = match state.and_then(|s| s.stencil) {
        Some(own) => crate::fog_gpu::layer_stencil(&own, depth),
        None => stencil,
    };
    let (blend, write_mask) = state.map_or((None, wgpu::ColorWrites::ALL), |s| {
        (s.blend, s.color_writes)
    });
    gfx::pipeline::PipelineKey {
        shader,
        vertex_buffers: Vec::new(),
        primitive,
        depth_stencil: Some(wgpu::DepthStencilState {
            format: depth,
            depth_write_enabled: Some(depth_write),
            depth_compare: Some(depth_compare),
            stencil,
            bias: wgpu::DepthBiasState::default(),
        }),
        color_target: wgpu::ColorTargetState {
            format: color,
            blend,
            write_mask,
        },
        samples: 1,
    }
}

/// A material's rigid program on the GPU ([`RigidProgram`]): one vertex
/// uniform buffer with a slot per placement the material draws, bound at
/// that slot's dynamic offset for the placement's draws, with the
/// material's pixel uniforms and texture groups.
pub(crate) struct Rigid {
    pub(crate) program: RigidProgram,
    buffer: wgpu::Buffer,
    pub(crate) uniforms: wgpu::BindGroup,
    /// Bytes between slots (the device's offset alignment).
    stride: u64,
    /// Per slot, ascending: the placement and its rows.
    slots: Vec<(u32, ModelRows)>,
    pub(crate) pipelines: WorldPipelines,
    /// The program's stipple variant, made when a faded placement is first
    /// selected ([`Rigid::stipple_for`]).
    stipple: Option<RigidStipple>,
}

/// The number of patterns in the engine's stipple table (`0x00487288`).
pub const STIPPLE_LEVELS: usize = 17;

/// A rigid program with the polygon stipple test
/// ([`crate::object_program::stipple_module`]) for the placements the
/// structure draws faded (retail's LOD display fade): the rigid vertex
/// slots bound as for the plain program, and one pixel block per table
/// pattern ([`STIPPLE_LEVELS`]), bound at the pattern's dynamic offset, so a
/// faded draw takes its pattern's rows without a block of its own.
pub(crate) struct RigidStipple {
    pipelines: WorldPipelines,
    uniforms: wgpu::BindGroup,
    pixel: wgpu::Buffer,
    /// Bytes between the pattern blocks (the device's offset alignment).
    stride: u64,
}

/// The placements `material`'s draws in `world` name, ascending, each with
/// its rows; `None` when one has none.
pub fn rigid_slots(world: &World, material: u16) -> Option<Vec<(u32, ModelRows)>> {
    let mut slots: Vec<(u32, ModelRows)> = Vec::new();
    for d in world.draws.iter().filter(|d| d.material == material) {
        let Owner::Instance(i) = d.owner else {
            continue;
        };
        if slots.last().is_some_and(|s| s.0 == i) {
            continue;
        }
        slots.push((i, *world.rows(i)?));
    }
    slots.sort_by_key(|s| s.0);
    slots.dedup_by_key(|s| s.0);
    Some(slots)
}

/// [`rigid_vertex`] of every slot, `stride` bytes apart: the slot buffer's
/// contents for one frame.
pub fn rigid_slot_bytes(
    base: &VertexUniforms,
    view: &ViewInputs,
    slots: &[(u32, ModelRows)],
    stride: usize,
) -> Vec<u8> {
    let mut out = vec![0u8; stride * slots.len().saturating_sub(1) + VertexUniforms::SIZE];
    for (n, (_, rows)) in slots.iter().enumerate() {
        rigid_vertex(base, view, rows)
            .write_bytes(&mut out[n * stride..n * stride + VertexUniforms::SIZE]);
    }
    out
}

impl Rigid {
    /// Uploads `gpu`'s rigid program for `material`'s placements in
    /// `world`; `None` without one, or with no placement to draw.
    pub(crate) fn new(
        device: &wgpu::Device,
        gpu: &Gpu,
        world: &World,
        material: u16,
    ) -> Option<Self> {
        Self::with_slots(device, gpu, rigid_slots(world, material)?)
    }

    pub(crate) fn with_slots(
        device: &wgpu::Device,
        gpu: &Gpu,
        slots: Vec<(u32, ModelRows)>,
    ) -> Option<Self> {
        let program = gpu.program.rigid.clone()?;
        if slots.is_empty() {
            return None;
        }
        let align = u64::from(device.limits().min_uniform_buffer_offset_alignment.max(1));
        let size = VertexUniforms::SIZE as u64;
        let stride = size.div_ceil(align) * align;
        let mut vertex = uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT);
        if let wgpu::BindingType::Buffer {
            has_dynamic_offset,
            min_binding_size,
            ..
        } = &mut vertex.ty
        {
            *has_dynamic_offset = true;
            *min_binding_size = wgpu::BufferSize::new(size);
        }
        let uniforms_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native world rigid uniforms"),
            entries: &[
                vertex,
                uniform_entry(1, wgpu::ShaderStages::VERTEX_FRAGMENT),
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("native world rigid program"),
            bind_group_layouts: &[Some(&uniforms_layout), Some(&gpu.textures_layout)],
            immediate_size: 0,
        });
        let mut initial = vec![0; (stride * (slots.len() as u64 - 1) + size) as usize];
        for (i, (_, rows)) in slots.iter().enumerate() {
            let mut v = program.vertex.clone();
            v.constants[MODEL_ROWS..MODEL_ROWS + 3].copy_from_slice(rows);
            let start = i * stride as usize;
            v.write_bytes(&mut initial[start..start + VertexUniforms::SIZE]);
        }
        let buffer = gfx::churn::create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("native world rigid vertex"),
                contents: &initial,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            },
        );
        let uniforms = gfx::churn::create_bind_group(
            device,
            &wgpu::BindGroupDescriptor {
                label: Some("native world rigid uniforms"),
                layout: &uniforms_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &buffer,
                            offset: 0,
                            size: wgpu::BufferSize::new(size),
                        }),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: gpu.pixel_buffer.as_entire_binding(),
                    },
                ],
            },
        );
        let pipelines =
            WorldPipelines::material(device, &program.wgsl, layout, WORLD_RIGID_PROGRAM, program.inputs)?;
        Some(Self {
            program,
            buffer,
            uniforms,
            stride,
            slots,
            pipelines,
            stipple: None,
        })
    }

    /// The stipple variant of this program over `g`'s pixel shader, made on
    /// first use. A module that does not link is a program bug, named.
    fn stipple_for(&mut self, device: &wgpu::Device, g: &Gpu) -> &mut RigidStipple {
        if self.stipple.is_none() {
            let wgsl = crate::object_program::stipple_module(&self.program.wgsl, &g.program)
                .unwrap_or_else(|e| panic!("native world: a rigid program's stipple variant: {e}"));
            let align = u64::from(device.limits().min_uniform_buffer_offset_alignment.max(1));
            let size = PixelUniforms::SIZE as u64;
            let stride = size.div_ceil(align) * align;
            let dynamic = |binding| {
                let mut entry = uniform_entry(binding, wgpu::ShaderStages::VERTEX_FRAGMENT);
                if let wgpu::BindingType::Buffer { has_dynamic_offset, min_binding_size, .. } = &mut entry.ty {
                    *has_dynamic_offset = true;
                    *min_binding_size = wgpu::BufferSize::new(if binding == 0 { VertexUniforms::SIZE as u64 } else { size });
                }
                entry
            };
            let uniforms_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("native world rigid stipple uniforms"),
                entries: &[dynamic(0), dynamic(1)],
            });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("native world rigid stipple program"),
                bind_group_layouts: &[Some(&uniforms_layout), Some(&g.textures_layout)],
                immediate_size: 0,
            });
            let pixel = gfx::churn::create_buffer_init(
                device,
                &wgpu::util::BufferInitDescriptor {
                    label: Some("native world rigid stipple pixel"),
                    contents: &vec![0; (stride * (STIPPLE_LEVELS as u64 - 1) + size) as usize],
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                },
            );
            let uniforms = gfx::churn::create_bind_group(
                device,
                &wgpu::BindGroupDescriptor {
                    label: Some("native world rigid stipple uniforms"),
                    layout: &uniforms_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                buffer: &self.buffer,
                                offset: 0,
                                size: wgpu::BufferSize::new(VertexUniforms::SIZE as u64),
                            }),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                buffer: &pixel,
                                offset: 0,
                                size: wgpu::BufferSize::new(size),
                            }),
                        },
                    ],
                },
            );
            let pipelines = WorldPipelines::material(device, &wgsl, layout, WORLD_RIGID_PROGRAM, self.program.inputs)
                .unwrap_or_else(|| panic!("native world: a rigid program's stipple variant does not validate"));
            self.stipple = Some(RigidStipple { pipelines, uniforms, pixel, stride });
        }
        self.stipple.as_mut().expect("made above")
    }

    /// The frame's view and rows in every slot.
    pub(crate) fn prepare(&self, uploads: &mut gfx::ring::EncoderUploads<'_>, view: &ViewInputs) {
        let v = frame_vertex(&self.program.vertex, view).to_bytes();
        // Only c[0..18] and the viewport/clip tail depend on the view.
        // Every placement keeps its own original c[50..53] in the GPU block.
        uploads.repeat(
            &self.buffer,
            0,
            self.stride,
            self.slots.len(),
            &v[..VIEW_CONSTANTS * 16],
        );
        uploads.repeat(
            &self.buffer,
            VERTEX_VIEW as u64,
            self.stride,
            self.slots.len(),
            &v[VERTEX_VIEW..VERTEX_VIEW + 32],
        );
    }

    /// Upload the sampled material registers and each placement's own rows.
    pub(crate) fn prepare_sampled(
        &self, uploads: &mut gfx::ring::EncoderUploads<'_>, view: &ViewInputs, vertex: &VertexUniforms,
    ) {
        for (slot, (_, rows)) in self.slots.iter().enumerate() {
            uploads.write(&self.buffer, self.stride * slot as u64, &rigid_vertex(vertex, view, rows).to_bytes());
        }
    }

    /// [`Rigid::prepare`]'s view rows for the slots of `instances` only, as
    /// writes ahead of a pass's packets. Unknown instances are skipped.
    pub(crate) fn prepare_writes(
        &self,
        view: &ViewInputs,
        instances: impl IntoIterator<Item = u32>,
        sampled: Option<&VertexUniforms>,
    ) -> Vec<(wgpu::Buffer, u64, Vec<u8>)> {
        let v = frame_vertex(&self.program.vertex, view).to_bytes();
        let mut writes = Vec::new();
        for instance in instances {
            let Ok(slot) = self.slots.binary_search_by_key(&instance, |s| s.0) else { continue };
            let offset = self.stride * slot as u64;
            if let Some(vertex) = sampled {
                let vertex = rigid_vertex(vertex, view, &self.slots[slot].1);
                writes.push((self.buffer.clone(), offset, vertex.to_bytes()));
                continue;
            }
            writes.push((
                self.buffer.clone(),
                offset,
                v[..VIEW_CONSTANTS * 16].to_vec(),
            ));
            writes.push((
                self.buffer.clone(),
                offset + VERTEX_VIEW as u64,
                v[VERTEX_VIEW..VERTEX_VIEW + 32].to_vec(),
            ));
        }
        writes
    }

    /// The dynamic offset of `instance`'s slot.
    pub(crate) fn offset(&self, instance: u32) -> Option<u32> {
        let n = self.slots.binary_search_by_key(&instance, |s| s.0).ok()?;
        u32::try_from(n as u64 * self.stride).ok()
    }

    /// Takes `program`'s constants (same module and inputs, a
    /// [`ProgramChange::Constants`] edit): every slot written again as
    /// [`Rigid::with_slots`] writes it.
    pub(crate) fn rewrite(&mut self, queue: &wgpu::Queue, program: &RigidProgram) {
        let stride = self.stride as usize;
        let mut bytes = vec![0; stride * (self.slots.len().max(1) - 1) + VertexUniforms::SIZE];
        for (i, (_, rows)) in self.slots.iter().enumerate() {
            let mut v = program.vertex.clone();
            v.constants[MODEL_ROWS..MODEL_ROWS + 3].copy_from_slice(rows);
            v.write_bytes(&mut bytes[i * stride..i * stride + VertexUniforms::SIZE]);
        }
        queue.write_buffer(&self.buffer, 0, &bytes);
        self.program = program.clone();
    }
}

impl ProgramSet {
    /// Uploads `programs` (by material) for `world`. `stage_image` decodes
    /// a stage's bitmap image; `lightmap` gives a batch's page. A material
    /// whose bitmaps do not decode is left out (it keeps stage 1's program).
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        world: &World,
        programs: HashMap<u16, MaterialProgram>,
        stage_image: &mut dyn FnMut(Stage) -> Option<Image>,
        lightmap: &mut dyn FnMut(LightmapKey) -> Option<Image>,
    ) -> Self {
        Self::with_layered(
            device,
            queue,
            world,
            programs,
            stage_image,
            lightmap,
            &HashMap::new(),
        )
    }

    pub fn with_layered(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        world: &World,
        programs: HashMap<u16, MaterialProgram>,
        stage_image: &mut dyn FnMut(Stage) -> Option<Image>,
        lightmap: &mut dyn FnMut(LightmapKey) -> Option<Image>,
        layered: &HashMap<(map::TagId, u32), crate::light_gpu::LayeredImage>,
    ) -> Self {
        let mut materials = HashMap::new();
        let mut rigid = HashMap::new();
        let mut images = HashMap::new();
        for (material, program) in programs {
            let g = material_gpu(device, queue, program, stage_image, layered, &mut images);
            if let Some(g) = g {
                if let Some(r) = Rigid::new(device, &g, world, material) {
                    rigid.insert(material, r);
                }
                materials.insert(material, g);
            }
        }
        let mut pages: HashMap<LightmapKey, Option<wgpu::TextureView>> = HashMap::new();
        let mut groups = HashMap::new();
        let batch_groups = world
            .batches
            .iter()
            .map(|b| {
                let g = materials.get(&b.material)?;
                groups
                    .entry((b.material, b.lightmap))
                    .or_insert_with(|| {
                        let page = b.lightmap.and_then(|k| {
                            pages
                                .entry(k)
                                .or_insert_with(|| {
                                    lightmap(k).map(|i| {
                                        texture(device, queue, &i, "native world lightmap")
                                    })
                                })
                                .clone()
                        });
                        g.batch_binding(device, page.as_ref())
                    })
                    .clone()
            })
            .collect();
        Self {
            materials,
            rigid,
            batch_groups,
            relit: None,
            cull: InheritedCull::default(),
            pages,
        }
    }

    /// Bind complete reconstructed RGBA pages at the same sampler slots as
    /// retail. Both arms share all other stage views, programs and uniforms.
    /// Publish only after every lightmap-using batch has both bindings.
    pub fn install_relight(
        &mut self, device: &wgpu::Device, queue: &wgpu::Queue, world: &World,
        [on, off]: &[HashMap<LightmapKey, Image>; 2],
    ) -> Result<(), &'static str> {
        let mut pages = HashMap::new();
        let mut groups = HashMap::new();
        let mut batches = Vec::with_capacity(world.batches.len());
        for (index, batch) in world.batches.iter().enumerate() {
            let gpu = self.materials.get(&batch.material).ok_or("world material unavailable")?;
            if !gpu.program.samples_lightmap() {
                batches.push(None);
                continue;
            }
            let key = batch.lightmap.ok_or("world batch has no lightmap key")?;
            self.lit_batch_binding(index, crate::lighting::Lighting::Retail).ok_or("world batch has no retail binding")?;
            let (on_view, off_view, changed) = match pages.entry(key) {
                std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                std::collections::hash_map::Entry::Vacant(e) => {
                    let a = on.get(&key).ok_or("lamp-on lightmap page missing")?;
                    let b = off.get(&key).ok_or("lamp-off lightmap page missing")?;
                    e.insert((texture(device, queue, a, "lamp on"), texture(device, queue, b, "lamp off"), a != b))
                }
            };
            let bindings = match groups.entry((batch.material, key)) {
                std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                std::collections::hash_map::Entry::Vacant(e) => e.insert(RelitBatch {
                    on: gpu.batch_binding(device, Some(on_view)).ok_or("lamp-on binding unavailable")?,
                    off: gpu.batch_binding(device, Some(off_view)).ok_or("lamp-off binding unavailable")?,
                    changed: *changed,
                }),
            };
            batches.push(Some(bindings.clone()));
        }
        if !batches.iter().flatten().any(|b| b.changed) {
            return Err("no native world batch samples a changed lightmap page");
        }
        self.relit = Some(batches);
        Ok(())
    }

    pub(crate) fn has_relight(&self) -> bool {
        self.relit.is_some()
    }

    pub(crate) fn discard_relight(&mut self) -> bool {
        self.relit.take().is_some()
    }

    pub(crate) fn changed_lightmap_batch(&self, index: usize) -> bool {
        self.relit.as_ref().and_then(|v| v.get(index)).and_then(Option::as_ref)
            .is_some_and(|b| b.changed)
    }

    pub(crate) fn lit_batch_textures(&self, index: usize, lighting: crate::lighting::Lighting) -> Option<&wgpu::BindGroup> {
        self.lit_batch_binding(index, lighting)?.resident()
    }

    pub(crate) fn lit_batch_binding(&self, index: usize, lighting: crate::lighting::Lighting) -> Option<&BatchTextures> {
        if let Some(batch) = self.relit.as_ref().and_then(|v| v.get(index)).and_then(Option::as_ref) {
            match lighting {
                crate::lighting::Lighting::RelightOn => return Some(&batch.on),
                crate::lighting::Lighting::RelightOff => return Some(&batch.off),
                _ => {}
            }
        }
        self.batch_groups.get(index)?.as_ref()
    }

    pub(crate) fn accumulation_group(
        &self, device: &wgpu::Device, material: u16, page: Option<&wgpu::TextureView>, target: &wgpu::TextureView,
    ) -> Option<wgpu::BindGroup> {
        let gpu = self.materials.get(&material)?;
        let supplied = gpu.program.texaccum_stages().into_iter().enumerate()
            .filter(|(_, on)| *on).map(|(n, _)| (n, target.clone())).collect();
        gpu.textures_supplied(device, page, &supplied)
    }

    /// The program material `material` draws with, if it has one.
    pub fn program(&self, material: u16) -> Option<&MaterialProgram> {
        self.materials.get(&material).map(|g| &g.program)
    }

    /// Whether material `material` draws with its own program.
    pub fn has(&self, material: u16) -> bool {
        self.materials.contains_key(&material)
    }

    /// Puts `edited` (by material, each compiled from the edited shader as
    /// the load compiles it) in place of the materials' programs
    /// (docs/editor/realtime-edits.md, "The native refresh"). A
    /// constants-only change ([`ProgramChange::Constants`]) rewrites the
    /// material's uniform buffers (and its rigid program's slots); any
    /// other builds its program, stage textures, rigid program and batch
    /// groups again, binding the lightmap pages the load uploaded. Every
    /// material is checked and built before the first is swapped: a
    /// material with no program here or a stage that does not decode
    /// refuses them all and writes nothing.
    pub fn refresh(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        world: &World,
        edited: HashMap<u16, MaterialProgram>,
        stage_image: &mut dyn FnMut(Stage) -> Option<Image>,
        layered: &HashMap<(map::TagId, u32), crate::light_gpu::LayeredImage>,
    ) -> Result<Refreshed, String> {
        let mut edited: Vec<(u16, MaterialProgram)> = edited.into_iter().collect();
        edited.sort_by_key(|(m, _)| *m);
        let mut done = Refreshed::default();
        let mut plan = Vec::new();
        let mut images = HashMap::new();
        for (m, program) in edited {
            let old = self
                .materials
                .get(&m)
                .ok_or_else(|| format!("material {m} has no program to refresh"))?;
            match old.program.change_to(&program) {
                ProgramChange::None => done.unchanged.push(m),
                ProgramChange::Constants => plan.push((m, Planned::Constants(Box::new(program)))),
                ProgramChange::Program => {
                    let g = material_gpu(device, queue, program, stage_image, layered, &mut images)
                        .ok_or_else(|| format!("material {m}: a stage's bitmap does not decode or the program does not build"))?;
                    // One group per page, as the load makes them; a page
                    // that did not decode leaves its batches unbound, as
                    // at the load.
                    let mut by_page: HashMap<Option<LightmapKey>, Option<BatchTextures>> =
                        HashMap::new();
                    let mut groups = Vec::new();
                    for (n, b) in world
                        .batches
                        .iter()
                        .enumerate()
                        .filter(|(_, b)| b.material == m)
                    {
                        let group = by_page.entry(b.lightmap).or_insert_with(|| {
                            let page = b
                                .lightmap
                                .and_then(|k| self.pages.get(&k).cloned().flatten());
                            g.batch_binding(device, page.as_ref())
                        });
                        groups.push((n, group.clone()));
                    }
                    let rigid = Rigid::new(device, &g, world, m);
                    plan.push((m, Planned::Rebuild(Box::new(g), rigid, groups)));
                }
            }
        }
        for (m, p) in plan {
            match p {
                Planned::Constants(program) => {
                    let g = self.materials.get_mut(&m).expect("checked above");
                    queue.write_buffer(&g.vertex_buffer, 0, &program.vertex.to_bytes());
                    queue.write_buffer(&g.pixel_buffer, 0, &program.pixel.to_bytes());
                    if let (Some(r), Some(edited)) =
                        (self.rigid.get_mut(&m), program.rigid.as_ref())
                    {
                        r.rewrite(queue, edited);
                    }
                    g.program = *program;
                    done.constants.push(m);
                }
                Planned::Rebuild(g, rigid, groups) => {
                    self.materials.insert(m, *g);
                    match rigid {
                        Some(r) => {
                            self.rigid.insert(m, r);
                        }
                        None => {
                            self.rigid.remove(&m);
                        }
                    }
                    for (n, group) in groups {
                        if let Some(slot) = self.batch_groups.get_mut(n) {
                            *slot = group;
                        }
                    }
                    done.rebuilt.push(m);
                }
            }
        }
        Ok(done)
    }

    /// The textures that hold lightmap page `key`, for
    /// [`crate::gpu::rewrite_pages`].
    pub fn page_textures(&self, key: LightmapKey) -> Vec<wgpu::Texture> {
        self.pages
            .get(&key)
            .and_then(|page| page.as_ref().map(|view| view.texture().clone()))
            .into_iter()
            .collect()
    }

    /// Every texture the materials' stages sample as image `image` of
    /// `bitmap`, with the stage and its signed channels, each texture once.
    pub fn held(&self, bitmap: map::TagId, image: u32) -> Vec<crate::gpu::Held> {
        self.image_textures(bitmap, image)
    }

    /// The stages whose programs sample image `image` of `bitmap`,
    /// ascending, each once.
    pub fn image_stages(&self, bitmap: map::TagId, image: u32) -> Vec<usize> {
        let mut stages: Vec<usize> = self
            .image_textures(bitmap, image)
            .into_iter()
            .map(|(_, n, _)| n)
            .collect();
        stages.sort_unstable();
        stages.dedup();
        stages
    }

    /// Each texture made from image `image` of `bitmap` (loads share one
    /// per signed variant), once, with a stage that samples it and the
    /// channels stored biased there.
    fn image_textures(
        &self,
        bitmap: map::TagId,
        image: u32,
    ) -> Vec<(wgpu::Texture, usize, [bool; 4])> {
        let want = Stage::Bitmap { bitmap, image };
        // `wgpu::Texture` hashes and compares by its id, not its interior state.
        #[allow(clippy::mutable_key_type)]
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        let mut materials: Vec<_> = self.materials.iter().collect();
        materials.sort_by_key(|(m, _)| **m);
        for (_, g) in materials {
            for (n, stage) in g.program.stages.iter().enumerate() {
                let Some(view) = g.stage_views[n].as_ref().filter(|_| *stage == want) else {
                    continue;
                };
                let t = view.texture().clone();
                if seen.insert(t.clone()) {
                    out.push((t, n, g.program.signed[n]));
                }
            }
        }
        out
    }

    /// Rewrite every texture made from image `image` of `bitmap` in place
    /// with `decoded` (by stage, [`ProgramSet::image_stages`]), biased for
    /// its signed channels as at load (docs/editor/realtime-edits.md,
    /// "The native refresh"). Every texture is checked before the first
    /// write; a size, level count or format change refuses them all.
    /// Returns how many textures were written.
    pub fn rewrite_image(
        &self,
        queue: &wgpu::Queue,
        bitmap: map::TagId,
        image: u32,
        decoded: &HashMap<usize, Image>,
    ) -> Result<usize, crate::gpu::ImageRefusal> {
        let mut writes = Vec::new();
        for (t, n, signed) in self.image_textures(bitmap, image) {
            let mut pixels = decoded
                .get(&n)
                .ok_or(crate::gpu::ImageRefusal::NotDecoded)?
                .clone();
            bias_signed(&mut pixels, signed);
            crate::gpu::check_rewrite(&t, &pixels)?;
            writes.push((t, pixels));
        }
        for (t, pixels) in &writes {
            crate::gpu::rewrite_texture(queue, t, pixels)?;
        }
        Ok(writes.len())
    }

    /// How many materials draw with their own programs.
    pub fn len(&self) -> usize {
        self.materials.len()
    }

    pub fn is_empty(&self) -> bool {
        self.materials.is_empty()
    }

    /// The frame's uniforms for every material.
    pub fn prepare(&self, uploads: &mut gfx::ring::EncoderUploads<'_>, view: &ViewInputs) {
        for g in self.materials.values() {
            uploads.write(
                &g.vertex_buffer,
                0,
                &frame_vertex(&g.program.vertex, view).to_bytes(),
            );
            uploads.write(
                &g.pixel_buffer,
                0,
                &frame_pixel(&g.program.pixel, view).to_bytes(),
            );
        }
        for r in self.rigid.values() {
            r.prepare(uploads, view);
        }
    }

    /// Makes the pipelines for these target formats, the stencil state
    /// the layer draws with and the cull the passes that inherit it take
    /// (before the pass; [`ProgramSet::batch`] and [`ProgramSet::rigid`]
    /// then give those).
    pub fn pipelines(
        &mut self,
        device: &wgpu::Device,
        color: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
        stencil: &wgpu::StencilState,
        cull: InheritedCull,
    ) {
        self.cull = cull;
        for (&material, g) in &mut self.materials {
            Self::material_pipelines(
                g,
                self.rigid.get_mut(&material),
                device,
                color,
                depth,
                stencil,
                cull,
                true,
                false,
                PipelineUse::Direct,
            );
        }
    }

    fn material_pipelines(
        g: &mut Gpu,
        rigid: Option<&mut Rigid>,
        device: &wgpu::Device,
        color: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
        stencil: &wgpu::StencilState,
        cull: InheritedCull,
        cluster: bool,
        stippled: bool,
        usage: PipelineUse,
    ) -> bool {
        let state = g.program.state.as_ref();
        let mut ready = true;
        if cluster {
            let c = cull.of(g.program.inherits_cull, true);
            ready &= g
                .pipelines
                .prepare(device, color, depth, state, c, stencil, usage);
        }
        if let Some(r) = rigid {
            let c = cull.of(g.program.inherits_cull, false);
            ready &= r
                .pipelines
                .prepare(device, color, depth, state, c, stencil, usage);
            if stippled {
                let state = g.program.state.as_ref();
                ready &= r
                    .stipple_for(device, g)
                    .pipelines
                    .prepare(device, color, depth, state, c, stencil, usage);
            }
        }
        ready
    }

    pub(crate) fn selected_pipelines(
        &mut self,
        device: &wgpu::Device,
        color: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
        stencil: &wgpu::StencilState,
        materials: &HashMap<u16, MaterialUse>,
        usage: PipelineUse,
    ) -> bool {
        let mut ready = true;
        for (&material, selected) in materials {
            if let Some(g) = self.materials.get_mut(&material) {
                let rigid = if selected.rigid {
                    self.rigid.get_mut(&material)
                } else {
                    None
                };
                ready &= Self::material_pipelines(
                    g,
                    rigid,
                    device,
                    color,
                    depth,
                    stencil,
                    self.cull,
                    selected.cluster,
                    selected.stippled,
                    usage,
                );
            } else {
                ready = false;
            }
        }
        ready
    }

    /// All selected programs see the same view bytes. Only the material constants
    /// and the model rows between these ranges differ, and those were uploaded at load.
    pub(crate) fn write_selected(
        &self,
        uploads: &mut gfx::ring::EncoderUploads<'_>,
        view: &ViewInputs,
        selection: &crate::gpu::WorldSelection,
    ) {
        // A sampled material's blocks are written whole, its view parts in.
        for (buffer, offset, bytes) in self.sampled_writes(view, selection) {
            uploads.write(&buffer, offset, &bytes);
        }
        let bytes = view_bytes(view);
        for (buffer, offset, block) in self.stipple_writes(view, selection, &bytes[VIEW_PREFIX + 32..]) {
            uploads.write(&buffer, offset, &block);
        }
        let (source, start) = uploads
            .arena
            .stage(uploads.device, uploads.queue, &bytes);
        let viewport = start + VIEW_PREFIX as u64;
        for material in selection.materials.keys() {
            if selection.constants.contains_key(material) || self.materials[material].program.needs_texaccum() {
                continue;
            }
            uploads.copy(
                &source,
                viewport + 32,
                &self.materials[material].pixel_buffer,
                PIXEL_TAIL as u64,
                32,
            );
        }
        for &(material, instance) in &selection.vertices {
            if selection.constants.contains_key(&material) || self.materials[&material].program.needs_texaccum() {
                continue;
            }
            let (buffer, offset) = if let Some(n) = instance {
                let r = &self.rigid[&material];
                (&r.buffer, u64::from(r.offset(n).expect("selected slot")))
            } else {
                (&self.materials[&material].vertex_buffer, 0)
            };
            uploads.copy(&source, start, buffer, offset, VIEW_PREFIX as u64);
            uploads.copy(&source, viewport, buffer, offset + VERTEX_VIEW as u64, 32);
        }
    }

    /// [`ProgramSet::write_selected`]'s copies as writes ahead of the
    /// world's packets (`gfx::external::ExternalDraws::writes`).
    pub(crate) fn selected_writes(
        &self,
        view: &ViewInputs,
        selection: &crate::gpu::WorldSelection,
    ) -> Vec<(wgpu::Buffer, u64, Vec<u8>)> {
        let bytes = view_bytes(view);
        let (prefix, rest) = bytes.split_at(VIEW_PREFIX);
        let (viewport, pixel) = rest.split_at(32);
        let mut writes = self.sampled_writes(view, selection);
        writes.extend(self.stipple_writes(view, selection, pixel));
        for material in selection.materials.keys() {
            if selection.constants.contains_key(material) || self.materials[material].program.needs_texaccum() {
                continue;
            }
            writes.push((
                self.materials[material].pixel_buffer.clone(),
                PIXEL_TAIL as u64,
                pixel.to_vec(),
            ));
        }
        for &(material, instance) in &selection.vertices {
            if selection.constants.contains_key(&material) || self.materials[&material].program.needs_texaccum() {
                continue;
            }
            let (buffer, offset) = if let Some(n) = instance {
                let r = &self.rigid[&material];
                (&r.buffer, u64::from(r.offset(n).expect("selected slot")))
            } else {
                (&self.materials[&material].vertex_buffer, 0)
            };
            writes.push((buffer.clone(), offset, prefix.to_vec()));
            writes.push((
                buffer.clone(),
                offset + VERTEX_VIEW as u64,
                viewport.to_vec(),
            ));
        }
        writes
    }

    /// The pixel blocks of the faded placements' stipple variants
    /// ([`RigidStipple`]) for the patterns the selection uses: the
    /// material's pixel block (sampled as [`ProgramSet::sampled_writes`]
    /// has it), the frame's scales, the pattern's rows, and the view's
    /// pixel tail (`pixel`) unless the block is sampled whole.
    fn stipple_writes(
        &self,
        view: &ViewInputs,
        selection: &crate::gpu::WorldSelection,
        tail: &[u8],
    ) -> Vec<(wgpu::Buffer, u64, Vec<u8>)> {
        let mut writes = Vec::new();
        let Some(table) = selection.stipple_table() else { return writes };
        let mut done = HashSet::new();
        for &(material, instance) in &selection.vertices {
            let Some(level) = instance.and_then(|n| selection.fade_level(n)) else { continue };
            if !done.insert((material, level)) { continue; }
            let Some(s) = self.rigid.get(&material).and_then(|r| r.stipple.as_ref()) else { continue };
            let program = &self.materials[&material].program;
            let sampled = selection.constants.get(&material);
            let size = selection.texaccum.filter(|_| program.needs_texaccum()).map(|t| t.size);
            let mut pixel = sampled.map_or(program.pixel, |c| c.pixel(&program.pixel));
            if let Some(size) = size {
                crate::object_program_gpu::texaccum_scale(&mut pixel, program.texaccum_stages(), size);
            }
            pixel.stipple = table[usize::from(level).min(STIPPLE_LEVELS - 1)];
            let mut bytes = frame_pixel(&pixel, view).to_bytes().to_vec();
            if sampled.is_none() && size.is_none() {
                bytes[PIXEL_TAIL..].copy_from_slice(tail);
            }
            writes.push((s.pixel.clone(), u64::from(level) * s.stride, bytes));
        }
        writes
    }

    /// The selected materials' animated constants at the render clock
    /// `time` ([`MaterialProgram::animation`]) into `selection`, which
    /// [`ProgramSet::write_selected`] and [`ProgramSet::selected_writes`]
    /// then write whole for those materials. Counted in
    /// `native_shader_animation_layer3_*`; an error is how many selected
    /// materials' animation refused (the clock did not read), and leaves
    /// none sampled.
    pub(crate) fn animate(&self, selection: &mut crate::gpu::WorldSelection, time: Option<f64>) -> Result<(), usize> {
        use timeline::native::ShaderAnimation;
        selection.constants.clear();
        let mut refused = 0;
        for material in selection.materials.keys() {
            let Some(animation) = self.materials.get(material).and_then(|g| g.program.animation.as_ref()) else {
                continue;
            };
            match animation.sample(time) {
                Ok(c) => {
                    selection.constants.insert(*material, c);
                }
                Err(_) => refused += 1,
            }
        }
        if refused > 0 {
            ShaderAnimation::Layer3Refused.count(refused as u64);
            selection.constants.clear();
            return Err(refused);
        }
        ShaderAnimation::Layer3Drawn.count(selection.constants.len() as u64);
        Ok(())
    }

    /// The whole blocks of the selection's sampled materials
    /// ([`ProgramSet::animate`]) with `view` in: the material's pixel
    /// block, its clusters' vertex block, and each selected placement's
    /// slot with its rows.
    fn sampled_writes(&self, view: &ViewInputs, selection: &crate::gpu::WorldSelection) -> Vec<(wgpu::Buffer, u64, Vec<u8>)> {
        let mut writes = Vec::new();
        for material in selection.materials.keys() {
            let sampled = selection.constants.get(material);
            let program = &self.materials[material].program;
            let size = selection.texaccum.filter(|_| program.needs_texaccum()).map(|t| t.size);
            if sampled.is_none() && size.is_none() { continue; }
            let mut pixel = sampled.map_or(program.pixel, |s| s.pixel(&program.pixel));
            if let Some(size) = size {
                crate::object_program_gpu::texaccum_scale(&mut pixel, program.texaccum_stages(), size);
            }
            let g = &self.materials[material];
            writes.push((g.pixel_buffer.clone(), 0, frame_pixel(&pixel, view).to_bytes()));
        }
        for &(material, instance) in &selection.vertices {
            let sampled = selection.constants.get(&material);
            let program = &self.materials[&material].program;
            let size = selection.texaccum.filter(|_| program.needs_texaccum()).map(|t| t.size);
            if sampled.is_none() && size.is_none() { continue; }
            let vertex = |base: &VertexUniforms| {
                let mut vertex = sampled.map_or_else(|| base.clone(), |s| s.vertex(base));
                if let Some(size) = size { program.apply_texaccum_dimensions(&mut vertex, size); }
                vertex
            };
            match instance {
                Some(n) => {
                    let r = &self.rigid[&material];
                    let slot = r.slots.binary_search_by_key(&n, |s| s.0).expect("selected slot");
                    let vertex = rigid_vertex(&vertex(&r.program.vertex), view, &r.slots[slot].1);
                    writes.push((r.buffer.clone(), r.stride * slot as u64, vertex.to_bytes()));
                }
                None => {
                    let g = &self.materials[&material];
                    writes.push((g.vertex_buffer.clone(), 0, frame_vertex(&vertex(&g.program.vertex), view).to_bytes()));
                }
            }
        }
        writes
    }

    /// Whether `material`'s pass draws a polygon offset: its fragments may
    /// draw in front of the depth that hides its bounds, so its draws keep
    /// their order (the frustum alone culls them, as [`crate::gpu::keeps_order`]'s).
    pub(crate) fn offsets_depth(&self, material: u16) -> bool {
        self.materials
            .get(&material)
            .is_some_and(|g| g.program.state.is_some_and(|s| s.depth_offset.is_some()))
    }

    /// What a world packet drawing `material`'s clusters (or, `rigid`, a
    /// placement's parts) reports to the pass it joins: the state its
    /// pipeline was made with ([`ProgramSet::cluster`], [`ProgramSet::rigid`]).
    pub(crate) fn described(
        &self,
        material: u16,
        rigid: bool,
        color: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
        stencil: &wgpu::StencilState,
    ) -> Option<gfx::pipeline::PipelineKey> {
        let g = self.materials.get(&material)?;
        let cull = self.cull.of(g.program.inherits_cull, !rigid);
        let shader = if rigid {
            WORLD_RIGID_PROGRAM
        } else {
            WORLD_CLUSTER_PROGRAM
        };
        Some(described_pipeline(
            shader,
            color,
            depth,
            g.program.state.as_ref(),
            cull,
            stencil.clone(),
        ))
    }

    /// Batch `index`'s pipeline and groups, when its material has a
    /// program and the pipelines for these formats and stencil state are
    /// made.
    pub fn batch(
        &self,
        index: usize,
        material: u16,
        color: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
        stencil: &wgpu::StencilState,
    ) -> Option<(&wgpu::RenderPipeline, &wgpu::BindGroup, &wgpu::BindGroup)> {
        let (pipeline, uniforms) = self.cluster(material, color, depth, stencil)?;
        Some((pipeline, uniforms, self.batch_textures(index)?))
    }

    pub(crate) fn batch_textures(&self, index: usize) -> Option<&wgpu::BindGroup> {
        self.batch_groups.get(index)?.as_ref()?.resident()
    }

    pub(crate) fn cluster(
        &self,
        material: u16,
        color: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
        stencil: &wgpu::StencilState,
    ) -> Option<(&wgpu::RenderPipeline, &wgpu::BindGroup)> {
        let g = self.materials.get(&material)?;
        let cull = self.cull.of(g.program.inherits_cull, true);
        Some((g.pipelines.get(color, depth, stencil, cull)?, &g.uniforms))
    }

    /// The rigid pipeline, uniforms and dynamic offset placement
    /// `instance`'s draws of `material` take, when it has a rigid program
    /// and a slot for the placement, and the pipelines for these formats
    /// are made (its texture group is the batch's, [`ProgramSet::batch`]).
    pub fn rigid(
        &self,
        material: u16,
        instance: u32,
        color: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
        stencil: &wgpu::StencilState,
    ) -> Option<(&wgpu::RenderPipeline, &wgpu::BindGroup, u32)> {
        let r = self.rigid.get(&material)?;
        let cull = self
            .cull
            .of(self.materials.get(&material)?.program.inherits_cull, false);
        Some((
            r.pipelines.get(color, depth, stencil, cull)?,
            &r.uniforms,
            r.offset(instance)?,
        ))
    }

    /// [`ProgramSet::rigid`] for a faded placement: the stipple variant's
    /// pipeline and uniforms, with the dynamic offsets of the placement's
    /// vertex slot and of pattern `level`'s pixel block
    /// ([`STIPPLE_LEVELS`]). `None` while its pipeline is not made.
    pub(crate) fn rigid_stipple(
        &self,
        material: u16,
        instance: u32,
        level: u8,
        color: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
        stencil: &wgpu::StencilState,
    ) -> Option<(&wgpu::RenderPipeline, &wgpu::BindGroup, [u32; 2])> {
        let r = self.rigid.get(&material)?;
        let s = r.stipple.as_ref()?;
        let cull = self.cull.of(self.materials.get(&material)?.program.inherits_cull, false);
        let pixel = u32::try_from(u64::from(level.min(STIPPLE_LEVELS as u8 - 1)) * s.stride).ok()?;
        Some((s.pipelines.get(color, depth, stencil, cull)?, &s.uniforms, [r.offset(instance)?, pixel]))
    }

    /// How many materials draw their placements with rigid programs.
    pub fn rigid_len(&self) -> usize {
        self.rigid.len()
    }

    /// The cull the passes that inherit it draw with, until the next
    /// [`ProgramSet::pipelines`] gives another.
    pub fn set_cull(&mut self, cull: InheritedCull) {
        self.cull = cull;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tags::material::Origin;

    #[test]
    fn the_inputs_follow_the_vertex_fields_and_registers() {
        let regs: Vec<u32> = INPUTS.iter().map(|i| i.0).collect();
        assert_eq!(regs, (0..7).collect::<Vec<_>>());
        assert_eq!(INPUT_MASK, INPUTS.iter().fold(0, |m, i| m | (1 << i.0)));
        assert_eq!(INPUTS[6], (6, 20, wgpu::VertexFormat::Float32x2));
        assert_eq!(
            FLOAT_INPUTS[6].1, 20,
            "the lights still read the float coordinate"
        );
        assert_eq!(
            INPUTS[5].1, 88,
            "v5 is the lighting vector's word, the last"
        );
        assert_eq!(
            FLOAT_INPUTS[5].1, 64,
            "the float lighting vector, before the words"
        );
        // The packed registers are words, decoded in the shader.
        for (n, (reg, _, format)) in INPUTS.iter().enumerate() {
            let packed = (2..=5).contains(reg);
            assert_eq!(*format == wgpu::VertexFormat::Uint32, packed, "v{n}");
            assert_eq!(WORLD_FETCH[n] == InputFetch::NormPacked3, packed, "v{n}");
        }
        assert_eq!(FLOAT_INPUTS.iter().map(|i| i.0).collect::<Vec<_>>(), regs);
        assert_eq!(std::mem::size_of::<WorldVertex>(), 96);
    }

    #[test]
    fn classified_baked_colour_uses_v6_without_a_lightmap() {
        use crate::world::{BAKED_COLOR_INPUT, Owner, PartInput, Placement, SectionInput, WorldBuilder};
        let mut section = SectionInput {
            positions: vec![[0.; 3], [1., 0., 0.], [0., 1., 0.]],
            uvs: vec![[0.; 2]; 3],
            normals: vec![[0., 0., 1.]; 3],
            binormals: vec![[0., 1., 0.]; 3],
            tangents: vec![[1., 0., 0.]; 3],
            lighting: vec![[1., 0., 0.]; 3],
            baked_colors: vec![[11, 31, 52, 73]; 3],
            parts: vec![PartInput { material: 2, transparent: false, triangles: vec![[0, 1, 2]] }],
            ..Default::default()
        };
        let mut b = WorldBuilder::new();
        b.add_section(Owner::Instance(2), &section, &Placement::IDENTITY, None);
        section.baked_colors.clear();
        section.lightmap_uvs = vec![[0.25, 0.75]; 3];
        b.add_section(Owner::Instance(3), &section, &Placement::IDENTITY, None);
        let w = b.finish();
        let mut program = compile(&plain_look(), &plain_program()).unwrap();
        program.stages = [Stage::Empty; 4];
        // The synthetic shader reads only position and colour. Admission
        // also checks the tangent frame declared by this classification.
        let semantics = [0, 3, 4, 5, 6, 8, 14];
        let required = vertex_lit_inputs(0x41, &semantics).unwrap();
        assert_eq!(required, 0x3f | BAKED_COLOR_INPUT);
        let no_page = |_| panic!("a vertex-lit pass has no lightmap fetch");
        assert_eq!(part_fit(&w.draws[0], required, &program, &no_page), Ok(()));
        assert!(part_fit(&w.draws[1], required, &program, &no_page).is_err());
        let (_, offset, format) = material_attributes(required)[6];
        assert_eq!(format, wgpu::VertexFormat::Unorm8x4);
        let offset = offset as usize;
        assert_eq!(&bytemuck::bytes_of(&w.program_vertices[0])[offset..offset + 4], &[11, 31, 52, 73]);
        assert_eq!(material_attributes(0x41)[6], INPUTS[6]);
        assert!(vertex_lit_inputs(0x41, &[0, 1, 2]).is_err());
        assert!(vertex_lit_inputs(0x41, &[0, 1, 2, 3, 4, 5, 9]).is_err());
    }

    #[test]
    fn instance_constants_write_only_their_components() {
        let mut regs = [[9.0f32; 4]; 4];
        put(
            &mut regs,
            &Register {
                index: 2,
                value: [1.0, 2.0, 3.0, 4.0],
                written: 0b0101,
                origin: Origin::Instance,
            },
        );
        assert_eq!(regs[2], [1.0, 9.0, 3.0, 9.0]);
        put(
            &mut regs,
            &Register {
                index: 7,
                value: [0.0; 4],
                written: 0xf,
                origin: Origin::Instance,
            },
        );
        assert_eq!(regs[3], [9.0; 4], "out of range: ignored");
    }

    /// A program reading only what the camera's view gives passes; one
    /// that reads `c[6]`, the block at `c[12..17]`, `c[5].x` or `c[10].w`
    /// is named, so its pass is refused rather than drawn from zeros.
    #[test]
    fn a_program_reading_an_ungiven_view_constant_is_named() {
        use shaders::vsh::encode::{Inst, Operand as O, program};
        use shaders::vsh::{MacOp, OutReg};
        let row = |c: u8, mask| {
            Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(O::c(c)), None).to_out(
                false,
                OutReg::Pos,
                mask,
            )
        };
        let reads = |extra: Option<Inst>| {
            let mut code = vec![row(0, 8), row(1, 4), row(2, 2), row(3, 1)];
            code.extend(extra);
            code.push(Inst::mac(MacOp::Mov, Some(O::v(1)), None, None).to_out(
                false,
                OutReg::T(0),
                0xf,
            ));
            program(&code)
        };
        let mov = |c: O| {
            Some(Inst::mac(MacOp::Mov, Some(c), None, None).to_out(false, OutReg::T(1), 0xf))
        };
        assert_eq!(ungiven_view_read(&reads(None), &CAMERA_GIVEN), None);
        assert_eq!(
            ungiven_view_read(&reads(mov(O::c(5).swz("w"))), &CAMERA_GIVEN),
            None,
            "the pack scale"
        );
        assert_eq!(
            ungiven_view_read(&reads(mov(O::c(10).swz("xyz"))), &CAMERA_GIVEN),
            None
        );
        for (operand, named) in [
            (O::c(6), "c[6].x"),
            (O::c(12).swz("y"), "c[12].y"),
            (O::c(17).swz("z"), "c[17].z"),
            (O::c(5).swz("xw"), "c[5].x"),
            (O::c(10).swz("w"), "c[10].w"),
            (O::c_rel(0).swz("x"), "c[a0.x + 0]"),
        ] {
            let why = ungiven_view_read(&reads(mov(operand)), &CAMERA_GIVEN).expect(named);
            assert!(why.contains(named), "{why}");
        }
        // The instance's and the model rows' registers are not the view's.
        assert_eq!(
            ungiven_view_read(&reads(mov(O::c(18))), &CAMERA_GIVEN),
            None
        );
        assert_eq!(
            ungiven_view_read(&reads(mov(O::c_rel(50))), &CAMERA_GIVEN),
            None
        );
    }

    /// A camera at the origin looking down +x, z up, 90 degrees vertical,
    /// 640x480, near 1/16, far 1024: the editor's constants are the
    /// game's layout for that view.
    #[test]
    fn a_cameras_view_constants_put_points_where_its_projection_does() {
        let camera = crate::camera::RenderCamera {
            position: [0.0; 3],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov: 90f32.to_radians(),
            projection_xy: None,
            viewport: [0, 0, 480, 640],
            near: 0.0625,
            far: 1024.0,
        };
        let v = ViewInputs::from_camera(&camera, (640, 480));
        let zs = crate::camera::D24_SCALE;
        let s = zs / 1024.0;
        let c = &v.constants;
        // Known rows: right is -y, up is z, w the depth along x, all by
        // zs / far; x by 1 / aspect, y by 1 (tan 45 degrees).
        let near = |a: [f32; 4], b: [f32; 4]| (0..4).all(|k| (a[k] - b[k]).abs() <= 1e-3 * s);
        assert!(near(c[0], [0.0, -0.75 * s, 0.0, 0.0]), "{:?}", c[0]);
        assert!(near(c[1], [0.0, 0.0, s, 0.0]), "{:?}", c[1]);
        assert!(near(c[3], [s, 0.0, 0.0, 0.0]), "{:?}", c[3]);
        assert_eq!(c[2], c[3], "z clips from w");
        assert_eq!(c[5][3], 0.5);
        assert_eq!(
            (c[10], c[11]),
            ([320.0, -240.0, 0.0, 0.0], [320.0, 240.0, 0.0, 0.0])
        );
        // The game's epilogue: window = clip.xy / w * c[10] + c[11].
        let window = |p: [f32; 3]| {
            let dph = |r: [f32; 4]| r[0] * p[0] + r[1] * p[1] + r[2] * p[2] + r[3];
            let w = dph(c[3]);
            (
                [
                    dph(c[0]) / w * c[10][0] + c[11][0],
                    dph(c[1]) / w * c[10][1] + c[11][1],
                ],
                w,
            )
        };
        let close = |(a, w): ([f32; 2], f32), b: [f32; 2], depth: f32| {
            assert!(
                (a[0] - b[0]).abs() < 1e-2 && (a[1] - b[1]).abs() < 1e-2,
                "{a:?} {b:?}"
            );
            assert!(
                (w - depth * s).abs() <= 1e-3 * depth * s,
                "w {w} for depth {depth}"
            );
        };
        close(window([10.0, 0.0, 0.0]), [320.0, 240.0], 10.0);
        close(window([10.0, 0.0, 10.0]), [320.0, 0.0], 10.0);
        close(
            window([20.0, -20.0 * 640.0 / 480.0, 0.0]),
            [640.0, 240.0],
            20.0,
        );
        close(window([5.0, 5.0 * 640.0 / 480.0, -5.0]), [0.0, 480.0], 5.0);
        // The game's clip range and size; frame_vertex takes them as given.
        assert_eq!(v.w_clip, Some(([0.0625 / 1024.0 * zs, zs], false)));
        assert_eq!(
            (v.guest_size, v.size, v.z_scale),
            ((640, 480), (640, 480), zs)
        );
        let u = frame_vertex(&VertexUniforms::default(), &v);
        assert_eq!(u.constants[..VIEW_CONSTANTS], v.constants[..]);
        assert_eq!(u.shift, [0.0; 2]);
    }

    #[test]
    fn a_frame_puts_the_view_constants_first_and_scales_the_shift() {
        let mut base = VertexUniforms::default();
        base.constants[18] = [10.0, 10.0, 0.0, 0.0];
        let mut constants = [[0.0; 4]; VIEW_CONSTANTS];
        constants[0] = [1.0, 2.0, 3.0, 4.0];
        let view = ViewInputs {
            constants,
            guest_size: (640, 480),
            size: (1280, 960),
            z_scale: 16_777_215.0,
            w_clip: None,
        };
        let u = frame_vertex(&base, &view);
        assert_eq!(u.constants[0], [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(
            u.constants[18],
            [10.0, 10.0, 0.0, 0.0],
            "instance constants kept"
        );
        assert_eq!(u.surface, [640.0, 480.0]);
        assert_eq!(u.w_clip, Some([0.0, 16_777_215.0]));
        assert_eq!(u.shift, [3.0 / 8.0; 2], "(2s - 1) / 4s at 2x");
        assert_eq!(
            frame_vertex(
                &base,
                &ViewInputs {
                    size: (640, 480),
                    ..view
                }
            )
            .shift,
            [0.0; 2]
        );
        let wide = ViewInputs {
            size: (1600, 480),
            ..view
        };
        assert_eq!(frame_vertex(&base, &wide).shift, [0.4, 0.0]);
        let p = frame_pixel(
            &PixelUniforms::default(),
            &ViewInputs {
                z_scale: 65_535.0,
                ..view
            },
        );
        assert_eq!((p.depth[0], p.depth_range[1]), (65_535.0, 65_535.0));
        let mut offset = PixelUniforms::default();
        crate::pass_state::DepthOffset::apply(
            Some(crate::pass_state::DepthOffset { units: -2.0, slope: 1.5 }),
            &mut offset,
        );
        let p = frame_pixel(&offset, &ViewInputs { z_scale: 65_535.0, ..view });
        assert_eq!(p.depth, [65_535.0, -2.0, 1.5, 1.0], "the pass's polygon offset over the frame's scale");
        // The device's clip range and clamp, as the D3D path takes them
        // (`w_clip_range`, `depth_clamp`): a cinematic's
        // range past the camera's far, clamped.
        let clipped = ViewInputs {
            w_clip: Some(([0.0, 33_554_430.0], true)),
            ..view
        };
        let u = frame_vertex(&base, &clipped);
        assert_eq!((u.w_clip, u.w_clamp), (Some([0.0, 33_554_430.0]), true));
        let p = frame_pixel(&PixelUniforms::default(), &clipped);
        assert_eq!(p.depth_range, [0.0, 33_554_430.0]);
        assert_eq!(
            p.stipple_scale,
            PixelUniforms::stipple_grid((640, 480), (1280, 960))
        );
        assert!(
            !frame_vertex(&base, &view).w_clamp,
            "none: the whole range, culled"
        );
    }

    #[test]
    fn selected_view_patches_match_the_full_uniform_blocks_byte_for_byte() {
        let mut vertex = VertexUniforms::default();
        vertex.constants[MODEL_ROWS] = [-0.0, 2.0, f32::from_bits(0x7fc1_2345), 4.0];
        vertex.defaults[1] = [3.0; 4];
        let pixel = PixelUniforms {
            lod_bias: [0.25; 4],
            stipple: [17; 32],
            ..Default::default()
        };
        let mut vb = vertex.to_bytes();
        let mut pb = pixel.to_bytes();
        for (size, w_clip) in [((8, 4), None), ((20, 9), Some(([2.0, 7.0], true)))] {
            let view = ViewInputs {
                constants: [[1.0, -0.0, 3.0, 4.0]; VIEW_CONSTANTS],
                size,
                guest_size: (8, 4),
                z_scale: 10.0,
                w_clip,
            };
            let bytes = view_bytes(&view);
            vb[..VIEW_PREFIX].copy_from_slice(&bytes[..VIEW_PREFIX]);
            vb[VERTEX_VIEW..VERTEX_VIEW + 32]
                .copy_from_slice(&bytes[VIEW_PREFIX..VIEW_PREFIX + 32]);
            pb[PIXEL_TAIL..].copy_from_slice(&bytes[VIEW_PREFIX + 32..]);
            assert_eq!(vb, frame_vertex(&vertex, &view).to_bytes());
            assert_eq!(pb, frame_pixel(&pixel, &view).to_bytes());
        }
    }

    #[test]
    fn bsp_resources_share_matching_signed_images_and_material_page_groups_only_within_the_load() {
        let Ok(ctx) = gfx::GfxContext::headless() else {
            return;
        };
        let page = LightmapKey {
            page: 1,
            palette: Some(2),
        };
        let other = LightmapKey { palette: Some(3), ..page };
        let batch = |material, lightmap| crate::world::Batch {
            material,
            lightmap: Some(lightmap),
            first_draw: 0,
            draw_count: 0,
        };
        let world = World {
            batches: vec![
                batch(0, page),
                batch(0, page),
                batch(0, other),
                batch(1, page),
                batch(2, page),
            ],
            ..Default::default()
        };
        let make = || {
            let mut program = compile(&plain_look(), &plain_program()).unwrap();
            program.dims[..2].fill(Some(TextureDim::D2));
            program.stages[0] = Stage::Bitmap {
                bitmap: map::TagId(7),
                image: 1,
            };
            program.stages[1] = Stage::Lightmap;
            let mut signed = program.clone();
            signed.signed[0][0] = true;
            let mut images = 0;
            let mut pages = 0;
            let set = ProgramSet::new(
                &ctx.device,
                &ctx.queue,
                &world,
                [(0, program.clone()), (1, program), (2, signed)].into(),
                &mut |_| {
                    images += 1;
                    Some(Image::solid([7; 4]))
                },
                &mut |_| {
                    pages += 1;
                    Some(Image::solid([9; 4]))
                },
            );
            assert_eq!((images, pages), (2, 2));
            set
        };
        let mut set = make();
        assert_eq!(
            set.materials[&0].stage_views[0],
            set.materials[&1].stage_views[0]
        );
        assert_ne!(
            set.materials[&0].stage_views[0],
            set.materials[&2].stage_views[0]
        );
        let groups = &set.batch_groups;
        assert!(groups.iter().all(Option::is_some));
        assert_eq!(groups[0], groups[1]);
        assert_ne!(groups[0], groups[2]);
        assert_ne!(groups[0], groups[3]);
        let replacement = make();
        assert_ne!(groups[0], replacement.batch_groups[0]);
        assert_ne!(
            set.materials[&0].stage_views[0],
            replacement.materials[&0].stage_views[0]
        );

        let format = wgpu::TextureFormat::Rgba8Unorm;
        let depth = wgpu::TextureFormat::Depth24PlusStencil8;
        let stencil = wgpu::StencilState::default();
        // A pending compile must refuse the whole reservation without running
        // the compiler on the frame thread. This pending value has no worker.
        let key = (format, depth, stencil.clone(), None);
        let pending = PendingPipeline::new(|| panic!("admission tried to compile a pipeline"));
        set.materials
            .get_mut(&1)
            .unwrap()
            .pipelines
            .pending
            .insert(key.clone(), pending.clone());
        let selected = [(
            1,
            MaterialUse {
                cluster: true,
                rigid: false,
                stippled: false,
            },
        )]
        .into();
        assert!(!set.selected_pipelines(
            &ctx.device,
            format,
            depth,
            &stencil,
            &selected,
            PipelineUse::Preflight
        ));
        assert!(pending.built_on().is_none());
        assert!(set.batch(3, 1, format, depth, &stencil).is_none());
        set.materials
            .get_mut(&1)
            .unwrap()
            .pipelines
            .pending
            .remove(&key);
        set.selected_pipelines(
            &ctx.device,
            format,
            depth,
            &stencil,
            &selected,
            PipelineUse::Direct,
        );
        let pipeline = set.batch(3, 1, format, depth, &stencil).unwrap().0.clone();
        set.selected_pipelines(
            &ctx.device,
            format,
            depth,
            &stencil,
            &selected,
            PipelineUse::Direct,
        );
        assert_eq!(
            &pipeline,
            set.batch(3, 1, format, depth, &stencil).unwrap().0
        );
        assert!(set.materials[&0].pipelines.pending.is_empty());
        assert!(replacement.materials[&1].pipelines.pending.is_empty());
    }

    /// `len` bytes of `buffer` from its start.
    fn read_buffer(ctx: &gfx::GfxContext, buffer: &wgpu::Buffer, len: u64) -> Vec<u8> {
        let out = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: len,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        enc.copy_buffer_to_buffer(buffer, 0, &out, 0, len);
        ctx.queue.submit([enc.finish()]);
        out.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
        ctx.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        out.slice(..).get_mapped_range().unwrap().to_vec()
    }

    /// A program with a bitmap stage and a lightmap stage, as a world
    /// material's.
    fn staged_program() -> MaterialProgram {
        let mut program = compile(&plain_look(), &plain_program()).unwrap();
        program.dims[..2].fill(Some(TextureDim::D2));
        program.stages[0] = Stage::Bitmap {
            bitmap: map::TagId(7),
            image: 1,
        };
        program.stages[1] = Stage::Lightmap;
        program
    }

    #[test]
    fn a_constant_edit_changes_constants_and_anything_else_the_program() {
        let base = staged_program();
        assert_eq!(base.change_to(&base.clone()), ProgramChange::None);
        let mut constant = base.clone();
        constant.vertex.constants[20][0] += 1.0;
        assert_eq!(base.change_to(&constant), ProgramChange::Constants);
        let mut colour = base.clone();
        colour.pixel = constant.pixel;
        colour.def.constant0[0] ^= 1;
        assert_eq!(
            base.change_to(&colour),
            ProgramChange::None,
            "the definition's constants reach the GPU through the uniforms"
        );
        let mut bitmap = base.clone();
        bitmap.stages[0] = Stage::Bitmap {
            bitmap: map::TagId(8),
            image: 1,
        };
        assert_eq!(base.change_to(&bitmap), ProgramChange::Program);
        let mut module = base.clone();
        module.wgsl.push_str("\n// edited");
        assert_eq!(base.change_to(&module), ProgramChange::Program);
    }

    /// A constants-only refresh rewrites the buffers in place; the set
    /// then holds what a set built from the edited program holds.
    #[test]
    fn a_refreshed_set_equals_one_built_from_the_edited_shader() {
        let Ok(ctx) = gfx::GfxContext::headless() else {
            return;
        };
        let page = LightmapKey {
            page: 0,
            palette: Some(0),
        };
        let world = World {
            batches: vec![crate::world::Batch {
                material: 0,
                lightmap: Some(page),
                first_draw: 0,
                draw_count: 0,
            }],
            ..Default::default()
        };
        let build = |program: &MaterialProgram| {
            ProgramSet::new(
                &ctx.device,
                &ctx.queue,
                &world,
                [(0, program.clone())].into(),
                &mut |_| Some(Image::solid([7; 4])),
                &mut |_| Some(Image::solid([9; 4])),
            )
        };
        let base = staged_program();
        let mut edited = base.clone();
        edited.vertex.constants[20] = [0.25, 0.5, 0.75, 1.0];
        edited.pixel.c0[0] = [0.5; 4];
        let mut set = build(&base);
        let (vertex, pixel) = (
            set.materials[&0].vertex_buffer.clone(),
            set.materials[&0].pixel_buffer.clone(),
        );
        let done = set
            .refresh(
                &ctx.device,
                &ctx.queue,
                &world,
                [(0, edited.clone())].into(),
                &mut |_| panic!("no decode for constants"),
                &HashMap::new(),
            )
            .unwrap();
        assert_eq!(
            done,
            Refreshed {
                constants: vec![0],
                ..Default::default()
            }
        );
        assert_eq!(set.materials[&0].vertex_buffer, vertex, "in place");
        let fresh = build(&edited);
        assert_eq!(
            set.program(0).unwrap().vertex,
            fresh.program(0).unwrap().vertex
        );
        assert_eq!(
            set.program(0).unwrap().pixel,
            fresh.program(0).unwrap().pixel
        );
        let (vl, pl) = (
            VertexUniforms::SIZE as u64,
            set.materials[&0].program.pixel.to_bytes().len() as u64,
        );
        assert_eq!(
            read_buffer(&ctx, &vertex, vl),
            read_buffer(&ctx, &fresh.materials[&0].vertex_buffer, vl)
        );
        assert_eq!(
            read_buffer(&ctx, &pixel, pl),
            read_buffer(&ctx, &fresh.materials[&0].pixel_buffer, pl)
        );
    }

    /// A change past the constants (a template's: another stage image)
    /// builds only the edited material again, with the load's lightmap
    /// page; the other material keeps every resource it had.
    #[test]
    fn a_program_change_rebuilds_only_the_materials_that_use_it() {
        let Ok(ctx) = gfx::GfxContext::headless() else {
            return;
        };
        let page = LightmapKey {
            page: 0,
            palette: Some(0),
        };
        let batch = |material| crate::world::Batch {
            material,
            lightmap: Some(page),
            first_draw: 0,
            draw_count: 0,
        };
        let world = World {
            batches: vec![batch(0), batch(1), batch(0)],
            ..Default::default()
        };
        let base = staged_program();
        let mut pages = 0;
        let mut set = ProgramSet::new(
            &ctx.device,
            &ctx.queue,
            &world,
            [(0, base.clone()), (1, base.clone())].into(),
            &mut |_| Some(Image::solid([7; 4])),
            &mut |_| {
                pages += 1;
                Some(Image::solid([9; 4]))
            },
        );
        assert_eq!(pages, 1);
        let other = (
            set.materials[&1].vertex_buffer.clone(),
            set.batch_groups[1].clone(),
        );
        let old = (
            set.materials[&0].vertex_buffer.clone(),
            set.batch_groups[0].clone(),
        );
        let mut edited = base.clone();
        edited.stages[0] = Stage::Bitmap {
            bitmap: map::TagId(8),
            image: 0,
        };
        let mut decoded = Vec::new();
        let done = set
            .refresh(
                &ctx.device,
                &ctx.queue,
                &world,
                [(0, edited)].into(),
                &mut |s| {
                    decoded.push(s);
                    Some(Image::solid([3; 4]))
                },
                &HashMap::new(),
            )
            .unwrap();
        assert_eq!(
            done,
            Refreshed {
                rebuilt: vec![0],
                ..Default::default()
            }
        );
        assert_eq!(
            decoded,
            vec![Stage::Bitmap {
                bitmap: map::TagId(8),
                image: 0
            }]
        );
        assert_ne!(set.materials[&0].vertex_buffer, old.0);
        assert_ne!(set.batch_groups[0], old.1);
        assert_eq!(
            set.batch_groups[0], set.batch_groups[2],
            "one group per material and page"
        );
        assert_eq!(
            (
                set.materials[&1].vertex_buffer.clone(),
                set.batch_groups[1].clone()
            ),
            other
        );
        let texture = set.materials[&0].stage_views[0]
            .as_ref()
            .unwrap()
            .texture()
            .clone();
        assert_eq!(first_texel(&ctx, &texture), [3; 4]);
        // A material with no program refuses the whole refresh.
        let refused = set.refresh(
            &ctx.device,
            &ctx.queue,
            &world,
            [(0, base.clone()), (5, base)].into(),
            &mut |_| None,
            &HashMap::new(),
        );
        assert!(refused.is_err());
        assert_eq!(
            set.program(0).unwrap().stages[0],
            Stage::Bitmap {
                bitmap: map::TagId(8),
                image: 0
            },
            "nothing swapped"
        );
    }

    /// The texel at the origin of `t`'s first level (an RGBA8 texture).
    fn first_texel(ctx: &gfx::GfxContext, t: &wgpu::Texture) -> [u8; 4] {
        let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 256,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            t.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(1),
                },
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        ctx.queue.submit([enc.finish()]);
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.unwrap());
        ctx.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        let bytes = buffer.slice(..).get_mapped_range().unwrap().to_vec();
        [bytes[0], bytes[1], bytes[2], bytes[3]]
    }

    /// A live edit's pixels land in every texture made from the image,
    /// each signed variant biased as at load; a size change is refused
    /// and writes nothing (docs/editor/realtime-edits.md, "The native
    /// refresh").
    #[test]
    fn an_image_rewrite_reaches_every_variant_and_a_size_change_is_refused() {
        let Ok(ctx) = gfx::GfxContext::headless() else {
            return;
        };
        let world = World {
            batches: vec![crate::world::Batch {
                material: 0,
                lightmap: None,
                first_draw: 0,
                draw_count: 0,
            }],
            ..Default::default()
        };
        let bitmap = map::TagId(7);
        let mut program = compile(&plain_look(), &plain_program()).unwrap();
        program.dims[0] = Some(TextureDim::D2);
        program.stages[0] = Stage::Bitmap { bitmap, image: 1 };
        let mut signed = program.clone();
        signed.signed[0][0] = true;
        let set = ProgramSet::new(
            &ctx.device,
            &ctx.queue,
            &world,
            [(0, program.clone()), (1, program), (2, signed)].into(),
            &mut |_| Some(Image::solid([1, 2, 3, 4])),
            &mut |_| None,
        );
        assert_eq!(set.image_stages(bitmap, 1), vec![0]);
        assert!(set.image_stages(bitmap, 0).is_empty());
        let plain = set.materials[&0].stage_views[0]
            .as_ref()
            .unwrap()
            .texture()
            .clone();
        let biased = set.materials[&2].stage_views[0]
            .as_ref()
            .unwrap()
            .texture()
            .clone();

        let decoded = HashMap::from([(0, Image::solid([10, 20, 30, 40]))]);
        assert_eq!(
            set.rewrite_image(&ctx.queue, bitmap, 1, &decoded),
            Ok(2),
            "shared plain texture once, signed once"
        );
        assert_eq!(first_texel(&ctx, &plain), [10, 20, 30, 40]);
        assert_eq!(first_texel(&ctx, &biased), [10 ^ 0x80, 20, 30, 40]);

        let bigger = HashMap::from([(0, Image::level(2, 2, vec![99; 16]).unwrap())]);
        assert_eq!(
            set.rewrite_image(&ctx.queue, bitmap, 1, &bigger),
            Err(crate::gpu::ImageRefusal::Size)
        );
        assert_eq!(
            first_texel(&ctx, &plain),
            [10, 20, 30, 40],
            "refused: nothing written"
        );
        assert_eq!(
            set.rewrite_image(&ctx.queue, bitmap, 1, &HashMap::new()),
            Err(crate::gpu::ImageRefusal::NotDecoded)
        );
        assert_eq!(
            set.rewrite_image(&ctx.queue, map::TagId(8), 1, &decoded),
            Ok(0),
            "no texture of another bitmap"
        );
    }

    /// A lightmap page rewritten in place changes the texel the batch
    /// group samples, and every holder of the page is one texture; a size
    /// change is refused and writes nothing (docs/editor/realtime-edits.md,
    /// "The native refresh").
    #[test]
    fn a_lightmap_page_rewrite_changes_the_sampled_texel() {
        use crate::world::{PartInput, Placement, SectionInput, WorldBuilder};
        let Ok(ctx) = gfx::GfxContext::headless() else {
            return;
        };
        let page = LightmapKey {
            page: 3,
            palette: Some(1),
        };
        let mut b = WorldBuilder::new();
        b.add_section(
            Owner::Cluster(0),
            &SectionInput {
                positions: vec![
                    [10.0, 2.0, -2.0],
                    [10.0, -2.0, -2.0],
                    [10.0, 2.0, 2.0],
                    [10.0, -2.0, 2.0],
                ],
                uvs: vec![[0.5; 2]; 4],
                lightmap_uvs: vec![[0.5; 2]; 4],
                normals: vec![[-1.0, 0.0, 0.0]; 4],
                parts: vec![PartInput {
                    material: 0,
                    transparent: false,
                    triangles: vec![[0, 1, 2], [1, 3, 2]],
                }],
                ..Default::default()
            },
            &Placement::IDENTITY,
            Some(page),
        );
        let world = b.finish();
        let mut program = compile(&plain_look(), &plain_program()).unwrap();
        program.dims[..2].fill(Some(TextureDim::D2));
        program.stages[0] = Stage::Bitmap {
            bitmap: map::TagId(7),
            image: 1,
        };
        program.stages[1] = Stage::Lightmap;
        let set = ProgramSet::new(
            &ctx.device,
            &ctx.queue,
            &world,
            [(0, program)].into(),
            &mut |_| Some(Image::solid([1, 2, 3, 4])),
            &mut |_| Some(Image::solid([7, 8, 9, 10])),
        );
        let held = set.page_textures(page);
        assert_eq!(held.len(), 1, "one texture holds the page");
        assert_eq!(first_texel(&ctx, &held[0]), [7, 8, 9, 10]);
        assert!(
            set.page_textures(LightmapKey {
                page: 3,
                palette: Some(2)
            })
            .is_empty(),
            "another palette is another page"
        );
        assert_eq!(
            crate::gpu::rewrite_pages(&ctx.queue, &held, &Image::solid([70, 80, 90, 100])),
            Ok(1)
        );
        assert_eq!(first_texel(&ctx, &held[0]), [70, 80, 90, 100]);
        let bigger = Image::level(2, 2, vec![99; 16]).unwrap();
        assert_eq!(
            crate::gpu::rewrite_pages(&ctx.queue, &held, &bigger),
            Err(crate::gpu::ImageRefusal::Size)
        );
        assert_eq!(
            first_texel(&ctx, &held[0]),
            [70, 80, 90, 100],
            "refused: nothing written"
        );
        assert_eq!(
            set.held(map::TagId(7), 1).len(),
            1,
            "the stage image is held beside the page"
        );
    }

    /// The world's packets (`GpuWorld::selection_draws`) with their view
    /// rows written at the front of the submission draw what the selected
    /// pass draws on its own encoder, adjacent triangle lists joined, and
    /// write exactly the ranges the ordered path writes.
    #[test]
    fn world_packets_draw_what_the_selected_pass_draws_with_the_view_written_up_front() {
        draw_world_samples(false);
    }

    #[test]
    fn combined_world_packets_sample_each_copys_accumulation_level_in_guest_texels() {
        draw_world_samples(true);
    }

    fn draw_world_samples(accumulation: bool) {
        use crate::camera::RenderCamera;
        use crate::gpu::{DepthRule, GpuWorld, PassViewport};
        use crate::world::{PartInput, Placement, SectionInput, WorldBuilder};
        use wgpu::{TextureFormat as F, TextureUsages as U};
        let Ok(ctx) = gfx::GfxContext::headless() else {
            return;
        };
        const SIZE: u32 = 32;
        let wall = |x: f32, material| SectionInput {
            positions: vec![
                [x, 2.0, -2.0],
                [x, -2.0, -2.0],
                [x, 2.0, 2.0],
                [x, -2.0, 2.0],
            ],
            uvs: vec![[0.5; 2]; 4],
            lightmap_uvs: vec![[0.5; 2]; 4],
            normals: vec![[-1.0, 0.0, 0.0]; 4],
            parts: vec![PartInput {
                material,
                transparent: false,
                triangles: vec![[0, 1, 2], [1, 3, 2]],
            }],
            ..Default::default()
        };
        let key = LightmapKey {
            page: 0,
            palette: Some(0),
        };
        let page = Some(key);
        let mut b = WorldBuilder::new();
        b.add_section(
            Owner::Cluster(0),
            &wall(10.0, 0),
            &Placement::IDENTITY,
            page,
        );
        b.add_section(
            Owner::Cluster(1),
            &wall(12.0, 0),
            &Placement::IDENTITY,
            page,
        );
        b.add_section(
            Owner::Cluster(2),
            &wall(14.0, 1),
            &Placement::IDENTITY,
            page,
        );
        let world = b.finish();
        // The shader returns the bound atlas texel, making a wrong copy
        // or a missed bind visible in the target (including the packet path).
        use psh::bits::{CHAN_ALPHA, REG_T1, TEX_PROJECT2D, texture_modes};
        let shader = PixelShaderDef {
            final_inputs_abcd: u32::from(REG_T1),
            final_inputs_efg: u32::from(REG_T1 | CHAN_ALPHA) << 8,
            texture_modes: texture_modes([0, TEX_PROJECT2D, 0, 0]),
            ..Default::default()
        };
        let stage = if accumulation { Stage::Engine(crate::object_program::TEXACCUM_TEXTURE) } else { Stage::Lightmap };
        let look = PassLook {
            textures: [Stage::Empty, stage, Stage::Empty, Stage::Empty],
            pixel_shader: shader.to_bytes(),
            ..plain_look()
        };
        let mut program = compile(&look, &plain_program()).unwrap();
        if accumulation {
            program.texaccum_dimensions.push((
                tags::material::EngineVertexConstant { bank: 1, reg: 2, mode: 9, stage: 2, source: 0 },
                0b0011,
            ));
            program.vertex.constants[20] = [0.0, 0.0, -7.0, 11.0];
        }
        let mut set = ProgramSet::new(
            &ctx.device, &ctx.queue, &world, [(0, program.clone()), (1, program)].into(),
            &mut |_| None, &mut |_| Some(Image::solid([10, 50, 90, 255])),
        );
        if !accumulation {
            let on = HashMap::from([(key, Image::solid([240, 60, 90, 255]))]);
            let off = HashMap::from([(key, Image::solid([20, 60, 90, 255]))]);
            assert!(set.install_relight(&ctx.device, &ctx.queue, &world, &[on.clone(), HashMap::new()]).is_err());
            assert!(!set.has_relight(), "a missing lamp-off page refuses both arms");
            set.install_relight(&ctx.device, &ctx.queue, &world, &[on, off]).unwrap();
        }
        let mut gpu = GpuWorld::with_programs(&ctx.device, &ctx.queue, world, set);
        let camera = RenderCamera {
            position: [0.0; 3],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov: 60f32.to_radians(),
            projection_xy: None,
            viewport: [0, 0, SIZE as i16, SIZE as i16],
            near: 0.0625,
            far: 1024.0,
        };
        let parts = [
            (Owner::Cluster(0), 0),
            (Owner::Cluster(1), 0),
            (Owner::Cluster(2), 0),
        ];
        let mut selection = gpu
            .select_parts(&parts, &camera)
            .expect("all parts in view");
        if accumulation {
            assert!(!gpu.select_texaccum(&mut selection, None));
            let target = crate::world_layers::Texaccum { surface: Some(17), texture: 21, size: (4, 2) };
            assert!(gpu.select_texaccum(&mut selection, Some(target)));
        }
        assert!(gpu.prepare_selection(&ctx.device, F::Rgba8Unorm, F::Depth32Float, &selection));
        let mut constants = [[0.0; 4]; VIEW_CONSTANTS];
        constants[..4].copy_from_slice(&[
            [0.0, -2.0, 0.0, 16.0],
            [0.0, 0.0, -2.0, 16.0],
            [0.5, 0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0],
        ]);
        let view = ViewInputs {
            constants,
            guest_size: (SIZE, SIZE),
            size: (SIZE, SIZE),
            z_scale: 16_777_215.0,
            w_clip: None,
        };
        let texture = |format| {
            ctx.device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: SIZE,
                    height: SIZE,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: U::RENDER_ATTACHMENT | U::COPY_SRC,
                view_formats: &[],
            })
        };
        assert_eq!(gpu.relit_lightmap_ranges(&selection), if accumulation { 0 } else { parts.len() });
        let mut images = Vec::new();
        // Packets first: the rows they read are only those written ahead.
        let retail_cases = [
            (crate::lighting::Lighting::Retail, true, [10, 50, 90]),
            (crate::lighting::Lighting::Retail, false, [10, 50, 90]),
            (crate::lighting::Lighting::RelightOn, true, [240, 60, 90]),
            (crate::lighting::Lighting::RelightOn, false, [240, 60, 90]),
            (crate::lighting::Lighting::RelightOff, true, [20, 60, 90]),
            (crate::lighting::Lighting::RelightOff, false, [20, 60, 90]),
            (crate::lighting::Lighting::Retail, true, [10, 50, 90]),
        ];
        let combined_cases = [
            (crate::lighting::Lighting::Retail, true, [31, 75, 95]),
            (crate::lighting::Lighting::Retail, true, [75, 31, 95]),
        ];
        let cases: &[_] = if accumulation { &combined_cases } else { &retail_cases };
        for &(lighting, packets, expected) in cases {
            let (color, depth) = (texture(F::Rgba8Unorm), texture(F::Depth32Float));
            let (cv, dv) = (
                color.create_view(&Default::default()),
                depth.create_view(&Default::default()),
            );
            let target = gfx::RenderTarget {
                color: &cv,
                color_format: F::Rgba8Unorm,
                depth: Some(&dv),
                depth_format: Some(F::Depth32Float),
                size: (SIZE, SIZE),
                samples: 1,
            };
            let arena = gfx::ring::UploadPool::default().begin();
            let mut enc = ctx.device.create_command_encoder(&Default::default());
            drop(enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &cv,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &dv,
                    stencil_ops: None,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            }));
            let source = accumulation.then(|| {
                // The constant texture coordinate (.5,.5) is the centre of
                // guest texel zero. Scale this copy 2x; every texel outside
                // its top-left 2x2 is black.
                let mut rgba = vec![0; 8 * 4 * 4];
                for (x, y) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let at = (y * 8 + x) * 4;
                    rgba[at..at + 4].copy_from_slice(&[expected[0], expected[1], expected[2], 255]);
                }
                crate::gpu::texture(&ctx.device, &ctx.queue, &Image::level(8, 4, rgba).unwrap(), "synthetic target 1")
            });
            if accumulation {
                assert!(gpu.selection_draws_lit(F::Rgba8Unorm, F::Depth32Float, (SIZE, SIZE),
                    &selection, &view, lighting).is_err());
            }
            let draws = gpu
                .selection_draws_bound(
                    F::Rgba8Unorm,
                    F::Depth32Float,
                    (SIZE, SIZE),
                    &selection,
                    &view,
                    lighting,
                    source.as_ref().map(|v| (&ctx.device, v)),
                )
                .unwrap();
            assert_eq!(
                (draws.ranges, draws.packets.len()),
                (3, 2),
                "clusters 0 and 1 join"
            );
            assert_eq!(
                draws.writes.len(),
                if accumulation { 4 } else { 2 + 2 * 2 },
                "each material's pixel tail, its vertex prefix and viewport"
            );
            if accumulation {
                let vertex_blocks: Vec<_> = draws.writes.iter().filter(|(_, _, data)| data.len() == VertexUniforms::SIZE).collect();
                assert_eq!(vertex_blocks.len(), 2);
                for (_, offset, data) in vertex_blocks {
                    assert_eq!(*offset, 0);
                    assert_eq!(&data[20 * 16..21 * 16], bytemuck::bytes_of(&[4.0f32, 2.0, -7.0, 11.0]));
                }
            }
            if packets {
                // Ahead of the submission (`GfxBackend::write_ahead`).
                for (buffer, offset, bytes) in &draws.writes {
                    arena.write_front(&ctx.device, &ctx.queue, buffer, *offset, bytes);
                }
                gfx::Renderer::encode_pass(
                    &ctx,
                    &mut enc,
                    &target,
                    gfx::Clear::NONE,
                    &draws.packets,
                    None,
                )
                .unwrap();
            } else {
                let mut uploads = gfx::ring::EncoderUploads {
                    device: &ctx.device,
                    queue: &ctx.queue,
                    arena: &arena,
                    encoder: &mut enc,
                };
                assert!(gpu.write_selection(
                    &mut uploads,
                    &selection,
                    Some(&view),
                    &camera,
                    DepthRule::D24
                ));
                assert!(
                    draws
                        .writes
                        .iter()
                        .all(|(buffer, ..)| arena.wrote_in_order(buffer))
                );
                let vp = PassViewport {
                    x: 0.0,
                    y: 0.0,
                    width: SIZE as f32,
                    height: SIZE as f32,
                };
                assert_eq!(
                    gpu.encode_selection_lit(
                        &mut enc,
                        &cv,
                        F::Rgba8Unorm,
                        &dv,
                        F::Depth32Float,
                        vp,
                        (SIZE, SIZE),
                        &selection,
                        lighting,
                    ),
                    3
                );
            }
            let front = arena.upload_commands(&ctx.queue);
            ctx.queue.submit(front.into_iter().chain([enc.finish()]));
            ctx.device
                .poll(wgpu::PollType::wait_indefinitely())
                .unwrap();
            let read = |t, aspect| gfx::readback::read_texture(&ctx, t, aspect, 4, (SIZE, SIZE));
            let pixels = read(&color, wgpu::TextureAspect::All);
            assert!(pixels.chunks_exact(4).any(|p| p[..3] == expected), "{lighting:?}, packets {packets}");
            images.push((pixels, read(&depth, wgpu::TextureAspect::DepthOnly)));
        }
        if accumulation {
            assert_ne!(images[0].0, images[1].0);
            assert_eq!(images[0].1, images[1].1);
            return;
        }
        assert_eq!(images[0], images[1]);
        assert_eq!(images[2], images[3]);
        assert_eq!(images[4], images[5]);
        assert_eq!(images[0], images[6], "reference survives both display draws");
        assert_eq!(images[0].1, images[4].1, "swapping the lightmap cannot move a surface");
        assert!(gpu.discard_relight());
        assert_eq!(gpu.relit_lightmap_ranges(&selection), 0);
    }

    /// A game view's selected parts culled on the GPU
    /// (`GpuWorld::culled_selection_draws`): the guest's list whole, a part
    /// behind the camera and a repeated part included, draws what the
    /// frustum-checked packets of its visible parts draw, and the cull's
    /// slots keep exactly the parts `Frustum::sides` keeps.
    #[test]
    fn culled_world_packets_draw_what_the_admitted_packets_draw() {
        use crate::camera::RenderCamera;
        use crate::cull::Frustum;
        use crate::gpu::GpuWorld;
        use crate::world::{PartInput, Placement, SectionInput, WorldBuilder};
        use wgpu::{TextureFormat as F, TextureUsages as U};
        let Ok(ctx) = gfx::GfxContext::headless() else { return };
        const SIZE: u32 = 32;
        let wall = |x: f32, y: f32| SectionInput {
            positions: vec![[x, y + 2.0, -2.0], [x, y - 2.0, -2.0], [x, y + 2.0, 2.0], [x, y - 2.0, 2.0]],
            uvs: vec![[0.5; 2]; 4],
            lightmap_uvs: vec![[0.5; 2]; 4],
            normals: vec![[-1.0, 0.0, 0.0]; 4],
            parts: vec![PartInput { material: 0, transparent: false, triangles: vec![[0, 1, 2], [1, 3, 2]] }],
            ..Default::default()
        };
        let key = LightmapKey { page: 0, palette: Some(0) };
        let mut b = WorldBuilder::new();
        b.add_section(Owner::Cluster(0), &wall(10.0, 0.0), &Placement::IDENTITY, Some(key));
        // Behind the camera, and far to its side: both outside the lens.
        b.add_section(Owner::Cluster(1), &wall(-10.0, 0.0), &Placement::IDENTITY, Some(key));
        b.add_section(Owner::Cluster(2), &wall(10.0, 60.0), &Placement::IDENTITY, Some(key));
        let world = b.finish();
        let program = compile(&plain_look(), &plain_program()).unwrap();
        let set = ProgramSet::new(&ctx.device, &ctx.queue, &world, [(0, program)].into(),
            &mut |_| None, &mut |_| Some(Image::solid([10, 50, 90, 255])));
        let mut gpu = GpuWorld::with_programs(&ctx.device, &ctx.queue, world, set);
        let camera = RenderCamera {
            position: [0.0; 3],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov: 60f32.to_radians(),
            projection_xy: None,
            viewport: [0, 0, SIZE as i16, SIZE as i16],
            near: 0.0625,
            far: 1024.0,
        };
        let mut constants = [[0.0; 4]; VIEW_CONSTANTS];
        constants[..4].copy_from_slice(&[
            [0.0, -2.0, 0.0, 16.0], [0.0, 0.0, -2.0, 16.0], [0.5, 0.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0],
        ]);
        let view = ViewInputs { constants, guest_size: (SIZE, SIZE), size: (SIZE, SIZE), z_scale: 16_777_215.0, w_clip: None };
        let guest = [(Owner::Cluster(0), 0), (Owner::Cluster(1), 0), (Owner::Cluster(2), 0), (Owner::Cluster(0), 0)];
        assert!(gpu.select_parts(&guest, &camera).is_none(), "the CPU admission refuses the list");
        let all = gpu.select_unculled(&guest).unwrap();
        let visible = gpu.select_parts(&[(Owner::Cluster(0), 0); 2], &camera).unwrap();
        let mut images = Vec::new();
        for culled in [false, true] {
            let selection = if culled { &all } else { &visible };
            assert!(gpu.prepare_selection(&ctx.device, F::Rgba8Unorm, F::Depth32Float, selection));
            let texture = |format| ctx.device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
                mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format,
                usage: U::RENDER_ATTACHMENT | U::COPY_SRC, view_formats: &[],
            });
            let (color, depth) = (texture(F::Rgba8Unorm), texture(F::Depth32Float));
            let (cv, dv) = (color.create_view(&Default::default()), depth.create_view(&Default::default()));
            let target = gfx::RenderTarget {
                color: &cv, color_format: F::Rgba8Unorm, depth: Some(&dv), depth_format: Some(F::Depth32Float),
                size: (SIZE, SIZE), samples: 1,
            };
            let arena = gfx::ring::UploadPool::default().begin();
            let mut enc = ctx.device.create_command_encoder(&Default::default());
            drop(enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &cv, depth_slice: None, resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &dv, stencil_ops: None,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                }),
                timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
            }));
            let draws = if culled {
                let crate::gpu::CulledSelection { draws, late, dispatch } = gpu.culled_selection_draws(&arena,
                    &ctx.device, &ctx.queue, F::Rgba8Unorm, F::Depth32Float, (SIZE, SIZE), selection, &view, &camera,
                    crate::lighting::Lighting::Retail, None, None).unwrap();
                assert!(late.is_empty() && !dispatch.has_late(), "no depth to test: the frustum's set");
                assert_eq!((draws.ranges, draws.packets.len()), (4, 2), "slots 0..3 in one run, the repeat its own");
                dispatch.record(&mut enc);
                draws
            } else {
                gpu.selection_draws_bound(F::Rgba8Unorm, F::Depth32Float, (SIZE, SIZE), selection, &view,
                    crate::lighting::Lighting::Retail, None).unwrap()
            };
            for (buffer, offset, bytes) in &draws.writes {
                arena.write_front(&ctx.device, &ctx.queue, buffer, *offset, bytes);
            }
            gfx::Renderer::encode_pass(&ctx, &mut enc, &target, gfx::Clear::NONE, &draws.packets, None).unwrap();
            let front = arena.upload_commands(&ctx.queue);
            ctx.queue.submit(front.into_iter().chain([enc.finish()]));
            ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            let read = |t, aspect| gfx::readback::read_texture(&ctx, t, aspect, 4, (SIZE, SIZE));
            images.push((read(&color, wgpu::TextureAspect::All), read(&depth, wgpu::TextureAspect::DepthOnly)));
        }
        assert!(images[0].1.chunks_exact(4).any(|d| f32::from_le_bytes([d[0], d[1], d[2], d[3]]) < 1.0), "the wall is drawn");
        assert_eq!(images[0], images[1], "culled packets draw what the admitted ones draw");
        let frustum = Frustum::sides(&camera.view_projection());
        let want: Vec<bool> = gpu.world().draws.iter().map(|d| frustum.may_see(&d.bounds)).collect();
        assert_eq!(want, [true, false, false]);
        assert_eq!(gpu.read_guest_drawn(&ctx.device, &ctx.queue), want);
        let stats = gpu.read_guest_cull_stats(&ctx.device, &ctx.queue);
        assert_eq!((stats.visible, stats.total), (1, 3));

        // The pyramid's model of the programs' depth bounds what they wrote,
        // to the step: the wall's least stored depth is its nearest corner's.
        let model = crate::cull_gpu::DepthModel::program(&view);
        let least = images[1].1.chunks_exact(4).map(|d| f32::from_le_bytes([d[0], d[1], d[2], d[3]]))
            .fold(f32::INFINITY, f32::min);
        let nearest = model.nearest(&gpu.world().draws[0].bounds);
        assert!(nearest <= least && least - nearest <= 1.0 / view.z_scale, "model {nearest}, stored {least}");
        // A clip range above the wall clamps it up: the model follows.
        let clamped = crate::cull_gpu::DepthModel::program(&ViewInputs { w_clip: Some(([40.0, 1000.0], true)), ..view });
        assert_eq!(clamped.nearest(&gpu.world().draws[0].bounds), 40.0 / view.z_scale);
    }

    /// A game view's occlusion (`GpuWorld::culled_selection_draws` with
    /// its depth): a box behind a wall drawn last view is drawn early and
    /// found hidden, so the next view draws only the wall; the box is drawn
    /// in the view the wall leaves, by the late run. A binding whose order
    /// matters (here depth-equal) has no late packets and draws from the
    /// frustum alone, early. A depth target the pass cannot read is a
    /// named skip.
    #[test]
    fn game_view_occlusion_hides_what_the_wall_covers_and_keeps_ordered_draws_early() {
        use crate::camera::RenderCamera;
        use crate::gpu::{CulledSelection, GpuWorld, PassViewport, keeps_order};
        use crate::world::{PartInput, Placement, SectionInput, WorldBuilder};
        use wgpu::{TextureFormat as F, TextureUsages as U};
        let Ok(ctx) = gfx::GfxContext::headless() else { return };
        const SIZE: u32 = 32;
        let quad = |x: f32, half: f32, material: u16| SectionInput {
            positions: vec![[x, half, -half], [x, -half, -half], [x, half, half], [x, -half, half]],
            uvs: vec![[0.5; 2]; 4],
            lightmap_uvs: vec![[0.5; 2]; 4],
            normals: vec![[-1.0, 0.0, 0.0]; 4],
            parts: vec![PartInput { material, transparent: false, triangles: vec![[0, 1, 2], [1, 3, 2]] }],
            ..Default::default()
        };
        let key = LightmapKey { page: 0, palette: Some(0) };
        let mut b = WorldBuilder::new();
        // The wall fills the lens; both boxes lie behind it.
        b.add_section(Owner::Cluster(0), &quad(10.0, 8.0, 0), &Placement::IDENTITY, Some(key));
        b.add_section(Owner::Cluster(1), &quad(20.0, 1.0, 0), &Placement::IDENTITY, Some(key));
        b.add_section(Owner::Cluster(2), &quad(20.0, 1.0, 1), &Placement::IDENTITY, Some(key));
        let world = b.finish();
        let opaque = compile(&plain_look(), &game_program()).unwrap();
        let mut equal = opaque.clone();
        equal.state = Some(PassState { depth_compare: wgpu::CompareFunction::Equal, ..world_pass(plain_look()).state });
        let set = ProgramSet::new(&ctx.device, &ctx.queue, &world, [(0, opaque), (1, equal)].into(),
            &mut |_| None, &mut |_| Some(Image::solid([10, 50, 90, 255])));
        let mut gpu = GpuWorld::with_programs(&ctx.device, &ctx.queue, world, set);
        let camera = RenderCamera {
            position: [0.0; 3],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov: 60f32.to_radians(),
            projection_xy: None,
            viewport: [0, 0, SIZE as i16, SIZE as i16],
            near: 0.0625,
            far: 1024.0,
        };
        let view = ViewInputs::from_camera(&camera, (SIZE, SIZE));
        let full = PassViewport { x: 0.0, y: 0.0, width: SIZE as f32, height: SIZE as f32 };
        let texture = |format, usage| ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format,
            usage, view_formats: &[],
        });
        let color = texture(F::Rgba8Unorm, U::RENDER_ATTACHMENT | U::COPY_SRC);
        let frame = |gpu: &mut GpuWorld, guest: &[(Owner, u8)], depth: &wgpu::Texture| {
            let selection = gpu.select_unculled(guest).unwrap();
            assert!(gpu.prepare_selection(&ctx.device, F::Rgba8Unorm, F::Depth32Float, &selection));
            let (cv, dv) = (color.create_view(&Default::default()), depth.create_view(&Default::default()));
            let target = gfx::RenderTarget {
                color: &cv, color_format: F::Rgba8Unorm, depth: Some(&dv), depth_format: Some(F::Depth32Float),
                size: (SIZE, SIZE), samples: 1,
            };
            let arena = gfx::ring::UploadPool::default().begin();
            let mut enc = ctx.device.create_command_encoder(&Default::default());
            drop(enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &cv, depth_slice: None, resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &dv, stencil_ops: None,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                }),
                timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
            }));
            let CulledSelection { draws, late, dispatch } = gpu.culled_selection_draws(&arena, &ctx.device,
                &ctx.queue, F::Rgba8Unorm, F::Depth32Float, (SIZE, SIZE), &selection, &view, &camera,
                crate::lighting::Lighting::Retail, None, Some((depth, &camera, full, 1))).unwrap();
            for (buffer, offset, bytes) in &draws.writes {
                arena.write_front(&ctx.device, &ctx.queue, buffer, *offset, bytes);
            }
            dispatch.record(&mut enc);
            gfx::Renderer::encode_pass(&ctx, &mut enc, &target, gfx::Clear::NONE, &draws.packets, None).unwrap();
            dispatch.record_late(&mut enc);
            if !late.is_empty() {
                gfx::Renderer::encode_pass(&ctx, &mut enc, &target, gfx::Clear::NONE, &late, None).unwrap();
            }
            let front = arena.upload_commands(&ctx.queue);
            ctx.queue.submit(front.into_iter().chain([enc.finish()]));
            ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            let stats = gpu.read_guest_cull_stats(&ctx.device, &ctx.queue);
            (draws.packets.len(), late.len(), gpu.read_guest_drawn(&ctx.device, &ctx.queue), stats.occluded)
        };
        let depth = texture(F::Depth32Float, U::RENDER_ATTACHMENT | U::TEXTURE_BINDING);
        let all = [(Owner::Cluster(0), 0), (Owner::Cluster(1), 0), (Owner::Cluster(2), 0)];
        // The first view knows no history: all late but the ordered box.
        // The second draws the last view's set early and finds the box
        // hidden; the third leaves it out.
        let first = frame(&mut gpu, &all, &depth);
        assert_eq!(first, (2, 1, vec![true, true, true], 0), "one late packet: the opaque binding's");
        frame(&mut gpu, &all, &depth);
        for _ in 0..2 {
            assert_eq!(frame(&mut gpu, &all, &depth), (2, 1, vec![true, false, true], 1), "the box is hidden");
        }
        // Without the wall the box is drawn the view it is revealed.
        let open = [(Owner::Cluster(1), 0), (Owner::Cluster(2), 0)];
        assert_eq!(frame(&mut gpu, &open, &depth), (2, 1, vec![false, true, true], 0), "revealed, drawn late");
        assert_eq!(gpu.guest_occlusion_skips().depth_unsampled, 0);
        // A depth the pyramid cannot read: the frustum's set, counted.
        let unread = texture(F::Depth32Float, U::RENDER_ATTACHMENT);
        assert_eq!(frame(&mut gpu, &all, &unread), (2, 0, vec![true, true, true], 0));
        let skips = gpu.guest_occlusion_skips();
        assert_eq!((skips.depth_unsampled, skips.depth_model), (1, 0));

        let pipeline = |state: Option<&PassState>| described_pipeline(WORLD_CLUSTER_PROGRAM, F::Rgba8Unorm,
            F::Depth32Float, state, None, wgpu::StencilState::default());
        let opaque = world_pass(plain_look()).state;
        assert!(!keeps_order(&pipeline(None)) && !keeps_order(&pipeline(Some(&opaque))));
        let blended = PassState { blend: Some(wgpu::BlendState::ALPHA_BLENDING), ..opaque };
        let equal = PassState { depth_compare: wgpu::CompareFunction::Equal, ..opaque };
        let unwritten = PassState { depth_write: false, ..opaque };
        for state in [blended, equal, unwritten] {
            assert!(keeps_order(&pipeline(Some(&state))), "{state:?}");
        }
    }

    /// A pass that sets its own stencil draws it, not the device's at the
    /// layer; one that sets none takes the device's.
    #[test]
    fn a_pass_with_its_own_stencil_describes_it_over_the_layers() {
        use gfx::state::{CompareFunc, StencilOp, StencilState};
        use wgpu::TextureFormat as F;
        let device = crate::fog_gpu::layer_stencil(
            &StencilState { enable: true, pass: StencilOp::Replace, ..Default::default() },
            F::Depth24PlusStencil8,
        );
        let own = StencilState { enable: true, func: CompareFunc::Equal, reference: 0x80, write_mask: 0, ..Default::default() };
        let opaque = world_pass(plain_look()).state;
        let key = |state: &PassState| {
            described_pipeline(WORLD_CLUSTER_PROGRAM, F::Rgba8Unorm, F::Depth24PlusStencil8, Some(state), None, device.clone())
                .depth_stencil
                .unwrap()
                .stencil
        };
        assert_eq!(key(&opaque), device);
        let with_own = PassState { stencil: Some(own), ..opaque };
        assert_eq!(key(&with_own), crate::fog_gpu::layer_stencil(&own, F::Depth24PlusStencil8));
        assert_eq!(key(&with_own).front.compare, wgpu::CompareFunction::Equal);
        let off = PassState { stencil: Some(StencilState::default()), ..opaque };
        assert_eq!(key(&off), wgpu::StencilState::default(), "the pass's own test off");
    }

    /// The game's convention: the clip position divided by w, then
    /// `c[10]` and `c[11]` to guest pixels (`ViewInputs::from_camera`);
    /// `oPos.w` keeps w for the W-buffer. `dp4 r0, v0, c[0..3];
    /// rcc r1.x, r0.w; mul r0.xyz, r0, c10; mad oPos.xyz, r0, r1.x, c11;
    /// mov oPos.w, r0.w`, with `plain_program`'s texcoords.
    fn game_program() -> Vec<u8> {
        use shaders::vsh::encode::{Inst, Operand as O, program};
        use shaders::vsh::{IluOp, MacOp, OutReg};
        let row = |c, mask| Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(O::c(c)), None).to_temp(0, mask);
        program(&[
            row(0, 8),
            row(1, 4),
            row(2, 2),
            row(3, 1),
            Inst::ilu(IluOp::Rcc, O::r(0).swz("w")).ilu_to_temp(1, 8),
            Inst::mac(MacOp::Mul, Some(O::r(0)), Some(O::c(10)), None).to_temp(0, 0xe),
            Inst::mac(MacOp::Mad, Some(O::r(0)), Some(O::r(1).swz("x")), Some(O::c(11))).to_out(false, OutReg::Pos, 0xe),
            Inst::mac(MacOp::Mov, Some(O::r(0)), None, None).to_out(false, OutReg::Pos, 1),
            Inst::mac(MacOp::Mov, Some(O::v(1)), None, None).to_out(false, OutReg::T(0), 0xf),
            Inst::mac(MacOp::Mov, Some(O::v(6)), None, None).to_out(false, OutReg::T(1), 0xf),
        ])
    }

    #[test]
    fn per_draw_engine_inputs_keep_the_material_on_the_d3d_path() {
        let look = PassLook {
            per_draw_inputs: true,
            ..plain_look()
        };
        assert_eq!(
            compile(&look, &plain_program()).unwrap_err(),
            "per-draw engine inputs"
        );
        assert!(
            compile(&plain_look(), b"")
                .unwrap_err()
                .starts_with("vertex program"),
            "no microcode"
        );
    }

    fn plain_look() -> PassLook {
        PassLook {
            pass: map::TagId(0xe000_0001),
            implementation: 0,
            vertex_shader: map::TagId(0xe000_0002),
            textures: [Stage::Empty; 4],
            stage_states: Vec::new(),
            pixel_shader: shaders::PixelShaderDef::default().to_bytes(),
            vertex_constants: vec![Register {
                index: 19,
                value: [2.0, 3.0, 0.0, 0.0],
                written: 0b0011,
                origin: Origin::Instance,
            }],
            vertex_defaults: Vec::new(),
            per_draw_inputs: false,
        }
    }

    /// `dp4 oPos, v0, c[0..3]; mov oT0, v1; mov oT1, v6`.
    fn plain_program() -> Vec<u8> {
        use shaders::vsh::encode::{Inst, Operand as O, program};
        use shaders::vsh::{MacOp, OutReg};
        let row = |c, mask| {
            Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(O::c(c)), None).to_out(
                false,
                OutReg::Pos,
                mask,
            )
        };
        program(&[
            row(0, 8),
            row(1, 4),
            row(2, 2),
            row(3, 1),
            Inst::mac(MacOp::Mov, Some(O::v(1)), None, None).to_out(false, OutReg::T(0), 0xf),
            Inst::mac(MacOp::Mov, Some(O::v(6)), None, None).to_out(false, OutReg::T(1), 0xf),
        ])
    }

    #[test]
    fn a_plain_pass_links_into_a_valid_module_with_its_constants() {
        let p = compile(&plain_look(), &plain_program()).expect("compiles");
        shaders::validate(&p.wgsl).expect("valid WGSL");
        assert_eq!(p.vertex.constants[19], [2.0, 3.0, 0.0, 0.0]);
        assert_eq!(
            p.vertex.constants[TEXCOORD_DECOMPRESSION],
            [1.0, 1.0, 0.0, 0.0]
        );
        assert_eq!(p.vertex.constants[MODEL_ROWS + 1], [0.0, 1.0, 0.0, 0.0]);
        assert_eq!(p.dims, [None; 4], "samples nothing");
        assert_eq!(p.signed, [[false; 4]; 4]);
    }

    #[test]
    fn a_transparent_pass_carries_its_alpha_test() {
        let p = compile_pass(
            &plain_look(),
            &plain_program(),
            Some((wgpu::CompareFunction::Greater, 0x40)),
        )
        .unwrap();
        shaders::validate(&p.wgsl).expect("valid WGSL");
        assert!(
            p.wgsl.contains("xps.alpha_ref.x"),
            "the test is in the fragment shader"
        );
        assert_eq!(p.pixel.alpha_ref[0], 64.0);
        let plain = compile(&plain_look(), &plain_program()).unwrap();
        assert_eq!(plain.pixel.alpha_ref[0], 0.0);
        assert!(!plain.wgsl.contains("xps.alpha_ref.x"));
    }

    #[test]
    fn a_program_reading_an_unsupplied_register_stays_on_the_d3d_path() {
        use shaders::vsh::encode::{Inst, Operand as O, program};
        use shaders::vsh::{MacOp, OutReg};
        let reads_v9 = program(&[Inst::mac(MacOp::Mov, Some(O::v(9)), None, None).to_out(
            false,
            OutReg::Pos,
            0xf,
        )]);
        let e = compile(&plain_look(), &reads_v9).unwrap_err();
        assert!(e.contains("do not supply"), "{e}");
    }

    #[test]
    fn layer_1_accepts_an_unstreamed_colour_only_when_its_diffuse_output_is_unused() {
        use crate::fog_gpu::{FogPass, FogProgram};
        use psh::bits;
        use vsh::encode::{Inst, Operand as O, program};

        let mov = |src, dst| Inst::mac(vsh::MacOp::Mov, Some(src), None, None)
            .to_out(false, dst, 0xf);
        let colour = mov(O::v(10), vsh::OutReg::D0);
        let pos = mov(O::v(0), vsh::OutReg::Pos);
        let uv = mov(O::v(1), vsh::OutReg::T(0));
        let code = program(&[pos.clone(), uv.clone(), colour.clone()]);
        let mut pass = FogPass {
            state: world_pass(plain_look()).state,
            look: plain_look(),
            fog: Default::default(),
            planar: None,
        };
        pass.look.textures[0] = Stage::Bitmap { bitmap: map::TagId(9), image: 0 };
        let mut def = PixelShaderDef {
            texture_modes: bits::TEX_PROJECT2D,
            final_inputs_abcd: bits::inputs(0, 0, 0, bits::REG_T0),
            final_inputs_efg: bits::final_efg(0, 0, bits::REG_T0 | bits::CHAN_ALPHA, 0),
            ..Default::default()
        };
        pass.look.pixel_shader = def.to_bytes();
        let layer = FogProgram::compile(&pass, &code).unwrap();
        assert_eq!(layer.program.inputs, 0b11);
        shaders::validate(&layer.program.wgsl).unwrap();
        struct Texture;
        impl psh::interp::Sampler for Texture {
            fn sample(&self, _: usize, _: TextureDim, _: [f32; 3]) -> [f32; 4] {
                [0.25, 0.5, 0.75, 1.0]
            }
        }
        for d0 in [[0.0; 4], [1.0; 4]] {
            let inputs = psh::interp::Inputs { d0, t: [[0.0, 0.0, 0.0, 1.0]; 4], ..Default::default() };
            let result = psh::interp::run(
                &def, &layer.program.pixel_options, &inputs, &layer.program.pixel, &Texture,
            );
            assert_eq!(result, Some([0.25, 0.5, 0.75, 1.0]));
        }

        let refused = |pass: &FogPass, code: &[u8]| {
            assert!(FogProgram::compile(pass, code).unwrap_err().contains("0x0400"));
        };
        // The diffuse copy is dead, but the same immediate also reaches oPos.
        refused(&pass, &program(&[pos, uv, colour, mov(O::v(10), vsh::OutReg::Pos)]));
        // Both the general and the final combiners can consume diffuse.
        def.final_inputs_efg = bits::final_efg(0, 0, bits::REG_V0 | bits::CHAN_ALPHA, 0);
        pass.look.pixel_shader = def.to_bytes();
        refused(&pass, &code);
        def.final_inputs_efg = bits::final_efg(0, 0, bits::REG_T0 | bits::CHAN_ALPHA, 0);
        def.combiner_count = 1;
        def.rgb_inputs[0] = bits::inputs(bits::REG_V0, bits::ONE, 0, 0);
        def.rgb_outputs[0] = bits::outputs(bits::REG_T0, 0, 0, 0);
        pass.look.pixel_shader = def.to_bytes();
        refused(&pass, &code);
    }

    #[test]
    fn registers_the_pass_sets_come_from_its_defaults() {
        use shaders::vsh::encode::{Inst, Operand as O, program};
        use shaders::vsh::{MacOp, OutReg};
        // 03a's transparents read v10 and v11 (mask 0x0c00), colours the
        // pass sets with SET_VERTEX_DATA4F_M.
        let reads = program(&[
            Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(O::c(0)), None).to_out(
                false,
                OutReg::Pos,
                0xf,
            ),
            Inst::mac(MacOp::Mov, Some(O::v(10)), None, None).to_out(false, OutReg::D0, 0xf),
            Inst::mac(MacOp::Mov, Some(O::v(11)), None, None).to_out(false, OutReg::D1, 0xf),
            Inst::mac(MacOp::Mov, Some(O::v(1)), None, None).to_out(false, OutReg::T(0), 0xf),
        ]);
        let colour = |index, value| Register {
            index,
            value,
            written: 0xf,
            origin: Origin::Instance,
        };
        let look = PassLook {
            vertex_defaults: vec![
                colour(10, [0.45, 0.47, 0.43, 0.0]),
                colour(11, [0.29, 0.27, 0.25, 0.0]),
            ],
            ..plain_look()
        };
        let p = compile(&look, &reads).expect("v10 and v11 are the pass's");
        shaders::validate(&p.wgsl).expect("valid WGSL");
        assert!(p.wgsl.contains("xvs.defaults[10]") && p.wgsl.contains("xvs.defaults[11]"));
        assert_eq!(p.vertex.defaults[10], [0.45, 0.47, 0.43, 0.0]);
        assert_eq!(p.vertex.defaults[11], [0.29, 0.27, 0.25, 0.0]);
        // Only v11 set: v10 is still nobody's.
        let only_11 = PassLook {
            vertex_defaults: vec![colour(11, [0.0; 4])],
            ..plain_look()
        };
        let e = compile(&only_11, &reads).unwrap_err();
        assert!(e.contains("(0x0400)"), "{e}");
        assert_eq!(
            defaults_mask(&[colour(1, [0.0; 4]), colour(10, [0.0; 4])]),
            0x0402
        );
        assert_eq!(
            defaults_mask(&[Register {
                written: 0b0111,
                ..colour(10, [0.0; 4])
            }]),
            0
        );
    }

    #[test]
    fn signed_channels_are_stored_biased() {
        assert_eq!(
            signed_channels(1 << 29 | 1 << 28),
            [true, false, false, true]
        );
        let mut i = Image::from_levels(vec![
            (2, 1, vec![0x7f, 0x01, 0xff, 0x80, 0, 0, 0, 0]),
            (1, 1, vec![0x81, 0, 0, 0]),
        ])
        .unwrap();
        bias_signed(&mut i, [true, false, false, true]);
        assert_eq!(i.rgba, vec![0xff, 0x01, 0xff, 0x00, 0x80, 0, 0, 0x80]);
        assert_eq!(i.mips, vec![vec![0x01, 0, 0, 0x80]], "every level");
        // A signed channel keeps a DXT image off BC, as on the D3D path.
        let dxt = || {
            Image::from_levels(vec![(4, 4, vec![0; 64])])
                .unwrap()
                .with_blocks(wgpu::TextureFormat::Bc1RgbaUnorm, vec![vec![0; 8]])
        };
        let mut d = dxt();
        bias_signed(&mut d, [false; 4]);
        assert!(d.blocks.is_some(), "nothing signed: the blocks stay");
        assert!(
            describe_image(&d, true).starts_with("Bc1RgbaUnorm 4x4 levels 1"),
            "{}",
            describe_image(&d, true)
        );
        assert!(describe_image(&d, false).starts_with("Rgba8Unorm 4x4 levels 1"));
        bias_signed(&mut d, [true, false, false, false]);
        assert_eq!(d.blocks, None);
    }

    #[test]
    fn samplers_follow_the_stage_states() {
        let mut s = StageStates {
            address_u: 1,
            address_v: 3,
            address_w: 2,
            mag_filter: 1,
            min_filter: 2,
            mip_filter: 0,
            mip_lod_bias: 0,
            max_mip_level: 0,
            max_anisotropy: 0,
            color_key_op: 0,
            color_sign: 0,
            alpha_kill: 0,
            border_color: 0,
            color_key_color: 0,
            unread: [0; 2],
        };
        let d = sampler_descriptor(Some(&s));
        assert_eq!(
            (d.address_mode_u, d.address_mode_v, d.address_mode_w),
            (
                wgpu::AddressMode::Repeat,
                wgpu::AddressMode::ClampToEdge,
                wgpu::AddressMode::MirrorRepeat
            )
        );
        assert_eq!(
            (d.mag_filter, d.min_filter),
            (wgpu::FilterMode::Nearest, wgpu::FilterMode::Linear)
        );
        assert_eq!(d.lod_max_clamp, 0.0, "no mip filter: level 0 only");
        s.mip_filter = 2;
        assert_eq!(
            sampler_descriptor(Some(&s)).mipmap_filter,
            wgpu::MipmapFilterMode::Linear
        );
    }

    #[test]
    fn samplers_are_made_as_the_d3d_path_makes_them() {
        // 03a's bump stage: anisotropic (filter 3) at 4, trilinear.
        let mut s = stage_states();
        (s.mag_filter, s.min_filter, s.mip_filter, s.max_anisotropy) = (3, 3, 2, 4);
        (s.address_u, s.address_v, s.address_w) = (5, 4, 1);
        s.max_mip_level = 2;
        let d = sampler_descriptor(Some(&s));
        let caps = gfx::translate::Capabilities::from_features(wgpu::Features::empty());
        let want =
            gfx::translate::SamplerKey::from_stage(&stage_state(&s).unwrap(), caps).descriptor();
        assert_eq!(d.anisotropy_clamp, 4);
        assert_eq!(
            (d.mag_filter, d.min_filter, d.mipmap_filter),
            (want.mag_filter, want.min_filter, want.mipmap_filter)
        );
        assert_eq!(
            (d.address_mode_u, d.address_mode_v, d.address_mode_w),
            (
                wgpu::AddressMode::ClampToEdge,
                wgpu::AddressMode::ClampToEdge,
                wgpu::AddressMode::Repeat
            ),
            "5 is clamp to edge, not mirror; the border clamps to the edge"
        );
        assert_eq!(
            (d.lod_min_clamp, d.lod_max_clamp),
            (2.0, 32.0),
            "MAXMIPLEVEL is the smallest LOD"
        );
        // A value gfx does not decode still makes a sampler.
        s.mag_filter = 9;
        assert_eq!(
            sampler_descriptor(Some(&s)).address_mode_u,
            wgpu::AddressMode::ClampToEdge
        );
    }

    fn stage_states() -> StageStates {
        StageStates {
            address_u: 1,
            address_v: 1,
            address_w: 1,
            mag_filter: 2,
            min_filter: 2,
            mip_filter: 2,
            mip_lod_bias: 0,
            max_mip_level: 0,
            max_anisotropy: 0,
            color_key_op: 0,
            color_sign: 0,
            alpha_kill: 0,
            border_color: 0,
            color_key_color: 0,
            unread: [0; 2],
        }
    }

    #[test]
    fn the_lod_bias_reaches_the_pixel_uniforms() {
        let mut s = stage_states();
        s.mip_lod_bias = (-0.5f32).to_bits();
        let mut nan = stage_states();
        nan.mip_lod_bias = f32::NAN.to_bits();
        let look = PassLook {
            stage_states: vec![s, nan],
            ..plain_look()
        };
        let p = compile(&look, &plain_program()).unwrap();
        assert_eq!(p.pixel.lod_bias, [-0.5, 0.0, 0.0, 0.0]);
    }

    /// `plain_look` with a final combiner (`rgb = r0`, D3D's own without
    /// fog), as the world's layer-3 passes have one.
    fn final_look(fog: bool) -> PassLook {
        let mut def = shaders::PixelShaderDef::default();
        (def.final_inputs_abcd, def.final_inputs_efg) = psh::d3d_default_final_combiner(fog, false);
        PassLook {
            pixel_shader: def.to_bytes(),
            ..plain_look()
        }
    }

    fn world_pass(look: PassLook) -> WorldPass {
        let state = PassState {
            blend: None,
            blend_constant: wgpu::Color::TRANSPARENT,
            color_writes: wgpu::ColorWrites::ALL,
            depth_write: true,
            depth_compare: wgpu::CompareFunction::LessEqual,
            primitive: wgpu::PrimitiveState::default(),
            alpha_test: Some((wgpu::CompareFunction::Greater, 0x40)),
            stencil: None,
            depth_offset: None,
        };
        WorldPass {
            look,
            state,
            inherits_cull: false,
            texaccum_dimensions: Vec::new(),
        }
    }

    #[test]
    fn a_reversed_winding_flips_the_culled_face_s_front_and_leaves_an_unculled_pass() {
        let cw: Cull = (Some(wgpu::Face::Back), wgpu::FrontFace::Cw);
        assert_eq!(
            reversed_winding(None, Some(cw)),
            Some((Some(wgpu::Face::Back), wgpu::FrontFace::Ccw)),
            "an inherited cull"
        );
        assert_eq!(reversed_winding(None, None), None, "nothing culled stays so");
        let own = PassState {
            blend: None,
            blend_constant: wgpu::Color::BLACK,
            color_writes: wgpu::ColorWrites::ALL,
            depth_write: true,
            depth_compare: wgpu::CompareFunction::LessEqual,
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Front),
                front_face: wgpu::FrontFace::Ccw,
                ..Default::default()
            },
            alpha_test: None,
            stencil: None,
            depth_offset: None,
        };
        assert_eq!(
            reversed_winding(Some(&own), None),
            Some((Some(wgpu::Face::Front), wgpu::FrontFace::Cw)),
            "the pass's own cull"
        );
    }

    #[test]
    fn a_pass_that_inherits_its_cull_takes_the_device_s() {
        let cw: Cull = (Some(wgpu::Face::Front), wgpu::FrontFace::Cw);
        let cull = InheritedCull {
            device: Some(cw),
            placements: true,
            clusters: false,
        };
        assert_eq!(cull.of(true, false), Some(cw), "a placement");
        assert_eq!(cull.of(true, true), None, "a cluster, unless asked");
        assert_eq!(cull.of(false, false), None, "a pass that sets its own");
        assert_eq!(
            InheritedCull {
                clusters: true,
                ..cull
            }
            .of(true, true),
            Some(cw)
        );
        assert_eq!(
            InheritedCull {
                device: None,
                ..cull
            }
            .of(true, false),
            None,
            "not known"
        );
        let mut pass = world_pass(final_look(false));
        let (primitive, depth) = pipeline_state(Some(&pass.state), cull.of(true, false));
        assert_eq!((primitive.cull_mode, primitive.front_face), cw);
        assert_eq!(depth, (true, wgpu::CompareFunction::LessEqual));
        pass.state.primitive.front_face = wgpu::FrontFace::Ccw;
        assert_eq!(
            pipeline_state(Some(&pass.state), None).0,
            pass.state.primitive,
            "its own without one"
        );
        let world = pipeline_state(None, None).0;
        assert_eq!(
            (world.topology, world.cull_mode),
            (wgpu::PrimitiveTopology::TriangleList, None)
        );
        pass.inherits_cull = true;
        let p = compile_world(&pass, &plain_program()).expect("compiles");
        assert!(p.inherits_cull);
    }

    #[test]
    fn a_world_pass_draws_its_polygon_offset_in_its_fragments() {
        let mut pass = world_pass(final_look(false));
        pass.state.depth_offset = Some(crate::pass_state::DepthOffset { units: -1.0, slope: 4.0 });
        let p = compile_world(&pass, &plain_program()).expect("compiles");
        assert_eq!((p.pixel.depth[1], p.pixel.depth[2]), (-1.0, 4.0));
        assert_eq!(p.state.and_then(|s| s.depth_offset), pass.state.depth_offset);
        let plain = compile_world(&world_pass(final_look(false)), &plain_program()).unwrap();
        assert_eq!((plain.pixel.depth[1], plain.pixel.depth[2]), (0.0, 0.0));
    }

    #[test]
    fn a_world_pass_compiles_with_its_state() {
        let p = compile_world(&world_pass(final_look(false)), &plain_program()).expect("compiles");
        shaders::validate(&p.wgsl).expect("valid WGSL");
        let state = p.state.expect("its own state");
        assert_eq!(
            state.alpha_test,
            Some((wgpu::CompareFunction::Greater, 0x40))
        );
        assert!(
            p.wgsl.contains("xps.alpha_ref.x"),
            "the alpha test is in the fragment shader"
        );
        assert_eq!(p.inputs, 0b100_0011, "v0, v1, v6");
        assert!(
            compile(&final_look(false), &plain_program())
                .unwrap()
                .state
                .is_none()
        );
    }

    #[test]
    fn world_passes_that_read_device_state_stay_on_the_d3d_path() {
        let refusal =
            |look: PassLook| compile_world(&world_pass(look), &plain_program()).unwrap_err();
        assert!(refusal(plain_look()).contains("no final combiner"));
        assert_eq!(refusal(final_look(true)), "reads the fog register");
        let mut def = PixelShaderDef::from_bytes(&final_look(false).pixel_shader).unwrap();
        def.texture_modes = 0x06 << 5;
        let bump = PassLook {
            pixel_shader: def.to_bytes(),
            ..final_look(false)
        };
        assert!(refusal(bump).contains("BumpEnvMap"));
        let mut border = stage_states();
        border.address_v = 4;
        let bordered = PassLook {
            textures: [Stage::Lightmap, Stage::Empty, Stage::Empty, Stage::Empty],
            stage_states: vec![border],
            ..final_look(false)
        };
        assert_eq!(refusal(bordered), "stage 0 addresses the border");
        let unsampled = PassLook {
            stage_states: vec![border],
            ..final_look(false)
        };
        assert!(
            compile_world(&world_pass(unsampled), &plain_program()).is_ok(),
            "only a sampled stage counts"
        );
    }

    #[test]
    fn a_world_program_reads_only_the_constants_the_native_pass_sets() {
        use shaders::vsh::encode::{Inst, Operand as O, program};
        use shaders::vsh::{MacOp, OutReg};
        let reads = |c: u8| {
            program(&[
                Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(O::c(c)), None).to_out(
                    false,
                    OutReg::Pos,
                    0xf,
                ),
                Inst::mac(MacOp::Mov, Some(O::v(1)), None, None).to_out(false, OutReg::T(0), 0xf),
            ])
        };
        let pass = world_pass(final_look(false));
        for c in [0, 7, 17, 19, 50, 52, 170, 171, 172] {
            assert!(compile_world(&pass, &reads(c)).is_ok(), "c[{c}] is set");
        }
        for c in [18, 21, 24, 31, 53, 174] {
            let e = compile_world(&pass, &reads(c)).unwrap_err();
            assert_eq!(
                e,
                format!("vertex program reads c[{c}], which the native pass does not set")
            );
        }
        let relative = program(&[
            Inst::mac(MacOp::Mov, Some(O::c_rel(10)), None, None).to_out(false, OutReg::Pos, 0xf),
        ]);
        assert!(
            compile_world(&pass, &relative)
                .unwrap_err()
                .contains("relative")
        );
    }

    /// The world programs' stage-2 coordinate: `oT2.xy = v1 · c[20].xy +
    /// c[20].zw`, optionally through a temporary, after the position.
    fn stage_two_program(via_temp: bool, also_pos: bool) -> Vec<u8> {
        use shaders::vsh::encode::{Inst, Operand as O, program};
        use shaders::vsh::{MacOp, OutReg};
        let mut v = vec![
            Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(O::c(0)), None).to_out(false, OutReg::Pos, 8),
            Inst::mac(MacOp::Mov, Some(O::v(1)), None, None).to_out(false, OutReg::T(0), 0xf),
        ];
        if via_temp {
            v.push(
                Inst::mac(MacOp::Mad, Some(O::v(1)), Some(O::c(20)), Some(O::c(20))).to_temp(3, 0xf),
            );
            v.push(Inst::mac(MacOp::Mov, Some(O::r(3)), None, None).to_out(false, OutReg::T(2), 3));
        } else {
            v.push(
                Inst::mac(MacOp::Mad, Some(O::v(1)), Some(O::c(20)), Some(O::c(20)))
                    .to_out(false, OutReg::T(2), 3),
            );
        }
        if also_pos {
            v.push(Inst::mac(MacOp::Mov, Some(O::c(20)), None, None).to_out(false, OutReg::Pos, 4));
        }
        program(&v)
    }

    #[test]
    fn an_unset_constant_that_only_feeds_a_dead_texcoord_is_not_a_read() {
        let read = |look: &PassLook, program: Vec<u8>| world_constants_read(look, &program);
        const C20: &str = "vertex program reads c[20], which the native pass does not set";
        // c[20] unbound, stage 2's texture mode NONE (the default): oT2
        // is never fetched with, directly or through a temporary.
        let dead = final_look(false);
        for via_temp in [false, true] {
            assert_eq!(read(&dead, stage_two_program(via_temp, false)), Ok(()), "{via_temp}");
        }
        // The same value also reaching oPos is a live read.
        assert_eq!(read(&dead, stage_two_program(false, true)).unwrap_err(), C20);
        // Stage 2 fetching (a 2D projective mode): the coordinate is live.
        let mut def = PixelShaderDef::from_bytes(&dead.pixel_shader).unwrap();
        def.texture_modes = 0x01 << 10;
        let live = PassLook { pixel_shader: def.to_bytes(), ..final_look(false) };
        for via_temp in [false, true] {
            assert_eq!(read(&live, stage_two_program(via_temp, false)).unwrap_err(), C20);
        }
        // A bound c[20] was never in question.
        let bound = PassLook {
            vertex_constants: vec![Register {
                index: 20,
                value: [1.0; 4],
                written: 0xf,
                origin: tags::material::Origin::Instance,
            }],
            ..live
        };
        assert_eq!(read(&bound, stage_two_program(false, true)), Ok(()));
    }

    #[test]
    fn c20_can_feed_a_passthrough_coordinate_unused_by_the_fragment() {
        let mut look = final_look(false);
        let mut def = PixelShaderDef::from_bytes(&look.pixel_shader).unwrap();
        def.texture_modes = 4 << 10; // stage 2 passes its coordinates through
        let code = stage_two_program(true, false);
        look.pixel_shader = def.to_bytes();
        let program = compile_world(&world_pass(look.clone()), &code).unwrap();
        shaders::validate(&program.wgsl).unwrap();

        let rejected = |def: PixelShaderDef| {
            let mut live = look.clone();
            live.pixel_shader = def.to_bytes();
            assert_eq!(world_constants_read(&live, &code),
                Err("vertex program reads c[20], which the native pass does not set".into()));
        };
        let mut general = def.clone();
        general.combiner_count = 1;
        general.rgb_inputs[0] = 0xa; // t2
        rejected(general.clone());
        general.rgb_inputs[0] = 0;
        general.alpha_inputs[0] = 0x1a; // t2.a
        rejected(general);
        let mut final_read = def.clone();
        final_read.final_inputs_abcd = 0xa; // D takes t2
        rejected(final_read);
        let mut dependent = def.clone();
        dependent.texture_modes |= 0x0f << 15; // stage 3 samples using the selected stage's AR
        dependent.input_texture = 2 << 20;
        rejected(dependent);
        let mut clip = def;
        clip.texture_modes = 5 << 10; // clip plane can discard without a combiner read
        rejected(clip);
    }

    /// The rigid classification's shape: `r10 = v0 · c[170] + c[171]`,
    /// `dph r0.xyz, r10, c[50..52]` (the placement's rows), then
    /// `dph oPos, r0, c[0..3]`, `mov oT0, v1`; `rows: false` places with
    /// `c[53..55]` instead.
    fn rigid_program(rows: bool) -> Vec<u8> {
        use shaders::vsh::encode::{Inst, Operand as O, program};
        use shaders::vsh::{MacOp, OutReg};
        let model = if rows {
            MODEL_ROWS as u8
        } else {
            MODEL_ROWS as u8 + 3
        };
        // One constant per instruction: the scale, then the offset.
        let mut code = vec![
            Inst::mac(MacOp::Mul, Some(O::v(0)), Some(O::c(170)), None).to_temp(10, 0xf),
            Inst::mac(MacOp::Add, Some(O::r(10)), None, Some(O::c(171))).to_temp(10, 0xf),
        ];
        for k in 0..3u8 {
            code.push(
                Inst::mac(MacOp::Dph, Some(O::r(10)), Some(O::c(model + k)), None)
                    .to_temp(0, 8 >> k),
            );
        }
        for k in 0..4u8 {
            code.push(
                Inst::mac(MacOp::Dph, Some(O::r(0)), Some(O::c(k)), None).to_out(
                    false,
                    OutReg::Pos,
                    8 >> k,
                ),
            );
        }
        code.push(Inst::mac(MacOp::Mov, Some(O::v(1)), None, None).to_out(
            false,
            OutReg::T(0),
            0xf,
        ));
        program(&code)
    }

    /// The native module is the D3D path's for the same draw, text for
    /// text: `gfx::guest_program` decodes the words as `Normal`, with
    /// the layout's fetch (the world's frame and lighting vector bound as
    /// `NORMPACKED3` words, as 03a's draws bind them), and links the
    /// pixel shader from the same definition and options. So the position
    /// math (and any fused multiply-add the compiler makes of it) is the
    /// same code on both paths.
    #[test]
    fn a_world_program_s_module_is_the_d3d_path_s() {
        use shaders::vsh::encode::{Inst, Operand as O, program};
        use shaders::vsh::{MacOp, OutReg, ProgramKind};
        let mut code = vec![
            Inst::mac(MacOp::Mul, Some(O::v(0)), Some(O::c(170)), None).to_temp(10, 0xf),
            Inst::mac(MacOp::Add, Some(O::r(10)), None, Some(O::c(171))).to_temp(10, 0xf),
        ];
        for k in 0..3u8 {
            code.push(
                Inst::mac(
                    MacOp::Dph,
                    Some(O::r(10)),
                    Some(O::c(MODEL_ROWS as u8 + k)),
                    None,
                )
                .to_temp(0, 8 >> k),
            );
        }
        for k in 0..4u8 {
            code.push(
                Inst::mac(MacOp::Dph, Some(O::r(0)), Some(O::c(k)), None).to_out(
                    false,
                    OutReg::Pos,
                    8 >> k,
                ),
            );
        }
        code.push(Inst::mac(MacOp::Mov, Some(O::v(1)), None, None).to_out(
            false,
            OutReg::T(0),
            0xf,
        ));
        code.push(Inst::mac(MacOp::Mov, Some(O::v(2)), None, None).to_out(
            false,
            OutReg::T(1),
            0xf,
        ));
        code.push(Inst::mac(MacOp::Mov, Some(O::v(5)), None, None).to_out(
            false,
            OutReg::T(2),
            0xf,
        ));
        let microcode = program(&code);
        let pass = world_pass(final_look(false));
        let p = compile_pass(&pass.look, &microcode, pass.state.alpha_test).expect("compiles");
        let words: Vec<u32> = microcode[4..]
            .chunks_exact(4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        let d3d_with = |fetch: [InputFetch; 16], def: &PixelShaderDef| {
            let program = vsh::Program::from_words(ProgramKind::Normal, &words).unwrap();
            let options = VertexOptions {
                provided_inputs: INPUT_MASK,
                flat_shading: false,
                input_fetch: fetch,
            };
            let vs = vsh::translate(&program, &options).unwrap();
            shaders::link(&vs, &psh::translate(def, &p.pixel_options)).unwrap()
        };
        let d3d = |fetch: [InputFetch; 16]| d3d_with(fetch, &p.def);
        assert_eq!(p.wgsl, d3d(WORLD_FETCH));
        assert!(
            p.wgsl.contains("xvs_normpacked3"),
            "v2 and v5 decoded in the shader"
        );
        assert_ne!(
            p.wgsl,
            d3d([InputFetch::Float; 16]),
            "floats were another module"
        );
        // The D3D path's definition carries the device's final-combiner
        // latch (`explicit_final`); with the final combiner's inputs set,
        // as a world pass's are, it translates to the same module.
        assert!(p.def.final_inputs_abcd != 0 || p.def.final_inputs_efg != 0);
        let latched = PixelShaderDef {
            explicit_final: true,
            ..p.def.clone()
        };
        assert_eq!(p.wgsl, d3d_with(WORLD_FETCH, &latched));
        shaders::validate(&p.wgsl).expect("valid WGSL");
    }

    #[test]
    fn a_rigid_program_draws_a_placement_with_its_rows() {
        let pass = world_pass(final_look(false));
        let r = compile_rigid(&pass, &rigid_program(true)).expect("compiles");
        shaders::validate(&r.wgsl).expect("valid WGSL");
        assert_eq!(r.inputs, 0b11, "v0, v1");
        assert_eq!(
            r.vertex.constants[POSITION_DECOMPRESSION], [1.0; 4],
            "the BSP's decompression is the identity"
        );
        assert_eq!(r.vertex.constants[POSITION_DECOMPRESSION + 1], [0.0; 4]);
        assert_eq!(
            r.vertex.constants[19],
            [2.0, 3.0, 0.0, 0.0],
            "the instance's constants"
        );
        let code = rigid_program(true);
        let count = vsh::Program::parse(&code).unwrap().instructions.len();
        assert_eq!(
            r.words.len(),
            4 * count,
            "four words an instruction, no header"
        );
        assert_eq!(
            r.words[0],
            u32::from_le_bytes([code[4], code[5], code[6], code[7]])
        );
        assert_eq!(
            compile_rigid(&pass, &rigid_program(false)).unwrap_err(),
            "the rigid vertex program does not read the model rows c[50..52]"
        );
        assert_eq!(
            compile_rigid(&pass, &plain_program()).unwrap_err(),
            "the rigid vertex program does not read the model rows c[50..52]",
            "the world classification places nothing"
        );
        // Per placement and frame: the view's constants, then its rows,
        // bit for bit (a NaN payload and a negative zero too).
        let rows = [
            [1.0, -0.0, f32::from_bits(0x7fc0_0001), 10.0],
            [0.0, 1.0, 0.0, f32::INFINITY],
            [0.0, 0.0, 1.0, -3.5],
        ];
        let mut constants = [[0.0; 4]; VIEW_CONSTANTS];
        constants[3] = [5.0, 6.0, 7.0, 8.0];
        let view = ViewInputs {
            constants,
            guest_size: (640, 480),
            size: (640, 480),
            z_scale: 16_777_215.0,
            w_clip: None,
        };
        let u = rigid_vertex(&r.vertex, &view, &rows);
        assert_eq!(u.constants[3], [5.0, 6.0, 7.0, 8.0]);
        assert_eq!(u.z_max, 16_777_215.0);
        let bits = |rows: &[[f32; 4]]| rows.iter().map(|r| r.map(f32::to_bits)).collect::<Vec<_>>();
        assert_eq!(
            bits(&u.constants[MODEL_ROWS..MODEL_ROWS + 3]),
            bits(&rows[..])
        );
        // The slot buffer: one placement's uniforms per stride.
        let other = [[2.0; 4]; 3];
        let stride = 0x1000;
        let bytes = rigid_slot_bytes(&r.vertex, &view, &[(4, rows), (9, other)], stride);
        assert_eq!(bytes.len(), stride + VertexUniforms::SIZE);
        assert_eq!(&bytes[..VertexUniforms::SIZE], &u.to_bytes()[..]);
        assert_eq!(
            &bytes[stride..],
            &rigid_vertex(&r.vertex, &view, &other).to_bytes()[..]
        );
    }

    #[test]
    fn a_rigid_program_fits_only_placements_with_their_rows_and_streams() {
        use crate::world::{Owner, PartInput, Placement, SectionInput, WorldBuilder};
        let section = |material, uvs: bool| SectionInput {
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            uvs: if uvs { vec![[0.0; 2]; 3] } else { Vec::new() },
            normals: vec![[0.0, 0.0, 1.0]; 3],
            parts: vec![PartInput {
                material,
                transparent: false,
                triangles: vec![[0, 1, 2]],
            }],
            ..Default::default()
        };
        let rows = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ];
        let mut b = WorldBuilder::new();
        b.add_section(
            Owner::Cluster(0),
            &section(1, false),
            &Placement::IDENTITY,
            None,
        );
        b.add_section(
            Owner::Instance(5),
            &section(1, true),
            &Placement::IDENTITY,
            None,
        );
        b.add_section(
            Owner::Instance(2),
            &section(1, true),
            &Placement::IDENTITY,
            None,
        );
        b.add_section(
            Owner::Instance(6),
            &section(2, false),
            &Placement::IDENTITY,
            None,
        );
        b.add_section(
            Owner::Instance(7),
            &section(3, true),
            &Placement::IDENTITY,
            None,
        );
        b.place_rows(5, rows);
        b.place_rows(2, [[3.0; 4]; 3]);
        b.place_rows(6, rows);
        let world = b.finish();
        let r = compile_rigid(&world_pass(final_look(false)), &rigid_program(true)).unwrap();
        assert_eq!(
            rigid_fit(&world, 1, &r),
            Ok(()),
            "a cluster without the stream is not the rigid program's"
        );
        assert_eq!(
            rigid_fit(&world, 2, &r),
            Err("a placement's part has no v1".to_string())
        );
        assert_eq!(
            rigid_fit(&world, 3, &r),
            Err("a placement's rows did not read".to_string())
        );
        let slots = rigid_slots(&world, 1).unwrap();
        assert_eq!(
            slots.iter().map(|s| s.0).collect::<Vec<_>>(),
            vec![2, 5],
            "by placement, the cluster has none"
        );
        assert_eq!(slots[0].1, [[3.0; 4]; 3]);
        assert_eq!(rigid_slots(&world, 3), None);
        assert_eq!(rigid_slots(&world, 9), Some(Vec::new()));
    }

    #[test]
    fn a_material_fits_the_world_only_when_every_part_can_be_drawn() {
        use crate::world::{Owner, PartInput, Placement, SectionInput, WorldBuilder};
        let page = LightmapKey {
            page: 1,
            palette: Some(0),
        };
        let section = |material, lit: bool, lighting: bool| SectionInput {
            source: None,
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            uvs: vec![[0.0; 2]; 3],
            lightmap_uvs: if lit { vec![[0.5; 2]; 3] } else { Vec::new() },
            normals: vec![[0.0, 0.0, 1.0]; 3],
            binormals: vec![[1.0, 0.0, 0.0]; 3],
            tangents: vec![[0.0, 1.0, 0.0]; 3],
            baked_colors: Vec::new(),
            bucket_colors: false,
            lighting: if lighting {
                vec![[0.0, 0.0, 1.0]; 3]
            } else {
                Vec::new()
            },
            parts: vec![PartInput {
                material,
                transparent: false,
                triangles: vec![[0, 1, 2]],
            }],
        };
        let mut b = WorldBuilder::new();
        b.add_section(
            Owner::Cluster(0),
            &section(1, true, true),
            &Placement::IDENTITY,
            Some(page),
        );
        b.add_section(
            Owner::Cluster(1),
            &section(2, true, true),
            &Placement::IDENTITY,
            Some(page),
        );
        b.add_section(
            Owner::Cluster(2),
            &section(2, true, false),
            &Placement::IDENTITY,
            Some(page),
        );
        b.add_section(
            Owner::Cluster(3),
            &section(3, false, true),
            &Placement::IDENTITY,
            Some(page),
        );
        // A colour page (render info palette -1, the converted Halo 1
        // pages) is a page like any other.
        let colour = LightmapKey { page: 2, palette: None };
        b.add_section(Owner::Cluster(4), &section(4, true, true), &Placement::IDENTITY, Some(colour));
        let world = b.finish();
        // The per-pixel lightmap program: every register, the lightmap at
        // stage 3.
        let mut p = compile(&plain_look(), &plain_program()).unwrap();
        p.inputs = INPUT_MASK;
        p.stages[3] = Stage::Lightmap;
        p.dims[3] = Some(TextureDim::D2);
        let all = |_: LightmapKey| Ok(());
        let missing = |key| {
            assert_eq!(key, page);
            Err("a part's lightmap page does not decode".to_string())
        };
        assert_eq!(world_fit(&world, 1, &p, &all), Ok(()));
        assert_eq!(world_fit(&world, 4, &p, &|k| if k == colour { Ok(()) } else { Err(format!("{k:?}")) }), Ok(()));
        assert!(part_fit(&world.draws[1], p.inputs, &p, &all).is_ok());
        assert!(part_fit(&world.draws[2], p.inputs, &p, &all).is_err());
        assert!(part_fit(&world.draws[2], p.inputs & !(1 << 5), &p, &all).is_ok());
        assert_eq!(
            world_fit(&world, 2, &p, &all),
            Err("a part has no lighting vectors (v5)".to_string())
        );
        assert_eq!(
            world_fit(&world, 3, &p, &all),
            Err("a part has no lightmap coordinates (v6)".to_string())
        );
        assert_eq!(
            world_fit(&world, 1, &p, &missing),
            Err("a part's lightmap page does not decode".to_string())
        );
        // A program that reads neither: the unlit part fits.
        p.inputs = 0b11;
        p.stages[3] = Stage::Empty;
        p.dims[3] = None;
        assert_eq!(world_fit(&world, 3, &p, &missing), Ok(()));
    }
}
