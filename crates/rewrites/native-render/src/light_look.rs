//! Stage 4: a lit material's layer-12 (diffuse) and layer-13 (specular)
//! pass as the game draws it for one light (docs/graphics/native-lights-constants.md,
//! "Implementations" and "Engine constants").
//!
//! The engine picks the implementation per light (`engine::light_passes`:
//! `diffuse`, `specular`) and resolves the pass with the light's engine
//! inputs, which the light's constants supply ([`LightInputs`],
//! [`engine_inputs`]). [`light_look`] takes a material resolved with those
//! inputs (`tags::material::ShaderLibrary::material`) and returns what the
//! native pass draws, or why the light keeps its D3D passes.

use std::collections::HashMap;

use map::TagId;
use tags::material::{
    EngineInputs, INSTANCE_INTS, INSTANCE_SCALARS, InputReads, Material, Register, TextureSource,
    VectorBinding, VectorTarget, pack_engine_colour,
};
use tags::shader::{PSDEF_SIZE, StageStates};

use shaders::psh::{PixelUniforms, TextureDim};
use shaders::vsh::VertexUniforms;

use crate::engine_states::{EngineStateRefusal, apply_static};
use crate::lights::LightConstants;
use crate::pass_state::{PassState, StateRefusal, pass_state};
use crate::program::{PassParts, translate_pass};

pub const DIFFUSE_LAYER: usize = 12;
pub const SPECULAR_LAYER: usize = 13;

/// The engine inputs a light supplies (Code, `0x0001b2c0`'s sources).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightInputs {
    /// Vector 5 (`0x00033a0b`): the frame's words 24..26, colour A scaled
    /// and clamped (`engine::light_constants::LightGlobals::colour_a_input`).
    pub colour_a: [f32; 3],
    /// Vector 6 (`0x00033a25`): the frame's words 28..30, colour B.
    pub colour_b: [f32; 3],
    /// Integers 0 and 1 (`0x00034067`): the colour scale mode
    /// `[0x00485a2c]`.
    pub mode: i32,
    /// Integer 2 (`0x0003406f`): the definition's word `+0x8a`, signed.
    pub definition_8a: i16,
    /// Integer 3 (`0x0003407f`): the definition's word `+0x80`, signed.
    pub definition_80: i16,
    /// The definition's first dword: integer 7 (`0x000340cc`) is its bit 1
    /// clear.
    pub definition_flags: u32,
    /// Falloff word 0 (`[0x00485a30]`): scalar 16 (`0x000336fb`) is it
    /// when below 1, else 1.
    pub falloff_0: f32,
}

impl LightInputs {
    /// The inputs of a light whose constants the hook computed.
    pub fn from_constants(c: &LightConstants) -> Self {
        let f = |w: [u32; 3]| w.map(f32::from_bits);
        Self {
            colour_a: f(c.colour_a),
            colour_b: f(c.colour_b),
            mode: c.mode as i32,
            definition_8a: c.definition_8a as i16,
            definition_80: c.definition_80 as i16,
            definition_flags: c.definition_flags,
            falloff_0: f32::from_bits(c.falloff_0),
        }
    }

    /// What resolving a material with these inputs depends on but the
    /// colours, as bits: lights with equal keys resolve every material
    /// alike once their colours are put in ([`colour_uniforms`]), so a
    /// light whose colours animate keeps its key.
    pub fn key(&self) -> InputKey {
        InputKey([
            self.mode as u32,
            self.definition_8a as u16 as u32,
            self.definition_80 as u16 as u32,
            self.definition_flags & 2,
            scalar_16(self.falloff_0).to_bits(),
        ])
    }

    /// These inputs with both colours zero: what the cache resolves with
    /// (the colours are put in per light).
    pub fn without_colours(&self) -> Self {
        Self {
            colour_a: [0.0; 3],
            colour_b: [0.0; 3],
            ..*self
        }
    }
}

/// [`LightInputs::key`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct InputKey(pub [u32; 5]);

/// Resolved light passes by (shader, layer, implementation, inputs), so a
/// material is resolved once per distinct light rather than once per
/// light per frame. Bounded: past `capacity` entries it starts again
/// (a bound, not a policy: the keys hold no colours, so few come and go).
pub struct LookCache<V> {
    entries: HashMap<(TagId, usize, usize, InputKey), V>,
    capacity: usize,
}

impl<V: Clone> LookCache<V> {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            capacity: capacity.max(1),
        }
    }

    /// The value for the key, made by `make` when missing.
    pub fn get_or_make(
        &mut self,
        shader: TagId,
        layer: usize,
        implementation: usize,
        key: InputKey,
        make: impl FnOnce() -> V,
    ) -> V {
        let k = (shader, layer, implementation, key);
        if let Some(v) = self.entries.get(&k) {
            return v.clone();
        }
        if self.entries.len() >= self.capacity {
            self.entries.clear();
        }
        let v = make();
        self.entries.insert(k, v.clone());
        v
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The inputs [`engine_inputs`] fills, beyond the instance's own.
pub const LIGHT_SCALARS: u64 = 1 << 16;
pub const LIGHT_VECTORS: u64 = 1 << 5 | 1 << 6;
/// Integer 6 (`0x0003409f`) is not here: it tests a bit of an argument not
/// traced yet.
pub const LIGHT_INTS: u32 = 0b1111 | 1 << 7;

/// The instance's inputs (`EngineInputs::for_shader`) with the light's.
pub fn engine_inputs(instance: &EngineInputs, l: &LightInputs) -> EngineInputs {
    let mut e = instance.clone();
    e.vectors[5] = l.colour_a;
    e.vectors[6] = l.colour_b;
    e.ints[0] = l.mode;
    e.ints[1] = l.mode;
    e.ints[2] = i32::from(l.definition_8a);
    e.ints[3] = i32::from(l.definition_80);
    e.ints[7] = i32::from(l.definition_flags & 2 == 0);
    e.scalars[16] = scalar_16(l.falloff_0);
    e
}

/// Scalar 16 from falloff word 0: `comiss one, falloff; jbe` keeps 1, so
/// NaN and 1 or more give 1.
fn scalar_16(falloff_0: f32) -> f32 {
    if 1.0 > falloff_0 { falloff_0 } else { 1.0 }
}

/// What a light pass's texture stage samples.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LightStage {
    Empty,
    /// An instance bitmap's image.
    Bitmap {
        bitmap: TagId,
        image: u32,
    },
    /// Engine texture 1: the globals' vector normalization cube map (S).
    Normalization,
    /// Engine texture 9: the distance attenuation volume (S).
    Attenuation,
    /// Engine texture 10: the light's gel (or its shader's render target
    /// 13).
    Gel,
    /// Engine texture 13: the frozen D24 shadow map.
    DepthShadow,
    /// Engine texture 8: the filtered colour-mask shadow map.
    ColourMask,
}

