//! The particle material in Rust: what the particle draw `0x002243c0`
//! leaves on the device before its walk's callback (`0x00224550`) emits,
//! as data. Its environment setup (`0x0002c3b0` and `0x0001bd50` on
//! `0x0055edd8`) writes no device state; `0x0023ea80(view, shader;
//! class, layer, pass)` does:
//!
//! 1. `STIPPLEENABLE` (`0x53`) off (`0x0023eb20`);
//! 2. the view's 3x4 matrix at `c78..c80` (`0x003f7330`, ECX `0xae`),
//!    rows from `[0x004687d0]` `+4/+0x10/+0x1c/+0x28`, `+8...`, `+0xc...`;
//! 3. the shader begin `0x0001bbf0(shader; lod 0, layer, pass,
//!    implementation 0, size 10000)`, the shader a `shad`, `prt3` or
//!    `PRTM` (`ShaderLibrary::drawn_shader`): with [`RESET`] set the
//!    device reset first ([`RESET_STATES`]), then `0x00016b90`: the
//!    implementation's render and stage states, its combiner and its
//!    bound bitmaps, its animation sampled (`tags::material`,
//!    docs/formats/shader-instances.md);
//! 4. program `class + 7` of implementation 0's `vrtx` (`+0x100`,
//!    `0x0001c590` with BL 1);
//! 5. the declaration of one stream of type [`STREAM_TYPE`], or
//!    [`STREAM_TYPE_OVERRIDE`] with both [`VERTEX_OVERRIDE`] bytes set
//!    (`0x0001ccb0(type)`, inline at `0x0023ec7f` for the first):
//!    `0x0001c6b0` clears the stream cache `0x0051f0f0`, stream 0's format
//!    is the type's record in `0x0043f408`, and `0x0001c710` builds the
//!    declaration from it and the program's inputs (`0x0001c330`,
//!    [`vertex_formats`]).
//!
//! [`Inputs`] are read at the draw's entry; [`material`] (with the map's
//! shader library) builds the [`Material`] or names the branch it does
//! not model.

#[cfg(feature = "gpu")]
use crate::effect_material::{MATRIX_SLOT, Texture, WORDS};
pub use crate::effect_material::Material;
use crate::effect_material::TAG_TABLE;

/// The particle draw, where the game reads [`Inputs`].
pub const DRAW: u32 = 0x0022_43c0;
/// The view whose matrix the setup writes (a pointer).
pub const VIEW: u32 = 0x0046_87d0;
/// `0x0001bbf0` resets the device first while this byte is set
/// ([`RESET_STATES`]).
pub const RESET: u32 = 0x0046_70bc;
/// The render states `0x0001bbf0`'s reset sets, by guest slot, in its
/// order: `LogicOp` 0 (`0x003f5b40`), `ZEnable` 2 (`0x003f54f0`), the
/// depth function `LESSEQUAL` (method `0x40354`), `ZBias` 0
/// (`0x003f59b0`), alpha test off (method `0x40300`), `SampleAlpha` 0
/// (`0x003f5b20`), alpha reference 0 (method `0x40340`), `CullMode` and
/// `FrontFace` `CW` (`0x003f53d0`, `0x003f5450` with `0x900`).
pub const RESET_STATES: [(u8, u32); 9] =
    [(150, 0), (143, 2), (57, 0x203), (149, 0), (60, 0), (158, 0), (61, 0), (147, 0x900), (146, 0x900)];
/// The stage word the reset zeroes in each stage (`0x00404df8 + 0x80 n`).
pub const RESET_STAGE_WORD: u8 = 10;
/// With both bytes set the setup declares stream type
/// [`STREAM_TYPE_OVERRIDE`] instead of [`STREAM_TYPE`] (`0x0023ec57`).
/// `0x00470a38` is also the emitter's gate (`crate::particles`).
pub const VERTEX_OVERRIDE: [u32; 2] = [0x0047_0a38, 0x0047_0a39];
/// The particle stream: position, axis, size and rotation, colour and
/// the frame windows as floats and a `D3DCOLOR` (`0x0043f77a`).
pub const STREAM_TYPE: u8 = 0x2a;
/// The emitter's stream: semantic 0 as four shorts (`0x0043f78f`), the
/// corners `0x0023ed30` pushes inline.
pub const STREAM_TYPE_OVERRIDE: u8 = 0x2b;
/// The program index is the system's class plus this.
pub const PROGRAM_BASE: u32 = 7;
/// `STIPPLEENABLE`, which the setup turns off.
pub const STIPPLE_ENABLE: u8 = 0x53;
/// The hardware slot of the implementation's vertex constant 0
/// (`lea ecx, [edi + 0x12]` at `0x00018eab`), which
/// `tags::material::Register::index` already holds (`c[18 + i]`).
pub const VERTEX_CONSTANT_BASE: usize = 0x12;

