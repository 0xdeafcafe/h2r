//! An object's layer-3 pass with its own programs, as the world's
//! [`crate::program`] path draws a world pass: the pass's vertex program
//! and pixel shader translated once per material, and per drawn object a
//! vertex constant block holding what the game's upload would hold
//! (docs/graphics/native-renderer.md, "Object programs"):
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

use shaders::psh::PixelUniforms;
use shaders::vsh::{self, VertexUniforms};
use tags::material::{Origin, ResolvedImplementation, VectorTarget, pack_engine_colour};

pub use crate::object_nodes::{
    Compression, IMPLEMENTATIONS, MAX_PALETTE, POSITION_DECOMPRESSION, Row, decompression_rows, node_rows, record_implementation,
};
use crate::look::{PassLook, Refusal, implementation_look};
use crate::object_lighting::{
    Defaults, LIGHTING_SCALARS, LIGHTING_VECTORS, Lighting, ShaderValues, scalar_input, vector_input,
};
use crate::program::{MODEL_ROWS, MaterialProgram, PassParts, TEXCOORD_DECOMPRESSION, VIEW_CONSTANTS, translate_pass};

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
    let r = imp.input_reads;
    let v = r.vectors & !LIGHTING_VECTORS;
    if v != 0 {
        return Err(format!("reads vector input {}, which is not the object's lighting", v.trailing_zeros()));
    }
    let s = r.scalars & !LIGHTING_SCALARS & !SHADER_SCALARS;
    if s != 0 {
        return Err(format!("reads scalar input {}, which is not the object's lighting", s.trailing_zeros()));
    }
    let i = r.ints & !SHADER_INTS;
    if i != 0 {
        return Err(format!("reads integer input {}", i.trailing_zeros()));
    }
    Ok(())
}