/// One implementation of a lit material's pass for one light.
#[derive(Clone, Debug, PartialEq)]
pub struct LightLook {
    pub layer: usize,
    /// The `spas` tag.
    pub pass: TagId,
    pub implementation: usize,
    /// The `vrtx` tag.
    pub vertex_shader: TagId,
    pub textures: [LightStage; 4],
    pub stage_states: Vec<StageStates>,
    pub pixel_shader: [u8; PSDEF_SIZE],
    /// Vertex constants `c[18 + i]`, the engine-driven ones computed from
    /// the inputs the material was resolved with.
    pub vertex_constants: Vec<Register>,
    pub vertex_defaults: Vec<Register>,
    /// Where the colours (vectors 5 and 6) landed.
    pub vector_bindings: Vec<VectorBinding>,
    pub state: PassState,
    pub inherits_cull: bool,
    /// The implementation's flag bit 20
    /// ([`tags::material::ResolvedImplementation::copies_target`]).
    pub copies_target: bool,
}

/// Why a light's pass stays on the D3D path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightRefusal {
    /// The requested level of detail has no draws in this layer.
    NoLayer,
    /// A host light without a pass selector found multiple passes.
    Passes(usize),
    /// No pass at the gathered selector.
    NoPass(u8),
    /// The chosen implementation is not in the pass.
    NoImplementation(usize),
    NoVertexShader,
    /// Something in the pass is animated, and the caller does not sample it.
    Animated,
    /// An animated pass that does not compile from its baseline or sample
    /// ([`crate::layer_pass::LayerRefusal::name`]).
    AnimationSample(&'static str),
    /// It reads engine inputs nobody supplies here.
    Reads(InputReads),
    /// A stage samples another engine texture (13 and 8, the shadow maps,
    /// among them).
    EngineTexture(u8),
    UnsetSlot(u8),
    DefaultTexture,
    Lightmap,
    /// Its render states do not fit the native pass.
    State(StateRefusal),
    /// Its engine-driven render or stage states do not fit
    /// ([`crate::engine_states::apply_static`]).
    EngineStates(EngineStateRefusal),
}

/// Implementation `implementation` of `material`'s pass in `layer` (12 or
/// 13, first level of detail), `material` resolved with the light's
/// [`engine_inputs`].
pub fn light_look(
    material: &Material,
    layer: usize,
    implementation: usize,
) -> Result<LightLook, LightRefusal> {
    selected_light_look(material, layer, implementation, None)
}

/// A gathered entry selects a particular template LOD and pass within the
/// layer. A host light without that binding can only issue a single pass.
/// An animated pass is refused; see [`animated_light_look`].
pub fn selected_light_look(
    material: &Material,
    layer: usize,
    implementation: usize,
    selection: Option<(u8, u8)>,
) -> Result<LightLook, LightRefusal> {
    animated_light_look(material, layer, implementation, selection, None).map(|(look, _)| look)
}

/// [`selected_light_look`] where a pass the instance animates compiles
/// from its baseline (clock 0, its animated targets cleared) and comes
/// with the animation to sample at each draw's clock
/// ([`crate::layer_pass::PassAnimation`]), as a world layer's or an
/// object's pass does. Without the instance's `properties` an animated
/// pass is refused ([`LightRefusal::Animated`]).
pub fn animated_light_look(
    material: &Material,
    layer: usize,
    implementation: usize,
    selection: Option<(u8, u8)>,
    properties: Option<&[tags::shader::AnimatedProperty]>,
) -> Result<(LightLook, Option<crate::layer_pass::PassAnimation>), LightRefusal> {
    let lod = selection.map_or(0, |(variant, _)| usize::from(variant));
    let l = material
        .layers
        .iter()
        .find(|l| l.lod == lod && l.layer == layer)
        .filter(|l| !l.passes.is_empty())
        .ok_or(LightRefusal::NoLayer)?;
    let pass = match selection {
        Some((_, selector)) => l.passes.iter()
            .find(|pass| pass.index == usize::from(selector))
            .ok_or(LightRefusal::NoPass(selector))?,
        None => {
            let [pass] = l.passes.as_slice() else {
                return Err(LightRefusal::Passes(l.passes.len()));
            };
            pass
        }
    };
    let found = pass
        .implementations
        .iter()
        .find(|i| i.index == implementation)
        .ok_or(LightRefusal::NoImplementation(implementation))?;
    let (baseline, animation) = match (found.animated.is_empty(), properties) {
        (true, _) => (None, None),
        (false, None) => return Err(LightRefusal::Animated),
        (false, Some(properties)) => {
            let (base, animation) = crate::layer_pass::PassAnimation::baseline(found, properties, false, pass.index)
                .map_err(|r| LightRefusal::AnimationSample(r.name()))?;
            (Some(base), animation)
        }
    };
    let imp = baseline.as_ref().unwrap_or(found);
    let vertex_shader = imp.vertex_shader.ok_or(LightRefusal::NoVertexShader)?;
    let r = imp.input_reads;
    let unsupplied = InputReads {
        scalars: r.scalars & !(LIGHT_SCALARS | INSTANCE_SCALARS),
        vectors: r.vectors & !LIGHT_VECTORS,
        ints: r.ints & !(LIGHT_INTS | INSTANCE_INTS),
    };
    if unsupplied != InputReads::default() {
        return Err(LightRefusal::Reads(unsupplied));
    }
    // Engine-driven states over the pass's own, from the instance's
    // inputs in `material`'s.
    let applied = apply_static(imp, &material.inputs).map_err(LightRefusal::EngineStates)?;
    let mut textures = [LightStage::Empty; 4];
    for (stage, t) in textures.iter_mut().zip(&imp.textures) {
        *stage = match t.map(|t| t.source) {
            None | Some(TextureSource::None) => LightStage::Empty,
            Some(TextureSource::Bitmap { bitmap, image, .. }) => {
                LightStage::Bitmap { bitmap, image }
            }
            Some(TextureSource::Engine(1)) => LightStage::Normalization,
            Some(TextureSource::Engine(9)) => LightStage::Attenuation,
            Some(TextureSource::Engine(10)) => LightStage::Gel,
            Some(TextureSource::Engine(id)) => return Err(LightRefusal::EngineTexture(id)),
            Some(TextureSource::UnsetSlot(s)) => return Err(LightRefusal::UnsetSlot(s)),
            Some(TextureSource::Default) => return Err(LightRefusal::DefaultTexture),
            Some(TextureSource::Lightmap) => return Err(LightRefusal::Lightmap),
        };
    }
    let state = pass_state(&applied.render_states).map_err(LightRefusal::State)?;
    let look = LightLook {
        layer,
        pass: pass.pass,
        implementation,
        vertex_shader,
        textures,
        stage_states: applied.stage_states,
        pixel_shader: imp.pixel_shader,
        vertex_constants: imp.vertex_constants.clone(),
        vertex_defaults: imp.vertex_defaults.clone(),
        vector_bindings: imp.vector_bindings.clone(),
        inherits_cull: !crate::look::sets_state(
            &imp.render_states,
            gfx::state::RenderStateType::CullMode,
        ),
        copies_target: imp.copies_target,
        state,
    };
    Ok((look, animation))
}

