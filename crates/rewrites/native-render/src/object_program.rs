//! An object's layer-3 pass with its own programs, as the world's
//! [`crate::program`] path draws a world pass: the pass's vertex program
//! and pixel shader translated once per material, and per drawn object a
//! vertex constant block holding what the game's upload would hold
//! (docs/graphics/native-objects.md, "Object programs"):
//!
//! - `c[0..18]`, the view's, from the device each frame
//!   ([`crate::program::frame_vertex`]);
//! - `c[18 + i]`, the implementation's constants: the instance's, with the
//!   engine-driven ones that read the object's lighting put in per object
//!   from [`crate::object_lighting`] ([`object_vertex_uniforms`]);
//! - `c[50..]`, the object's node rows as the draw uploads them for the
//!   render record ([`node_rows`], [`crate::object_nodes`]);
//! - `c[170..173]`, the model's decompression ([`decompression_rows`]);
//!
//! and its pixel block with the combiner constants that read the lighting
//! packed per object ([`object_pixel_uniforms`]). The implementation a
//! record draws is the build's (`0x0004cbf0`, [`record_implementation`]);
//! its vertex streams are the game's declaration
//! ([`crate::object_streams`]).
//!
//! A pass with animated constants (`shad +0x3c`) compiles from its
//! registers at clock 0 and keeps its animation ([`ObjectPass::animation`]):
//! each draw samples it at the render clock through the structure
//! planner's sampler ([`crate::layer_pass::PassAnimation`]) before the
//! object's lighting goes in ([`animated_blocks`]).

use shaders::psh::PixelUniforms;
use shaders::vsh::{self, VertexUniforms};
use tags::material::{Origin, ResolvedImplementation, VectorTarget, pack_engine_colour};
use tags::shader::AnimatedProperty;

use crate::layer_pass::PassAnimation;
use crate::look::{PassLook, Refusal, implementation_look};
use crate::object_lighting::{
    Defaults, LIGHTING_SCALARS, LIGHTING_VECTORS, Lighting, ShaderValues, scalar_input,
    vector_input,
};
pub use crate::object_nodes::{
    Compression, IMPLEMENTATIONS, MAX_PALETTE, POSITION_DECOMPRESSION, Row, decompression_rows,
    node_rows, record_implementation,
};
use crate::program::{
    MODEL_ROWS, MaterialProgram, PassParts, TEXCOORD_DECOMPRESSION, VIEW_CONSTANTS,
    translate_object_pass,
};

/// The scalar inputs every draw of a shader holds the same
/// (`0x0001bbf0` copies them from the `shad`), and the integer ones.
const SHADER_SCALARS: u64 = 1 << 21;
const SHADER_INTS: u32 = 1 << 4 | 1 << 5;

/// Whether every engine input `imp` reads is one the object path gives:
/// the object's lighting ([`crate::object_lighting`]) or the shader's
/// own. Lighting inputs may land in combiner constants too (packed per
/// object, [`object_pixel_uniforms`]). The first input that is not given,
/// as the reason the object stays on D3D.
pub fn inputs_supported(imp: &ResolvedImplementation) -> Result<(), String> {
    check_inputs(imp, false)
}

fn check_inputs(imp: &ResolvedImplementation, has_layer_inputs: bool) -> Result<(), String> {
    use scene_extract::layout::layer_inputs::{VIEW_SCALARS, VIEW_VECTORS_GIVEN};

    let r = imp.input_reads;
    let (view_vectors, view_scalars) = if has_layer_inputs {
        (VIEW_VECTORS_GIVEN, VIEW_SCALARS)
    } else {
        (0, 0)
    };
    let v = r.vectors & !(LIGHTING_VECTORS | view_vectors);
    if v != 0 {
        return Err(format!(
            "reads vector input {}, which is not the object's lighting",
            v.trailing_zeros()
        ));
    }
    let s = r.scalars & !(LIGHTING_SCALARS | SHADER_SCALARS | view_scalars);
    if s != 0 {
        return Err(format!(
            "reads scalar input {}, which is not the object's lighting",
            s.trailing_zeros()
        ));
    }
    let i = r.ints & !SHADER_INTS;
    if i != 0 {
        return Err(format!("reads integer input {}", i.trailing_zeros()));
    }
    Ok(())
}

/// The one layer-3 pass (first level of detail) of an object material, or
/// why not.
pub fn object_layer3(
    material: &tags::material::Material,
) -> Result<&tags::material::ResolvedPass, String> {
    let layer = material
        .layers
        .iter()
        .find(|l| l.lod == 0 && l.layer == crate::look::LIGHTMAP_LAYER)
        .ok_or("no layer 3")?;
    match layer.passes.as_slice() {
        [one] => Ok(one),
        passes => Err(format!("{} passes in layer 3", passes.len())),
    }
}

/// The pipeline state of an object pass's implementation, or why not:
/// its render states as [`crate::look::layer3_pass`] takes a world pass's
/// (state bindings resolved, layer 3 opaque); where it does
/// not set the cull mode it culls nothing, unless the pass is given the
/// device's cull, which it inherits ([`sets_cull`],
/// `MaterialProgram::inherits_cull`; the pass resolves every field it
/// leaves at the layer-3 call, `object_state::resolve`).
pub fn object_pass_state(
    imp: &ResolvedImplementation,
) -> Result<crate::pass_state::PassState, String> {
    object_pass_state_with(imp, false)
}

/// [`object_pass_state`], a blend the implementation sets kept when
/// `blends` (a world layer's pass other than layer 3's,
/// [`object_layer_pass`]).
fn object_pass_state_with(
    imp: &ResolvedImplementation,
    blends: bool,
) -> Result<crate::pass_state::PassState, String> {
    if !imp.engine_render_states.is_empty() {
        return Err("engine-driven render states".into());
    }
    if !imp.engine_stage_states.is_empty() {
        return Err("engine-driven stage states".into());
    }
    let mut state = crate::pass_state::object_program_state(&imp.render_states)
        .map_err(|e| format!("{e:?}"))?;
    if !blends && state.blend.is_some_and(|b| b != wgpu::BlendState::REPLACE) {
        return Err("the pass blends".into());
    }
    if !sets_cull(imp) {
        state.primitive.cull_mode = None;
    }
    Ok(state)
}

/// Whether `imp` sets `CULLMODE`; one that does not draws with the cull
/// the device holds (as a world pass, `crate::look::WorldPass::inherits_cull`).
pub fn sets_cull(imp: &ResolvedImplementation) -> bool {
    let table = gfx::xdk::RenderStateTable::for_build(gfx::xdk::HALO2_BUILD);
    imp.render_states.iter().any(|s| {
        gfx::state::RenderStateType::from_guest_index(&table, u32::from(s.state))
            == Some(gfx::state::RenderStateType::CullMode)
    })
}

/// One implementation of an object material's layer-3 pass compiled for
/// one vertex declaration: the program (its state set) and the resolved
/// implementation its per-object constants are put in from.
#[derive(Clone, Debug)]
pub struct ObjectPass {
    pub program: MaterialProgram,
    /// The material leaves hardware c[18] untouched and the program reads it.
    pub inherits_c18: bool,
    /// The rows of `c[21..25]` the program reads and the pass does not set
    /// (bit `n`: `c[21 + n]`): what the prologue uploads from the active
    /// structure's parameter block for a shader with flag bit 0
    /// (`frame_scene::StructureParameters`). An item without that upload
    /// refuses a program reading them
    /// (`object_program_gpu::with_structure_parameters`).
    pub structure_reads: u8,
    /// The object program supplies D3D's default final combiner using
    /// the fog and specular uniforms.
    pub default_final: bool,
    /// Layer 18's engine-driven blend constant, evaluated for each item.
    pub(crate) blend_color: Option<std::sync::Arc<crate::object_state::LayerBlendColor>>,
    /// The implementation as compiled: its animated constants at clock 0,
    /// its animated targets cleared.
    pub implementation: ResolvedImplementation,
    /// The pass's animated constants, sampled per draw
    /// ([`animated_blocks`]); `None` when it animates nothing.
    pub animation: Option<PassAnimation>,
    /// The stages that sample the vector normalization cube (empty in
    /// `program.stages`; the host binds the cube).
    pub normalization: [bool; 4],
    /// The stages that sample the texture accumulation target
    /// ([`TEXACCUM_TEXTURE`]; empty in `program.stages`; the host binds the
    /// level the D3D path drew, in texel coordinates).
    pub texaccum: [bool; 4],
    /// The stages that sample the view's own target ([`VIEW_TEXTURE`];
    /// empty in `program.stages`; the host binds a copy of the bound
    /// colour made before the pass, in texel coordinates).
    pub view: [bool; 4],
    /// The stages that sample the scene copy ([`SCENE_COPY_TEXTURE`];
    /// empty in `program.stages`; the host binds the downsample of the
    /// bound colour made once per sorted-handler call, in texel
    /// coordinates of its guest size).
    pub scene_copy: [bool; 4],
    /// The vertex program's instruction words (four per instruction, no
    /// header) and the translation's options: what the D3D path's draw of
    /// the same part binds.
    pub words: Vec<u32>,
    pub options: shaders::vsh::VertexOptions,
    /// A layer-5 PRT variant: what its PRT stream feeds
    /// (`object_program_gpu::compile_layer_sections`); `None` otherwise.
    pub prt: Option<crate::object_program_gpu::PrtInput>,
}

/// Compiles `imp` of `pass` with `microcode` for a vertex buffer
/// supplying `provided` (bit `n`: `v<n>`), or says why not. `lightmap`:
/// whether a stage may sample the lightmap (the host binds what an
/// object's draw binds there, [`OBJECT_LIGHTMAP`]). `properties`: the
/// shader instance's animated properties, which `imp`'s animated targets
/// read ([`ObjectPass::animation`]).
pub fn object_pass(
    pass: map::TagId,
    imp: &ResolvedImplementation,
    properties: &[AnimatedProperty],
    microcode: &[u8],
    provided: u16,
    fetch: [shaders::vsh::InputFetch; 16],
    lightmap: bool,
) -> Result<ObjectPass, String> {
    object_pass_with(pass, imp, properties, microcode, provided, fetch, lightmap, false)
}