/// The one layer-3 pass (first level of detail) of an object material, or
/// why not.
pub fn object_layer3(material: &tags::material::Material) -> Result<&tags::material::ResolvedPass, String> {
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
/// (no engine-driven render or stage states, no blending); where it does
/// not set the cull mode the native pass culls nothing.
pub fn object_pass_state(imp: &ResolvedImplementation) -> Result<crate::pass_state::PassState, String> {
    if !imp.engine_render_states.is_empty() {
        return Err("engine-driven render states".into());
    }
    if !imp.engine_stage_states.is_empty() {
        return Err("engine-driven stage states".into());
    }
    let mut state = crate::pass_state::pass_state(&imp.render_states).map_err(|e| format!("{e:?}"))?;
    if state.blend.is_some() {
        return Err("the pass blends".into());
    }
    let table = gfx::xdk::RenderStateTable::for_build(gfx::xdk::HALO2_BUILD);
    let culls = imp.render_states.iter().any(|s| {
        gfx::state::RenderStateType::from_guest_index(&table, u32::from(s.state)) == Some(gfx::state::RenderStateType::CullMode)
    });
    if !culls {
        state.primitive.cull_mode = None;
    }
    Ok(state)
}

/// One implementation of an object material's layer-3 pass compiled for
/// one vertex declaration: the program (its state set) and the resolved
/// implementation its per-object constants are put in from.
#[derive(Clone, Debug)]
pub struct ObjectPass {
    pub program: MaterialProgram,
    pub implementation: ResolvedImplementation,
    /// The stages that sample the vector normalization cube (empty in
    /// `program.stages`; the host binds the cube).
    pub normalization: [bool; 4],
}

/// Compiles `imp` of `pass` with `microcode` for a vertex buffer
/// supplying `provided` (bit `n`: `v<n>`), or says why not. `lightmap`:
/// whether a stage may sample the lightmap (the host binds what an
/// object's draw binds there, [`OBJECT_LIGHTMAP`]).
pub fn object_pass(
    pass: map::TagId,
    imp: &ResolvedImplementation,
    microcode: &[u8],
    provided: u16,
    lightmap: bool,
) -> Result<ObjectPass, String> {
    let state = object_pass_state(imp)?;
    let (look, normalization) = object_look_stages(pass, imp)?;
    if !lightmap && look.textures.contains(&crate::look::Stage::Lightmap) {
        return Err("a stage samples the lightmap, and the object's (the globals' default) is not uniform or does not load".into());
    }
    let mut program = compile_object_stages(&look, normalization, microcode, state.alpha_test, provided)?;
    program.state = Some(state);
    Ok(ObjectPass { program, implementation: imp.clone(), normalization })
}

/// Why objects' parts with `material` stay on D3D, or `None` when an
/// implementation of its layer-3 passes (first level of detail) reads only
/// what the object path gives: the program path then draws the
/// implementations that compile, and an object whose records draw another
/// stays on D3D. Every opaque material goes this way, whether or not it
/// reads the object's lighting: a stand-in (the models' base map and fixed
/// light, posed from the object's own nodes) matches neither the game's
/// shading nor, for an animated object, the rows its draw uploads. A
/// material without a layer-3 pass is not drawn at layer 3 at all.
pub fn program_refusal(material: &tags::material::Material) -> Option<String> {
    let layers = material.layers.iter().filter(|l| l.lod == 0 && l.layer == crate::look::LIGHTMAP_LAYER);
    let mut reasons = Vec::new();
    for imp in layers.flat_map(|l| &l.passes).flat_map(|p| &p.implementations) {
        match inputs_supported(imp) {
            Ok(()) => return None,
            Err(e) => reasons.push(format!("implementation {}: {e}", imp.index)),
        }
    }
    if reasons.is_empty() {
        return Some("it has no layer-3 pass".into());
    }
    Some(reasons.join("; "))
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

/// [`object_look`] with the stages that sample the vector normalization
/// cube (engine texture 1, bound by the host, sampled as a cube without a
/// bias as the lights do); in the look those stages are empty.
pub fn object_look_stages(pass: map::TagId, imp: &ResolvedImplementation) -> Result<(PassLook, [bool; 4]), String> {
    inputs_supported(imp)?;
    let mut instance = imp.clone();
    for r in instance.vertex_constants.iter_mut().chain(instance.vertex_defaults.iter_mut()) {
        r.origin = Origin::Instance;
    }
    let mut normalization = [false; 4];
    for (n, t) in instance.textures.iter_mut().enumerate() {
        if let Some(t) = t
            && t.source == tags::material::TextureSource::Engine(NORMALIZATION_TEXTURE)
        {
            t.source = tags::material::TextureSource::None;
            normalization[n] = true;
        }
    }
    let mut look = implementation_look(pass, &instance).map_err(|r: Refusal| format!("{r:?}"))?;
    look.per_draw_inputs = false;
    Ok((look, normalization))
}

/// Whether an object pass sets constant `c` (hardware numbering) for
/// instance constants `look`: the view's, the implementation's, the node
/// rows and the decompression.
fn object_sets(look: &PassLook, c: usize) -> bool {
    c < VIEW_CONSTANTS
        || (MODEL_ROWS..MODEL_ROWS + 3).contains(&c)
        || (POSITION_DECOMPRESSION..=TEXCOORD_DECOMPRESSION).contains(&c)
        || look.vertex_constants.iter().any(|r| usize::from(r.index) == c && r.written != 0)
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
    compile_object_stages(look, [false; 4], microcode, alpha_test, provided_inputs)
}

/// [`compile_object`] with the stages that sample the normalization cube
/// ([`object_look_stages`]).
pub fn compile_object_stages(
    look: &PassLook,
    normalization: [bool; 4],
    microcode: &[u8],
    alpha_test: Option<(wgpu::CompareFunction, u8)>,
    provided_inputs: u16,
) -> Result<MaterialProgram, String> {
    let program = vsh::Program::parse(microcode).map_err(|e| format!("vertex program: {e:?}"))?;
    for i in &program.instructions {
        for s in i.sources() {
            if let vsh::SrcReg::Const { index, relative } = s.reg {
                let c = usize::from(index);
                if relative && !(MODEL_ROWS..POSITION_DECOMPRESSION).contains(&c) {
                    return Err(format!("vertex program reads c[a0.x + {c}], outside the node rows"));
                }
                if !relative && !object_sets(look, c) {
                    return Err(format!("vertex program reads c[{c}], which the object pass does not set"));
                }
            }
        }
    }
    let t = translate_pass(&PassParts {
        microcode,
        pixel_shader: &look.pixel_shader,
        dims: std::array::from_fn(|n| {
            if normalization[n] {
                Some(shaders::psh::TextureDim::Cube)
            } else {
                (look.textures[n] != crate::look::Stage::Empty).then_some(shaders::psh::TextureDim::D2)
            }
        }),
        biased: look.textures.map(|s| matches!(s, crate::look::Stage::Bitmap { .. })),
        stage_states: &look.stage_states,
        vertex_constants: &look.vertex_constants,
        vertex_defaults: &look.vertex_defaults,
        alpha_test,
        provided_inputs,
    })?;
    let mut vertex = t.vertex;
    vertex.constants[POSITION_DECOMPRESSION..=TEXCOORD_DECOMPRESSION].copy_from_slice(&decompression_rows(&Compression::default()));
    Ok(MaterialProgram {
        wgsl: t.wgsl,
        stages: look.textures,
        samplers: t.samplers,
        dims: t.dims,
        signed: t.signed,
        vertex,
        pixel: t.pixel,
        inputs: t.inputs,
        state: None,
        rigid: None,
    })
}

/// One drawn object's inputs.
#[derive(Clone, Copy, Debug)]
pub struct ObjectInputs<'a> {
    pub lighting: Option<&'a Lighting>,
    pub defaults: &'a Defaults,
    pub shader: ShaderValues,
}

/// The vertex block an object's draw of `imp` reads: `base` (the
/// compiled pass's, [`compile_object`]) with the engine constants that
/// read the lighting evaluated for this object, its node rows at `c[50]`
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
        let VectorTarget::Vertex { bank, reg, components } = b.target else { continue };
        if b.source >= 64 || LIGHTING_VECTORS & (1u64 << b.source) == 0 {
            continue;
        }
        let v = vector_input(u32::from(b.source), inputs.lighting, inputs.defaults, inputs.shader)
            .ok_or_else(|| format!("lighting vector {} is not given for this object", b.source))?;
        put(bank, reg, components & 0b0111, v, false);
    }
    for b in &imp.scalar_bindings {
        let VectorTarget::Vertex { bank, reg, components } = b.target else { continue };
        if b.source >= 64 || LIGHTING_SCALARS & (1u64 << b.source) == 0 {
            continue;
        }
        let s = scalar_input(u32::from(b.source), inputs.lighting)
            .ok_or_else(|| format!("lighting scalar {} is not given for this object", b.source))?;
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
/// a lighting input packed for this object as `0x00019ca0` packs it
/// (`tags::material::pack_engine_colour`: a vector with alpha 0, a scalar
/// through its binding's transform, `tags::material::engine_scalar`, in
/// its one channel; halved first when the binding says so) into the byte
/// lanes the binding keeps. An error names an input the port cannot give
/// for this object.
pub fn object_pixel_uniforms(base: &PixelUniforms, imp: &ResolvedImplementation, inputs: &ObjectInputs<'_>) -> Result<PixelUniforms, String> {
    let mut u = base.clone();
    for b in &imp.vector_bindings {
        let VectorTarget::Pixel { c1, slot, mask, half, .. } = b.target else { continue };
        if b.source >= 64 || LIGHTING_VECTORS & (1u64 << b.source) == 0 {
            continue;
        }
        let v = vector_input(u32::from(b.source), inputs.lighting, inputs.defaults, inputs.shader)
            .ok_or_else(|| format!("lighting vector {} is not given for this object", b.source))?;
        let packed = pack_engine_colour([f32::from_bits(v[0]), f32::from_bits(v[1]), f32::from_bits(v[2]), 0.0], half);
        put_packed(&mut u, c1, slot, mask, packed);
    }
    for b in &imp.scalar_bindings {
        let VectorTarget::Pixel { c1, slot, mask, half, transform } = b.target else { continue };
        if b.source >= 64 || LIGHTING_SCALARS & (1u64 << b.source) == 0 {
            continue;
        }
        let s = scalar_input(u32::from(b.source), inputs.lighting)
            .ok_or_else(|| format!("lighting scalar {} is not given for this object", b.source))?;
        // The scalar lands in the one channel of its mode; the binding's
        // mask is that channel.
        let x = tags::material::engine_scalar(f32::from_bits(s), usize::from(transform));
        put_packed(&mut u, c1, slot, mask, pack_engine_colour([x; 4], half));
    }
    Ok(u)
}

/// `packed`'s lanes in `mask` into combiner constant `slot` of C1 or C0.
fn put_packed(u: &mut PixelUniforms, c1: bool, slot: u8, mask: u32, packed: u32) {
    let bank = if c1 { &mut u.c1 } else { &mut u.c0 };
    if let Some(c) = bank.get_mut(usize::from(slot)) {
        *c = shaders::psh::argb((crate::light_look::repack(*c) & !mask) | (packed & mask));
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::look::Stage;
    use crate::object_nodes::tests::rows_of;
    use shaders::vsh::encode::{Inst, Operand as O, program};
    use shaders::vsh::{MacOp, OutReg};
    use tags::material::{InputReads, Register, VectorBinding};

    /// The registers `v0..v6` from the vertex buffer.
    const WORLD: u16 = crate::program::INPUT_MASK;

    fn implementation(reads: InputReads, vectors: Vec<VectorBinding>, scalars: Vec<VectorBinding>) -> ResolvedImplementation {
        let reg = |index, origin| Register { index, value: [7.0; 4], written: 0xf, origin };
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
            vertex_constants: vec![reg(18, Origin::Engine), reg(19, Origin::Engine), reg(20, Origin::Instance)],
            vertex_defaults: Vec::new(),
            animated: Vec::new(),
            engine_render_states: Vec::new(),
            engine_stage_states: Vec::new(),
        }
    }

    fn vertex(reg: u8, components: u8) -> VectorTarget {
        VectorTarget::Vertex { bank: 1, reg, components }
    }

    /// A lit object implementation: vector 15 into `c[18].xyz`, vector 40
    /// into `c[19].xyz`, scalar 23 into `c[19].w`.
    pub(crate) fn lit() -> ResolvedImplementation {
        implementation(
            InputReads { vectors: 1 << 15 | 1 << 40, scalars: 1 << 23 | 1 << 21, ints: 1 << 4 },
            vec![
                VectorBinding { source: 15, target: vertex(0, 0b0111) },
                VectorBinding { source: 40, target: vertex(1, 0b0111) },
            ],
            vec![VectorBinding { source: 23, target: vertex(1, 0b1000) }],
        )
    }

    #[test]
    fn every_opaque_material_goes_through_its_programs_lit_or_not() {
        use tags::material::{Material, ResolvedLayer, ResolvedPass};
        let of = |layer: u8, implementations: Vec<ResolvedImplementation>| Material {
            shader: map::TagId(0xe000_0010),
            template: map::TagId(0xe000_0011),
            inputs: Default::default(),
            layers: vec![ResolvedLayer {
                lod: 0,
                layer,
                passes: vec![ResolvedPass { index: 0, pass: map::TagId(0xe000_0001), implementations }],
            }],
        };
        let layer3 = crate::look::LIGHTMAP_LAYER;
        // Reading none of the object's lighting: the programs still draw
        // it (no base-map stand-in).
        let unlit = implementation(InputReads::default(), Vec::new(), Vec::new());
        assert_eq!(program_refusal(&of(layer3, vec![unlit.clone()])), None);
        assert_eq!(program_refusal(&of(layer3, vec![lit()])), None);
        let mut time = unlit.clone();
        time.input_reads.scalars |= 1 << 2;
        assert!(program_refusal(&of(layer3, vec![time.clone()])).unwrap().contains("implementation 0: reads scalar input 2"));
        assert_eq!(program_refusal(&of(layer3, vec![time, unlit.clone()])), None, "one implementation is enough");
        assert_eq!(program_refusal(&of(layer3 + 1, vec![unlit])).as_deref(), Some("it has no layer-3 pass"));
    }

    #[test]
    fn inputs_other_than_the_lighting_and_the_shaders_are_refused() {
        assert_eq!(inputs_supported(&lit()), Ok(()));
        let mut time = lit();
        time.input_reads.scalars |= 1 << 2;
        assert_eq!(inputs_supported(&time).unwrap_err(), "reads scalar input 2, which is not the object's lighting");
        let mut camera = lit();
        camera.input_reads.vectors |= 1 << 5;
        assert!(inputs_supported(&camera).unwrap_err().starts_with("reads vector input 5"));
        let mut pixel = lit();
        pixel.vector_bindings.push(VectorBinding {
            source: 15,
            target: VectorTarget::Pixel { c1: false, slot: 0, mask: 0x00ff_ffff, half: false, transform: 0 },
        });
        pixel.scalar_bindings.push(VectorBinding {
            source: 23,
            target: VectorTarget::Pixel { c1: false, slot: 1, mask: 0xff00_0000, half: false, transform: 1 },
        });
        assert_eq!(inputs_supported(&pixel), Ok(()), "lighting inputs in combiner constants are packed per object");
    }

    #[test]
    fn an_objects_block_holds_what_the_games_upload_would() {
        // The lighting: +0x30 (vector 15) = (0.5, 0.25, 0.125); +0x10 = 0;
        // its object's root value 0.75.
        let mut words = [0u32; 30];
        for (i, v) in [0.5f32, 0.25, 0.125].iter().enumerate() {
            words[12 + i] = v.to_bits();
        }
        let l = Lighting { words, root: Some(0.75f32.to_bits()) };
        let d = Defaults::default();
        let inputs = ObjectInputs { lighting: Some(&l), defaults: &d, shader: ShaderValues::default() };
        let imp = lit();
        let look = object_look(map::TagId(0xe000_0001), &imp).unwrap();
        let p = compile_object(&look, &rigid_program(), None, WORLD).unwrap();
        let nodes = rows_of(3);
        let decompression = decompression_rows(&Compression { position: Some([0.0, 2.0, 0.0, 2.0, 0.0, 2.0]), texcoord: None });
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
        let e = object_vertex_uniforms(&p.vertex, &imp, &ObjectInputs { lighting: Some(&stale), ..inputs }, &nodes, decompression);
        assert_eq!(e.unwrap_err(), "lighting vector 40 is not given for this object");
    }

    #[test]
    fn an_objects_pixel_block_packs_its_lighting_as_the_games_upload_does() {
        // Vector 15 (+0x30) = (0.5, 0.25, 0.125).
        let mut words = [0u32; 30];
        for (i, v) in [0.5f32, 0.25, 0.125].iter().enumerate() {
            words[12 + i] = v.to_bits();
        }
        let l = Lighting { words, root: Some(0) };
        let d = Defaults::default();
        let inputs = ObjectInputs { lighting: Some(&l), defaults: &d, shader: ShaderValues::default() };
        let mut imp = lit();
        imp.vector_bindings = vec![
            // c0[0] RGB, its alpha the instance's.
            VectorBinding { source: 15, target: VectorTarget::Pixel { c1: false, slot: 0, mask: 0x00ff_ffff, half: false, transform: 0 } },
            // c1[1] green only, halved: (x + 1) / 2.
            VectorBinding { source: 15, target: VectorTarget::Pixel { c1: true, slot: 1, mask: 0x0000_ff00, half: true, transform: 0 } },
        ];
        imp.scalar_bindings = vec![
            // Scalar 23 (the luminance of vector 15) complemented into
            // c0[2]'s alpha, and through the cosine wave into c1[3]'s blue.
            VectorBinding { source: 23, target: VectorTarget::Pixel { c1: false, slot: 2, mask: 0xff00_0000, half: false, transform: 1 } },
            VectorBinding { source: 23, target: VectorTarget::Pixel { c1: true, slot: 3, mask: 0x0000_00ff, half: false, transform: 2 } },
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
        assert_eq!(scalar_input(23, Some(&l)), Some(luma.to_bits()), "the scalar the test assumes");
        assert_eq!(u.c0[2], shaders::psh::argb(0xb055_6677));
        assert_eq!(u.c1[3], shaders::psh::argb(0x9988_7750));
        assert_eq!(u.c0[1], base.c0[1], "the rest as compiled");
        assert_eq!(u.c0[3..], base.c0[3..]);
        let none = ObjectInputs { lighting: None, ..inputs };
        assert!(object_pixel_uniforms(&base, &imp, &none).is_ok(), "vector 15 falls back on the default without a lighting");
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
            Inst::mac(MacOp::Mad, Some(O::v(1)), Some(O::c(172).swz("xyxy")), Some(O::c(172).swz("zwzw")))
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
    fn lightmap_and_sh_dynamic_shaped_programs_compile_with_their_constants() {
        let imp = lit();
        let look = object_look(map::TagId(0xe000_0001), &imp).unwrap();
        assert_eq!(look.textures, [Stage::Empty; 4]);
        for (name, microcode) in [("lightmap_dynamic", rigid_program()), ("sh_dynamic", skinned_program())] {
            let p = compile_object(&look, &microcode, None, WORLD).unwrap_or_else(|e| panic!("{name}: {e}"));
            shaders::validate(&p.wgsl).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert_eq!(p.vertex.constants[170..173], decompression_rows(&Compression::default())[..], "{name}");
            assert_eq!(p.vertex.constants[20], [7.0; 4], "{name}: the instance constant");
        }
        let p = compile_object(&look, &skinned_program(), None, WORLD).unwrap();
        assert!(p.wgsl.contains("a0"), "the palette is read relative to a0.x");
    }

    #[test]
    fn an_object_program_reading_what_the_pass_does_not_set_is_refused() {
        let look = object_look(map::TagId(0xe000_0001), &lit()).unwrap();
        let reads = |src: O| {
            program(&[Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(src), None).to_out(false, OutReg::Pos, 0xf)])
        };
        for c in [0, 17, 18, 19, 20, 50, 52, 170, 171, 172] {
            assert!(compile_object(&look, &reads(O::c(c)), None, WORLD).is_ok(), "c[{c}] is set");
        }
        for c in [21, 31, 53, 173] {
            let e = compile_object(&look, &reads(O::c(c)), None, WORLD).unwrap_err();
            assert_eq!(e, format!("vertex program reads c[{c}], which the object pass does not set"));
        }
        let e = compile_object(&look, &reads(O::c_rel(18)), None, WORLD).unwrap_err();
        assert_eq!(e, "vertex program reads c[a0.x + 18], outside the node rows");
    }
}