/// The light's frame block and the five words after it, as `0x0003a550`
/// uploads them (`engine::light_constants::LightGlobals::frame_constants`):
/// `c[31]`..`c[39]`, hardware numbering.
pub const FRAME_REGISTER: usize = 31;
pub const FRAME_DWORDS: usize = 36;

/// A light pass translated ([`compile_light`]).
#[derive(Clone, Debug)]
pub struct LightProgram {
    /// The linked module (`vs_main`, `fs_main`).
    pub wgsl: String,
    pub words: Vec<u32>,
    pub inputs: u16,
    pub def: shaders::psh::PixelShaderDef,
    pub pixel_options: shaders::psh::PixelOptions,
    pub stages: [LightStage; 4],
    pub samplers: [Option<StageStates>; 4],
    /// The texture each stage's fetch expects (`None`: not sampled).
    pub dims: [Option<TextureDim>; 4],
    /// Per stage, the channels read as signed (only instance bitmaps,
    /// stored biased).
    pub signed: [[bool; 4]; 4],
    /// The vertex uniforms but for the view's constants and the light's
    /// frame ([`light_vertex`]).
    pub vertex: VertexUniforms,
    pub pixel: PixelUniforms,
    /// Where the colours land in those uniforms ([`colour_uniforms`]).
    pub colours: Vec<VectorBinding>,
    pub state: PassState,
    pub inherits_cull: bool,
    /// The pass's flag bit 20: a flagged shader's draw with it makes the
    /// frame's structure target copy (`0x0003bea0`).
    pub copies_target: bool,
    /// What the instance animates in the pass, sampled at each draw's
    /// clock over this program's uniforms ([`LightDraw::sample`]);
    /// `None` when nothing animates.
    ///
    /// [`LightDraw::sample`]: crate::light_gpu::LightDraw::sample
    pub animation: Option<crate::layer_pass::PassAnimation>,
}

/// Vertex fetch for the world-space light classification.
pub fn vertex_options() -> shaders::vsh::VertexOptions {
    shaders::vsh::VertexOptions {
        provided_inputs: crate::program::INPUT_MASK,
        input_fetch: crate::program::WORLD_FETCH,
        ..Default::default()
    }
}

/// The texture a light stage binds: instance bitmaps and the gel as 2D
/// (S: the lit passes' instance bitmaps are 2D), the normalization map as
/// a cube, the attenuation map as a volume (`project3d` on 03a).
pub fn stage_dim(stage: LightStage) -> Option<TextureDim> {
    match stage {
        LightStage::Empty => None,
        LightStage::Bitmap { .. }
        | LightStage::Gel
        | LightStage::DepthShadow
        | LightStage::ColourMask => Some(TextureDim::D2),
        LightStage::Normalization => Some(TextureDim::Cube),
        LightStage::Attenuation => Some(TextureDim::D3),
    }
}

/// Translates `look` with its vertex program `microcode` (the world-space
/// classification), or says why the light keeps its D3D passes.
pub fn compile_light(look: &LightLook, microcode: &[u8]) -> Result<LightProgram, String> {
    let options = vertex_options();
    compile(
        look,
        microcode,
        options.provided_inputs,
        options.input_fetch,
        false,
    )
}

/// Translates `look` for a model's part (docs/graphics/native-lights.md,
/// "Gathered groups in the scene"): `microcode` the program of the
/// classification the sorted entry names, over a section's declaration
/// supplying `provided` (bit `n`: `v<n>`) fetched as `fetch`
/// (`crate::object_program_gpu::input_fetch`). Beside what
/// [`compile_light`] allows, the program may read the node rows the
/// draw puts in (`c[50..]`, [`crate::light_gpu::model_vertex`]), relative
/// to `a0.x` from there as a skinned program does, and the section's
/// decompression (`c[170..172]`).
pub fn compile_light_model(
    look: &LightLook,
    microcode: &[u8],
    provided: u16,
    fetch: [shaders::vsh::InputFetch; 16],
) -> Result<LightProgram, String> {
    compile(look, microcode, provided, fetch, true)
}