/// The passes of an object material's world layer `layer` at shader
/// level `level` (in the order `0x00016610` draws them), or why none:
/// the layers an object item draws besides layer 3
/// (`object_program_gpu::OBJECT_LAYERS`). An item names its level
/// (`frame_scene::LayerItem::level`), so every level compiles.
pub fn object_layer_passes(
    material: &tags::material::Material,
    layer: u8,
    level: usize,
) -> Result<&[tags::material::ResolvedPass], String> {
    material
        .layers
        .iter()
        .find(|l| l.lod == level && l.layer == usize::from(layer))
        .map(|l| l.passes.as_slice())
        .filter(|p| !p.is_empty())
        .ok_or_else(|| format!("no layer {layer} at level {level}"))
}

/// The shader levels `material` has passes of in `layer`, in order.
pub fn object_layer_levels(material: &tags::material::Material, layer: u8) -> Vec<usize> {
    let mut levels: Vec<usize> = material
        .layers
        .iter()
        .filter(|l| l.layer == usize::from(layer) && !l.passes.is_empty())
        .map(|l| l.lod)
        .collect();
    levels.sort_unstable();
    levels.dedup();
    levels
}

/// [`object_pass`] for one pass of a world layer other than 3: its blend
/// kept (the pipeline's, `object_state::resolve_layer` resolving what it
/// inherits), and no lightmap stage, since outside layer 3 the geometry
/// draw binds no lightmap page and adds no coordinate stream
/// (`0x0004d28b`).
pub fn object_layer_pass(
    pass: map::TagId,
    imp: &ResolvedImplementation,
    properties: &[AnimatedProperty],
    microcode: &[u8],
    provided: u16,
    fetch: [shaders::vsh::InputFetch; 16],
) -> Result<ObjectPass, String> {
    object_pass_with(pass, imp, properties, microcode, provided, fetch, false, true)
}

#[allow(clippy::too_many_arguments)]
fn object_pass_with(
    pass: map::TagId,
    imp: &ResolvedImplementation,
    properties: &[AnimatedProperty],
    microcode: &[u8],
    provided: u16,
    fetch: [shaders::vsh::InputFetch; 16],
    lightmap: bool,
    blends: bool,
) -> Result<ObjectPass, String> {
    let (baseline, animation) = PassAnimation::baseline_with_frames(imp, properties, true, 0)
        .map_err(|r| format!("animated constants: {}: {r:?}", r.name()))?;
    let imp = &baseline;
    let state = object_pass_state_with(imp, blends)?;
    let (
        look,
        EngineBound {
            normalization,
            texaccum,
            view,
            scene_copy,
        },
    ) = object_look_stages_with(pass, imp, blends)?;
    if !lightmap && look.textures.contains(&crate::look::Stage::Lightmap) {
        if blends {
            return Err(
                "a stage samples the lightmap, which the draw binds only in layer 3".into(),
            );
        }
        return Err("a stage samples the lightmap, and the object's (the globals' default) is not uniform or does not load".into());
    }
    let (mut program, inherits_c18, structure_reads) = compile_with_layer_constants(
        &look,
        EngineBound {
            normalization,
            texaccum,
            view,
            scene_copy,
        },
        microcode,
        state.alpha_test,
        provided,
        fetch,
        blends,
        true,
    )?;
    let default_final = !program.def.has_final_combiner();
    program.state = Some(state);
    program.inherits_cull = !sets_cull(imp);
    let count = vsh::Program::parse(microcode)
        .map_err(|e| format!("vertex program: {e:?}"))?
        .instructions
        .len();
    let words = microcode
        .get(4..4 + 16 * count)
        .unwrap_or_default()
        .chunks_exact(4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    let options = shaders::vsh::VertexOptions {
        provided_inputs: provided,
        input_fetch: fetch,
        ..Default::default()
    };
    Ok(ObjectPass {
        program,
        inherits_c18,
        structure_reads,
        default_final,
        blend_color: None,
        implementation: baseline.clone(),
        animation,
        normalization,
        texaccum,
        view,
        scene_copy,
        words,
        options,
        prt: None,
    })
}

/// The blocks an object's draw of `pass` starts from: the compiled
/// pass's, or for an animated pass its registers sampled at `time` (the
/// render clock) over them ([`PassAnimation::sample_with_frames`]),
/// counted in `native_shader_animation_objects_*`; and per stage an
/// animated bitmap frame reaches, the image it selects
/// ([`crate::layer_pass::frame_image`] over the stage bitmap's count,
/// [`PassAnimation::frame_counts`]), `None` where the stage binds its
/// compiled image. The object's lighting and rows go in after
/// ([`object_vertex_uniforms`], [`object_pixel_uniforms`]), so a lighting
/// binding wins a lane an animation also writes. An error names the
/// sampler's refusal, or a frame that binds the rasterizer default
/// texture; the item keeps its D3D draw.
pub fn animated_blocks(
    pass: &ObjectPass,
    time: Option<f64>,
) -> Result<(std::borrow::Cow<'_, VertexUniforms>, PixelUniforms, [Option<u32>; 4]), String> {
    use timeline::native::ShaderAnimation;
    let Some(animation) = &pass.animation else {
        return Ok((std::borrow::Cow::Borrowed(&pass.program.vertex), pass.program.pixel, [None; 4]));
    };
    let sampled = animation.sample_with_frames(time).map_err(|r| format!("animated constants: {}: {r:?}", r.name()));
    let drawn = sampled.and_then(|(at, frames)| {
        let counts = animation.frame_counts();
        let mut images = [None; 4];
        for (n, frame) in frames.iter().enumerate() {
            let Some(frame) = *frame else { continue };
            let count = counts[n].unwrap_or(0);
            images[n] = Some(crate::layer_pass::frame_image(frame, count).ok_or_else(|| {
                format!("animated frame {frame} of stage {n} ({count} images) binds the rasterizer default texture")
            })?);
        }
        Ok((std::borrow::Cow::Owned(at.vertex(&pass.program.vertex)), at.pixel(&pass.program.pixel), images))
    });
    match drawn {
        Ok(_) => ShaderAnimation::ObjectsDrawn.count(1),
        Err(_) => ShaderAnimation::ObjectsRefused.count(1),
    }
    drawn
}

/// `pass`'s implementation `imp` as an object draws it, or why not: what
/// [`implementation_look`] takes, except that engine vertex constants are
/// allowed when [`inputs_supported`] (they are put in per object by
/// [`object_vertex_uniforms`]; the look holds their resolved values).
pub fn object_look(pass: map::TagId, imp: &ResolvedImplementation) -> Result<PassLook, String> {
    object_look_stages(pass, imp).map(|(look, _)| look)
}

/// What an object's layer-3 draw binds where a pass samples the lightmap
/// (Code). The draw (`0x0004d0b0`, draw 3) leaves the current lightmap
/// page `[0x00485a60]` at -1 for any record with a model datum at `+4`
/// (`0x0004d2c4`..`0x0004d2de` and `0x0004d362`, then `0x0004d420`): it
/// binds a cluster's page and palette (`0x00015c90`) only for records
/// without one (the BSP's instanced geometry, `0x0004d367`). With the page
/// at -1 the stage setup (`0x00018c43`..`0x00018c4c`, then `0x00018d1f`)
/// binds the globals' rasterizer data element's bitmap at `+0x64`
/// (`[[0x00485a80] + 0x64]`, the datum of the reference at `+0x60`), and
/// no palette. The coordinates the draw adds (`0x0004dc60`: a stream of
/// type `0x2e` from the BSP's `+0x64` element, offset per section) then
/// read that bitmap, so a uniform one is the same wherever they fall.
pub const OBJECT_LIGHTMAP: u32 = 0x64;

/// The stream type of the lightmap coordinates the draw adds
/// (`0x0004dc60`).
pub const LIGHTMAP_STREAM: u8 = 0x2e;

/// The engine texture the game binds at a stage for the vector
/// normalization cube (the globals' rasterizer data, as the lights'
/// [`crate::light_look::LightStage::Normalization`]).
pub const NORMALIZATION_TEXTURE: u8 = 1;

/// The engine texture of the texture accumulation target (Code): render
/// target 1 (`0x00019530`'s entry for 3, `0x000195b2`, binds what
/// `0x00014f60` gives for target 1: its kind, the byte at `0x00015154 + 1`,
/// takes `0x00015044`, the texture `0x0001dcc0(1)` returns). Layer 1's
/// `_combined` passes draw into it; layer 3 samples it in screen space,
/// a linear texture read in texels.
pub const TEXACCUM_TEXTURE: u8 = 3;

/// The texture header `0x0001dcc0(1)` returns for target 1: the target
/// descriptors are `0x98` bytes from `0x004b4b58`, the texture at `+4`
/// (valid while `+0x88` is set and the byte at `+0x95` clear,
/// [`crate::object_nodes::texaccum_bound`]).
pub const TEXACCUM_HEADER: u32 = crate::object_nodes::TEXACCUM_HEADER;

/// The engine texture of the view's own target (Code): the engine-texture
/// table (`0x00019530`, the jump table at `0x00019b68`) binds for 5 the
/// render target the view is drawing into
/// (`crate::screen_look::EngineTexture::View`). The camouflage layers'
/// passes sample it as the scene behind the object; a render target is a
/// linear texture, read in texels as the texture accumulation target is.
pub const VIEW_TEXTURE: u8 = 5;

/// The engine texture of the scene copy (Code): the table binds for 17
/// (`crate::screen_look::engine_texture`: `Target(0x13)`) the target the
/// guest's `0x000352e0` fills at a sorted handler's entry with a four-stage
/// copy of the view's colour, 320 by 240. The camouflage layer (18) bumps
/// its coordinates and the hologram layer (21) projects it; both read it
/// in texels of that size.
pub const SCENE_COPY_TEXTURE: u8 = 17;

/// The scene copy's size in guest pixels, before the render scale.
pub const SCENE_COPY_SIZE: (u32, u32) = (320, 240);

/// The stages of an object pass the host binds an engine texture at
/// (empty in the look).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EngineBound {
    /// The vector normalization cube (engine texture 1, sampled as a cube
    /// without a bias as the lights do).
    pub normalization: [bool; 4],
    /// The texture accumulation target (engine texture 3, 2D, unbiased).
    pub texaccum: [bool; 4],
    /// The view's own target (engine texture 5, 2D, unbiased): a copy of
    /// the bound colour.
    pub view: [bool; 4],
    /// The scene copy (engine texture 17, 2D, unbiased): the downsample of
    /// the bound colour a sorted handler's layers 18 and 21 sample.
    pub scene_copy: [bool; 4],
}

