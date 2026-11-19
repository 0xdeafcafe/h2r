//! A world material drawn with its own programs: the pass's vertex program
//! and pixel shader definition through `shaders`, the instance constants
//! where the game puts them, and the view's constants from the device
//! each frame (docs/graphics/native-renderer.md, "What a world pass reads").
//! [`compile`] is pure (WGSL and uniform values); [`ProgramSet`] holds the
//! GPU side for [`crate::gpu::GpuWorld`].

use std::collections::HashMap;

use shaders::psh::{self, PixelOptions, PixelShaderDef, PixelUniforms, TextureDim};
use shaders::vsh::{self, InputFetch, VertexOptions, VertexUniforms};
use tags::material::Register;
use tags::shader::StageStates;

use crate::gpu::{Image, texture};
use crate::look::{PassLook, Stage, WorldPass};
use crate::pass_state::PassState;
use crate::world::{LightmapKey, ModelRows, Owner, World, WorldVertex};

/// The per-view vertex constants the device gives the pass
/// (`d3d::VIEW_CONSTANTS`).
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
    (0, std::mem::offset_of!(WorldVertex, position) as u64, wgpu::VertexFormat::Float32x3),
    (1, std::mem::offset_of!(WorldVertex, uv) as u64, wgpu::VertexFormat::Float32x2),
    (2, std::mem::offset_of!(WorldVertex, packed) as u64, wgpu::VertexFormat::Uint32),
    (3, std::mem::offset_of!(WorldVertex, packed) as u64 + 4, wgpu::VertexFormat::Uint32),
    (4, std::mem::offset_of!(WorldVertex, packed) as u64 + 8, wgpu::VertexFormat::Uint32),
    (5, std::mem::offset_of!(WorldVertex, packed) as u64 + 12, wgpu::VertexFormat::Uint32),
    (6, std::mem::offset_of!(WorldVertex, lightmap_uv) as u64, wgpu::VertexFormat::Float32x2),
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

/// [`INPUTS`] with the frame and lighting vector as floats (the
/// [`WorldVertex`] fields): what the native lights read, from the placed
/// copy of the world (`crate::light_gpu`; their programs translate with
/// float fetch).
pub const FLOAT_INPUTS: [(u32, u64, wgpu::VertexFormat); 7] = [
    (0, std::mem::offset_of!(WorldVertex, position) as u64, wgpu::VertexFormat::Float32x3),
    (1, std::mem::offset_of!(WorldVertex, uv) as u64, wgpu::VertexFormat::Float32x2),
    (2, std::mem::offset_of!(WorldVertex, normal) as u64, wgpu::VertexFormat::Float32x3),
    (3, std::mem::offset_of!(WorldVertex, binormal) as u64, wgpu::VertexFormat::Float32x3),
    (4, std::mem::offset_of!(WorldVertex, tangent) as u64, wgpu::VertexFormat::Float32x3),
    (5, std::mem::offset_of!(WorldVertex, lighting) as u64, wgpu::VertexFormat::Float32x3),
    (6, std::mem::offset_of!(WorldVertex, lightmap_uv) as u64, wgpu::VertexFormat::Float32x2),
];

/// Bit `n` for each supplied `v<n>`.
pub const INPUT_MASK: u16 = 0x7f;

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
    /// The vertex registers (bit `n` for `v<n>`) the program reads from
    /// the world vertex ([`INPUTS`]).
    pub inputs: u16,
    /// The pixel shader definition and options it was translated with
    /// (for comparing with the D3D path's: `d3d::uniform_compare`).
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
    /// The vertex registers the rigid program reads.
    pub inputs: u16,
    /// The program's instruction words (four per instruction, no
    /// header): what the D3D path's draw of a placement binds, for
    /// telling its draws apart (`d3d::uniform_compare`).
    pub words: Vec<u32>,
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
    if look.per_draw_inputs {
        // Resolved with the instance's inputs only; per-draw ones (object
        // functions, lighting, time) are not evaluated here.
        return Err("per-draw engine inputs".into());
    }
    let t = translate_pass(&PassParts {
        microcode,
        pixel_shader: &look.pixel_shader,
        dims: look.textures.map(|s| (s != Stage::Empty).then_some(TextureDim::D2)),
        // Only a stage's own bitmap is stored biased here.
        biased: look.textures.map(|s| matches!(s, Stage::Bitmap { .. })),
        stage_states: &look.stage_states,
        vertex_constants: &look.vertex_constants,
        vertex_defaults: &look.vertex_defaults,
        alpha_test,
        provided_inputs: INPUT_MASK,
        input_fetch: WORLD_FETCH,
        w_buffer: true,
    })?;
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
    world_parity(&pass.look)?;
    world_constants_read(&pass.look, microcode)?;
    let mut p = compile_pass(&pass.look, microcode, pass.state.alpha_test)?;
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
    if let Some(c) = constants_read(&program)?.into_iter().find(|&c| !set(c)) {
        return Err(format!("vertex program reads c[{c}], which the native pass does not set"));
    }
    Ok(())
}