const SHAD: u32 = u32::from_be_bytes(*b"shad");
const PRT3: u32 = u32::from_be_bytes(*b"prt3");
const PRTM: u32 = u32::from_be_bytes(*b"PRTM");
/// The groups the shader begin draws: a `shad`, a particle's embedded
/// shader, a model particle's `shad`.
const DRAWN: [u32; 3] = [SHAD, PRT3, PRTM];

/// What the setup reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Inputs {
    /// The shader datum (third argument).
    pub shader: u32,
    /// Its tag group as the instance table holds it.
    pub group: u32,
    pub layer: u32,
    pub pass: u32,
    /// The system's `+4` (`movzx`).
    pub class: u16,
    /// [`RESET`] set at the draw's entry.
    pub reset: bool,
    /// Both [`VERTEX_OVERRIDE`] bytes set.
    pub vertex_override: bool,
    /// `c78..c80`.
    pub matrix: [[f32; 4]; 3],
}

/// Reads the inputs at `0x002243c0`'s entry (`esp` the return address's
/// slot, `eax` the system).
pub fn read(word: &dyn Fn(u32) -> Option<u32>, esp: u32, eax: u32) -> Option<Inputs> {
    let arg = |n: u32| word(esp.checked_add(4 * n)?);
    let shader = arg(3)?;
    let index = shader as u16 as i16 as i32;
    let group = word(word(TAG_TABLE)?.wrapping_add((index * 16) as u32))?;
    let byte = |a: u32| word(a).map(|w| w & 0xff != 0);
    let view = word(VIEW)?;
    let f = |o: u32| word(view.checked_add(o)?).map(f32::from_bits);
    let row = |o: u32| Some([f(o)?, f(o + 0xc)?, f(o + 0x18)?, f(o + 0x24)?]);
    Some(Inputs {
        shader,
        group,
        layer: arg(4)?,
        pass: arg(5)?,
        class: word(eax.checked_add(4)?)? as u16,
        reset: byte(RESET)?,
        vertex_override: byte(VERTEX_OVERRIDE[0])? && byte(VERTEX_OVERRIDE[1])?,
        matrix: [row(4)?, row(8)?, row(0xc)?],
    })
}

/// A branch the port does not model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// A tag that is neither a `shad` nor a particle (`prt3`, `PRTM`).
    NotShader,
    /// The shader, its template or pass does not resolve or has no
    /// such layer or pass.
    Tags,
    /// An animated parameter that does not sample at the draw: the
    /// [`crate::layer_pass::LayerRefusal`] it gives (an object function's
    /// input, which the particle provider `0x0055edd8` supplies, a ranged
    /// curve's second input, an animated render or stage state, a curve
    /// that does not parse).
    Animation(&'static str),
    /// The implementation copies the frame first (`0x0003bea0`).
    TargetCopy,
    /// A combiner variant chosen by inputs the draw supplies.
    EngineVariant,
    /// An engine-driven render or stage state.
    EngineState,
    /// A stage bound from the lightmap or an engine texture that is not
    /// one of the rasterizer data's bitmaps or the cleared stage.
    EngineTexture,
    /// Implementation 0 has no vertex shader.
    NoVertexShader,
    /// A vertex constant or default `0x00018eab`/`0x00018e3b` writes
    /// whole from lanes the resolver did not all write: the rest are the
    /// setup's buffer's.
    VertexConstantLanes,
    /// A vertex constant from the engine's inputs per draw, or from the
    /// size of a stage whose size the resolver does not know (an engine
    /// texture's, `0x00019be0`): its value is the draw's, not the tag's.
    VertexConstantDriven,
}

impl Refused {
    pub fn name(self) -> &'static str {
        match self {
            Self::NotShader => "not_shader",
            Self::Tags => "tags",
            Self::Animation(cause) => cause,
            Self::TargetCopy => "target_copy",
            Self::EngineVariant => "engine_variant",
            Self::EngineState => "engine_state",
            Self::EngineTexture => "engine_texture",
            Self::NoVertexShader => "no_vertex_shader",
            Self::VertexConstantLanes => "vertex_constant_lanes",
            Self::VertexConstantDriven => "vertex_constant_driven",
        }
    }
}