/// [`object_look`] with the stages that sample the vector normalization
/// cube, the texture accumulation target or the view's own target, which
/// the host binds; in the look those stages are empty.
pub fn object_look_stages(
    pass: map::TagId,
    imp: &ResolvedImplementation,
) -> Result<(PassLook, EngineBound), String> {
    object_look_stages_with(pass, imp, false)
}

fn object_look_stages_with(
    pass: map::TagId,
    imp: &ResolvedImplementation,
    has_layer_inputs: bool,
) -> Result<(PassLook, EngineBound), String> {
    check_inputs(imp, has_layer_inputs)?;
    let mut instance = imp.clone();
    for r in instance
        .vertex_constants
        .iter_mut()
        .chain(instance.vertex_defaults.iter_mut())
    {
        r.origin = Origin::Instance;
    }
    let mut bound = EngineBound::default();
    for (n, t) in instance.textures.iter_mut().enumerate() {
        let Some(t) = t else { continue };
        let slot = match t.source {
            tags::material::TextureSource::Engine(NORMALIZATION_TEXTURE) => {
                &mut bound.normalization[n]
            }
            tags::material::TextureSource::Engine(TEXACCUM_TEXTURE) => &mut bound.texaccum[n],
            tags::material::TextureSource::Engine(VIEW_TEXTURE) => &mut bound.view[n],
            tags::material::TextureSource::Engine(SCENE_COPY_TEXTURE) => &mut bound.scene_copy[n],
            _ => continue,
        };
        *slot = true;
        t.source = tags::material::TextureSource::None;
    }
    let mut look = implementation_look(pass, &instance).map_err(|r: Refusal| format!("{r:?}"))?;
    look.per_draw_inputs = false;
    Ok((look, bound))
}

/// Whether an object pass sets constant `c` (hardware numbering) for
/// instance constants `look`: the view's, the implementation's, the node
/// rows and the decompression.
fn object_sets(look: &PassLook, c: usize) -> bool {
    c < VIEW_CONSTANTS
        || (MODEL_ROWS..MODEL_ROWS + 3).contains(&c)
        || (POSITION_DECOMPRESSION..=TEXCOORD_DECOMPRESSION).contains(&c)
        || look
            .vertex_constants
            .iter()
            .any(|r| usize::from(r.index) == c && r.written != 0)
}

/// Translates an object pass with its vertex program `microcode`, or says
/// why it stays on D3D. Every constant it reads directly must be one
/// [`object_sets`]; a read relative to `a0.x` must start in the node rows
/// (the skinned palette, which the draw fills for the part's matrices).
pub fn compile_object(
    look: &PassLook,
    microcode: &[u8],
    alpha_test: Option<(wgpu::CompareFunction, u8)>,
    provided_inputs: u16,
) -> Result<MaterialProgram, String> {
    compile_object_stages(
        look,
        EngineBound::default(),
        microcode,
        alpha_test,
        provided_inputs,
        [shaders::vsh::InputFetch::Float; 16],
    )
}

/// [`compile_object`] with the stages the host binds an engine texture at
/// ([`object_look_stages`]).
pub fn compile_object_stages(
    look: &PassLook,
    engine: EngineBound,
    microcode: &[u8],
    alpha_test: Option<(wgpu::CompareFunction, u8)>,
    provided_inputs: u16,
    input_fetch: [shaders::vsh::InputFetch; 16],
) -> Result<MaterialProgram, String> {
    compile_with_layer_constants(look, engine, microcode, alpha_test, provided_inputs, input_fetch, false, false)
        .map(|(program, _, _)| program)
}

/// `program`'s module with its pixel shader translated again with the
/// polygon stipple test (`PixelOptions::stipple`): what an item drawn
/// while the device's stipple is on (a faded record, `0x00013800`) draws
/// with, its 32 rows in [`PixelUniforms::stipple`]. The vertex half is
/// the compiled module's own: [`shaders::link`] appends the pixel body.
pub fn stipple_wgsl(program: &MaterialProgram) -> Result<String, String> {
    stipple_module(&program.wgsl, program)
}

/// [`stipple_wgsl`] for another module that ends with `program`'s pixel
/// shader (its rigid program's: the placements' vertex half, the material's
/// pixel half).
pub fn stipple_module(module: &str, program: &MaterialProgram) -> Result<String, String> {
    use shaders::psh::{PixelOptions, translate_with_default_final_uniform as translate};
    let plain = translate(&program.def, &program.pixel_options);
    let head = module
        .strip_suffix(plain.body.as_str())
        .ok_or("the module does not end with its pixel shader")?;
    let stippled = translate(&program.def, &PixelOptions { stipple: true, ..program.pixel_options });
    Ok(format!("{head}{}", stippled.body))
}

/// [`compile_object_stages`] for an object pass: with
/// `inherits_material_constants`, a read of `c[18]` the pass does not set
/// is the device's (the second value); with `structure_constants`, reads
/// of `c[21..25]` it does not set are the structure parameters' (the third
/// value, [`ObjectPass::structure_reads`]).
#[allow(clippy::too_many_arguments)]
fn compile_with_layer_constants(
    look: &PassLook,
    engine: EngineBound,
    microcode: &[u8],
    alpha_test: Option<(wgpu::CompareFunction, u8)>,
    provided_inputs: u16,
    input_fetch: [shaders::vsh::InputFetch; 16],
    inherits_material_constants: bool,
    structure_constants: bool,
) -> Result<(MaterialProgram, bool, u8), String> {
    const STRUCTURE: usize = frame_scene::StructureParameters::FIRST_CONSTANT;
    let mut inherits_c18 = false;
    let mut structure_reads = 0u8;
    let program = vsh::Program::parse(microcode).map_err(|e| format!("vertex program: {e:?}"))?;
    for i in &program.instructions {
        for s in i.sources() {
            if let vsh::SrcReg::Const { index, relative: true } = s.reg {
                let c = usize::from(index);
                if !(MODEL_ROWS..POSITION_DECOMPRESSION).contains(&c) {
                    return Err(format!(
                        "vertex program reads c[a0.x + {c}], outside the node rows"
                    ));
                }
            }
        }
    }
    // A constant the pass does not set is read only where its value can
    // reach a live output: one reaching only a coordinate the pixel shader
    // ignores is not (as on the world path, `crate::program::dead_texcoords`).
    // The stencil programs write `oT1..oT3` from `c[20..22]` for every
    // stage, sampled or not. Each live read is the device's `c[18]`, a
    // structure row, or a refusal, in program order.
    let dead = crate::program::dead_texcoords(look);
    let mut taken = [false; vsh::CONSTANT_COUNT];
    loop {
        let missing = |reg: vsh::SrcReg| match reg {
            vsh::SrcReg::Const { index, relative: false }
                if !object_sets(look, usize::from(index)) && !taken.get(usize::from(index)).copied().unwrap_or(true) =>
            {
                Some(index)
            }
            _ => None,
        };
        let Some(index) = crate::program::live_source_read(&program, missing, |out| {
            matches!(out, vsh::OutReg::T(k) if dead.get(usize::from(k)) == Some(&true))
        }) else {
            break;
        };
        let c = usize::from(index);
        if c == 18 && inherits_material_constants {
            inherits_c18 = true;
        } else if structure_constants && (STRUCTURE..STRUCTURE + 5).contains(&c) {
            structure_reads |= 1 << (c - STRUCTURE);
        } else {
            return Err(format!(
                "vertex program reads c[{c}], which the object pass does not set"
            ));
        }
        taken[c] = true;
    }
    let t = translate_object_pass(
        &PassParts {
            microcode,
            pixel_shader: &look.pixel_shader,
            dims: std::array::from_fn(|n| {
                if engine.normalization[n] {
                    Some(shaders::psh::TextureDim::Cube)
                } else if engine.texaccum[n] || engine.view[n] || engine.scene_copy[n] {
                    Some(shaders::psh::TextureDim::D2)
                } else {
                    (look.textures[n] != crate::look::Stage::Empty)
                        .then_some(shaders::psh::TextureDim::D2)
                }
            }),
            biased: look
                .textures
                .map(|s| matches!(s, crate::look::Stage::Bitmap { .. })),
            stage_states: &look.stage_states,
            vertex_constants: &look.vertex_constants,
            vertex_defaults: &look.vertex_defaults,
            alpha_test,
            provided_inputs,
            input_fetch,
            w_buffer: true,
            shadow: [None; 4],
        },
        shaders::psh::translate_with_default_final_uniform,
    )?;
    let mut vertex = t.vertex;
    vertex.constants[POSITION_DECOMPRESSION..=TEXCOORD_DECOMPRESSION]
        .copy_from_slice(&decompression_rows(&Compression::default()));
    Ok((MaterialProgram {
        wgsl: t.wgsl,
        stages: look.textures,
        samplers: t.samplers,
        dims: t.dims,
        signed: t.signed,
        vertex,
        pixel: t.pixel,
        inputs: t.inputs,
        def: t.def,
        pixel_options: t.pixel_options,
        state: None,
        inherits_cull: false,
        rigid: None,
        animation: None,
        texaccum_dimensions: Vec::new(),
    }, inherits_c18, structure_reads))
}