/// `pass` with its vertex program's rigid classification `microcode`
/// ([`crate::look::rigid_program`]), for the material's instanced
/// placements, or why they stay on the D3D path. As [`compile_world`]
/// (whose checks `pass` has passed), and the program must read the model
/// rows `c[50..52]`: the D3D path places a placement's vertices with them
/// on the GPU, and the native pass uploads the same rows per placement.
pub fn compile_rigid(pass: &WorldPass, microcode: &[u8]) -> Result<RigidProgram, String> {
    let program = vsh::Program::parse(microcode).map_err(|e| format!("rigid vertex program: {e:?}"))?;
    let read = constants_read(&program)?;
    if !(MODEL_ROWS..MODEL_ROWS + 3).all(|c| read.contains(&(c as u8))) {
        return Err("the rigid vertex program does not read the model rows c[50..52]".into());
    }
    let set = set_constants(&pass.look.vertex_constants);
    if let Some(c) = read.into_iter().find(|&c| !set(c)) {
        return Err(format!("rigid vertex program reads c[{c}], which the native pass does not set"));
    }
    let p = compile_pass(&pass.look, microcode, pass.state.alpha_test)?;
    let words = microcode[4..4 + 16 * program.instructions.len()]
        .chunks_exact(4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    Ok(RigidProgram { wgsl: p.wgsl, vertex: p.vertex, inputs: p.inputs, words })
}

/// Whether every instanced-placement part of `material` in `world` can be
/// drawn with `rigid`: each supplies the registers it reads, and its
/// placement's rows were read at load ([`World::rows`]).
pub fn rigid_fit(world: &World, material: u16, rigid: &RigidProgram) -> Result<(), String> {
    for d in world.draws.iter().filter(|d| d.material == material) {
        let Owner::Instance(i) = d.owner else { continue };
        let missing = rigid.inputs & !d.inputs;
        if missing != 0 {
            return Err(format!("a placement's part has no v{}", missing.trailing_zeros()));
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
fn set_constants(instance: &[Register]) -> impl Fn(u8) -> bool + '_ {
    move |c| {
        let c = usize::from(c);
        c < VIEW_CONSTANTS
            || (MODEL_ROWS..MODEL_ROWS + 3).contains(&c)
            || (POSITION_DECOMPRESSION..POSITION_DECOMPRESSION + 2).contains(&c)
            || c == TEXCOORD_DECOMPRESSION
            || instance.iter().any(|r| usize::from(r.index) == c && r.written != 0)
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
    let def = PixelShaderDef::from_bytes(&look.pixel_shader).map_err(|e| format!("pixel shader: {e:?}"))?;
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
            M::BumpEnvMap | M::BumpEnvMapLum | M::Brdf | M::DotRflctDiff | M::DotRflctSpec | M::DotRflctSpecConst
        ) {
            return Err(format!("stage {n} texture mode {m:?} reads device state"));
        }
    }
    for (n, s) in look.stage_states.iter().enumerate().take(4) {
        let sampled = look.textures[n] != Stage::Empty;
        if sampled && [s.address_u, s.address_v, s.address_w].contains(&ADDRESS_BORDER) {
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
/// `page` has it decoded. The game's skip is per shader, so one part that
/// cannot be drawn keeps the whole material on the D3D path.
pub fn world_fit(world: &World, material: u16, program: &MaterialProgram, page: &dyn Fn(LightmapKey) -> bool) -> Result<(), String> {
    const NAMES: [&str; 7] = [
        "positions",
        "texture coordinates",
        "normals",
        "binormals",
        "tangents",
        "lighting vectors",
        "lightmap coordinates",
    ];
    let lightmap = program
        .stages
        .iter()
        .zip(&program.dims)
        .any(|(s, d)| *s == Stage::Lightmap && d.is_some());
    for d in world.draws.iter().filter(|d| d.material == material) {
        let missing = program.inputs & !d.inputs;
        if missing != 0 {
            let n = missing.trailing_zeros() as usize;
            return Err(format!("a part has no {} (v{n})", NAMES.get(n).unwrap_or(&"stream")));
        }
        if lightmap {
            let Some(key) = d.lightmap else {
                return Err("a part has no lightmap page".into());
            };
            if !page(key) {
                return Err("a part's lightmap page does not decode".to_string());
            }
        }
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
}

/// A translated pass.
pub(crate) struct Translated {
    pub wgsl: String,
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
    let program = vsh::Program::parse(p.microcode).map_err(|e| format!("vertex program: {e:?}"))?;
    let unsupplied = program.inputs_read() & !p.provided_inputs & !defaults_mask(p.vertex_defaults);
    if unsupplied != 0 {
        return Err(format!("vertex program reads registers we do not supply ({unsupplied:#06x})"));
    }
    let vs = vsh::translate(&program, &VertexOptions { provided_inputs: p.provided_inputs, input_fetch: p.input_fetch, ..Default::default() })
        .map_err(|e| format!("vertex program: {e:?}"))?;
    let def = PixelShaderDef::from_bytes(p.pixel_shader).map_err(|e| format!("pixel shader: {e:?}"))?;
    let samplers: [Option<StageStates>; 4] = std::array::from_fn(|n| p.stage_states.get(n).copied());
    let mut options = PixelOptions {
        texture_dims: p.dims,
        // The game's world is W-buffered (docs/graphics/native-renderer.md).
        w_buffer: p.w_buffer,
        alpha_test: p.alpha_test.map(|(f, _)| alpha_func(f)),
        ..Default::default()
    };
    let mut signed = [[false; 4]; 4];
    for (n, s) in samplers.iter().enumerate() {
        let Some(s) = s else { continue };
        let sign = psh::bits::color_sign(s.color_sign);
        if sign != 0 {
            if !p.biased[n] {
                return Err(format!("stage {n} reads signed channels of a texture not stored biased"));
            }
            options.color_sign[n] = sign;
            signed[n] = signed_channels(s.color_sign);
        }
        options.alpha_kill[n] = s.alpha_kill != 0;
    }
    let ps = psh::translate(&def, &options);
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
    vertex.constants[MODEL_ROWS..MODEL_ROWS + 3]
        .copy_from_slice(&[[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]]);
    for r in p.vertex_constants {
        put(&mut vertex.constants, r);
    }
    for r in p.vertex_defaults {
        put(&mut vertex.defaults, r);
    }
    let (c0, c1) = def.embedded_constants();
    // `D3DTSS_MIPMAPLODBIAS`, applied in the shader as the D3D path does
    // (`d3d::nv2a`; a wgpu sampler has no bias).
    let mut pixel = PixelUniforms { c0, c1, lod_bias: lod_bias(samplers), ..Default::default() };
    if let Some((_, reference)) = p.alpha_test {
        // Compared with the alpha rounded to 1/255 (`xps.alpha_ref.x`).
        pixel.alpha_ref = [f32::from(reference), 0.0, 0.0, 0.0];
    }
    let inputs = program.inputs_read() & p.provided_inputs;
    Ok(Translated { wgsl, samplers, dims: ps.textures, signed, vertex, pixel, inputs, def, pixel_options: options })
}

/// Each stage's `D3DTSS_MIPMAPLODBIAS` (f32 bits), zero when not finite,
/// as `d3d::nv2a` passes it (the shader clamps it to the NV2A's range).
pub fn lod_bias(samplers: [Option<StageStates>; 4]) -> [f32; 4] {
    samplers.map(|s| {
        let b = s.map_or(0.0, |s| f32::from_bits(s.mip_lod_bias));
        if b.is_finite() { b } else { 0.0 }
    })
}

fn alpha_func(f: wgpu::CompareFunction) -> psh::AlphaFunc {
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
    /// The placements' rigid draws take it (the D3D path's draws of a
    /// placement were compared: `d3d::uniform_compare`).
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
        let takes = if cluster { self.clusters } else { self.placements };
        if inherits && takes { self.device } else { None }
    }
}

/// The primitive and the depth write and compare a program's pipeline
/// draws with ([`render_pipeline`]): its pass's state, or the opaque
/// world's (a triangle list, no cull, `LessEqual` and write), with `cull`
/// over its cull and winding ([`InheritedCull::of`]). No depth bias: the
/// polygon offset is the fragment's (`PixelUniforms::depth`).
pub fn pipeline_state(state: Option<&PassState>, cull: Option<Cull>) -> (wgpu::PrimitiveState, (bool, wgpu::CompareFunction)) {
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
    let depth = state.map_or((true, wgpu::CompareFunction::LessEqual), |s| (s.depth_write, s.depth_compare));
    (primitive, depth)
}

/// What an image is uploaded as on a device that takes BC textures
/// (`bc`) or not ([`crate::gpu::texture`]): format, size, levels and an
/// FNV-1a hash of level 0's bytes, for the uniform compare.
pub fn describe_image(image: &Image, bc: bool) -> String {
    let (format, level0, levels) = match &image.blocks {
        Some(b) if bc => (format!("{:?}", b.format), &b.levels[0], b.levels.len()),
        _ => ("Rgba8Unorm".to_string(), &image.rgba, 1 + image.mips.len()),
    };
    let hash = level0.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3));
    format!("{format} {}x{} levels {levels} level 0 {hash:016x}", image.width, image.height)
}

/// Stores the signed channels of an RGBA8 image as `s + 128`, so that
/// filtering interpolates signed values (`shaders::psh::PixelOptions::color_sign`).
pub fn bias_signed(image: &mut Image, signed: [bool; 4]) {
    let flip: [u8; 4] = signed.map(|s| if s { 0x80 } else { 0 });
    if flip == [0; 4] {
        return;
    }
    // A signed channel keeps a DXT image off BC, as on the D3D path
    // (`gfx::texture::DecodeOptions::signed`).
    image.blocks = None;
    for level in std::iter::once(&mut image.rgba).chain(image.mips.iter_mut()) {
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
fn put(regs: &mut [[f32; 4]], r: &Register) {
    let Some(reg) = regs.get_mut(usize::from(r.index)) else { return };
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
    /// the pass is issued (`d3d::DepthClip`: `SET_CLIP_MIN`/`MAX`,
    /// `D3DRS_DEPTHCLIPCONTROL`), what the D3D path draws the layer's
    /// draws with. `None`: the whole range, culled.
    pub w_clip: Option<([f32; 2], bool)>,
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
    let s = view.size.0 as f32 / view.guest_size.0.max(1) as f32;
    // As the D3D path shifts a scaled target (gfx::renderer::window_shift).
    u.shift = if s > 1.0 { (2.0 * s - 1.0) / (4.0 * s) } else { 0.0 };
    u
}

/// `base` with the frame's depth scale.
pub fn frame_pixel(base: &PixelUniforms, view: &ViewInputs) -> PixelUniforms {
    PixelUniforms {
        depth: [view.z_scale, 0.0, 0.0, 1.0],
        depth_range: {
            let [lo, hi] = view.w_clip.map_or([0.0, view.z_scale], |(range, _)| range);
            [lo, hi, 0.0, 0.0]
        },
        ..*base
    }
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
    let filter = |f: u8| if f == 1 { wgpu::FilterMode::Nearest } else { wgpu::FilterMode::Linear };
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
    module: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    textures_layout: wgpu::BindGroupLayout,
    vertex_buffer: wgpu::Buffer,
    pixel_buffer: wgpu::Buffer,
    pub(crate) uniforms: wgpu::BindGroup,
    samplers: [wgpu::Sampler; 4],
    stage_views: [Option<wgpu::TextureView>; 4],
    /// What each stage's bitmap was uploaded as ([`describe_image`]).
    stage_info: [Option<String>; 4],
    pub(crate) pipelines: HashMap<(wgpu::TextureFormat, wgpu::TextureFormat), wgpu::RenderPipeline>,
    /// The world's pipelines, by target formats, the stencil state the
    /// layer draws with and the cull ([`ProgramSet::pipelines`]).
    world_pipelines: HashMap<PipelineKey, wgpu::RenderPipeline>,
}

/// A world or rigid pipeline's key: target formats, stencil state and the
/// inherited cull it was made with ([`InheritedCull::of`]).
type PipelineKey = (wgpu::TextureFormat, wgpu::TextureFormat, wgpu::StencilState, Option<Cull>);

/// The materials drawn with their own programs, and each batch's texture
/// group (its lightmap page bound where the pass samples the lightmap).
pub struct ProgramSet {
    materials: HashMap<u16, Gpu>,
    /// The materials whose placements draw with their rigid programs.
    rigid: HashMap<u16, Rigid>,
    /// Per batch of the world: the texture group, when the batch's
    /// material has a program.
    batch_groups: Vec<Option<wgpu::BindGroup>>,
    /// The cull the passes that inherit it draw with
    /// ([`ProgramSet::pipelines`]).
    cull: InheritedCull,
}

pub(crate) fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
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
    /// Uploads `program`; `None` when a stage's bitmap does not decode.
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        program: MaterialProgram,
        stage_image: &mut dyn FnMut(Stage) -> Option<Image>,
    ) -> Option<Self> {
        let mut stage_views: [Option<wgpu::TextureView>; 4] = Default::default();
        let mut stage_info: [Option<String>; 4] = Default::default();
        for (n, s) in program.stages.iter().enumerate() {
            if let Stage::Bitmap { .. } = s
                && program.dims[n].is_some()
            {
                let mut image = stage_image(*s)?;
                bias_signed(&mut image, program.signed[n]);
                stage_views[n] = Some(texture(device, queue, &image, "native world stage"));
                stage_info[n] = Some(describe_image(&image, device.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC)));
            }
        }
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native world program"),
            source: wgpu::ShaderSource::Wgsl(program.wgsl.clone().into()),
        });
        let uniforms_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native world program uniforms"),
            entries: &[
                uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
                uniform_entry(1, wgpu::ShaderStages::VERTEX_FRAGMENT),
            ],
        });
        let mut entries = Vec::new();
        for (n, d) in program.dims.iter().enumerate() {
            if d.is_none() {
                continue;
            }
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: 2 * n as u32,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
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
        let buffer = |label, size: usize| {
            gfx::churn::create_buffer(device, &wgpu::BufferDescriptor {
                label: Some(label),
                size: size as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let vertex_buffer = buffer("native world program vertex", VertexUniforms::SIZE);
        let pixel_buffer = buffer("native world program pixel", PixelUniforms::SIZE);
        let uniforms = gfx::churn::create_bind_group(device, &wgpu::BindGroupDescriptor {
            label: Some("native world program uniforms"),
            layout: &uniforms_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: vertex_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: pixel_buffer.as_entire_binding() },
            ],
        });
        let samplers = std::array::from_fn(|n| device.create_sampler(&sampler_descriptor(program.samplers[n].as_ref())));
        Some(Gpu {
            program,
            module,
            layout,
            textures_layout,
            vertex_buffer,
            pixel_buffer,
            uniforms,
            samplers,
            stage_views,
            stage_info,
            pipelines: HashMap::new(),
            world_pipelines: HashMap::new(),
        })
    }

    /// The texture group with `page` bound where the program samples the
    /// lightmap; `None` when a sampled stage has nothing to bind.
    pub(crate) fn textures(&self, device: &wgpu::Device, page: Option<&wgpu::TextureView>) -> Option<wgpu::BindGroup> {
        let mut views = Vec::new();
        for (n, d) in self.program.dims.iter().enumerate() {
            if d.is_none() {
                continue;
            }
            let view = match self.program.stages[n] {
                Stage::Lightmap => page?.clone(),
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
        Some(gfx::churn::create_bind_group(device, &wgpu::BindGroupDescriptor {
            label: Some("native world program textures"),
            layout: &self.textures_layout,
            entries: &entries,
        }))
    }

    /// The frame's uniforms.
    pub(crate) fn prepare(&self, queue: &wgpu::Queue, view: &ViewInputs) {
        queue.write_buffer(&self.vertex_buffer, 0, &frame_vertex(&self.program.vertex, view).to_bytes());
        queue.write_buffer(&self.pixel_buffer, 0, &frame_pixel(&self.program.pixel, view).to_bytes());
    }

    /// [`Gpu::prepare`] with the pixel uniforms `pixel` in place of the
    /// program's own: for a pass whose pixel state is resolved per view
    /// (the fog layer's `PixelUniforms::fog` and `fog_color`,
    /// `crate::fog_gpu`).
    pub(crate) fn prepare_with(&self, queue: &wgpu::Queue, view: &ViewInputs, pixel: &PixelUniforms) {
        queue.write_buffer(&self.vertex_buffer, 0, &frame_vertex(&self.program.vertex, view).to_bytes());
        queue.write_buffer(&self.pixel_buffer, 0, &frame_pixel(pixel, view).to_bytes());
    }

    /// Makes the pipeline for these target formats: the world's opaque
    /// state without `state`, a transparent pass's with it.
    pub(crate) fn pipeline(
        &mut self,
        device: &wgpu::Device,
        color: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
        state: Option<&PassState>,
    ) {
        let (module, layout) = (&self.module, &self.layout);
        self.pipelines
            .entry((color, depth))
            .or_insert_with(|| render_pipeline(device, module, layout, color, depth, state, None, wgpu::StencilState::default()));
    }
}

/// A program's pipeline over the world vertex ([`INPUTS`]): the world's
/// opaque state without `state`, a pass's own with it, `cull` over its
/// cull ([`pipeline_state`]); `stencil` the stencil test and write.
#[allow(clippy::too_many_arguments)]
fn render_pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    color: wgpu::TextureFormat,
    depth: wgpu::TextureFormat,
    state: Option<&PassState>,
    cull: Option<Cull>,
    stencil: wgpu::StencilState,
) -> wgpu::RenderPipeline {
    let attributes: Vec<wgpu::VertexAttribute> = INPUTS
        .iter()
        .map(|&(location, offset, format)| wgpu::VertexAttribute { format, offset, shader_location: location })
        .collect();
    let (primitive, (depth_write, depth_compare)) = pipeline_state(state, cull);
    let (blend, write_mask) = state.map_or((None, wgpu::ColorWrites::ALL), |s| (s.blend, s.color_writes));
    gfx::churn::create_render_pipeline(device, &wgpu::RenderPipelineDescriptor {
        label: Some("native world program"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<WorldVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &attributes,
            })],
        },
        primitive,
        depth_stencil: Some(wgpu::DepthStencilState {
            format: depth,
            depth_write_enabled: Some(depth_write),
            depth_compare: Some(depth_compare),
            stencil,
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState { format: color, blend, write_mask })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// A material's rigid program on the GPU ([`RigidProgram`]): one vertex
/// uniform buffer with a slot per placement the material draws, bound at
/// that slot's dynamic offset for the placement's draws, with the
/// material's pixel uniforms and texture groups.
pub(crate) struct Rigid {
    program: RigidProgram,
    module: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    buffer: wgpu::Buffer,
    uniforms: wgpu::BindGroup,
    /// Bytes between slots (the device's offset alignment).
    stride: u64,
    /// Per slot, ascending: the placement and its rows.
    slots: Vec<(u32, ModelRows)>,
    pipelines: HashMap<PipelineKey, wgpu::RenderPipeline>,
}

/// The placements `material`'s draws in `world` name, ascending, each with
/// its rows; `None` when one has none.
pub fn rigid_slots(world: &World, material: u16) -> Option<Vec<(u32, ModelRows)>> {
    let mut slots: Vec<(u32, ModelRows)> = Vec::new();
    for d in world.draws.iter().filter(|d| d.material == material) {
        let Owner::Instance(i) = d.owner else { continue };
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
pub fn rigid_slot_bytes(base: &VertexUniforms, view: &ViewInputs, slots: &[(u32, ModelRows)], stride: usize) -> Vec<u8> {
    let mut out = vec![0u8; stride * slots.len().saturating_sub(1) + VertexUniforms::SIZE];
    for (n, (_, rows)) in slots.iter().enumerate() {
        rigid_vertex(base, view, rows).write_bytes(&mut out[n * stride..n * stride + VertexUniforms::SIZE]);
    }
    out
}

impl Rigid {
    /// Uploads `gpu`'s rigid program for `material`'s placements in
    /// `world`; `None` without one, or with no placement to draw.
    fn new(device: &wgpu::Device, gpu: &Gpu, world: &World, material: u16) -> Option<Self> {
        let program = gpu.program.rigid.clone()?;
        let slots = rigid_slots(world, material)?;
        if slots.is_empty() {
            return None;
        }
        let align = u64::from(device.limits().min_uniform_buffer_offset_alignment.max(1));
        let size = VertexUniforms::SIZE as u64;
        let stride = size.div_ceil(align) * align;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native world rigid program"),
            source: wgpu::ShaderSource::Wgsl(program.wgsl.clone().into()),
        });
        let mut vertex = uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT);
        if let wgpu::BindingType::Buffer { has_dynamic_offset, min_binding_size, .. } = &mut vertex.ty {
            *has_dynamic_offset = true;
            *min_binding_size = wgpu::BufferSize::new(size);
        }
        let uniforms_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native world rigid uniforms"),
            entries: &[vertex, uniform_entry(1, wgpu::ShaderStages::VERTEX_FRAGMENT)],
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
        let buffer = gfx::churn::create_buffer_init(device, &wgpu::util::BufferInitDescriptor {
            label: Some("native world rigid vertex"),
            contents: &initial,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let uniforms = gfx::churn::create_bind_group(device, &wgpu::BindGroupDescriptor {
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
                wgpu::BindGroupEntry { binding: 1, resource: gpu.pixel_buffer.as_entire_binding() },
            ],
        });
        Some(Self { program, module, layout, buffer, uniforms, stride, slots, pipelines: HashMap::new() })
    }

    /// The frame's view and rows in every slot.
    fn prepare(&self, uploads: &mut gfx::ring::EncoderUploads<'_>, view: &ViewInputs) {
        let v = frame_vertex(&self.program.vertex, view).to_bytes();
        // Only c[0..18] and the viewport/clip tail depend on the view.
        // Every placement keeps its own original c[50..53] in the GPU block.
        let tail = VertexUniforms::SIZE - 32;
        uploads.repeat(&self.buffer, 0, self.stride, self.slots.len(), &v[..VIEW_CONSTANTS * 16]);
        uploads.repeat(&self.buffer, tail as u64, self.stride, self.slots.len(), &v[tail..]);
    }

    /// The dynamic offset of `instance`'s slot.
    fn offset(&self, instance: u32) -> Option<u32> {
        let n = self.slots.binary_search_by_key(&instance, |s| s.0).ok()?;
        u32::try_from(n as u64 * self.stride).ok()
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
        let mut materials = HashMap::new();
        let mut rigid = HashMap::new();
        for (material, program) in programs {
            if let Some(g) = Gpu::new(device, queue, program, stage_image) {
                if let Some(r) = Rigid::new(device, &g, world, material) {
                    rigid.insert(material, r);
                }
                materials.insert(material, g);
            }
        }
        let mut pages: HashMap<LightmapKey, Option<wgpu::TextureView>> = HashMap::new();
        let batch_groups = world
            .batches
            .iter()
            .map(|b| {
                let g = materials.get(&b.material)?;
                let page = match b.lightmap {
                    Some(k) => pages
                        .entry(k)
                        .or_insert_with(|| lightmap(k).map(|i| texture(device, queue, &i, "native world lightmap")))
                        .clone(),
                    None => None,
                };
                g.textures(device, page.as_ref())
            })
            .collect();
        Self { materials, rigid, batch_groups, cull: InheritedCull::default() }
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
            uploads.write(&g.vertex_buffer, 0, &frame_vertex(&g.program.vertex, view).to_bytes());
            uploads.write(&g.pixel_buffer, 0, &frame_pixel(&g.program.pixel, view).to_bytes());
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
        for g in self.materials.values_mut() {
            let state = g.program.state;
            let c = cull.of(g.program.inherits_cull, true);
            let (module, layout) = (&g.module, &g.layout);
            g.world_pipelines
                .entry((color, depth, stencil.clone(), c))
                .or_insert_with(|| render_pipeline(device, module, layout, color, depth, state.as_ref(), c, stencil.clone()));
        }
        for (m, r) in &mut self.rigid {
            let program = self.materials.get(m).map(|g| &g.program);
            let state = program.and_then(|p| p.state);
            let c = cull.of(program.is_some_and(|p| p.inherits_cull), false);
            let (module, layout) = (&r.module, &r.layout);
            r.pipelines
                .entry((color, depth, stencil.clone(), c))
                .or_insert_with(|| render_pipeline(device, module, layout, color, depth, state.as_ref(), c, stencil.clone()));
        }
    }

    /// Batch `index`'s pipeline and groups, when its material has a
    /// program and the pipelines for these formats and stencil state are
    /// made.
    pub fn batch(
        &self, index: usize, material: u16, color: wgpu::TextureFormat, depth: wgpu::TextureFormat, stencil: &wgpu::StencilState,
    ) -> Option<(&wgpu::RenderPipeline, &wgpu::BindGroup, &wgpu::BindGroup)> {
        let g = self.materials.get(&material)?;
        let textures = self.batch_groups.get(index)?.as_ref()?;
        let cull = self.cull.of(g.program.inherits_cull, true);
        Some((g.world_pipelines.get(&(color, depth, stencil.clone(), cull))?, &g.uniforms, textures))
    }

    /// The rigid pipeline, uniforms and dynamic offset placement
    /// `instance`'s draws of `material` take, when it has a rigid program
    /// and a slot for the placement, and the pipelines for these formats
    /// are made (its texture group is the batch's, [`ProgramSet::batch`]).
    pub fn rigid(
        &self, material: u16, instance: u32, color: wgpu::TextureFormat, depth: wgpu::TextureFormat, stencil: &wgpu::StencilState,
    ) -> Option<(&wgpu::RenderPipeline, &wgpu::BindGroup, u32)> {
        let r = self.rigid.get(&material)?;
        let cull = self.cull.of(self.materials.get(&material)?.program.inherits_cull, false);
        Some((r.pipelines.get(&(color, depth, stencil.clone(), cull))?, &r.uniforms, r.offset(instance)?))
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

    /// What placement `instance`'s draws of each material take in a frame
    /// of `view`, by material ascending: the rigid program's words and
    /// its vertex and pixel uniforms, as [`ProgramSet::prepare`] uploads
    /// them (a diagnostic, `H2_NATIVE_WORLD_DUMP`).
    pub fn rigid_uniforms(&self, instance: u32, view: &ViewInputs) -> Vec<RigidUniforms> {
        let mut out: Vec<RigidUniforms> = self
            .rigid
            .iter()
            .filter_map(|(&material, r)| {
                let (_, rows) = r.slots.iter().find(|s| s.0 == instance)?;
                let program = &self.materials.get(&material)?.program;
                Some(RigidUniforms {
                    material,
                    words: r.program.words.clone(),
                    vertex: rigid_vertex(&r.program.vertex, view, rows),
                    pixel: frame_pixel(&program.pixel, view),
                    samplers: std::array::from_fn(|n| {
                        program.dims[n].and(program.samplers[n].as_ref()).and_then(sampler_key)
                    }),
                    // As `translate_pass` translates it.
                    options: VertexOptions { provided_inputs: INPUT_MASK, input_fetch: WORLD_FETCH, ..Default::default() },
                    def: program.def.clone(),
                    pixel_options: program.pixel_options,
                    textures: self.materials.get(&material)?.stage_info.clone(),
                    pipeline: pipeline_state(program.state.as_ref(), self.cull.of(program.inherits_cull, false)),
                })
            })
            .collect();
        out.sort_by_key(|u| u.material);
        out
    }
}

/// One material's rigid draw of a placement ([`ProgramSet::rigid_uniforms`]).
#[derive(Clone, Debug)]
pub struct RigidUniforms {
    pub material: u16,
    /// The rigid program's instruction words ([`RigidProgram::words`]).
    pub words: Vec<u32>,
    pub vertex: VertexUniforms,
    pub pixel: PixelUniforms,
    /// Each sampled stage's sampler ([`sampler_key`]).
    pub samplers: [Option<gfx::translate::SamplerKey>; 4],
    /// The vertex translation's options.
    pub options: VertexOptions,
    /// The pixel shader definition and options.
    pub def: PixelShaderDef,
    pub pixel_options: PixelOptions,
    /// Each stage's bitmap as uploaded ([`describe_image`]; `None`: the
    /// lightmap, an engine texture or nothing).
    pub textures: [Option<String>; 4],
    /// The pipeline's primitive and depth write and compare
    /// ([`pipeline_state`], with the cull of the last
    /// [`ProgramSet::pipelines`]).
    pub pipeline: (wgpu::PrimitiveState, (bool, wgpu::CompareFunction)),
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
        assert_eq!(INPUTS[6].1, 20, "v6 is the lightmap coordinate, after position and uv");
        assert_eq!(INPUTS[5].1, 88, "v5 is the lighting vector's word, the last");
        assert_eq!(FLOAT_INPUTS[5].1, 64, "the float lighting vector, before the words");
        // The packed registers are words, decoded in the shader.
        for (n, (reg, _, format)) in INPUTS.iter().enumerate() {
            let packed = (2..=5).contains(reg);
            assert_eq!(*format == wgpu::VertexFormat::Uint32, packed, "v{n}");
            assert_eq!(WORLD_FETCH[n] == InputFetch::NormPacked3, packed, "v{n}");
        }
        assert_eq!(FLOAT_INPUTS.iter().map(|i| i.0).collect::<Vec<_>>(), regs);
        assert_eq!(std::mem::size_of::<WorldVertex>(), 92);
    }

    #[test]
    fn instance_constants_write_only_their_components() {
        let mut regs = [[9.0f32; 4]; 4];
        put(&mut regs, &Register { index: 2, value: [1.0, 2.0, 3.0, 4.0], written: 0b0101, origin: Origin::Instance });
        assert_eq!(regs[2], [1.0, 9.0, 3.0, 9.0]);
        put(&mut regs, &Register { index: 7, value: [0.0; 4], written: 0xf, origin: Origin::Instance });
        assert_eq!(regs[3], [9.0; 4], "out of range: ignored");
    }

    #[test]
    fn a_frame_puts_the_view_constants_first_and_scales_the_shift() {
        let mut base = VertexUniforms::default();
        base.constants[18] = [10.0, 10.0, 0.0, 0.0];
        let mut constants = [[0.0; 4]; VIEW_CONSTANTS];
        constants[0] = [1.0, 2.0, 3.0, 4.0];
        let view = ViewInputs { constants, guest_size: (640, 480), size: (1280, 960), z_scale: 16_777_215.0, w_clip: None };
        let u = frame_vertex(&base, &view);
        assert_eq!(u.constants[0], [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(u.constants[18], [10.0, 10.0, 0.0, 0.0], "instance constants kept");
        assert_eq!(u.surface, [640.0, 480.0]);
        assert_eq!(u.w_clip, Some([0.0, 16_777_215.0]));
        assert_eq!(u.shift, 3.0 / 8.0, "(2s - 1) / 4s at 2x");
        assert_eq!(frame_vertex(&base, &ViewInputs { size: (640, 480), ..view }).shift, 0.0);
        let p = frame_pixel(&PixelUniforms::default(), &ViewInputs { z_scale: 65_535.0, ..view });
        assert_eq!((p.depth[0], p.depth_range[1]), (65_535.0, 65_535.0));
        // The device's clip range and clamp, as the D3D path takes them
        // (`d3d::nv2a`'s `w_clip_range`, `depth_clamp`): a cinematic's
        // range past the camera's far, clamped.
        let clipped = ViewInputs { w_clip: Some(([0.0, 33_554_430.0], true)), ..view };
        let u = frame_vertex(&base, &clipped);
        assert_eq!((u.w_clip, u.w_clamp), (Some([0.0, 33_554_430.0]), true));
        let p = frame_pixel(&PixelUniforms::default(), &clipped);
        assert_eq!(p.depth_range, [0.0, 33_554_430.0, 0.0, 0.0]);
        assert!(!frame_vertex(&base, &view).w_clamp, "none: the whole range, culled");
    }

    #[test]
    fn per_draw_engine_inputs_keep_the_material_on_the_d3d_path() {
        let look = PassLook { per_draw_inputs: true, ..plain_look() };
        assert_eq!(compile(&look, &plain_program()).unwrap_err(), "per-draw engine inputs");
        assert!(compile(&plain_look(), b"").unwrap_err().starts_with("vertex program"), "no microcode");
    }

    fn plain_look() -> PassLook {
        PassLook {
            pass: map::TagId(0xe000_0001),
            implementation: 0,
            vertex_shader: map::TagId(0xe000_0002),
            textures: [Stage::Empty; 4],
            stage_states: Vec::new(),
            pixel_shader: shaders::PixelShaderDef::default().to_bytes(),
            vertex_constants: vec![Register { index: 19, value: [2.0, 3.0, 0.0, 0.0], written: 0b0011, origin: Origin::Instance }],
            vertex_defaults: Vec::new(),
            per_draw_inputs: false,
        }
    }

    /// `dp4 oPos, v0, c[0..3]; mov oT0, v1; mov oT1, v6`.
    fn plain_program() -> Vec<u8> {
        use shaders::vsh::encode::{Inst, Operand as O, program};
        use shaders::vsh::{MacOp, OutReg};
        let row = |c, mask| Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(O::c(c)), None).to_out(false, OutReg::Pos, mask);
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
        assert_eq!(p.vertex.constants[TEXCOORD_DECOMPRESSION], [1.0, 1.0, 0.0, 0.0]);
        assert_eq!(p.vertex.constants[MODEL_ROWS + 1], [0.0, 1.0, 0.0, 0.0]);
        assert_eq!(p.dims, [None; 4], "samples nothing");
        assert_eq!(p.signed, [[false; 4]; 4]);
    }

    #[test]
    fn a_transparent_pass_carries_its_alpha_test() {
        let p = compile_pass(&plain_look(), &plain_program(), Some((wgpu::CompareFunction::Greater, 0x40))).unwrap();
        shaders::validate(&p.wgsl).expect("valid WGSL");
        assert!(p.wgsl.contains("xps.alpha_ref.x"), "the test is in the fragment shader");
        assert_eq!(p.pixel.alpha_ref[0], 64.0);
        let plain = compile(&plain_look(), &plain_program()).unwrap();
        assert_eq!(plain.pixel.alpha_ref[0], 0.0);
        assert!(!plain.wgsl.contains("xps.alpha_ref.x"));
    }

    #[test]
    fn a_program_reading_an_unsupplied_register_stays_on_the_d3d_path() {
        use shaders::vsh::encode::{Inst, Operand as O, program};
        use shaders::vsh::{MacOp, OutReg};
        let reads_v9 = program(&[Inst::mac(MacOp::Mov, Some(O::v(9)), None, None).to_out(false, OutReg::Pos, 0xf)]);
        let e = compile(&plain_look(), &reads_v9).unwrap_err();
        assert!(e.contains("do not supply"), "{e}");
    }

    #[test]
    fn registers_the_pass_sets_come_from_its_defaults() {
        use shaders::vsh::encode::{Inst, Operand as O, program};
        use shaders::vsh::{MacOp, OutReg};
        // 03a's transparents read v10 and v11 (mask 0x0c00), colours the
        // pass sets with SET_VERTEX_DATA4F_M.
        let reads = program(&[
            Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(O::c(0)), None).to_out(false, OutReg::Pos, 0xf),
            Inst::mac(MacOp::Mov, Some(O::v(10)), None, None).to_out(false, OutReg::D0, 0xf),
            Inst::mac(MacOp::Mov, Some(O::v(11)), None, None).to_out(false, OutReg::D1, 0xf),
            Inst::mac(MacOp::Mov, Some(O::v(1)), None, None).to_out(false, OutReg::T(0), 0xf),
        ]);
        let colour = |index, value| Register { index, value, written: 0xf, origin: Origin::Instance };
        let look = PassLook {
            vertex_defaults: vec![colour(10, [0.45, 0.47, 0.43, 0.0]), colour(11, [0.29, 0.27, 0.25, 0.0])],
            ..plain_look()
        };
        let p = compile(&look, &reads).expect("v10 and v11 are the pass's");
        shaders::validate(&p.wgsl).expect("valid WGSL");
        assert!(p.wgsl.contains("xvs.defaults[10]") && p.wgsl.contains("xvs.defaults[11]"));
        assert_eq!(p.vertex.defaults[10], [0.45, 0.47, 0.43, 0.0]);
        assert_eq!(p.vertex.defaults[11], [0.29, 0.27, 0.25, 0.0]);
        // Only v11 set: v10 is still nobody's.
        let only_11 = PassLook { vertex_defaults: vec![colour(11, [0.0; 4])], ..plain_look() };
        let e = compile(&only_11, &reads).unwrap_err();
        assert!(e.contains("(0x0400)"), "{e}");
        assert_eq!(defaults_mask(&[colour(1, [0.0; 4]), colour(10, [0.0; 4])]), 0x0402);
        assert_eq!(defaults_mask(&[Register { written: 0b0111, ..colour(10, [0.0; 4]) }]), 0);
    }

    #[test]
    fn signed_channels_are_stored_biased() {
        assert_eq!(signed_channels(1 << 29 | 1 << 28), [true, false, false, true]);
        let mut i = Image::from_levels(vec![(2, 1, vec![0x7f, 0x01, 0xff, 0x80, 0, 0, 0, 0]), (1, 1, vec![0x81, 0, 0, 0])]).unwrap();
        bias_signed(&mut i, [true, false, false, true]);
        assert_eq!(i.rgba, vec![0xff, 0x01, 0xff, 0x00, 0x80, 0, 0, 0x80]);
        assert_eq!(i.mips, vec![vec![0x01, 0, 0, 0x80]], "every level");
        // A signed channel keeps a DXT image off BC, as on the D3D path.
        let dxt = || Image::from_levels(vec![(4, 4, vec![0; 64])]).unwrap().with_blocks(wgpu::TextureFormat::Bc1RgbaUnorm, vec![vec![0; 8]]);
        let mut d = dxt();
        bias_signed(&mut d, [false; 4]);
        assert!(d.blocks.is_some(), "nothing signed: the blocks stay");
        assert!(describe_image(&d, true).starts_with("Bc1RgbaUnorm 4x4 levels 1"), "{}", describe_image(&d, true));
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
            (wgpu::AddressMode::Repeat, wgpu::AddressMode::ClampToEdge, wgpu::AddressMode::MirrorRepeat)
        );
        assert_eq!((d.mag_filter, d.min_filter), (wgpu::FilterMode::Nearest, wgpu::FilterMode::Linear));
        assert_eq!(d.lod_max_clamp, 0.0, "no mip filter: level 0 only");
        s.mip_filter = 2;
        assert_eq!(sampler_descriptor(Some(&s)).mipmap_filter, wgpu::MipmapFilterMode::Linear);
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
        let want = gfx::translate::SamplerKey::from_stage(&stage_state(&s).unwrap(), caps).descriptor();
        assert_eq!(d.anisotropy_clamp, 4);
        assert_eq!((d.mag_filter, d.min_filter, d.mipmap_filter), (want.mag_filter, want.min_filter, want.mipmap_filter));
        assert_eq!(
            (d.address_mode_u, d.address_mode_v, d.address_mode_w),
            (wgpu::AddressMode::ClampToEdge, wgpu::AddressMode::ClampToEdge, wgpu::AddressMode::Repeat),
            "5 is clamp to edge, not mirror; the border clamps to the edge"
        );
        assert_eq!((d.lod_min_clamp, d.lod_max_clamp), (2.0, 32.0), "MAXMIPLEVEL is the smallest LOD");
        // A value gfx does not decode still makes a sampler.
        s.mag_filter = 9;
        assert_eq!(sampler_descriptor(Some(&s)).address_mode_u, wgpu::AddressMode::ClampToEdge);
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
        let look = PassLook { stage_states: vec![s, nan], ..plain_look() };
        let p = compile(&look, &plain_program()).unwrap();
        assert_eq!(p.pixel.lod_bias, [-0.5, 0.0, 0.0, 0.0]);
    }

    /// `plain_look` with a final combiner (`rgb = r0`, D3D's own without
    /// fog), as the world's layer-3 passes have one.
    fn final_look(fog: bool) -> PassLook {
        let mut def = shaders::PixelShaderDef::default();
        (def.final_inputs_abcd, def.final_inputs_efg) = psh::d3d_default_final_combiner(fog, false);
        PassLook { pixel_shader: def.to_bytes(), ..plain_look() }
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
        };
        WorldPass { look, state, inherits_cull: false }
    }

    #[test]
    fn a_pass_that_inherits_its_cull_takes_the_device_s() {
        let cw: Cull = (Some(wgpu::Face::Front), wgpu::FrontFace::Cw);
        let cull = InheritedCull { device: Some(cw), placements: true, clusters: false };
        assert_eq!(cull.of(true, false), Some(cw), "a placement");
        assert_eq!(cull.of(true, true), None, "a cluster, unless asked");
        assert_eq!(cull.of(false, false), None, "a pass that sets its own");
        assert_eq!(InheritedCull { clusters: true, ..cull }.of(true, true), Some(cw));
        assert_eq!(InheritedCull { device: None, ..cull }.of(true, false), None, "not known");
        let mut pass = world_pass(final_look(false));
        let (primitive, depth) = pipeline_state(Some(&pass.state), cull.of(true, false));
        assert_eq!((primitive.cull_mode, primitive.front_face), cw);
        assert_eq!(depth, (true, wgpu::CompareFunction::LessEqual));
        pass.state.primitive.front_face = wgpu::FrontFace::Ccw;
        assert_eq!(pipeline_state(Some(&pass.state), None).0, pass.state.primitive, "its own without one");
        let world = pipeline_state(None, None).0;
        assert_eq!((world.topology, world.cull_mode), (wgpu::PrimitiveTopology::TriangleList, None));
        pass.inherits_cull = true;
        let p = compile_world(&pass, &plain_program()).expect("compiles");
        assert!(p.inherits_cull);
    }

    #[test]
    fn a_world_pass_compiles_with_its_state() {
        let p = compile_world(&world_pass(final_look(false)), &plain_program()).expect("compiles");
        shaders::validate(&p.wgsl).expect("valid WGSL");
        let state = p.state.expect("its own state");
        assert_eq!(state.alpha_test, Some((wgpu::CompareFunction::Greater, 0x40)));
        assert!(p.wgsl.contains("xps.alpha_ref.x"), "the alpha test is in the fragment shader");
        assert_eq!(p.inputs, 0b100_0011, "v0, v1, v6");
        assert!(compile(&final_look(false), &plain_program()).unwrap().state.is_none());
    }

    #[test]
    fn world_passes_that_read_device_state_stay_on_the_d3d_path() {
        let refusal = |look: PassLook| compile_world(&world_pass(look), &plain_program()).unwrap_err();
        assert!(refusal(plain_look()).contains("no final combiner"));
        assert_eq!(refusal(final_look(true)), "reads the fog register");
        let mut def = PixelShaderDef::from_bytes(&final_look(false).pixel_shader).unwrap();
        def.texture_modes = 0x06 << 5;
        let bump = PassLook { pixel_shader: def.to_bytes(), ..final_look(false) };
        assert!(refusal(bump).contains("BumpEnvMap"));
        let mut border = stage_states();
        border.address_v = 4;
        let bordered = PassLook {
            textures: [Stage::Lightmap, Stage::Empty, Stage::Empty, Stage::Empty],
            stage_states: vec![border],
            ..final_look(false)
        };
        assert_eq!(refusal(bordered), "stage 0 addresses the border");
        let unsampled = PassLook { stage_states: vec![border], ..final_look(false) };
        assert!(compile_world(&world_pass(unsampled), &plain_program()).is_ok(), "only a sampled stage counts");
    }

    #[test]
    fn a_world_program_reads_only_the_constants_the_native_pass_sets() {
        use shaders::vsh::encode::{Inst, Operand as O, program};
        use shaders::vsh::{MacOp, OutReg};
        let reads = |c: u8| {
            program(&[
                Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(O::c(c)), None).to_out(false, OutReg::Pos, 0xf),
                Inst::mac(MacOp::Mov, Some(O::v(1)), None, None).to_out(false, OutReg::T(0), 0xf),
            ])
        };
        let pass = world_pass(final_look(false));
        for c in [0, 7, 17, 19, 50, 52, 170, 171, 172] {
            assert!(compile_world(&pass, &reads(c)).is_ok(), "c[{c}] is set");
        }
        for c in [18, 21, 24, 31, 53, 174] {
            let e = compile_world(&pass, &reads(c)).unwrap_err();
            assert_eq!(e, format!("vertex program reads c[{c}], which the native pass does not set"));
        }
        let relative = program(&[Inst::mac(MacOp::Mov, Some(O::c_rel(10)), None, None).to_out(false, OutReg::Pos, 0xf)]);
        assert!(compile_world(&pass, &relative).unwrap_err().contains("relative"));
    }

    /// The rigid classification's shape: `r10 = v0 · c[170] + c[171]`,
    /// `dph r0.xyz, r10, c[50..52]` (the placement's rows), then
    /// `dph oPos, r0, c[0..3]`, `mov oT0, v1`; `rows: false` places with
    /// `c[53..55]` instead.
    fn rigid_program(rows: bool) -> Vec<u8> {
        use shaders::vsh::encode::{Inst, Operand as O, program};
        use shaders::vsh::{MacOp, OutReg};
        let model = if rows { MODEL_ROWS as u8 } else { MODEL_ROWS as u8 + 3 };
        // One constant per instruction: the scale, then the offset.
        let mut code = vec![
            Inst::mac(MacOp::Mul, Some(O::v(0)), Some(O::c(170)), None).to_temp(10, 0xf),
            Inst::mac(MacOp::Add, Some(O::r(10)), None, Some(O::c(171))).to_temp(10, 0xf),
        ];
        for k in 0..3u8 {
            code.push(Inst::mac(MacOp::Dph, Some(O::r(10)), Some(O::c(model + k)), None).to_temp(0, 8 >> k));
        }
        for k in 0..4u8 {
            code.push(Inst::mac(MacOp::Dph, Some(O::r(0)), Some(O::c(k)), None).to_out(false, OutReg::Pos, 8 >> k));
        }
        code.push(Inst::mac(MacOp::Mov, Some(O::v(1)), None, None).to_out(false, OutReg::T(0), 0xf));
        program(&code)
    }

    /// The native module is the D3D path's for the same draw, text for
    /// text: `d3d::nv2a`'s `translate` decodes the words as `Normal`, with
    /// the layout's fetch (the world's frame and lighting vector bound as
    /// `NORMPACKED3` words, the uniform compare's 03a draws), and links the
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
            code.push(Inst::mac(MacOp::Dph, Some(O::r(10)), Some(O::c(MODEL_ROWS as u8 + k)), None).to_temp(0, 8 >> k));
        }
        for k in 0..4u8 {
            code.push(Inst::mac(MacOp::Dph, Some(O::r(0)), Some(O::c(k)), None).to_out(false, OutReg::Pos, 8 >> k));
        }
        code.push(Inst::mac(MacOp::Mov, Some(O::v(1)), None, None).to_out(false, OutReg::T(0), 0xf));
        code.push(Inst::mac(MacOp::Mov, Some(O::v(2)), None, None).to_out(false, OutReg::T(1), 0xf));
        code.push(Inst::mac(MacOp::Mov, Some(O::v(5)), None, None).to_out(false, OutReg::T(2), 0xf));
        let microcode = program(&code);
        let pass = world_pass(final_look(false));
        let p = compile_pass(&pass.look, &microcode, pass.state.alpha_test).expect("compiles");
        let words: Vec<u32> = microcode[4..].chunks_exact(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
        let d3d_with = |fetch: [InputFetch; 16], def: &PixelShaderDef| {
            let program = vsh::Program::from_words(ProgramKind::Normal, &words).unwrap();
            let options = VertexOptions { provided_inputs: INPUT_MASK, flat_shading: false, input_fetch: fetch };
            let vs = vsh::translate(&program, &options).unwrap();
            shaders::link(&vs, &psh::translate(def, &p.pixel_options)).unwrap()
        };
        let d3d = |fetch: [InputFetch; 16]| d3d_with(fetch, &p.def);
        assert_eq!(p.wgsl, d3d(WORLD_FETCH));
        assert!(p.wgsl.contains("xvs_normpacked3"), "v2 and v5 decoded in the shader");
        assert_ne!(p.wgsl, d3d([InputFetch::Float; 16]), "floats were another module");
        // The D3D path's definition carries the device's final-combiner
        // latch (`explicit_final`); with the final combiner's inputs set,
        // as a world pass's are, it translates to the same module.
        assert!(p.def.final_inputs_abcd != 0 || p.def.final_inputs_efg != 0);
        let latched = PixelShaderDef { explicit_final: true, ..p.def.clone() };
        assert_eq!(p.wgsl, d3d_with(WORLD_FETCH, &latched));
        shaders::validate(&p.wgsl).expect("valid WGSL");
    }

    #[test]
    fn a_rigid_program_draws_a_placement_with_its_rows() {
        let pass = world_pass(final_look(false));
        let r = compile_rigid(&pass, &rigid_program(true)).expect("compiles");
        shaders::validate(&r.wgsl).expect("valid WGSL");
        assert_eq!(r.inputs, 0b11, "v0, v1");
        assert_eq!(r.vertex.constants[POSITION_DECOMPRESSION], [1.0; 4], "the BSP's decompression is the identity");
        assert_eq!(r.vertex.constants[POSITION_DECOMPRESSION + 1], [0.0; 4]);
        assert_eq!(r.vertex.constants[19], [2.0, 3.0, 0.0, 0.0], "the instance's constants");
        let code = rigid_program(true);
        let count = vsh::Program::parse(&code).unwrap().instructions.len();
        assert_eq!(r.words.len(), 4 * count, "four words an instruction, no header");
        assert_eq!(r.words[0], u32::from_le_bytes([code[4], code[5], code[6], code[7]]));
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
        let view = ViewInputs { constants, guest_size: (640, 480), size: (640, 480), z_scale: 16_777_215.0, w_clip: None };
        let u = rigid_vertex(&r.vertex, &view, &rows);
        assert_eq!(u.constants[3], [5.0, 6.0, 7.0, 8.0]);
        assert_eq!(u.z_max, 16_777_215.0);
        let bits = |rows: &[[f32; 4]]| rows.iter().map(|r| r.map(f32::to_bits)).collect::<Vec<_>>();
        assert_eq!(bits(&u.constants[MODEL_ROWS..MODEL_ROWS + 3]), bits(&rows[..]));
        // The slot buffer: one placement's uniforms per stride.
        let other = [[2.0; 4]; 3];
        let stride = 0x1000;
        let bytes = rigid_slot_bytes(&r.vertex, &view, &[(4, rows), (9, other)], stride);
        assert_eq!(bytes.len(), stride + VertexUniforms::SIZE);
        assert_eq!(&bytes[..VertexUniforms::SIZE], &u.to_bytes()[..]);
        assert_eq!(&bytes[stride..], &rigid_vertex(&r.vertex, &view, &other).to_bytes()[..]);
    }

    #[test]
    fn a_rigid_program_fits_only_placements_with_their_rows_and_streams() {
        use crate::world::{Owner, PartInput, Placement, SectionInput, WorldBuilder};
        let section = |material, uvs: bool| SectionInput {
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            uvs: if uvs { vec![[0.0; 2]; 3] } else { Vec::new() },
            normals: vec![[0.0, 0.0, 1.0]; 3],
            parts: vec![PartInput { material, transparent: false, triangles: vec![[0, 1, 2]] }],
            ..Default::default()
        };
        let rows = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];
        let mut b = WorldBuilder::new();
        b.add_section(Owner::Cluster(0), &section(1, false), &Placement::IDENTITY, None);
        b.add_section(Owner::Instance(5), &section(1, true), &Placement::IDENTITY, None);
        b.add_section(Owner::Instance(2), &section(1, true), &Placement::IDENTITY, None);
        b.add_section(Owner::Instance(6), &section(2, false), &Placement::IDENTITY, None);
        b.add_section(Owner::Instance(7), &section(3, true), &Placement::IDENTITY, None);
        b.place_rows(5, rows);
        b.place_rows(2, [[3.0; 4]; 3]);
        b.place_rows(6, rows);
        let world = b.finish();
        let r = compile_rigid(&world_pass(final_look(false)), &rigid_program(true)).unwrap();
        assert_eq!(rigid_fit(&world, 1, &r), Ok(()), "a cluster without the stream is not the rigid program's");
        assert_eq!(rigid_fit(&world, 2, &r), Err("a placement's part has no v1".to_string()));
        assert_eq!(rigid_fit(&world, 3, &r), Err("a placement's rows did not read".to_string()));
        let slots = rigid_slots(&world, 1).unwrap();
        assert_eq!(slots.iter().map(|s| s.0).collect::<Vec<_>>(), vec![2, 5], "by placement, the cluster has none");
        assert_eq!(slots[0].1, [[3.0; 4]; 3]);
        assert_eq!(rigid_slots(&world, 3), None);
        assert_eq!(rigid_slots(&world, 9), Some(Vec::new()));
    }

    #[test]
    fn a_material_fits_the_world_only_when_every_part_can_be_drawn() {
        use crate::world::{Owner, PartInput, Placement, SectionInput, WorldBuilder};
        let page = LightmapKey { page: 1, palette: 0 };
        let section = |material, lit: bool, lighting: bool| SectionInput {
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            uvs: vec![[0.0; 2]; 3],
            lightmap_uvs: if lit { vec![[0.5; 2]; 3] } else { Vec::new() },
            normals: vec![[0.0, 0.0, 1.0]; 3],
            binormals: vec![[1.0, 0.0, 0.0]; 3],
            tangents: vec![[0.0, 1.0, 0.0]; 3],
            lighting: if lighting { vec![[0.0, 0.0, 1.0]; 3] } else { Vec::new() },
            parts: vec![PartInput { material, transparent: false, triangles: vec![[0, 1, 2]] }],
        };
        let mut b = WorldBuilder::new();
        b.add_section(Owner::Cluster(0), &section(1, true, true), &Placement::IDENTITY, Some(page));
        b.add_section(Owner::Cluster(1), &section(2, true, true), &Placement::IDENTITY, Some(page));
        b.add_section(Owner::Cluster(2), &section(2, true, false), &Placement::IDENTITY, Some(page));
        b.add_section(Owner::Cluster(3), &section(3, false, true), &Placement::IDENTITY, Some(page));
        let world = b.finish();
        // The per-pixel lightmap program: every register, the lightmap at
        // stage 3.
        let mut p = compile(&plain_look(), &plain_program()).unwrap();
        p.inputs = INPUT_MASK;
        p.stages[3] = Stage::Lightmap;
        p.dims[3] = Some(TextureDim::D2);
        let all = |_: LightmapKey| true;
        assert_eq!(world_fit(&world, 1, &p, &all), Ok(()));
        assert_eq!(world_fit(&world, 2, &p, &all), Err("a part has no lighting vectors (v5)".to_string()));
        assert_eq!(world_fit(&world, 3, &p, &all), Err("a part has no lightmap coordinates (v6)".to_string()));
        assert_eq!(world_fit(&world, 1, &p, &|_: LightmapKey| false), Err("a part's lightmap page does not decode".to_string()));
        // A program that reads neither: the unlit part fits.
        p.inputs = 0b11;
        p.stages[3] = Stage::Empty;
        p.dims[3] = None;
        assert_eq!(world_fit(&world, 3, &p, &|_: LightmapKey| false), Ok(()));
    }
}