/// Whether the inputs reach `0x00016b90` as modelled: a `shad` or a
/// particle tag ([`tags::material::ShaderLibrary::drawn_shader`]).
pub fn check(i: &Inputs) -> Result<(), Refused> {
    if !DRAWN.contains(&i.group) {
        return Err(Refused::NotShader);
    }
    Ok(())
}

/// The stream type the setup declares for `i`.
pub fn stream_type(i: &Inputs) -> u8 {
    if i.vertex_override { STREAM_TYPE_OVERRIDE } else { STREAM_TYPE }
}

/// The vertex array formats the declaration of one stream of `elements`
/// gives a program whose register `n` reads `semantics[n]`
/// (`0x0001c330`): each register's attribute type with the stream's
/// vertex size as its stride, every other register off (`0x02`).
pub fn vertex_formats(semantics: &[u16], elements: &[crate::object_streams::StreamElement]) -> [u32; 16] {
    use crate::object_streams::{StreamElement, declaration};
    let stride: usize = elements
        .iter()
        .map(|e| match *e {
            StreamElement::Skip(n) => usize::from(n),
            StreamElement::Attribute { format, .. } => format.size(),
        })
        .sum();
    let mut formats = [0x02; 16];
    for e in declaration(semantics, &[elements.to_vec()]) {
        formats[usize::from(e.register)] = e.format.xbox_type() | (stride as u32) << 8;
    }
    formats
}

/// The combiner word a pixel constant lands in: `PSCONSTANT0_n` 10 + n,
/// `PSCONSTANT1_n` 18 + n, the final combiner's 43 and 44.
#[cfg(feature = "gpu")]
fn constant_word(c1: bool, slot: u8) -> usize {
    match slot {
        8 => 43 + usize::from(c1),
        n => 10 + 8 * usize::from(c1) + usize::from(n),
    }
}

/// A stage's states as `0x000189a0` writes them into `0x00404dd0`.
#[cfg(feature = "gpu")]
fn stage_words(s: &tags::shader::StageStates) -> [(u8, u32); 14] {
    [
        (0, s.address_u.into()),
        (1, s.address_v.into()),
        (2, s.address_w.into()),
        (3, s.mag_filter.into()),
        (4, s.min_filter.into()),
        (5, s.mip_filter.into()),
        (6, s.mip_lod_bias),
        (7, s.max_mip_level.into()),
        (8, s.max_anisotropy.into()),
        (9, s.color_key_op.into()),
        (10, s.color_sign),
        (11, s.alpha_kill.into()),
        (0x1d, s.border_color),
        (0x1e, s.color_key_color),
    ]
}

/// The shader a particle draw's inputs select, resolved from the map:
/// implementation 0 of the pass, the animated properties of the
/// postprocess it is drawn from and the inputs it is resolved under.
/// [`Particle::material`] makes each draw's material from it.
#[cfg(feature = "gpu")]
#[derive(Clone, Debug, PartialEq)]
pub struct Particle {
    pub implementation: tags::material::ResolvedImplementation,
    pub properties: Vec<tags::shader::AnimatedProperty>,
    pub inputs: tags::material::EngineInputs,
    pub pass: usize,
}

/// [`Particle`] for `i` ([`check`] first).
#[cfg(feature = "gpu")]
pub fn resolve(lib: &mut tags::material::ShaderLibrary, i: &Inputs) -> Result<Particle, Refused> {
    use tags::material::PassSelection;
    check(i)?;
    let drawn = lib.drawn_shader(map::TagId(i.shader)).map_err(|_| Refused::Tags)?;
    let selection = PassSelection { lod: 0, layer: i.layer as usize, pass: i.pass as usize, implementation: 0 };
    let selected = lib.selected_drawn(&drawn, selection).map_err(|_| Refused::Tags)?;
    Ok(Particle {
        implementation: selected.implementation,
        properties: drawn.postprocess.properties,
        inputs: drawn.inputs,
        pass: i.pass as usize,
    })
}

/// The material `0x0023ea80` sets for `i`, the shader resolved by `lib`,
/// its animation sampled at `time` (the render clock).
#[cfg(feature = "gpu")]
pub fn material(lib: &mut tags::material::ShaderLibrary, i: &Inputs, time: Option<f64>) -> Result<Material, Refused> {
    resolve(lib, i)?.material(i, time)
}