/// Puts the texture accumulation target's size (`guest_size`, what
/// `0x00019530` writes for the stage when it binds it) into the engine
/// vertex constants of `imp` that read the size of a stage in `stages`
/// (`ResolvedImplementation::dimension_bindings`): the resolver knew no
/// size for an engine texture and left 0 by 0 there.
pub fn texaccum_dimensions(
    u: &mut VertexUniforms,
    imp: &ResolvedImplementation,
    stages: [bool; 4],
    guest_size: (u32, u32),
) {
    let size = (guest_size.0 as f32, guest_size.1 as f32);
    for c in &imp.dimension_bindings {
        let Some(stage) = usize::from(c.stage).checked_sub(1) else {
            continue;
        };
        if !stages.get(stage).copied().unwrap_or(false) {
            continue;
        }
        let row = match c.bank {
            1 => u.constants.get_mut(18 + usize::from(c.reg)),
            _ => u.defaults.get_mut(usize::from(c.reg)),
        };
        if let Some(row) = row {
            c.eval_size(row, size);
        }
    }
}

/// One drawn object's inputs.
#[derive(Clone, Copy, Debug)]
pub struct ObjectInputs<'a> {
    /// Material inputs captured at this layer's begin, including the fog
    /// coefficients. Base and first-person submissions carry lighting only.
    pub layer: Option<&'a frame_scene::LayerInputs>,
    pub lighting: Option<&'a Lighting>,
    pub defaults: &'a Defaults,
    pub shader: ShaderValues,
}

impl ObjectInputs<'_> {
    fn vector(&self, source: u8) -> Result<[u32; 3], String> {
        if source < 64 && LIGHTING_VECTORS >> source & 1 != 0 {
            return vector_input(u32::from(source), self.lighting, self.defaults, self.shader)
                .ok_or_else(|| format!("lighting vector {source} is not given for this object"));
        }
        self.layer.and_then(|v| v.vector(usize::from(source))).map(|v| v.map(f32::to_bits))
            .ok_or_else(|| format!("view vector {source} is not given for this layer"))
    }

    fn scalar(&self, source: u8) -> Result<u32, String> {
        if source < 64 && LIGHTING_SCALARS >> source & 1 != 0 {
            return scalar_input(u32::from(source), self.lighting)
                .ok_or_else(|| format!("lighting scalar {source} is not given for this object"));
        }
        self.layer.and_then(|v| v.scalar(usize::from(source))).map(f32::to_bits)
            .ok_or_else(|| format!("view scalar {source} is not given for this layer"))
    }
}

/// The vertex block an object's draw of `imp` reads: `base` (the
/// compiled pass's, [`compile_object`]) with the engine constants that
/// read the lighting or layer inputs evaluated for this draw, its node rows at `c[50]`
/// and its decompression at `c[170]`. The view's `c[0..18]` are put in
/// per frame. An error names a lighting input the port cannot give for
/// this object (then it stays on D3D) or rows that do not fit.
pub fn object_vertex_uniforms(
    base: &VertexUniforms,
    imp: &ResolvedImplementation,
    inputs: &ObjectInputs<'_>,
    nodes: &[Row],
    decompression: [Row; 3],
) -> Result<VertexUniforms, String> {
    let mut u = base.clone();
    let mut put = |bank: u8, reg: u8, components: u8, value: [u32; 3], scalar: bool| {
        let row = match bank {
            1 => u.constants.get_mut(18 + usize::from(reg)),
            _ => u.defaults.get_mut(usize::from(reg)),
        };
        let Some(row) = row else { return };
        for (c, slot) in row.iter_mut().enumerate() {
            if components & (1 << c) != 0 {
                *slot = f32::from_bits(if scalar { value[0] } else { value[c.min(2)] });
            }
        }
    };
    for b in &imp.vector_bindings {
        let VectorTarget::Vertex {
            bank,
            reg,
            components,
        } = b.target
        else {
            continue;
        };
        let v = inputs.vector(b.source)?;
        put(bank, reg, components & 0b0111, v, false);
    }
    for b in &imp.scalar_bindings {
        let VectorTarget::Vertex {
            bank,
            reg,
            components,
        } = b.target
        else {
            continue;
        };
        // The shader's scalar was resolved with the material at load.
        if b.source < 64 && SHADER_SCALARS >> b.source & 1 != 0 {
            continue;
        }
        let s = inputs.scalar(b.source)?;
        put(bank, reg, components, [s, s, s], true);
    }
    if nodes.len() > 3 * MAX_PALETTE {
        return Err(format!("{} node rows run past c[170]", nodes.len()));
    }
    u.constants[MODEL_ROWS..MODEL_ROWS + nodes.len()].copy_from_slice(nodes);
    u.constants[POSITION_DECOMPRESSION..=TEXCOORD_DECOMPRESSION].copy_from_slice(&decompression);
    Ok(u)
}

/// The pixel block an object's draw of `imp` reads: `base` (the compiled
/// pass's, the view's parts put in) with each combiner constant that reads
/// a lighting or layer input packed for this object as `0x00019ca0` packs it
/// (`tags::material::pack_engine_colour`: a vector with alpha 0, a scalar
/// through its binding's transform, `tags::material::engine_scalar`, in
/// its one channel; halved first when the binding says so) into the byte
/// lanes the binding keeps. An error names an input the port cannot give
/// for this object.
pub fn object_pixel_uniforms(
    base: &PixelUniforms,
    imp: &ResolvedImplementation,
    inputs: &ObjectInputs<'_>,
) -> Result<PixelUniforms, String> {
    let mut u = *base;
    for b in &imp.vector_bindings {
        let VectorTarget::Pixel {
            c1,
            slot,
            mask,
            half,
            ..
        } = b.target
        else {
            continue;
        };
        let v = inputs.vector(b.source)?;
        let packed = pack_engine_colour(
            [
                f32::from_bits(v[0]),
                f32::from_bits(v[1]),
                f32::from_bits(v[2]),
                0.0,
            ],
            half,
        );
        put_packed(&mut u, c1, slot, mask, packed);
    }
    for b in &imp.scalar_bindings {
        let VectorTarget::Pixel {
            c1,
            slot,
            mask,
            half,
            transform,
        } = b.target
        else {
            continue;
        };
        // The shader's scalar was resolved with the material at load.
        if b.source < 64 && SHADER_SCALARS >> b.source & 1 != 0 {
            continue;
        }
        let s = inputs.scalar(b.source)?;
        // The scalar lands in the one channel of its mode; the binding's
        // mask is that channel.
        let x = tags::material::engine_scalar(f32::from_bits(s), usize::from(transform));
        put_packed(&mut u, c1, slot, mask, pack_engine_colour([x; 4], half));
    }
    Ok(u)
}

