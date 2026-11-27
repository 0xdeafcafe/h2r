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
//!    implementation 0, size 10000)`, which runs `0x00016b90`: the
//!    implementation's render and stage states, its combiner and its
//!    bound bitmaps (`tags::material`, docs/formats/shader-instances.md);
//! 4. program `class + 7` of implementation 0's `vrtx` (`+0x100`,
//!    `0x0001c590` with BL 1).
//!
//! [`Inputs`] are read at the draw's entry; [`material`] (with the map's
//! shader library) builds the [`Material`] or names the branch it does
//! not model.

#[cfg(feature = "gpu")]
use crate::effect_material::{MATRIX_REGISTER, Texture, WORDS};
pub use crate::effect_material::Material;
use crate::effect_material::TAG_TABLE;

/// The particle draw, where the game reads [`Inputs`].
pub const DRAW: u32 = 0x0022_43c0;
/// The view whose matrix the setup writes (a pointer).
pub const VIEW: u32 = 0x0046_87d0;
/// `0x0001bbf0` resets the device first while this byte is set.
pub const RESET: u32 = 0x0046_70bc;
/// With both bytes set the setup selects engine program `0x2b` instead
/// (`0x0001ccb0`).
pub const VERTEX_OVERRIDE: [u32; 2] = [0x0047_0a38, 0x0047_0a39];
/// The program index is the system's class plus this.
pub const PROGRAM_BASE: u32 = 7;
/// `STIPPLEENABLE`, which the setup turns off.
pub const STIPPLE_ENABLE: u8 = 0x53;

const SHAD: u32 = u32::from_be_bytes(*b"shad");
const PARTICLE_MODELS: [u32; 2] = [u32::from_be_bytes(*b"PRTM"), u32::from_be_bytes(*b"prt3")];

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
    /// A `PRTM` or `prt3` in the shader's place: the setup takes its
    /// template through the tag's own table (`0x00137bd0`).
    ParticleModel,
    /// Any other tag that is not a `shad`.
    NotShader,
    /// The device reset `0x0001bbf0` runs first ([`RESET`]).
    Reset,
    /// The engine program `0x2b` ([`VERTEX_OVERRIDE`]).
    VertexOverride,
    /// The shader, its template or pass does not resolve or has no
    /// such layer or pass.
    Tags,
    /// Animated parameters, which the resolver lists but does not
    /// evaluate.
    Animated,
    /// The implementation copies the frame first (`0x0003bea0`).
    TargetCopy,
    /// A combiner variant chosen by inputs the draw supplies.
    EngineVariant,
    /// An engine-driven render or stage state.
    EngineState,
    /// A stage bound from an engine texture or the lightmap.
    EngineTexture,
    /// Implementation 0 has no vertex shader.
    NoVertexShader,
}

impl Refused {
    pub fn name(self) -> &'static str {
        match self {
            Self::ParticleModel => "particle_model",
            Self::NotShader => "not_shader",
            Self::Reset => "reset",
            Self::VertexOverride => "vertex_override",
            Self::Tags => "tags",
            Self::Animated => "animated",
            Self::TargetCopy => "target_copy",
            Self::EngineVariant => "engine_variant",
            Self::EngineState => "engine_state",
            Self::EngineTexture => "engine_texture",
            Self::NoVertexShader => "no_vertex_shader",
        }
    }
}

/// Whether the inputs reach `0x00016b90` as modelled: a `shad`, no
/// reset, no override.
pub fn check(i: &Inputs) -> Result<(), Refused> {
    if PARTICLE_MODELS.contains(&i.group) {
        return Err(Refused::ParticleModel);
    }
    if i.group != SHAD {
        return Err(Refused::NotShader);
    }
    if i.reset {
        return Err(Refused::Reset);
    }
    if i.vertex_override {
        return Err(Refused::VertexOverride);
    }
    Ok(())
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

/// The material `0x0023ea80` sets for `i`, the shader resolved by `lib`.
#[cfg(feature = "gpu")]
pub fn material(lib: &mut tags::material::ShaderLibrary, i: &Inputs) -> Result<Material, Refused> {
    use tags::material::{EngineInputs, PassSelection, TextureSource, VectorTarget};
    check(i)?;
    let id = map::TagId(i.shader);
    let selection = PassSelection { lod: 0, layer: i.layer as usize, pass: i.pass as usize, implementation: 0 };
    let selected = lib.selected_implementation(id, selection, None, None).map_err(|_| Refused::Tags)?;
    let instance = EngineInputs::for_shader(&lib.shader(id).map_err(|_| Refused::Tags)?);
    let imp = selected.implementation;
    if !imp.animated.is_empty() {
        return Err(Refused::Animated);
    }
    if imp.copies_target {
        return Err(Refused::TargetCopy);
    }
    if imp.per_draw_inputs && imp.variant {
        return Err(Refused::EngineVariant);
    }
    let applied = crate::engine_states::apply_static(&imp, &instance).map_err(|_| Refused::EngineState)?;
    let mut states = vec![(STIPPLE_ENABLE, 0)];
    states.extend(applied.render_states.iter().map(|s| (s.state, s.value)));
    let stages = applied
        .stage_states
        .iter()
        .take(4)
        .enumerate()
        .flat_map(|(n, s)| stage_words(s).map(|(index, value)| (n as u8, index, value)))
        .collect();
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
    for (out, t) in textures.iter_mut().zip(&imp.textures) {
        *out = match t.map(|t| t.source) {
            None | Some(TextureSource::UnsetSlot(_)) => None,
            Some(TextureSource::Bitmap { bitmap, image, .. }) => Some(Texture::Bitmap { bitmap: bitmap.0, image: image as i32 }),
            Some(TextureSource::Default) => Some(Texture::Bitmap { bitmap: u32::MAX, image: 0 }),
            Some(TextureSource::None) => Some(Texture::Empty),
            Some(TextureSource::Lightmap | TextureSource::Engine(_)) => return Err(Refused::EngineTexture),
        };
    }
    let vrtx = imp.vertex_shader.ok_or(Refused::NoVertexShader)?;
    Ok(Material {
        states,
        stages,
        combiner,
        combiner_driven,
        textures,
        constants: i.matrix.iter().enumerate().map(|(n, row)| (MATRIX_REGISTER + n, *row)).collect(),
        program: (vrtx.0, u32::from(i.class) + PROGRAM_BASE),
    })
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
    fn only_a_shad_without_reset_or_override_reaches_the_shader_setup() {
        let w = guest();
        let i = read(&|a| w.word(a), 0x100, 0x500).unwrap();
        assert_eq!(check(&i), Err(Refused::VertexOverride));
        let i = Inputs { vertex_override: false, ..i };
        assert_eq!(check(&i), Ok(()));
        assert_eq!(check(&Inputs { reset: true, ..i }), Err(Refused::Reset));
        assert_eq!(check(&Inputs { group: PARTICLE_MODELS[1], ..i }), Err(Refused::ParticleModel));
        assert_eq!(check(&Inputs { group: u32::from_be_bytes(*b"bitm"), ..i }), Err(Refused::NotShader));
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
        assert_eq!(material(&mut lib, &i), Err(Refused::Tags));
    }
}