#[cfg(feature = "gpu")]
impl Particle {
    /// The material for the draw whose inputs are `i`: the animated
    /// parameters sampled at `time` as `0x0001b230` does under the
    /// particle provider, game-time curves only
    /// ([`crate::layer_pass::sample_with_frames`]), each animated bitmap
    /// slot's frame bound as `0x0001b0a0` binds it ([`Texture::Frame`]).
    pub fn material(&self, i: &Inputs, time: Option<f64>) -> Result<Material, Refused> {
        use tags::material::{TextureSource, VectorTarget};
        let base = &self.implementation;
        if base.copies_target {
            return Err(Refused::TargetCopy);
        }
        if base.per_draw_inputs && base.variant {
            return Err(Refused::EngineVariant);
        }
        let sampled;
        let (imp, frames) = if self.implementation.animated.is_empty() {
            (&self.implementation, Default::default())
        } else {
            let (out, frames) =
                crate::layer_pass::sample_with_frames(&self.implementation, &self.properties, time, true, self.pass)
                    .map_err(|r| Refused::Animation(r.name()))?;
            sampled = out;
            (&sampled, frames)
        };
        let applied = crate::engine_states::apply_static(imp, &self.inputs).map_err(|_| Refused::EngineState)?;
        // With [`RESET`] set the reset runs between the stipple and the
        // implementation's states: its states, each stage's word 10 and
        // the current textures zeroed (`0x0051f3d8..e4`), so
        // `0x0001cf50` unbinds every stage the implementation leaves.
        let mut states = vec![(STIPPLE_ENABLE, 0)];
        let mut stages: Vec<(u8, u8, u32)> = Vec::new();
        if i.reset {
            states.extend(RESET_STATES);
            stages.extend((0..4).map(|n| (n, RESET_STAGE_WORD, 0)));
        }
        states.extend(applied.render_states.iter().map(|s| (s.state, s.value)));
        stages.extend(
            applied
                .stage_states
                .iter()
                .take(4)
                .enumerate()
                .flat_map(|(n, s)| stage_words(s).map(|(index, value)| (n as u8, index, value))),
        );
        let mut combiner = [0u32; WORDS];
        for (w, b) in combiner.iter_mut().zip(imp.pixel_shader.chunks_exact(4)) {
            *w = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        }
        // Constants from the draw's own inputs change per draw: not compared.
        let mut combiner_driven = 0u64;
        if imp.per_draw_inputs {
            for b in imp.vector_bindings.iter().chain(&imp.scalar_bindings) {
                if let VectorTarget::Pixel { c1, slot, .. } = b.target {
                    combiner_driven |= 1 << constant_word(c1, slot);
                }
            }
        }
        let mut textures = [None; 4];
        for (n, (out, t)) in textures.iter_mut().zip(&imp.textures).enumerate() {
            *out = match t.map(|t| t.source) {
                None | Some(TextureSource::UnsetSlot(_)) => None,
                Some(TextureSource::Bitmap { bitmap, image, .. }) => Some(match frames[n] {
                    Some(frame) => frame_texture(imp, n, bitmap.0, frame)?,
                    None => Texture::Bitmap { bitmap: bitmap.0, image: image as i32 },
                }),
                Some(TextureSource::Default) => Some(Texture::Bitmap { bitmap: u32::MAX, image: 0 }),
                Some(TextureSource::None) => Some(Texture::Empty),
                Some(TextureSource::Engine(id)) => Some(engine_texture(id)?),
                Some(TextureSource::Lightmap) => return Err(Refused::EngineTexture),
            };
        }
        if i.reset {
            for t in textures.iter_mut().filter(|t| t.is_none()) {
                *t = Some(Texture::Empty);
            }
        }
        let vrtx = imp.vertex_shader.ok_or(Refused::NoVertexShader)?;
        let driven = imp.per_draw_inputs
            && imp.vector_bindings.iter().chain(&imp.scalar_bindings).any(|b| matches!(b.target, VectorTarget::Vertex { .. }));
        if driven {
            return Err(Refused::VertexConstantDriven);
        }
        // `0x00019be0`'s size modes: the resolver's value holds the stage's
        // size where it knows it (stage 0 is 1 by 1).
        for d in &imp.dimension_bindings {
            let stage = usize::from(d.stage);
            let known = stage == 0
                || imp.textures.get(stage - 1).copied().flatten().is_some_and(|t| {
                    t.stage_size.is_some() && !matches!(t.source, TextureSource::Engine(_) | TextureSource::Lightmap)
                });
            if !known {
                return Err(Refused::VertexConstantDriven);
            }
        }
        let whole = |r: &tags::material::Register| if r.written == 0xf { Ok((r.index, r.value)) } else { Err(Refused::VertexConstantLanes) };
        // `0x00018eab`: `c[18 + i]`, its hardware slot `Register::index`
        // (`0x003f7230` loads ECX as the slot), the resolver's whole row.
        let mut constants = Vec::with_capacity(imp.vertex_constants.len() + 3);
        for r in &imp.vertex_constants {
            let (index, value) = whole(r)?;
            constants.push((usize::from(index), value));
        }
        constants.extend(i.matrix.iter().enumerate().map(|(n, row)| (MATRIX_SLOT + n, *row)));
        let defaults = imp.vertex_defaults.iter().map(whole).collect::<Result<_, _>>()?;
        Ok(Material {
            states,
            stages,
            combiner,
            combiner_driven,
            textures,
            constants,
            defaults,
            program: (vrtx.0, u32::from(i.class) + PROGRAM_BASE),
        })
    }
}