fn compile(
    look: &LightLook,
    microcode: &[u8],
    provided: u16,
    fetch: [shaders::vsh::InputFetch; 16],
    model: bool,
) -> Result<LightProgram, String> {
    let t = translate_pass(&PassParts {
        microcode,
        pixel_shader: &look.pixel_shader,
        dims: look.textures.map(stage_dim),
        biased: look
            .textures
            .map(|s| matches!(s, LightStage::Bitmap { .. })),
        stage_states: &look.stage_states,
        vertex_constants: &look.vertex_constants,
        vertex_defaults: &look.vertex_defaults,
        alpha_test: look.state.alpha_test,
        provided_inputs: provided,
        input_fetch: fetch,
        w_buffer: true,
        shadow: [None; 4],
    })?;
    crate::program::pixel_parity(&t.def, &look.stage_states, t.dims.map(|d| d.is_some()))?;
    // The frame is uploaded before the material binds its constants. Our
    // per-light upload supplies all nine rows together.
    if look.vertex_constants.iter().any(|r| {
        r.written != 0
            && (FRAME_REGISTER..FRAME_REGISTER + FRAME_DWORDS / 4).contains(&usize::from(r.index))
    }) {
        return Err("material writes into the light frame constants".into());
    }
    let vs = shaders::vsh::Program::parse(microcode)
        .map_err(|e| format!("light vertex program: {e:?}"))?;
    let set = crate::program::set_constants(&look.vertex_constants);
    let frame =
        |c: u8| (FRAME_REGISTER..FRAME_REGISTER + FRAME_DWORDS / 4).contains(&usize::from(c));
    if model {
        use crate::program::{MODEL_ROWS, POSITION_DECOMPRESSION};
        for i in &vs.instructions {
            for s in i.sources() {
                let shaders::vsh::SrcReg::Const { index, relative } = s.reg else {
                    continue;
                };
                if relative && !(MODEL_ROWS..POSITION_DECOMPRESSION).contains(&usize::from(index)) {
                    return Err(format!(
                        "light vertex program reads c[a0.x + {index}], outside the node rows"
                    ));
                }
                if !relative && !set(index) && !frame(index) {
                    return Err(format!("light vertex program reads unsupplied c[{index}]"));
                }
            }
        }
    } else if let Some(c) = crate::program::constants_read(&vs)?
        .into_iter()
        .find(|&c| !set(c) && !frame(c))
    {
        return Err(format!("light vertex program reads unsupplied c[{c}]"));
    }
    // The pass's polygon offset is its fragments' (`frame_pixel` keeps it).
    let mut pixel = t.pixel;
    crate::pass_state::DepthOffset::apply(look.state.depth_offset, &mut pixel);
    let words = microcode[4..4 + 16 * vs.instructions.len()]
        .chunks_exact(4)
        .map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
        .collect();
    Ok(LightProgram {
        words,
        def: t.def,
        pixel_options: t.pixel_options,
        inputs: t.inputs,
        inherits_cull: look.inherits_cull,
        copies_target: look.copies_target,
        animation: None,
        wgsl: t.wgsl,
        stages: look.textures,
        samplers: t.samplers,
        dims: t.dims,
        signed: t.signed,
        vertex: t.vertex,
        pixel,
        colours: look.vector_bindings.clone(),
        state: look.state,
    })
}

/// `p`'s uniforms with one light's colours (engine vectors 5 and 6) put
/// where the resolve put them: vertex constants and defaults take the
/// colour, combiner constants its packing in their own lanes, as
/// `0x00019be0` and `0x00019ca0` would. Bindings of other vectors are left
/// (`light_look` refuses passes reading them).
pub fn colour_uniforms(
    p: &LightProgram,
    colour_a: [f32; 3],
    colour_b: [f32; 3],
) -> (VertexUniforms, PixelUniforms) {
    colour_uniforms_over(p, p.vertex.clone(), p.pixel, colour_a, colour_b)
}

/// [`colour_uniforms`] over `vertex` and `pixel` in place of the
/// program's own: its animation sampled at a draw's clock
/// ([`crate::layer_pass::PassAnimation::sample`]), whose combiner
/// constants the colours then go into.
pub fn colour_uniforms_over(
    p: &LightProgram,
    mut vertex: VertexUniforms,
    mut pixel: PixelUniforms,
    colour_a: [f32; 3],
    colour_b: [f32; 3],
) -> (VertexUniforms, PixelUniforms) {
    for b in &p.colours {
        let colour = match b.source {
            5 => colour_a,
            6 => colour_b,
            _ => continue,
        };
        match b.target {
            VectorTarget::Vertex {
                bank,
                reg,
                components,
            } => {
                let r = if bank == 1 {
                    vertex.constants.get_mut(18 + usize::from(reg))
                } else {
                    vertex.defaults.get_mut(usize::from(reg))
                };
                let Some(r) = r else { continue };
                for (k, c) in colour.iter().enumerate() {
                    if components & (1 << k) != 0 {
                        r[k] = *c;
                    }
                }
            }
            VectorTarget::Pixel {
                c1,
                slot,
                mask,
                half,
                ..
            } => {
                let bank = if c1 { &mut pixel.c1 } else { &mut pixel.c0 };
                let Some(c) = bank.get_mut(usize::from(slot)) else {
                    continue;
                };
                let packed = pack_engine_colour([colour[0], colour[1], colour[2], 0.0], half);
                *c = shaders::psh::argb((repack(*c) & !mask) | (packed & mask));
            }
        }
    }
    (vertex, pixel)
}

/// `shaders::psh::argb` undone (its values are multiples of 1/255).
pub(crate) fn repack(c: [f32; 4]) -> u32 {
    let ch = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u32;
    ch(c[3]) << 24 | ch(c[0]) << 16 | ch(c[1]) << 8 | ch(c[2])
}