/// `packed`'s lanes in `mask` into combiner constant `slot` of C1 or C0.
pub(crate) fn put_packed(u: &mut PixelUniforms, c1: bool, slot: u8, mask: u32, packed: u32) {
    let bank = if c1 { &mut u.c1 } else { &mut u.c0 };
    if let Some(c) = bank.get_mut(usize::from(slot)) {
        *c = shaders::psh::argb((crate::light_look::repack(*c) & !mask) | (packed & mask));
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::look::Stage;
    use scene_extract::testing::rows_of;
    use shaders::vsh::encode::{Inst, Operand as O, program};
    use shaders::vsh::{MacOp, OutReg};
    use tags::material::{InputReads, Register, VectorBinding};

    /// The registers `v0..v6` from the vertex buffer.
    const WORLD: u16 = crate::program::INPUT_MASK;

    fn implementation(
        reads: InputReads,
        vectors: Vec<VectorBinding>,
        scalars: Vec<VectorBinding>,
    ) -> ResolvedImplementation {
        let reg = |index, origin| Register {
            index,
            value: [7.0; 4],
            written: 0xf,
            origin,
        };
        ResolvedImplementation {
            index: 0,
            vertex_shader: Some(map::TagId(0xe000_0002)),
            textures: [None; 4],
            stage_states: Vec::new(),
            render_states: Vec::new(),
            restore_states: Vec::new(),
            pixel_shader: shaders::PixelShaderDef::default().to_bytes(),
            variant: false,
            c0_written: 0,
            c1_written: 0,
            pixel_constants_from_engine: false,
            per_draw_inputs: true,
            input_reads: reads,
            vector_bindings: vectors,
            scalar_bindings: scalars,
            vertex_constants: vec![
                reg(18, Origin::Engine),
                reg(19, Origin::Engine),
                reg(20, Origin::Instance),
            ],
            vertex_defaults: Vec::new(),
            animated: Vec::new(),
            engine_render_states: Vec::new(),
            engine_stage_states: Vec::new(),
            dimension_bindings: Vec::new(),
            copies_target: false,
        }
    }

    fn vertex(reg: u8, components: u8) -> VectorTarget {
        VectorTarget::Vertex {
            bank: 1,
            reg,
            components,
        }
    }

    /// A lit object implementation: vector 15 into `c[18].xyz`, vector 40
    /// into `c[19].xyz`, scalar 23 into `c[19].w`.
    pub(crate) fn lit() -> ResolvedImplementation {
        implementation(
            InputReads {
                vectors: 1 << 15 | 1 << 40,
                scalars: 1 << 23 | 1 << 21,
                ints: 1 << 4,
            },
            vec![
                VectorBinding {
                    source: 15,
                    target: vertex(0, 0b0111),
                },
                VectorBinding {
                    source: 40,
                    target: vertex(1, 0b0111),
                },
            ],
            vec![VectorBinding {
                source: 23,
                target: vertex(1, 0b1000),
            }],
        )
    }

    #[test]
    fn the_texture_accumulation_target_is_a_stage_the_host_binds_with_its_size() {
        use tags::material::{EngineVertexConstant, StageTexture, TextureSource};
        let mut imp = lit();
        imp.textures[2] = Some(StageTexture {
            source: TextureSource::Engine(TEXACCUM_TEXTURE),
            size: None,
            stage_size: None,
        });
        // The combiners sample t2 (PROJECT2D, 5 bits a stage).
        imp.pixel_shader = shaders::PixelShaderDef {
            texture_modes: 1 << 10,
            ..Default::default()
        }
        .to_bytes();
        // c[20].xy the size of stage 2's texture (mode 9; stage 3 names
        // texture 2), which the resolver left 0 by 0 for an engine texture.
        imp.dimension_bindings = vec![EngineVertexConstant {
            bank: 1,
            reg: 2,
            mode: 9,
            stage: 3,
            source: 0,
        }];
        assert!(
            crate::look::implementation_look(map::TagId(0xe000_0001), &imp).is_err(),
            "the world's look refuses it"
        );
        let (look, bound) = object_look_stages(map::TagId(0xe000_0001), &imp).unwrap();
        assert_eq!(bound.texaccum, [false, false, true, false]);
        assert_eq!(bound.normalization, [false; 4]);
        assert_eq!(look.textures[2], Stage::Empty, "bound by the host");
        let p = compile_object_stages(
            &look,
            bound,
            &rigid_program(),
            None,
            WORLD,
            [shaders::vsh::InputFetch::Float; 16],
        )
        .unwrap();
        assert_eq!(p.dims[2], Some(shaders::psh::TextureDim::D2));
        let mut u = p.vertex.clone();
        texaccum_dimensions(&mut u, &imp, bound.texaccum, (640, 480));
        assert_eq!(u.constants[20][..2], [640.0, 480.0]);
        assert_eq!(
            u.constants[20][2..],
            p.vertex.constants[20][2..],
            "mode 9 writes x and y"
        );
        // A binding of another stage's size is left as resolved.
        let mut other = p.vertex.clone();
        texaccum_dimensions(&mut other, &imp, [true, false, false, false], (640, 480));
        assert_eq!(other.constants[20], p.vertex.constants[20]);
    }

    #[test]
    fn the_views_own_target_is_a_stage_the_host_binds_as_a_copy() {
        use gfx::state::RenderStateType as R;
        use tags::material::{StageTexture, TextureSource};
        let table = gfx::xdk::RenderStateTable::for_build(gfx::xdk::HALO2_BUILD);
        let state = |t: R, value: u32| tags::shader::RenderStateValue {
            state: (0..=255u8)
                .find(|&s| R::from_guest_index(&table, u32::from(s)) == Some(t))
                .expect("a slot"),
            value,
        };
        let mut imp = lit();
        // A layer pass names the states it draws with.
        imp.render_states = vec![state(R::AlphaBlendEnable, 0), state(R::ZWriteEnable, 0)];
        imp.textures[1] = Some(StageTexture {
            source: TextureSource::Engine(VIEW_TEXTURE),
            size: None,
            stage_size: None,
        });
        // The combiners sample t1 (PROJECT2D, 5 bits a stage).
        imp.pixel_shader = shaders::PixelShaderDef {
            texture_modes: 1 << 5,
            ..Default::default()
        }
        .to_bytes();
        assert_eq!(
            crate::screen_look::engine_texture(VIEW_TEXTURE),
            crate::screen_look::EngineTexture::View
        );
        assert!(
            crate::look::implementation_look(map::TagId(0xe000_0001), &imp).is_err(),
            "the world's look refuses it"
        );
        let (look, bound) = object_look_stages(map::TagId(0xe000_0001), &imp).unwrap();
        assert_eq!(bound.view, [false, true, false, false]);
        assert_eq!(
            (bound.texaccum, bound.normalization),
            ([false; 4], [false; 4])
        );
        assert_eq!(look.textures[1], Stage::Empty, "bound by the host");
        let p = compile_object_stages(
            &look,
            bound,
            &rigid_program(),
            None,
            WORLD,
            [shaders::vsh::InputFetch::Float; 16],
        )
        .unwrap();
        assert_eq!(p.dims[1], Some(shaders::psh::TextureDim::D2));
        let fetch = [shaders::vsh::InputFetch::Float; 16];
        let pass = object_layer_pass(
            map::TagId(0xe000_0001),
            &imp,
            &[],
            &rigid_program(),
            WORLD,
            fetch,
        )
        .unwrap();
        assert_eq!(pass.view, [false, true, false, false]);
        assert_eq!(pass.texaccum, [false; 4]);
    }

    #[test]
    fn a_camouflage_pass_binds_the_scene_copy() {
        use gfx::state::RenderStateType as R;
        use tags::material::{StageTexture, TextureSource};
        let table = gfx::xdk::RenderStateTable::for_build(gfx::xdk::HALO2_BUILD);
        let state = |t: R, value: u32| tags::shader::RenderStateValue {
            state: (0..=255u8)
                .find(|&s| R::from_guest_index(&table, u32::from(s)) == Some(t))
                .expect("a slot"),
            value,
        };
        let mut imp = lit();
        imp.render_states = vec![state(R::AlphaBlendEnable, 0), state(R::ZWriteEnable, 0)];
        // The camouflage bump sampled through texture 17 at stage 1.
        imp.textures[1] = Some(StageTexture {
            source: TextureSource::Engine(SCENE_COPY_TEXTURE),
            size: None,
            stage_size: None,
        });
        imp.pixel_shader = shaders::PixelShaderDef {
            texture_modes: 1 << 5,
            ..Default::default()
        }
        .to_bytes();
        assert!(
            crate::look::implementation_look(map::TagId(0xe000_0001), &imp).is_err(),
            "the world's look refuses it"
        );
        let (look, bound) = object_look_stages(map::TagId(0xe000_0001), &imp).unwrap();
        assert_eq!(bound.scene_copy, [false, true, false, false]);
        assert_eq!((bound.view, bound.texaccum), ([false; 4], [false; 4]));
        assert_eq!(look.textures[1], Stage::Empty, "bound by the host");
        let fetch = [shaders::vsh::InputFetch::Float; 16];
        let pass = object_layer_pass(
            map::TagId(0xe000_0001),
            &imp,
            &[],
            &rigid_program(),
            WORLD,
            fetch,
        )
        .unwrap();
        assert_eq!(pass.scene_copy, [false, true, false, false]);
        assert_eq!(pass.view, [false; 4]);
        assert_eq!(pass.program.dims[1], Some(shaders::psh::TextureDim::D2));
    }

    #[test]
    fn inputs_other_than_the_lighting_and_the_shaders_are_refused() {
        assert_eq!(inputs_supported(&lit()), Ok(()));
        let mut time = lit();
        time.input_reads.scalars |= 1 << 2;
        assert_eq!(
            inputs_supported(&time).unwrap_err(),
            "reads scalar input 2, which is not the object's lighting"
        );
        let mut camera = lit();
        camera.input_reads.vectors |= 1 << 5;
        assert!(
            inputs_supported(&camera)
                .unwrap_err()
                .starts_with("reads vector input 5")
        );
        let mut pixel = lit();
        pixel.vector_bindings.push(VectorBinding {
            source: 15,
            target: VectorTarget::Pixel {
                c1: false,
                slot: 0,
                mask: 0x00ff_ffff,
                half: false,
                transform: 0,
            },
        });
        pixel.scalar_bindings.push(VectorBinding {
            source: 23,
            target: VectorTarget::Pixel {
                c1: false,
                slot: 1,
                mask: 0xff00_0000,
                half: false,
                transform: 1,
            },
        });
        assert_eq!(
            inputs_supported(&pixel),
            Ok(()),
            "lighting inputs in combiner constants are packed per object"
        );
    }

    #[test]
    fn an_objects_block_holds_what_the_games_upload_would() {
        // The lighting: +0x30 (vector 15) = (0.5, 0.25, 0.125); +0x10 = 0;
        // its object's root value 0.75.
        let mut words = [0u32; 30];
        for (i, v) in [0.5f32, 0.25, 0.125].iter().enumerate() {
            words[12 + i] = v.to_bits();
        }
        let l = Lighting {
            words,
            root: Some(0.75f32.to_bits()),
        };
        let d = Defaults::default();
        let inputs = ObjectInputs {
            layer: None,
            lighting: Some(&l),
            defaults: &d,
            shader: ShaderValues::default(),
        };
        let imp = lit();
        let look = object_look(map::TagId(0xe000_0001), &imp).unwrap();
        let p = compile_object(&look, &rigid_program(), None, WORLD).unwrap();
        let nodes = rows_of(3);
        let decompression = decompression_rows(&Compression {
            position: Some([0.0, 2.0, 0.0, 2.0, 0.0, 2.0]),
            texcoord: None,
        });
        let u = object_vertex_uniforms(&p.vertex, &imp, &inputs, &nodes, decompression).unwrap();
        // What the game would hold: c[18] = vector 15 (w the resolved value),
        // c[19] = vector 40 broadcast and scalar 23 in w, c[20] the instance's.
        assert_eq!(u.constants[18], [0.5, 0.25, 0.125, 7.0]);
        let luma = (0.125f32 * 0.114 + 0.25 * 0.587 + 0.5 * 0.299).clamp(0.0, 1.0);
        assert_eq!(u.constants[19], [0.75, 0.75, 0.75, luma]);
        assert_eq!(u.constants[20], [7.0; 4]);
        assert_eq!(u.constants[50..53], nodes[..]);
        assert_eq!(u.constants[170..173], decompression[..]);
        // No root value (a parent cycle): not drawn.
        let stale = Lighting { root: None, ..l };
        let e = object_vertex_uniforms(
            &p.vertex,
            &imp,
            &ObjectInputs {
                lighting: Some(&stale),
                ..inputs
            },
            &nodes,
            decompression,
        );
        assert_eq!(
            e.unwrap_err(),
            "lighting vector 40 is not given for this object"
        );
    }

    #[test]
    fn an_objects_pixel_block_packs_its_lighting_as_the_games_upload_does() {
        // Vector 15 (+0x30) = (0.5, 0.25, 0.125).
        let mut words = [0u32; 30];
        for (i, v) in [0.5f32, 0.25, 0.125].iter().enumerate() {
            words[12 + i] = v.to_bits();
        }
        let l = Lighting {
            words,
            root: Some(0),
        };
        let d = Defaults::default();
        let inputs = ObjectInputs {
            layer: None,
            lighting: Some(&l),
            defaults: &d,
            shader: ShaderValues::default(),
        };
        let mut imp = lit();
        imp.vector_bindings = vec![
            // c0[0] RGB, its alpha the instance's.
            VectorBinding {
                source: 15,
                target: VectorTarget::Pixel {
                    c1: false,
                    slot: 0,
                    mask: 0x00ff_ffff,
                    half: false,
                    transform: 0,
                },
            },
            // c1[1] green only, halved: (x + 1) / 2.
            VectorBinding {
                source: 15,
                target: VectorTarget::Pixel {
                    c1: true,
                    slot: 1,
                    mask: 0x0000_ff00,
                    half: true,
                    transform: 0,
                },
            },
        ];
        imp.scalar_bindings = vec![
            // Scalar 23 (the luminance of vector 15) complemented into
            // c0[2]'s alpha, and through the cosine wave into c1[3]'s blue.
            VectorBinding {
                source: 23,
                target: VectorTarget::Pixel {
                    c1: false,
                    slot: 2,
                    mask: 0xff00_0000,
                    half: false,
                    transform: 1,
                },
            },
            VectorBinding {
                source: 23,
                target: VectorTarget::Pixel {
                    c1: true,
                    slot: 3,
                    mask: 0x0000_00ff,
                    half: false,
                    transform: 2,
                },
            },
        ];
        let mut base = PixelUniforms::default();
        base.c0[0] = shaders::psh::argb(0x8011_2233);
        base.c1[1] = shaders::psh::argb(0x1122_3344);
        base.c0[2] = shaders::psh::argb(0x0055_6677);
        base.c1[3] = shaders::psh::argb(0x9988_7766);
        let u = object_pixel_uniforms(&base, &imp, &inputs).unwrap();
        // The game's ARGB: round(255 x) per channel; 0.5 -> 0x80,
        // 0.25 -> 0x40, 0.125 -> 0x20; halved, 0.25 -> 0.625 -> 0x9f.
        assert_eq!(u.c0[0], shaders::psh::argb(0x8080_4020));
        assert_eq!(u.c1[1], shaders::psh::argb(0x1122_9f44));
        // Scalar 23 = luminance 0.299·0.5 + 0.587·0.25 + 0.114·0.125 =
        // 0.3105; 1 - 0.3105 -> 175.8 of 255, 0xb0; (cos(2π·0.3105) + 1) / 2
        // -> 80.2 of 255, 0x50.
        let luma = (0.125f32 * 0.114 + 0.25 * 0.587 + 0.5 * 0.299).clamp(0.0, 1.0);
        assert_eq!(
            scalar_input(23, Some(&l)),
            Some(luma.to_bits()),
            "the scalar the test assumes"
        );
        assert_eq!(u.c0[2], shaders::psh::argb(0xb055_6677));
        assert_eq!(u.c1[3], shaders::psh::argb(0x9988_7750));
        assert_eq!(u.c0[1], base.c0[1], "the rest as compiled");
        assert_eq!(u.c0[3..], base.c0[3..]);
        let none = ObjectInputs {
            lighting: None,
            ..inputs
        };
        assert!(
            object_pixel_uniforms(&base, &imp, &none).is_ok(),
            "vector 15 falls back on the default without a lighting"
        );
    }

    /// Shaped like a `lightmap_dynamic` object program: decompress the
    /// position (`c[170]`, `c[171]`), place it with the node rows
    /// (`c[50..53]`), project (`c[0..4]`), decompress the texture
    /// coordinate (`c[172]`), light with the implementation's `c[18]`,
    /// `c[19]`.
    pub(crate) fn rigid_program() -> Vec<u8> {
        let r = |n| O::r(n);
        program(&[
            Inst::mac(MacOp::Mul, Some(O::v(0)), Some(O::c(170)), None).to_temp(0, 0xf),
            Inst::mac(MacOp::Add, Some(r(0)), None, Some(O::c(171))).to_temp(0, 0xf),
            Inst::mac(MacOp::Dp4, Some(r(0)), Some(O::c(50)), None).to_temp(1, 8),
            Inst::mac(MacOp::Dp4, Some(r(0)), Some(O::c(51)), None).to_temp(1, 4),
            Inst::mac(MacOp::Dp4, Some(r(0)), Some(O::c(52)), None).to_temp(1, 2),
            Inst::mac(MacOp::Mov, Some(O::c(170)), None, None).to_temp(1, 1),
            Inst::mac(MacOp::Dp4, Some(r(1)), Some(O::c(0)), None).to_out(false, OutReg::Pos, 8),
            Inst::mac(MacOp::Dp4, Some(r(1)), Some(O::c(1)), None).to_out(false, OutReg::Pos, 4),
            Inst::mac(MacOp::Dp4, Some(r(1)), Some(O::c(2)), None).to_out(false, OutReg::Pos, 2),
            Inst::mac(MacOp::Dp4, Some(r(1)), Some(O::c(3)), None).to_out(false, OutReg::Pos, 1),
            Inst::mac(
                MacOp::Mad,
                Some(O::v(1)),
                Some(O::c(172).swz("xyxy")),
                Some(O::c(172).swz("zwzw")),
            )
            .to_out(false, OutReg::T(0), 0xf),
            Inst::mac(MacOp::Dp3, Some(O::v(2)), Some(O::c(18)), None).to_temp(2, 0xf),
            Inst::mac(MacOp::Mul, Some(r(2)), Some(O::c(19)), None).to_out(false, OutReg::D0, 0xf),
        ])
    }

    /// Shaped like an `sh_dynamic` skinned program: the node index from
    /// the vertex into `a0.x`, the palette read relative to it
    /// (`c[a0.x + 50..53]`), the lighting from `c[18..21]`.
    fn skinned_program() -> Vec<u8> {
        let r = |n| O::r(n);
        program(&[
            Inst::mac(MacOp::Mul, Some(O::v(0)), Some(O::c(170)), None).to_temp(0, 0xf),
            Inst::mac(MacOp::Add, Some(r(0)), None, Some(O::c(171))).to_temp(0, 0xf),
            Inst::mac(MacOp::Arl, Some(O::v(3)), None, None),
            Inst::mac(MacOp::Dp4, Some(r(0)), Some(O::c_rel(50)), None).to_temp(1, 8),
            Inst::mac(MacOp::Dp4, Some(r(0)), Some(O::c_rel(51)), None).to_temp(1, 4),
            Inst::mac(MacOp::Dp4, Some(r(0)), Some(O::c_rel(52)), None).to_temp(1, 2),
            Inst::mac(MacOp::Mov, Some(O::c(170)), None, None).to_temp(1, 1),
            Inst::mac(MacOp::Dp4, Some(r(1)), Some(O::c(0)), None).to_out(false, OutReg::Pos, 8),
            Inst::mac(MacOp::Dp4, Some(r(1)), Some(O::c(1)), None).to_out(false, OutReg::Pos, 4),
            Inst::mac(MacOp::Dp4, Some(r(1)), Some(O::c(2)), None).to_out(false, OutReg::Pos, 2),
            Inst::mac(MacOp::Dp4, Some(r(1)), Some(O::c(3)), None).to_out(false, OutReg::Pos, 1),
            Inst::mac(MacOp::Mov, Some(O::v(1)), None, None).to_out(false, OutReg::T(0), 0xf),
            Inst::mac(MacOp::Dp3, Some(O::v(2)), Some(O::c(18)), None).to_temp(2, 0xf),
            Inst::mac(MacOp::Mul, Some(r(2)), Some(O::c(19)), None).to_temp(3, 0xf),
            Inst::mac(MacOp::Add, Some(r(3)), None, Some(O::c(20))).to_out(false, OutReg::D0, 0xf),
        ])
    }

    #[test]
    fn an_object_pass_that_does_not_set_its_cull_takes_the_devices() {
        use gfx::state::{CompareFunc, CullMode, RenderStateType as R};
        let table = gfx::xdk::RenderStateTable::for_build(gfx::xdk::HALO2_BUILD);
        let slot = |t: R| {
            (0..=255u8)
                .find(|&s| R::from_guest_index(&table, u32::from(s)) == Some(t))
                .unwrap()
        };
        // The states every opaque pass sets (`pass_state` refuses one
        // without ALPHABLENDENABLE or ZWRITEENABLE), then `extra`.
        let with = |extra: &[(R, u32)]| {
            let mut imp = lit();
            imp.render_states = [
                (R::ColorWriteEnable, 0x0101_0101),
                (R::AlphaBlendEnable, 0),
                (R::ZWriteEnable, 1),
            ]
            .iter()
            .chain(extra)
            .map(|&(t, value)| tags::shader::RenderStateValue {
                state: slot(t),
                value,
            })
            .collect();
            imp
        };
        let fetch = [shaders::vsh::InputFetch::Float; 16];
        // The rebar's shape: depth equal, no writes, CULLMODE unset.
        let equal = with(&[
            (R::ZFunc, CompareFunc::Equal.xbox_value()),
            (R::ZWriteEnable, 0),
        ]);
        assert!(!sets_cull(&equal));
        let p = object_pass(
            map::TagId(0xe000_0001),
            &equal,
            &[],
            &rigid_program(),
            WORLD,
            fetch,
            true,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert!(
            p.program.inherits_cull,
            "CULLMODE unset: the device's at the pass"
        );
        assert_eq!(
            p.program.state.unwrap().primitive.cull_mode,
            None,
            "its own state culls nothing"
        );
        // Drawn as `LitObjects` draws it: the device's cull and winding.
        let device = (Some(wgpu::Face::Back), wgpu::FrontFace::Cw);
        let (primitive, depth) =
            crate::program::pipeline_state(p.program.state.as_ref(), Some(device));
        assert_eq!((primitive.cull_mode, primitive.front_face), device);
        assert_eq!(
            depth,
            (false, wgpu::CompareFunction::Equal),
            "the depth state is its own"
        );
        // One that sets it keeps its own.
        let own = with(&[(R::CullMode, CullMode::Cw.xbox_value())]);
        assert!(sets_cull(&own));
        let p = object_pass(
            map::TagId(0xe000_0001),
            &own,
            &[],
            &rigid_program(),
            WORLD,
            fetch,
            true,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert!(!p.program.inherits_cull);
        assert!(p.program.state.unwrap().primitive.cull_mode.is_some());
    }

    #[test]
    fn lightmap_and_sh_dynamic_shaped_programs_compile_with_their_constants() {
        let imp = lit();
        let look = object_look(map::TagId(0xe000_0001), &imp).unwrap();
        assert_eq!(look.textures, [Stage::Empty; 4]);
        for (name, microcode) in [
            ("lightmap_dynamic", rigid_program()),
            ("sh_dynamic", skinned_program()),
        ] {
            let p = compile_object(&look, &microcode, None, WORLD)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            shaders::validate(&p.wgsl).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert_eq!(
                p.vertex.constants[170..173],
                decompression_rows(&Compression::default())[..],
                "{name}"
            );
            assert_eq!(
                p.vertex.constants[20], [7.0; 4],
                "{name}: the instance constant"
            );
        }
        let p = compile_object(&look, &skinned_program(), None, WORLD).unwrap();
        assert!(
            p.wgsl.contains("a0"),
            "the palette is read relative to a0.x"
        );
    }

    #[test]
    fn an_object_program_reading_what_the_pass_does_not_set_is_refused() {
        let look = object_look(map::TagId(0xe000_0001), &lit()).unwrap();
        let reads = |src: O| {
            program(&[
                Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(src), None).to_out(
                    false,
                    OutReg::Pos,
                    0xf,
                ),
            ])
        };
        for c in [0, 17, 18, 19, 20, 50, 52, 170, 171, 172] {
            assert!(
                compile_object(&look, &reads(O::c(c)), None, WORLD).is_ok(),
                "c[{c}] is set"
            );
        }
        for c in [21, 31, 53, 173] {
            let e = compile_object(&look, &reads(O::c(c)), None, WORLD).unwrap_err();
            assert_eq!(
                e,
                format!("vertex program reads c[{c}], which the object pass does not set")
            );
        }
        let e = compile_object(&look, &reads(O::c_rel(18)), None, WORLD).unwrap_err();
        assert_eq!(
            e,
            "vertex program reads c[a0.x + 18], outside the node rows"
        );
    }

    /// The stencil programs (`stencil_emissive`, `stencil_texture`) write
    /// every stage's coordinate, `oTn = v1 · c[19 + n].xy + c[19 + n].zw`,
    /// whether or not the pass samples the stage, and a pass binding only
    /// the rows of its sampled stages leaves the others unset. A row whose
    /// value reaches only a coordinate the pixel shader ignores is not a
    /// read; one reaching a sampled stage, or the position, is.
    #[test]
    fn an_unset_constant_that_only_feeds_an_unsampled_coordinate_is_not_a_read() {
        let coordinates = |also_pos: bool| {
            let mut v = vec![
                Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(O::c(0)), None).to_out(false, OutReg::Pos, 0xf),
                Inst::mac(MacOp::Mad, Some(O::v(1)), Some(O::c(19)), Some(O::c(19))).to_out(false, OutReg::T(0), 3),
                // Through a temporary, as the skinned classifications do.
                Inst::mac(MacOp::Mad, Some(O::v(1)), Some(O::c(21)), Some(O::c(21))).to_temp(9, 3),
                Inst::mac(MacOp::Mov, Some(O::r(9)), None, None).to_out(false, OutReg::T(1), 3),
                Inst::mac(MacOp::Mad, Some(O::v(1)), Some(O::c(22)), Some(O::c(22))).to_out(false, OutReg::T(2), 3),
                Inst::mac(MacOp::Mad, Some(O::v(1)), Some(O::c(23)), Some(O::c(23))).to_out(false, OutReg::T(3), 3),
            ];
            if also_pos {
                v.push(Inst::mac(MacOp::Mov, Some(O::c(22)), None, None).to_out(false, OutReg::Pos, 4));
            }
            program(&v)
        };
        let unset = |c: usize| format!("vertex program reads c[{c}], which the object pass does not set");
        // No stage fetches (the default definition): every coordinate is dead.
        let look = object_look(map::TagId(0xe000_0001), &lit()).unwrap();
        let p = compile_object(&look, &coordinates(false), None, WORLD).unwrap();
        shaders::validate(&p.wgsl).unwrap();
        // The value also reaching the position is a live read.
        assert_eq!(compile_object(&look, &coordinates(true), None, WORLD).unwrap_err(), unset(22));
        // Stage 1 fetching (2D projective): its coordinate is live, the
        // others stay dead.
        let mut def = shaders::PixelShaderDef::from_bytes(&look.pixel_shader).unwrap();
        def.texture_modes = 0x01 << 5;
        let live = PassLook { pixel_shader: def.to_bytes(), ..look.clone() };
        assert_eq!(compile_object(&live, &coordinates(false), None, WORLD).unwrap_err(), unset(21));
        // Stage 0's coordinate reads c[19], which the pass sets: the
        // constants pass, and the compile goes on to the stage (bound
        // nothing in this look).
        def.texture_modes = 0x01;
        let first = PassLook { pixel_shader: def.to_bytes(), ..look };
        assert_eq!(
            compile_object(&first, &coordinates(false), None, WORLD).unwrap_err(),
            "stage 0 is sampled with nothing bound"
        );
    }

    /// A faded item's variant is the same module but for its pixel
    /// shader, which tests the polygon stipple rows; it validates.
    #[test]
    fn the_stipple_variant_keeps_the_vertex_half_and_tests_the_stipple_rows() {
        let p = frame_pass(3).program;
        let stippled = stipple_wgsl(&p).unwrap();
        shaders::validate(&stippled).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(!p.wgsl.contains("xps.stipple["), "the compiled module has no stipple test");
        assert!(stippled.contains("xps.stipple["), "the variant reads the rows");
        let vertex = p.wgsl.find("fn fs_main").unwrap();
        assert_eq!(stippled[..vertex], p.wgsl[..vertex], "the vertex half is the module's own");
        let edited = MaterialProgram { wgsl: format!("{}// edited", p.wgsl), ..p };
        assert!(stipple_wgsl(&edited).is_err(), "a module not ending with its pixel shader is refused");
    }

    /// A rigid program's module (another vertex half over the material's
    /// pixel shader) takes the same stipple pixel half
    /// ([`stipple_module`]), which a world placement draws faded with.
    #[test]
    fn another_module_over_the_same_pixel_shader_takes_the_stipple_half() {
        let p = frame_pass(3).program;
        let fs = p.wgsl.find("@fragment").unwrap();
        let other = format!("// another vertex half\n{}", &p.wgsl[fs..]);
        let stippled = stipple_module(&other, &p).unwrap();
        assert!(stippled.starts_with("// another vertex half\n"), "its own vertex half is kept");
        assert!(stippled.contains("xps.stipple["));
        assert_eq!(stippled.len() - other.len(), stipple_wgsl(&p).unwrap().len() - p.wgsl.len());
    }

    /// The stencil and transparent programs copy the vertex colour `v10`
    /// into `oD0` whatever the pass reads, and the object streams do not
    /// supply it. A pass whose pixel shader leaves the diffuse colour
    /// unread compiles; one whose combiners read `v0` does not.
    #[test]
    fn an_unsupplied_colour_that_reaches_only_an_unread_diffuse_is_not_a_read() {
        let code = program(&[
            Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(O::c(0)), None).to_out(false, OutReg::Pos, 0xf),
            Inst::mac(MacOp::Mov, Some(O::v(10)), None, None).to_out(false, OutReg::D0, 0xf),
        ]);
        let look = object_look(map::TagId(0xe000_0001), &lit()).unwrap();
        let mut def = shaders::PixelShaderDef::from_bytes(&look.pixel_shader).unwrap();
        (def.final_inputs_abcd, def.final_inputs_efg) = shaders::psh::d3d_default_final_combiner(false, false);
        let unread = PassLook { pixel_shader: def.to_bytes(), ..look.clone() };
        let p = compile_object(&unread, &code, None, WORLD).unwrap();
        shaders::validate(&p.wgsl).unwrap();
        def.combiner_count = 1;
        def.rgb_inputs[0] = 0x4; // v0
        let read = PassLook { pixel_shader: def.to_bytes(), ..look.clone() };
        assert_eq!(
            compile_object(&read, &code, None, WORLD).unwrap_err(),
            "vertex program reads registers we do not supply (0x0400)"
        );
        // Without a final combiner the object draw supplies the default
        // one, which is not read here: refused as before.
        assert_eq!(
            compile_object(&look, &code, None, WORLD).unwrap_err(),
            "vertex program reads registers we do not supply (0x0400)"
        );
    }

    /// A structure row that reaches only an unsampled stage's coordinate is
    /// not one of [`ObjectPass::structure_reads`]: `stencil_emissive`
    /// writes `oT2` from `c[21]` for a pass sampling stage 0 alone, and
    /// the item then needs no upload of the structure's parameter block.
    #[test]
    fn a_structure_row_that_reaches_only_an_unsampled_coordinate_is_not_recorded() {
        let code = |live: bool| {
            let out = if live { OutReg::Pos } else { OutReg::T(2) };
            program(&[
                Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(O::c(0)), None).to_out(false, OutReg::Pos, 0xf),
                Inst::mac(MacOp::Mad, Some(O::v(1)), Some(O::c(21)), Some(O::c(21))).to_out(false, out, 3),
            ])
        };
        let fetch = [shaders::vsh::InputFetch::Float; 16];
        let pass = |microcode: &[u8]| {
            object_pass(map::TagId(0xe000_0001), &lit(), &[], microcode, WORLD, fetch, true)
        };
        assert_eq!(pass(&code(false)).unwrap().structure_reads, 0);
        assert_eq!(pass(&code(true)).unwrap().structure_reads, 1);
    }

    /// An object pass's program may read the rows of `c[21..25]` the pass
    /// does not set: a flagged shader's prologue uploads them from the
    /// structure's parameter block, and the pass records which
    /// (`ObjectPass::structure_reads`) for the draw to refuse without the
    /// upload. `c[26]` stays refused, as does `c[21]` outside an object
    /// pass.
    #[test]
    fn an_object_pass_records_the_structure_rows_its_program_reads() {
        let reads = |src: O| {
            program(&[
                Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(src), None).to_out(
                    false,
                    OutReg::Pos,
                    0xf,
                ),
            ])
        };
        let fetch = [shaders::vsh::InputFetch::Float; 16];
        let pass = |microcode: &[u8]| {
            object_pass(map::TagId(0xe000_0001), &lit(), &[], microcode, WORLD, fetch, true)
        };
        for (c, mask) in [(21, 1u8), (23, 1 << 2), (25, 1 << 4)] {
            let p = pass(&reads(O::c(c))).unwrap_or_else(|e| panic!("c[{c}]: {e}"));
            assert_eq!(p.structure_reads, mask, "c[{c}]");
        }
        assert_eq!(pass(&rigid_program()).unwrap().structure_reads, 0);
        assert_eq!(
            pass(&reads(O::c(26))).unwrap_err(),
            "vertex program reads c[26], which the object pass does not set"
        );
    }

    #[test]
    fn object_templates_can_leave_blend_and_depth_writes_to_the_device() {
        let mut imp = lit();
        imp.render_states.clear();
        assert!(object_pass_state(&imp).is_ok());
        assert!(crate::pass_state::pass_state(&imp.render_states).is_err());
        assert!(crate::object_state::resolve(&imp.render_states, &Default::default(), None).is_err());
    }

    #[test]
    fn a_world_layers_pass_keeps_its_blend_and_cannot_sample_the_lightmap() {
        use gfx::state::{BlendFactor, RenderStateType as R};
        use tags::material::{StageTexture, TextureSource};
        let table = gfx::xdk::RenderStateTable::for_build(gfx::xdk::HALO2_BUILD);
        let state = |t: R, value: u32| tags::shader::RenderStateValue {
            state: (0..=255u8)
                .find(|&s| R::from_guest_index(&table, u32::from(s)) == Some(t))
                .expect("a slot"),
            value,
        };
        let mut imp = lit();
        imp.render_states = vec![
            state(R::AlphaBlendEnable, 1),
            state(R::SrcBlend, BlendFactor::One.xbox_value()),
            state(R::DestBlend, BlendFactor::One.xbox_value()),
            // Its own blend op, so layer 3's state reads its blend.
            state(R::BlendOp, 0x8006),
            state(R::ZWriteEnable, 0),
        ];
        let fetch = [shaders::vsh::InputFetch::Float; 16];
        let pass = map::TagId(0xe000_0001);
        let e = object_pass(pass, &imp, &[], &rigid_program(), WORLD, fetch, true).unwrap_err();
        assert!(e.contains("blends"), "layer 3's refuse it: {e}");
        let p = object_layer_pass(pass, &imp, &[], &rigid_program(), WORLD, fetch).unwrap();
        assert!(
            p.program.state.as_ref().unwrap().blend.is_some(),
            "the pipeline's blend"
        );
        // Outside layer 3 the draw binds no lightmap page.
        imp.textures[3] = Some(StageTexture {
            source: TextureSource::Lightmap,
            size: None,
            stage_size: None,
        });
        imp.pixel_shader = shaders::PixelShaderDef {
            texture_modes: 1 << 15,
            ..Default::default()
        }
        .to_bytes();
        let e = object_layer_pass(pass, &imp, &[], &rigid_program(), WORLD, fetch).unwrap_err();
        assert!(e.contains("only in layer 3"), "{e}");
    }

    #[test]
    fn a_materials_world_layer_passes_are_its_first_level_of_details() {
        use tags::material::{ResolvedLayer, ResolvedPass};
        let layer = |lod, layer, passes: usize| ResolvedLayer {
            lod,
            layer,
            passes: (0..passes)
                .map(|index| ResolvedPass {
                    index,
                    pass: map::TagId(0xe000_0001),
                    implementations: vec![lit()],
                })
                .collect(),
        };
        let material = tags::material::Material {
            shader: map::TagId(0xe000_0010),
            template: map::TagId(0xe000_0011),
            inputs: Default::default(),
            layers: vec![
                layer(0, 3, 1),
                layer(0, 7, 2),
                layer(1, 8, 1),
                layer(0, 20, 0),
            ],
        };
        assert_eq!(object_layer_passes(&material, 7, 0).unwrap().len(), 2);
        assert_eq!(object_layer_passes(&material, 8, 0).unwrap_err(), "no layer 8 at level 0", "only level 1's");
        assert_eq!(object_layer_passes(&material, 8, 1).unwrap().len(), 1, "level 1's pass, for an item at level 1");
        assert_eq!(object_layer_passes(&material, 20, 0).unwrap_err(), "no layer 20 at level 0", "no passes");
        assert_eq!(object_layer_levels(&material, 8), [1]);
        assert_eq!(object_layer_levels(&material, 20), [] as [usize; 0], "no passes");
        assert_eq!(object_layer_passes(&material, 1, 0).unwrap_err(), "no layer 1 at level 0");
    }

    /// An object pass whose constants animate compiles from its clock-0
    /// registers, and each draw samples them at its clock.
    #[test]
    fn an_animated_object_pass_compiles_and_samples_its_constants_per_draw() {
        use gfx::state::RenderStateType as R;
        let table = gfx::xdk::RenderStateTable::for_build(gfx::xdk::HALO2_BUILD);
        let slot = |t: R| {
            (0..=255u8)
                .find(|&s| R::from_guest_index(&table, u32::from(s)) == Some(t))
                .unwrap()
        };
        let (mut target, props) = crate::layer_pass::tests::animated_z();
        target.binding.target = 0x55; // c[23].z
        let mut imp = lit();
        imp.render_states = [(R::ColorWriteEnable, 0x0101_0101), (R::AlphaBlendEnable, 0), (R::ZWriteEnable, 1)]
            .iter()
            .map(|&(t, value)| tags::shader::RenderStateValue { state: slot(t), value })
            .collect();
        let still = object_pass(map::TagId(0xe000_0001), &imp, &[], &rigid_program(), WORLD, [shaders::vsh::InputFetch::Float; 16], true)
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(still.animation.is_none());
        let (vertex, _, _) = animated_blocks(&still, None).unwrap();
        assert_eq!(*vertex, still.program.vertex, "nothing animates: the compiled block, no clock needed");

        imp.animated = vec![target];
        let fetch = [shaders::vsh::InputFetch::Float; 16];
        let e = object_pass(map::TagId(0xe000_0001), &imp, &[], &rigid_program(), WORLD, fetch, true).unwrap_err();
        assert!(e.contains("animated constants: property"), "{e}");
        let p = object_pass(map::TagId(0xe000_0001), &imp, &props, &rigid_program(), WORLD, fetch, true)
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(p.implementation.animated.is_empty() && p.animation.is_some());
        let (vertex, _, images) = animated_blocks(&p, Some(11.0)).unwrap();
        assert_eq!(images, [None; 4], "no animated frame: every stage binds its compiled image");
        assert_eq!(vertex.constants[23][2], 1.5, "11 s over 4 s: 0.75 of 0..2");
        assert_eq!(p.program.vertex.constants[23][2], 0.0, "the compiled block holds clock 0");
        let e = animated_blocks(&p, None).unwrap_err();
        assert!(e.contains("time"), "no clock: the item keeps its D3D draw: {e}");
    }

    /// An object pass whose stage 0 takes an animated frame of slot 5
    /// (`images` images in its bitmap): its frame runs 0..8 over 4 s,
    /// alongside the unread component 8.
    pub(crate) fn frame_pass(images: u32) -> ObjectPass {
        use crate::layer_pass::tests::{animated, identity, timed};
        let mut target = animated(tags::material::AnimationKind::Bitmap, &[(0, 0), (0, 8)]);
        target.binding.target = 5;
        target.stages = vec![tags::material::FrameStage { stage: 0, images }];
        let mut imp = lit();
        imp.animated = vec![target];
        let props = [timed(4.0, identity([0.0, 8.0], 0))];
        object_pass(map::TagId(0xe000_0001), &imp, &props, &rigid_program(), WORLD, [shaders::vsh::InputFetch::Float; 16], true)
            .unwrap_or_else(|e| panic!("{e}"))
    }

    /// Each draw binds the image its clock's frame selects
    /// (`0x0001b0a0`): the frame's remainder over the bitmap's image
    /// count, floored; a one-image bitmap always its image 0; a frame
    /// that binds the default texture keeps the item's D3D draw.
    #[test]
    fn an_animated_frame_selects_its_stages_image_per_draw() {
        let p = frame_pass(3);
        assert_eq!(p.animation.as_ref().unwrap().frame_counts(), [Some(3), None, None, None]);
        for (time, image) in [(0.0, 0), (1.0, 2), (2.0, 1), (3.5, 1)] {
            // 1 s: 2.0; 2 s: 4.0 wraps to 1; 3.5 s: 7.0 wraps to 1.
            let (_, _, images) = animated_blocks(&p, Some(time)).unwrap();
            assert_eq!(images, [Some(image), None, None, None], "{time} s");
        }
        let one = frame_pass(1);
        for time in [0.5, 1.0, 3.9] {
            assert_eq!(animated_blocks(&one, Some(time)).unwrap().2[0], Some(0), "{time} s");
        }
        let none = frame_pass(0);
        let e = animated_blocks(&none, Some(1.0)).unwrap_err();
        assert!(e.contains("default texture"), "{e}");
    }

}