/// The texture `0x0001b0a0` binds at stage `stage`, whose slot holds
/// `bitmap` and whose frame an animation wrote: the image
/// [`crate::layer_pass::frame_image`] gives over the slot's image count
/// (`tags::material::FrameStage::images`), the rasterizer default
/// (datum -1) where it gives none. A frame that is not finite takes the
/// CRT's error path (`texture_frame`).
#[cfg(feature = "gpu")]
fn frame_texture(imp: &tags::material::ResolvedImplementation, stage: usize, bitmap: u32, frame: f32) -> Result<Texture, Refused> {
    if !frame.is_finite() {
        return Err(Refused::Animation("texture_frame"));
    }
    let images = imp
        .animated
        .iter()
        .flat_map(|a| &a.stages)
        .filter(|s| usize::from(s.stage) == stage)
        .last()
        .map_or(0, |s| s.images);
    Ok(match crate::layer_pass::frame_image(frame, images) {
        Some(image) => Texture::Bitmap { bitmap, image: image as i32 },
        None => Texture::Bitmap { bitmap: u32::MAX, image: 0 },
    })
}

/// What engine texture `id` binds on a particle's stage
/// (`crate::screen_look::engine_texture`): a rasterizer data bitmap (14
/// its image 0, 15 its image 1, the others image 0), or nothing for the
/// cleared stage (4). A render target, the view's, the light's or a
/// texture chosen at run time is the draw's.
#[cfg(feature = "gpu")]
fn engine_texture(id: u8) -> Result<Texture, Refused> {
    use crate::screen_look::EngineTexture;
    match crate::screen_look::engine_texture(id) {
        EngineTexture::Rasterizer(offset) => Ok(Texture::Rasterizer { offset, image: i32::from(id == 15) }),
        EngineTexture::Cleared => Ok(Texture::Empty),
        _ => Err(Refused::EngineTexture),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect_material::tests::Words;

    fn guest() -> Words {
        let mut w = Words::default();
        let esp = 0x100;
        w.set(esp + 12, 0xe000_0004);
        w.set(esp + 16, 2);
        w.set(esp + 20, 1);
        w.set(0x500 + 4, 0x0003_0006);
        w.set(TAG_TABLE, 0x1000);
        w.set(0x1000 + 4 * 16, SHAD);
        w.set(RESET, 0);
        w.set(VERTEX_OVERRIDE[0], 0x0101);
        w.set(VERTEX_OVERRIDE[1], 0x01);
        w.set(VIEW, 0x2000);
        for n in 0..12u32 {
            w.set(0x2004 + 4 * n, (n as f32).to_bits());
        }
        w
    }

    #[test]
    fn the_inputs_are_the_draws_arguments_and_the_views_matrix_by_column() {
        let w = guest();
        let i = read(&|a| w.word(a), 0x100, 0x500).unwrap();
        assert_eq!((i.shader, i.group, i.layer, i.pass, i.class), (0xe000_0004, SHAD, 2, 1, 6));
        assert!(!i.reset && i.vertex_override);
        // Floats 0..12 at view + 4: row 0 takes +4, +0x10, +0x1c, +0x28.
        assert_eq!(i.matrix, [[0.0, 3.0, 6.0, 9.0], [1.0, 4.0, 7.0, 10.0], [2.0, 5.0, 8.0, 11.0]]);
    }

    #[test]
    fn a_shad_or_a_particle_without_reset_reaches_the_shader_setup() {
        let w = guest();
        let i = read(&|a| w.word(a), 0x100, 0x500).unwrap();
        for group in [SHAD, PRT3, PRTM] {
            assert_eq!(check(&Inputs { group, ..i }), Ok(()));
        }
        assert_eq!(check(&Inputs { reset: true, ..i }), Ok(()));
        assert_eq!(check(&Inputs { group: u32::from_be_bytes(*b"bitm"), ..i }), Err(Refused::NotShader));
    }

    /// What a material's draw takes, past the setup's fixed parts: the
    /// stage textures, the vertex constants before the view matrix, the
    /// vertex defaults.
    #[cfg(feature = "gpu")]
    type Made = Result<([Option<Texture>; 4], Vec<(usize, [f32; 4])>, Vec<(u8, [f32; 4])>), &'static str>;

    /// Every kind of particle shader implementation the material handles,
    /// and every kind it names a cause for, at 11 s on the render clock.
    #[cfg(feature = "gpu")]
    #[test]
    fn each_particle_shader_kind_makes_its_material_or_names_its_cause() {
        use crate::layer_pass::tests::{animated, animated_z, identity, implementation, timed};
        use tags::material::{
            AnimationKind, EngineInputs, EngineVertexConstant, FrameStage, InputReads, Origin, Register, StageTexture,
            TextureSource, VectorBinding, VectorTarget,
        };
        let w = guest();
        let i = read(&|a| w.word(a), 0x100, 0x500).unwrap();
        let stage = |source, stage_size| Some(StageTexture { source, size: None, stage_size });
        let bitmap = |image| TextureSource::Bitmap { bitmap: map::TagId(0xe000_0010), image, lod_offset: 0.0 };
        let c18 = |value| (18usize, value);
        let zero = [0.0; 4];
        let size_of_stage_1 = EngineVertexConstant { bank: 1, reg: 0, mode: 5, stage: 1, source: 0 };
        let frame = |images| {
            let mut a = animated(AnimationKind::Bitmap, &[(0, 0)]);
            a.stages = vec![FrameStage { stage: 0, images }];
            a
        };
        type Edit = Box<dyn Fn(&mut Particle)>;
        let rows: Vec<(&str, Edit, Made)> = vec![
            ("static", Box::new(|_| {}), Ok(([None; 4], vec![c18(zero)], vec![]))),
            (
                "instance bitmap, default and none",
                Box::new(move |p| {
                    let t = &mut p.implementation.textures;
                    (t[0], t[1], t[2]) = (stage(bitmap(1), None), stage(TextureSource::Default, None), stage(TextureSource::None, None));
                }),
                Ok((
                    [
                        Some(Texture::Bitmap { bitmap: 0xe000_0010, image: 1 }),
                        Some(Texture::Bitmap { bitmap: u32::MAX, image: 0 }),
                        Some(Texture::Empty),
                        None,
                    ],
                    vec![c18(zero)],
                    vec![],
                )),
            ),
            (
                "rasterizer engine textures and the cleared stage",
                Box::new(move |p| {
                    let e = |id| stage(TextureSource::Engine(id), None);
                    p.implementation.textures = [e(14), e(15), e(1), e(4)];
                }),
                Ok((
                    [
                        Some(Texture::Rasterizer { offset: 0x14, image: 0 }),
                        Some(Texture::Rasterizer { offset: 0x14, image: 1 }),
                        Some(Texture::Rasterizer { offset: 0xc, image: 0 }),
                        Some(Texture::Empty),
                    ],
                    vec![c18(zero)],
                    vec![],
                )),
            ),
            ("render target engine texture", Box::new(move |p| p.implementation.textures[0] = stage(TextureSource::Engine(20), None)), Err("engine_texture")),
            ("lightmap", Box::new(move |p| p.implementation.textures[0] = stage(TextureSource::Lightmap, None)), Err("engine_texture")),
            (
                "game-time vertex constant",
                Box::new(|p| {
                    let (a, properties) = animated_z();
                    p.implementation.animated = vec![a];
                    p.properties = properties;
                }),
                // 11 s over 4 s wraps to 0.75, mapped onto 0..2.
                Ok(([None; 4], vec![c18([0.0, 0.0, 1.5, 0.0])], vec![])),
            ),
            (
                "object-function property",
                Box::new(|p| {
                    let (a, mut properties) = animated_z();
                    properties[0].input = 0x1234;
                    p.implementation.animated = vec![a];
                    p.properties = properties;
                }),
                Err("object_animation"),
            ),
            (
                "animated frame",
                Box::new(move |p| {
                    p.implementation.textures[0] = stage(bitmap(0), None);
                    p.implementation.animated = vec![frame(4)];
                    p.properties = vec![timed(4.0, identity([0.0, 8.0], 0))];
                }),
                // 0.75 of 0..8 is frame 6, image 2 of 4.
                Ok(([Some(Texture::Bitmap { bitmap: 0xe000_0010, image: 2 }), None, None, None], vec![c18(zero)], vec![])),
            ),
            (
                "animated frame of a slot without images",
                Box::new(move |p| {
                    p.implementation.textures[0] = stage(bitmap(0), None);
                    p.implementation.animated = vec![frame(0)];
                    p.properties = vec![timed(4.0, identity([0.0, 8.0], 0))];
                }),
                Ok(([Some(Texture::Bitmap { bitmap: u32::MAX, image: 0 }), None, None, None], vec![c18(zero)], vec![])),
            ),
            ("target copy", Box::new(|p| p.implementation.copies_target = true), Err("target_copy")),
            (
                "variant by the draw's inputs",
                Box::new(|p| {
                    p.implementation.per_draw_inputs = true;
                    p.implementation.variant = true;
                }),
                Err("engine_variant"),
            ),
            (
                "vertex constant from the draw's inputs",
                Box::new(|p| {
                    p.implementation.per_draw_inputs = true;
                    p.implementation.vector_bindings =
                        vec![VectorBinding { source: 7, target: VectorTarget::Vertex { bank: 1, reg: 0, components: 7 } }];
                }),
                Err("vertex_constant_driven"),
            ),
            (
                "size of a bitmap stage",
                Box::new(move |p| {
                    p.implementation.textures[0] = stage(bitmap(0), Some((64, 32)));
                    p.implementation.dimension_bindings = vec![size_of_stage_1];
                }),
                Ok(([Some(Texture::Bitmap { bitmap: 0xe000_0010, image: 0 }), None, None, None], vec![c18(zero)], vec![])),
            ),
            (
                "size of an engine texture",
                Box::new(move |p| {
                    p.implementation.textures[0] = stage(TextureSource::Engine(14), None);
                    p.implementation.dimension_bindings = vec![size_of_stage_1];
                }),
                Err("vertex_constant_driven"),
            ),
            (
                "vertex defaults",
                Box::new(|p| {
                    p.implementation.vertex_defaults =
                        vec![Register { index: 3, value: [1.0, 2.0, 0.0, 0.0], written: 0xf, origin: Origin::Instance }];
                }),
                Ok(([None; 4], vec![c18(zero)], vec![(3, [1.0, 2.0, 0.0, 0.0])])),
            ),
            ("partly written constant", Box::new(|p| p.implementation.vertex_constants[0].written = 0x3), Err("vertex_constant_lanes")),
            ("no vertex shader", Box::new(|p| p.implementation.vertex_shader = None), Err("no_vertex_shader")),
        ];
        // The reset: its states and stage words before the
        // implementation's, and every stage it leaves unbound.
        let mut p = Particle {
            implementation: implementation(0, InputReads::default(), Vec::new()),
            properties: Vec::new(),
            inputs: EngineInputs::default(),
            pass: 0,
        };
        p.implementation.textures[1] = stage(bitmap(0), None);
        p.implementation.render_states = vec![tags::shader::RenderStateValue { state: 0x93, value: 0 }];
        let reset = p.material(&Inputs { reset: true, ..i }, None).unwrap();
        let mut states = vec![(STIPPLE_ENABLE, 0)];
        states.extend(RESET_STATES);
        states.push((0x93, 0));
        assert_eq!(reset.states, states);
        assert_eq!(reset.stages, [(0, 10, 0), (1, 10, 0), (2, 10, 0), (3, 10, 0)]);
        let empty = Some(Texture::Empty);
        assert_eq!(reset.textures, [empty, Some(Texture::Bitmap { bitmap: 0xe000_0010, image: 0 }), empty, empty]);
        let kept = p.material(&i, None).unwrap();
        assert_eq!((kept.states.len(), kept.stages.len(), kept.textures[0]), (2, 0, None));

        for (name, edit, expected) in rows {
            let mut p = Particle {
                implementation: implementation(0, InputReads::default(), Vec::new()),
                properties: Vec::new(),
                inputs: EngineInputs::default(),
                pass: 0,
            };
            edit(&mut p);
            let made: Made = p.material(&i, Some(11.0)).map_err(Refused::name).map(|m| {
                let n = m.constants.len() - 3;
                assert_eq!(m.constants[n..], i.matrix.iter().enumerate().map(|(k, r)| (MATRIX_SLOT + k, *r)).collect::<Vec<_>>());
                assert_eq!(m.program, (0xe000_0002, u32::from(i.class) + PROGRAM_BASE));
                (m.textures, m.constants[..n].to_vec(), m.defaults)
            });
            assert_eq!(made, expected, "{name}");
        }
    }

    /// The two records as the disc has them (`0x0043f77a`, `0x0043f78f`).
    fn records() -> crate::object_streams::StreamFormats {
        let mut b = vec![0u8; crate::object_streams::STREAM_FORMAT_SIZE * 0x2c];
        let at = |t: usize| t * crate::object_streams::STREAM_FORMAT_SIZE;
        b[at(0x2a)..at(0x2b)].copy_from_slice(&[
            0x2a, 0, 3, 1, 2, 2, 0, 3, 3, 4, 2, 5, 1, 6, 3, 7, 3, 9, 0x11, 0xff, 0,
        ]);
        b[at(0x2b)..at(0x2b) + 4].copy_from_slice(&[0x2b, 0, 0x0b, 0xff]);
        crate::object_streams::StreamFormats::from_bytes(&b)
    }

    #[test]
    fn the_override_declares_the_emitters_short_corners_and_otherwise_the_float_stream() {
        let w = guest();
        let i = read(&|a| w.word(a), 0x100, 0x500).unwrap();
        assert_eq!(stream_type(&i), STREAM_TYPE_OVERRIDE);
        assert_eq!(stream_type(&Inputs { vertex_override: false, ..i }), STREAM_TYPE);
        let t = records();
        // v0 reads semantic 0: four S32K shorts in an 8-byte vertex.
        let f = vertex_formats(&[0, 3], &t.elements(STREAM_TYPE_OVERRIDE).unwrap());
        assert_eq!(f[0], 0x0845);
        assert!(f[1..].iter().all(|&w| w == 0x02));
        // The float stream is 104 bytes: v0 its float4 position, v1 its
        // colour (semantic 9), the rest off.
        let f = vertex_formats(&[0, 9], &t.elements(STREAM_TYPE).unwrap());
        assert_eq!((f[0], f[1]), (104 << 8 | 0x42, 104 << 8 | 0x40));
        assert!(f[2..].iter().all(|&w| w == 0x02));
    }

    #[cfg(feature = "gpu")]
    #[test]
    fn pixel_constants_land_in_their_definition_words_and_stage_states_in_the_binders_slots() {
        assert_eq!((constant_word(false, 0), constant_word(false, 7), constant_word(true, 2)), (10, 17, 20));
        assert_eq!((constant_word(false, 8), constant_word(true, 8)), (43, 44));
        let s = tags::shader::StageStates {
            address_u: 1,
            address_v: 2,
            address_w: 3,
            mag_filter: 4,
            min_filter: 5,
            mip_filter: 6,
            mip_lod_bias: 7,
            max_mip_level: 8,
            max_anisotropy: 9,
            color_key_op: 10,
            color_sign: 11,
            alpha_kill: 12,
            border_color: 13,
            color_key_color: 14,
            unread: [0, 0],
        };
        let words = stage_words(&s);
        assert!(words[..12].iter().enumerate().all(|(n, &(i, v))| usize::from(i) == n && v == n as u32 + 1));
        assert_eq!(words[12..], [(0x1d, 13), (0x1e, 14)]);
    }

    #[cfg(feature = "gpu")]
    #[test]
    fn a_shader_that_does_not_resolve_is_refused_by_name() {
        let w = guest();
        let i = Inputs { vertex_override: false, ..read(&|a| w.word(a), 0x100, 0x500).unwrap() };
        let mut lib = tags::material::ShaderLibrary::new(tags::shader::Metadata::new(0x1000, vec![0; 16]), []);
        assert_eq!(material(&mut lib, &i, None), Err(Refused::Tags));
        assert_eq!(resolve(&mut lib, &Inputs { group: PRT3, ..i }), Err(Refused::Tags));
    }
}