/// `base` with one light's frame constants (their bits as floats).
pub fn light_vertex(base: &VertexUniforms, frame: &[u32; FRAME_DWORDS]) -> VertexUniforms {
    let mut u = base.clone();
    for (reg, words) in u.constants[FRAME_REGISTER..FRAME_REGISTER + FRAME_DWORDS / 4]
        .iter_mut()
        .zip(frame.chunks_exact(4))
    {
        *reg = std::array::from_fn(|k| f32::from_bits(words[k]));
    }
    u
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use tags::material::{
        Origin, ResolvedImplementation, ResolvedLayer, ResolvedPass, StageTexture,
    };

    fn texture(source: TextureSource) -> Option<StageTexture> {
        Some(StageTexture {
            source,
            size: None,
            stage_size: None,
        })
    }

    /// 03a's `lightaccum_frustum_no_specular_gel_tex` shape: base, the
    /// normalization cube, bump, gel; additive, no depth write.
    pub(crate) fn implementation(index: usize, stage_3: TextureSource) -> ResolvedImplementation {
        use gfx::state::{BlendFactor, RenderStateType as R};
        use tags::shader::RenderStateValue;
        let table = gfx::xdk::RenderStateTable::for_build(gfx::xdk::HALO2_BUILD);
        let slot = |t: R| {
            (0..=255u8)
                .find(|&s| R::from_guest_index(&table, u32::from(s)) == Some(t))
                .unwrap()
        };
        let render_states = vec![
            RenderStateValue {
                state: slot(R::AlphaBlendEnable),
                value: 1,
            },
            RenderStateValue {
                state: slot(R::SrcBlend),
                value: BlendFactor::One.xbox_value(),
            },
            RenderStateValue {
                state: slot(R::DestBlend),
                value: BlendFactor::One.xbox_value(),
            },
            RenderStateValue {
                state: slot(R::ZWriteEnable),
                value: 0,
            },
        ];
        ResolvedImplementation {
            index,
            vertex_shader: Some(TagId(0xe000_0010 + index as u32)),
            textures: [
                texture(TextureSource::Bitmap {
                    bitmap: TagId(0xe000_0001),
                    image: 0,
                    lod_offset: 0.0,
                }),
                texture(TextureSource::Engine(1)),
                texture(TextureSource::Bitmap {
                    bitmap: TagId(0xe000_0002),
                    image: 0,
                    lod_offset: 0.0,
                }),
                texture(stage_3),
            ],
            stage_states: Vec::new(),
            render_states,
            restore_states: Vec::new(),
            pixel_shader: [7; PSDEF_SIZE],
            variant: false,
            c0_written: 0,
            c1_written: 0,
            pixel_constants_from_engine: true,
            per_draw_inputs: true,
            input_reads: InputReads {
                scalars: 0,
                vectors: 1 << 5,
                ints: 1 << 4,
            },
            vector_bindings: Vec::new(),
            scalar_bindings: Vec::new(),
            vertex_constants: vec![Register {
                index: 18,
                value: [1.0, 1.0, 0.0, 0.0],
                written: 0xf,
                origin: Origin::Instance,
            }],
            vertex_defaults: Vec::new(),
            animated: Vec::new(),
            engine_render_states: Vec::new(),
            engine_stage_states: Vec::new(),
            dimension_bindings: Vec::new(),
            copies_target: false,
        }
    }

    pub(crate) fn material(layer: usize, imps: Vec<ResolvedImplementation>) -> Material {
        Material {
            shader: TagId(0xe000_0100),
            template: TagId(0xe000_0101),
            inputs: EngineInputs::default(),
            layers: vec![ResolvedLayer {
                lod: 0,
                layer,
                passes: vec![ResolvedPass {
                    index: 0,
                    pass: TagId(0xe000_0200),
                    implementations: imps,
                }],
            }],
        }
    }

    #[test]
    fn the_light_fills_its_inputs_over_the_instance() {
        let mut instance = EngineInputs::default();
        instance.ints[4] = 1;
        instance.scalars[21] = 0.3;
        let l = LightInputs {
            colour_a: [0.5, 0.25, 1.0],
            colour_b: [0.0, 0.1, 0.2],
            mode: 2,
            definition_8a: -1,
            definition_80: 3,
            definition_flags: 2,
            falloff_0: 0.75,
        };
        let e = engine_inputs(&instance, &l);
        assert_eq!((e.ints[4], e.scalars[21]), (1, 0.3), "the instance's kept");
        assert_eq!((e.vectors[5], e.vectors[6]), (l.colour_a, l.colour_b));
        assert_eq!(&e.ints[..4], &[2, 2, -1, 3]);
        assert_eq!(e.ints[7], 0, "definition bit 1 set");
        assert_eq!(e.scalars[16], 0.75);
        let e = engine_inputs(
            &instance,
            &LightInputs {
                falloff_0: 4.0,
                definition_flags: 0,
                ..l
            },
        );
        assert_eq!((e.scalars[16], e.ints[7]), (1.0, 1));
        assert_eq!(
            engine_inputs(
                &instance,
                &LightInputs {
                    falloff_0: f32::NAN,
                    ..l
                }
            )
            .scalars[16],
            1.0
        );
    }

    #[test]
    fn an_animated_pass_is_refused_unless_sampled_and_then_compiles_from_its_baseline() {
        let (target, props) = crate::layer_pass::tests::animated_z();
        let mut animated = implementation(3, TextureSource::Engine(10));
        animated.animated = vec![target];
        let m = material(DIFFUSE_LAYER, vec![animated]);
        assert_eq!(light_look(&m, DIFFUSE_LAYER, 3).unwrap_err(), LightRefusal::Animated, "nobody samples it");
        let (look, animation) =
            animated_light_look(&m, DIFFUSE_LAYER, 3, None, Some(&props)).expect("compiles from its baseline");
        let animation = animation.expect("the animation to sample");
        let at = animation.sample(Some(11.0)).unwrap();
        assert_eq!(
            at.vertex(&VertexUniforms::default()).constants[18][2],
            1.5,
            "the instance's c[18].z at 11 s over its 4 s period"
        );
        assert_eq!(look.implementation, 3);
        assert_eq!(
            animated_light_look(&m, DIFFUSE_LAYER, 3, None, Some(&[])).unwrap_err(),
            LightRefusal::AnimationSample("property"),
            "the instance lacks the property the animation reads"
        );
        let still = material(DIFFUSE_LAYER, vec![implementation(3, TextureSource::Engine(10))]);
        assert!(animated_light_look(&still, DIFFUSE_LAYER, 3, None, Some(&props)).unwrap().1.is_none());
    }

    #[test]
    fn a_gel_implementation_is_drawn_with_the_engine_stages_named() {
        let m = material(
            DIFFUSE_LAYER,
            vec![
                implementation(2, TextureSource::None),
                implementation(3, TextureSource::Engine(10)),
            ],
        );
        let look = light_look(&m, DIFFUSE_LAYER, 3).expect("fits");
        assert_eq!(look.implementation, 3);
        assert_eq!(look.pass, TagId(0xe000_0200));
        assert_eq!(look.textures[1], LightStage::Normalization);
        assert_eq!(look.textures[3], LightStage::Gel);
        assert!(
            look.state.blend.is_some() && !look.state.depth_write,
            "additive, no depth write"
        );
        assert_eq!(
            light_look(&m, DIFFUSE_LAYER, 2).unwrap().textures[3],
            LightStage::Empty
        );
        assert_eq!(
            light_look(&m, DIFFUSE_LAYER, 4).unwrap_err(),
            LightRefusal::NoImplementation(4)
        );
        assert_eq!(
            light_look(&m, SPECULAR_LAYER, 0).unwrap_err(),
            LightRefusal::NoLayer
        );
    }

    #[test]
    fn a_gathered_light_selects_the_exact_lod_pass_and_implementation() {
        let mut m = material(DIFFUSE_LAYER, vec![implementation(0, TextureSource::Engine(9))]);
        let mut lod = m.layers[0].clone();
        lod.lod = 2;
        let second = ResolvedPass {
            index: 1,
            pass: TagId(75),
            implementations: vec![implementation(1, TextureSource::Engine(10))],
        };
        lod.passes.push(second.clone());
        m.layers[0].passes.push(second);
        m.layers.push(lod);
        m.layers[0].passes[1].pass = TagId(25);

        let select = |lod, pass, imp| selected_light_look(&m, DIFFUSE_LAYER, imp, Some((lod, pass)));
        let look = select(2, 1, 1).unwrap();
        assert_eq!((look.pass, look.textures[3]), (TagId(75), LightStage::Gel));
        assert_eq!(select(0, 1, 1).unwrap().pass, TagId(25));
        assert_eq!(select(1, 1, 1).unwrap_err(), LightRefusal::NoLayer);
        assert_eq!(select(2, 2, 1).unwrap_err(), LightRefusal::NoPass(2));
        assert_eq!(select(2, 1, 0).unwrap_err(), LightRefusal::NoImplementation(0));
        assert_eq!(light_look(&m, DIFFUSE_LAYER, 0).unwrap_err(), LightRefusal::Passes(2));
    }

    #[test]
    fn shadow_maps_and_unsupplied_inputs_keep_the_d3d_pass() {
        let m = material(
            DIFFUSE_LAYER,
            vec![implementation(0, TextureSource::Engine(13))],
        );
        assert_eq!(
            light_look(&m, DIFFUSE_LAYER, 0).unwrap_err(),
            LightRefusal::EngineTexture(13)
        );
        let m = material(
            DIFFUSE_LAYER,
            vec![implementation(1, TextureSource::Engine(9))],
        );
        assert_eq!(
            light_look(&m, DIFFUSE_LAYER, 1).unwrap().textures[3],
            LightStage::Attenuation
        );

        let mut imp = implementation(1, TextureSource::Engine(9));
        imp.input_reads = InputReads {
            scalars: 1 << 16 | 1 << 23,
            vectors: 1 << 6 | 1 << 35,
            ints: 1 << 6 | 1 << 5,
        };
        let e = light_look(&material(DIFFUSE_LAYER, vec![imp]), DIFFUSE_LAYER, 1).unwrap_err();
        assert_eq!(
            e,
            LightRefusal::Reads(InputReads {
                scalars: 1 << 23,
                vectors: 1 << 35,
                ints: 1 << 6
            })
        );
    }

    /// A driven state from the instance's scalar 21 fits; one from the
    /// light's colour (vector 5) changes per light and keeps the D3D pass.
    #[test]
    fn engine_driven_states_from_the_light_keep_the_d3d_pass() {
        use tags::shader::ExternalBinding;
        let mut imp = implementation(1, TextureSource::Engine(9));
        imp.input_reads = InputReads {
            scalars: 1 << 21,
            vectors: 1 << 5,
            ints: 0,
        };
        // LINEWIDTH from scalar 21: inert for triangles.
        imp.engine_render_states = vec![ExternalBinding {
            bytes: [5, 1, 0, 21],
        }];
        assert!(
            light_look(
                &material(DIFFUSE_LAYER, vec![imp.clone()]),
                DIFFUSE_LAYER,
                1
            )
            .is_ok()
        );
        // FOGCOLOR from the light's colour.
        imp.engine_render_states = vec![ExternalBinding {
            bytes: [6, 0, 0, 5],
        }];
        let e = light_look(&material(DIFFUSE_LAYER, vec![imp]), DIFFUSE_LAYER, 1).unwrap_err();
        assert_eq!(
            e,
            LightRefusal::EngineStates(EngineStateRefusal::PerDraw {
                state: 0x8a,
                stage: None
            })
        );
    }

    /// `dp4 oPos, v0, c[0..3]; mov oT0, v1; mov oT1, v5; mov oT3, v1`.
    fn light_program() -> Vec<u8> {
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
            Inst::mac(MacOp::Mov, Some(O::v(5)), None, None).to_out(false, OutReg::T(1), 0xf),
            Inst::mac(MacOp::Mov, Some(O::c(31)), None, None).to_out(false, OutReg::T(3), 0xf),
        ])
    }

    /// Implementation 1 of a layer-12 pass with a pixel shader that fits
    /// its stages (2D, cube, none, volume).
    fn sphere_look() -> LightLook {
        use shaders::psh::bits::{
            TEX_CUBEMAP, TEX_NONE, TEX_PROJECT2D, TEX_PROJECT3D, texture_modes,
        };
        let mut def = shaders::PixelShaderDef {
            texture_modes: texture_modes([TEX_PROJECT2D, TEX_CUBEMAP, TEX_NONE, TEX_PROJECT3D]),
            ..Default::default()
        };
        (def.final_inputs_abcd, def.final_inputs_efg) =
            shaders::psh::d3d_default_final_combiner(false, false);
        let m = material(
            DIFFUSE_LAYER,
            vec![implementation(1, TextureSource::Engine(9))],
        );
        LightLook {
            pixel_shader: def.to_bytes(),
            ..light_look(&m, DIFFUSE_LAYER, 1).unwrap()
        }
    }

    #[test]
    fn a_sphere_light_pass_binds_a_cube_and_a_volume() {
        let look = sphere_look();
        let p = compile_light(&look, &light_program()).expect("compiles");
        shaders::validate(&p.wgsl).expect("valid WGSL");
        assert_eq!(
            p.dims,
            [
                Some(TextureDim::D2),
                Some(TextureDim::Cube),
                None,
                Some(TextureDim::D3)
            ]
        );
        assert!(p.wgsl.contains("texture_cube<f32>") && p.wgsl.contains("texture_3d<f32>"));
        assert_eq!(
            p.vertex.constants[18],
            [1.0, 1.0, 0.0, 0.0],
            "instance constants"
        );
        assert!(p.state.blend.is_some());

        // The gel is 2D: the projective fetch follows the bound texture,
        // as D3D's texture-mode adjust does.
        let mut gel = look.clone();
        gel.textures[3] = LightStage::Gel;
        let p = compile_light(&gel, &light_program()).expect("compiles");
        assert_eq!(p.dims[3], Some(TextureDim::D2));
    }

    /// A skinned model's light program: the position decompressed
    /// (`c[170..171]`), posed by the node `v3` names (`c[a0.x + 50..53]`),
    /// projected (`c[0..3]`); the frame's first row as a coordinate.
    fn skinned_light_program(rows_at: u8) -> Vec<u8> {
        use shaders::vsh::encode::{Inst, Operand as O, program};
        use shaders::vsh::{MacOp, OutReg};
        let r = |n| O::r(n);
        program(&[
            Inst::mac(MacOp::Mul, Some(O::v(0)), Some(O::c(170)), None).to_temp(0, 0xf),
            Inst::mac(MacOp::Add, Some(r(0)), None, Some(O::c(171))).to_temp(0, 0xf),
            Inst::mac(MacOp::Arl, Some(O::v(3)), None, None),
            Inst::mac(MacOp::Dp4, Some(r(0)), Some(O::c_rel(rows_at)), None).to_temp(1, 8),
            Inst::mac(MacOp::Dp4, Some(r(0)), Some(O::c_rel(rows_at + 1)), None).to_temp(1, 4),
            Inst::mac(MacOp::Dp4, Some(r(0)), Some(O::c_rel(rows_at + 2)), None).to_temp(1, 2),
            Inst::mac(MacOp::Mov, Some(O::c(170)), None, None).to_temp(1, 1),
            Inst::mac(MacOp::Dp4, Some(r(1)), Some(O::c(0)), None).to_out(false, OutReg::Pos, 8),
            Inst::mac(MacOp::Dp4, Some(r(1)), Some(O::c(1)), None).to_out(false, OutReg::Pos, 4),
            Inst::mac(MacOp::Dp4, Some(r(1)), Some(O::c(2)), None).to_out(false, OutReg::Pos, 2),
            Inst::mac(MacOp::Dp4, Some(r(1)), Some(O::c(3)), None).to_out(false, OutReg::Pos, 1),
            Inst::mac(MacOp::Mov, Some(O::v(1)), None, None).to_out(false, OutReg::T(0), 0xf),
            Inst::mac(MacOp::Mov, Some(O::c(31)), None, None).to_out(false, OutReg::T(3), 0xf),
        ])
    }

    #[test]
    fn a_model_light_pass_reads_its_node_rows_and_its_declarations_registers() {
        use shaders::vsh::InputFetch;
        let look = sphere_look();
        // The world compile has no node palette to address.
        assert!(
            compile_light(&look, &skinned_light_program(50))
                .unwrap_err()
                .contains("relative")
        );
        let mut fetch = [InputFetch::Float; 16];
        fetch[1] = InputFetch::Xyz1;
        let p = compile_light_model(&look, &skinned_light_program(50), 0b1011, fetch)
            .expect("compiles");
        shaders::validate(&p.wgsl).expect("valid WGSL");
        assert_eq!(
            p.dims,
            [
                Some(TextureDim::D2),
                Some(TextureDim::Cube),
                None,
                Some(TextureDim::D3)
            ]
        );
        // A palette read below the node rows is not one the draw supplies.
        let e = compile_light_model(&look, &skinned_light_program(40), 0b1011, fetch).unwrap_err();
        assert!(e.contains("outside the node rows"), "{e}");
        // The frame rows stay the light's own.
        let mut writes = look.clone();
        writes.vertex_constants.push(Register {
            index: 33,
            value: [0.0; 4],
            written: 0xf,
            origin: Origin::Instance,
        });
        assert!(
            compile_light_model(&writes, &skinned_light_program(50), 0b1011, fetch)
                .unwrap_err()
                .contains("frame constants")
        );
    }

    #[test]
    fn a_light_pass_draws_its_polygon_offset_in_its_fragments() {
        use gfx::state::RenderStateType as R;
        use tags::shader::RenderStateValue;
        let table = gfx::xdk::RenderStateTable::for_build(gfx::xdk::HALO2_BUILD);
        let slot = |t: R| (0..=255u8).find(|&s| R::from_guest_index(&table, u32::from(s)) == Some(t)).unwrap();
        let mut imp = implementation(0, TextureSource::Engine(9));
        imp.render_states.extend([
            RenderStateValue { state: slot(R::SolidOffsetEnable), value: 1 },
            RenderStateValue { state: slot(R::PolygonOffsetZOffset), value: (-2.0f32).to_bits() },
            RenderStateValue { state: slot(R::PolygonOffsetZSlopeScale), value: 0.5f32.to_bits() },
        ]);
        let look = light_look(&material(DIFFUSE_LAYER, vec![imp]), DIFFUSE_LAYER, 0).expect("not refused");
        let offset = crate::pass_state::DepthOffset { units: -2.0, slope: 0.5 };
        assert_eq!(look.state.depth_offset, Some(offset));

        let mut look = sphere_look();
        look.state.depth_offset = Some(offset);
        let p = compile_light(&look, &light_program()).expect("compiles");
        assert_eq!((p.pixel.depth[1], p.pixel.depth[2]), (-2.0, 0.5));
        let (_, pixel) = colour_uniforms(&p, [1.0; 3], [1.0; 3]);
        assert_eq!((pixel.depth[1], pixel.depth[2]), (-2.0, 0.5), "the colours keep it");
        let plain = compile_light(&sphere_look(), &light_program()).unwrap();
        assert_eq!((plain.pixel.depth[1], plain.pixel.depth[2]), (0.0, 0.0));
    }

    #[test]
    fn a_light_cannot_claim_pixel_state_or_frame_lanes_it_does_not_supply() {
        let mut look = sphere_look();
        let mut def = shaders::PixelShaderDef::from_bytes(&look.pixel_shader).unwrap();
        def.final_inputs_abcd = 0;
        def.final_inputs_efg = 0;
        look.pixel_shader = def.to_bytes();
        assert!(
            compile_light(&look, &light_program())
                .unwrap_err()
                .contains("final combiner")
        );

        let mut look = sphere_look();
        look.vertex_constants.push(Register {
            index: FRAME_REGISTER as u8,
            value: [0.0; 4],
            written: 1,
            origin: Origin::Instance,
        });
        assert!(
            compile_light(&look, &light_program())
                .unwrap_err()
                .contains("frame constants")
        );
    }

    #[test]
    fn engine_textures_are_sampled_unsigned_or_the_pass_stays_on_d3d() {
        let stage = |color_sign| tags::shader::StageStates {
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
            color_sign,
            alpha_kill: 0,
            border_color: 0,
            color_key_color: 0,
            unread: [0; 2],
        };
        let mut look = sphere_look();
        look.stage_states = vec![stage(0); 4];
        let p = compile_light(&look, &light_program()).expect("unsigned engine stages compile");
        assert_eq!(p.signed, [[false; 4]; 4], "stored as decoded, no bias");
        // D3DTSIGN_RSIGNED on the normalization cube (stage 1) or the
        // attenuation volume (stage 3): their textures are not stored
        // biased, so the light keeps its D3D passes rather than read them
        // unsigned.
        for n in [1, 3] {
            let mut signed = look.clone();
            signed.stage_states[n] = stage(1 << 29);
            let e = compile_light(&signed, &light_program()).unwrap_err();
            assert!(e.contains("not stored biased"), "stage {n}: {e}");
        }
        // An instance bitmap's signed channels are stored biased.
        let mut bitmap = look.clone();
        bitmap.stage_states[0] = stage(1 << 29);
        assert_eq!(
            compile_light(&bitmap, &light_program())
                .expect("biased")
                .signed[0],
            [true, false, false, false]
        );
    }

    #[test]
    fn lights_differing_only_in_constants_share_a_gpu_pass() {
        use crate::light_gpu::light_gpu_key;
        let look = sphere_look();
        let p = compile_light(&look, &light_program()).expect("compiles");
        let mut brighter = p.clone();
        brighter.vertex.constants[18] = [2.0; 4];
        assert_eq!(
            light_gpu_key(&p),
            light_gpu_key(&brighter),
            "constants are uniforms"
        );
        let mut gel = look.clone();
        gel.textures[3] = LightStage::Gel;
        let q = compile_light(&gel, &light_program()).expect("compiles");
        assert_ne!(
            light_gpu_key(&p),
            light_gpu_key(&q),
            "another stage texture"
        );
    }

    #[test]
    fn the_light_frame_fills_c31_to_c39() {
        let mut frame = [0u32; FRAME_DWORDS];
        for (i, w) in frame.iter_mut().enumerate() {
            *w = (i as f32).to_bits();
        }
        let base = VertexUniforms::default();
        let u = light_vertex(&base, &frame);
        assert_eq!(u.constants[31], [0.0, 1.0, 2.0, 3.0]);
        assert_eq!(u.constants[39], [32.0, 33.0, 34.0, 35.0]);
        assert_eq!(u.constants[30], base.constants[30]);
        assert_eq!(u.constants[40], base.constants[40]);
    }

    #[test]
    fn inputs_come_from_the_hooks_constants_and_key_the_cache() {
        let c = LightConstants {
            frame: [0; 36],
            flags: 1,
            colour_a: [0.5f32.to_bits(), 0.25f32.to_bits(), 1.0f32.to_bits()],
            colour_b: [0; 3],
            mode: 1,
            definition_flags: 0x22,
            definition_8a: 0xffff,
            definition_80: 3,
            falloff_0: 2.0f32.to_bits(),
            gel: None,
            animated_gel: false,
        };
        let l = LightInputs::from_constants(&c);
        assert_eq!(l.colour_a, [0.5, 0.25, 1.0]);
        assert_eq!((l.mode, l.definition_8a, l.definition_80), (1, -1, 3));
        assert!(!c.shadowed());

        let mut cache = LookCache::new(2);
        let shader = TagId(0xe000_0100);
        let mut made = 0;
        let mut get = |cache: &mut LookCache<u32>, l: &LightInputs| {
            cache.get_or_make(shader, DIFFUSE_LAYER, 1, l.key(), || {
                made += 1;
                made
            })
        };
        assert_eq!(get(&mut cache, &l), 1);
        assert_eq!(get(&mut cache, &l), 1, "same inputs: cached");
        // A falloff of 2 and of 3 both read as scalar 1: the same key.
        let far = LightInputs {
            falloff_0: 3.0,
            ..l
        };
        assert_eq!(get(&mut cache, &far), 1);
        // Colours are put in per light: an animated colour keeps the key.
        let other = LightInputs {
            colour_a: [0.1, 0.2, 0.3],
            colour_b: [0.0, 0.0, 0.5],
            ..l
        };
        assert_eq!(get(&mut cache, &other), 1);
        assert_eq!(other.without_colours().colour_a, [0.0; 3]);
        assert_eq!(other.without_colours().key(), other.key());
        let second = LightInputs {
            definition_80: 4,
            ..l
        };
        assert_eq!(get(&mut cache, &second), 2);
        assert_eq!(cache.len(), 2);
        let third = LightInputs { mode: 2, ..l };
        assert_eq!(get(&mut cache, &third), 3);
        assert_eq!(cache.len(), 1, "full: started again");
    }

    #[test]
    fn colours_go_where_the_resolve_put_them() {
        let look = sphere_look();
        let mut p = compile_light(&look, &light_program()).expect("compiles");
        p.vertex.constants[20] = [0.0, 0.0, 0.0, 7.0];
        p.pixel.c0[2] = shaders::psh::argb(0x4000_0011);
        p.colours = vec![
            VectorBinding {
                source: 5,
                target: VectorTarget::Vertex {
                    bank: 1,
                    reg: 2,
                    components: 0b0011,
                },
            },
            VectorBinding {
                source: 6,
                target: VectorTarget::Vertex {
                    bank: 0,
                    reg: 10,
                    components: 0b0111,
                },
            },
            VectorBinding {
                source: 6,
                target: VectorTarget::Pixel {
                    c1: false,
                    slot: 2,
                    mask: 0x00ff_ff00,
                    half: false,
                    transform: 0,
                },
            },
            VectorBinding {
                source: 5,
                target: VectorTarget::Pixel {
                    c1: true,
                    slot: 8,
                    mask: 0x00ff_ffff,
                    half: true,
                    transform: 0,
                },
            },
            VectorBinding {
                source: 7,
                target: VectorTarget::Vertex {
                    bank: 1,
                    reg: 3,
                    components: 0b0111,
                },
            },
        ];
        let (v, px) = colour_uniforms(&p, [0.5, 0.25, 1.0], [1.0, 0.0, 0.5]);
        assert_eq!(v.constants[20], [0.5, 0.25, 0.0, 7.0], "x and y only");
        assert_eq!(v.defaults[10][..3], [1.0, 0.0, 0.5]);
        assert_eq!(
            v.constants[21], p.vertex.constants[21],
            "another vector is left"
        );
        // C0 of stage 2: red and green from colour B, alpha and blue kept.
        assert_eq!(repack(px.c0[2]), 0x40ff_0011);
        // The final C1: colour A at (x + 1) / 2.
        assert_eq!(repack(px.c1[8]) & 0x00ff_ffff, 0x00bf_9fff);
        assert_eq!(px.c0[3], p.pixel.c0[3]);
    }
}
