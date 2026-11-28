//! A HUD widget's shader as the overlay draws it, resolved from the tags
//! with no guest shader setup (docs/graphics/retire-d3d.md, slice R6;
//! docs/graphics/native-hud-overlay.md, "The material in Rust").
//!
//! The HUD shader quad `0x0022a664` sets the HUD's input provider
//! (`0x0001bd50` with `0x005021d0`: context, callback `0x0022ace7`) and
//! begins the widget's shader (`0x0001bbf0`), whose setup evaluates the
//! shader's animated properties through that provider (`0x0001b230`).
//! [`HudShader`] is the same material read from the map: the one pass
//! implementation the shader draws, its texture stages, its entry render
//! and stage states, and its combiner words with each draw's animated
//! constants ([`HudShader::words`]). A shader whose setup reads anything
//! else (an engine input per draw, a texture the engine supplies other
//! than the widget's bitmap or the text stage, an animated state) is
//! refused by name and stays on the guest's setup.

use frame_scene::{OverlayBlend, OverlayTexture, Sampler};
use gfx::state::{RenderState, RenderStateType, TextureAddress, TextureFilter};
use gfx::xdk::{HALO2_BUILD, RenderStateTable};
use map::TagId;
use tags::material::{AnimationKind, ResolvedImplementation, ShaderLibrary, TextureSource};
use tags::shader::AnimatedProperty;

use crate::layer_pass::{LayerRefusal, ObjectInputs, sample_with_inputs};

/// The engine texture `0x00019530` binds from the HUD's bitmap
/// (`[0x004e6978]` datum, `[0x004e697c]` image; jump-table entry 15,
/// `0x00019817`), which the quad sets before the shader begins.
pub const WIDGET_BITMAP_SOURCE: u8 = 16;

/// The engine texture a HUD text shader's stage 0 names (`text_flash`,
/// `dim_text_flash`: `h2 shader <map> 'ui\hud\shaders\text_flash'`).
/// `0x00019530` binds `[[0x00485a80] + 0x14]` for it (entry 14,
/// `0x000197fb`); the string draw's batch then binds the glyph cache to
/// stage 0 in its place (`0x00035b90`) and keeps the shader's combiner
/// (`+0x97`), so the stage samples the glyphs.
pub const TEXT_STAGE_SOURCE: u8 = 15;

/// Why a HUD shader stays on the guest's setup.
#[derive(Clone, Debug, PartialEq)]
pub enum HudRefusal {
    /// The material does not resolve from the map.
    Material(String),
    /// Not exactly one pass implementation across the template's layers:
    /// which one the quad's `0x0001bbf0` call selects is not read.
    Implementations(usize),
    /// The implementation reads an engine input that changes per draw.
    EngineInputs,
    /// An engine-driven render or stage state (`0x0001a170`, `0x0001ab50`).
    EngineStates,
    /// An animated render or stage state, or an animated bitmap frame.
    AnimatedState(AnimationKind),
    /// A stage whose texture is neither the widget's bitmap, an instance
    /// bitmap nor none.
    Texture { stage: usize },
    /// A render state the overlay needs is not set by the implementation,
    /// or a value `gfx` does not decode.
    State(&'static str),
    /// An animated constant did not sample (the provider's value of an
    /// input did not read, a curve that did not parse).
    Animation(&'static str),
}

impl HudRefusal {
    pub fn name(&self) -> &'static str {
        match self {
            HudRefusal::Material(_) => "material",
            HudRefusal::Implementations(_) => "implementations",
            HudRefusal::EngineInputs => "engine_inputs",
            HudRefusal::EngineStates => "engine_states",
            HudRefusal::AnimatedState(_) => "animated_state",
            HudRefusal::Texture { .. } => "texture",
            HudRefusal::State(_) => "state",
            HudRefusal::Animation(_) => "animation",
        }
    }
}

/// The overlay state an implementation's entry render states give: what
/// the capture reads from the device at the draw (`game::hud_overlay`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HudState {
    pub blend: OverlayBlend,
    /// RGBA, 0 to 1, for a blend that names the constant; zero otherwise.
    pub blend_constant: [f32; 4],
    /// Alpha compare and reference when the test is on.
    pub alpha_test: Option<(u32, u8)>,
    /// The Xbox per-channel write mask.
    pub color_write: u32,
}

/// One HUD shader, resolved once per map.
#[derive(Clone, Debug)]
pub struct HudShader {
    pub shader: TagId,
    implementation: ResolvedImplementation,
    properties: Vec<AnimatedProperty>,
    /// Each stage's texture, the widget's bitmap as [`StageTexture::Widget`].
    pub stages: [StageTexture; 4],
    /// Each stage's sampler, for the stages with a texture.
    pub samplers: [Sampler; 4],
    /// Stages discarding samples whose alpha is zero (stage state bit 2).
    pub alpha_kill: u8,
    pub state: HudState,
}

/// What a stage samples.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StageTexture {
    #[default]
    None,
    /// The quad's own bitmap and image (engine source [`WIDGET_BITMAP_SOURCE`]).
    Widget,
    /// An instance bitmap.
    Bitmap { tag: u32, image: u16 },
    /// The text stage (engine source [`TEXT_STAGE_SOURCE`]): the glyphs
    /// of the string drawn. No widget quad samples it.
    Glyphs,
}

impl StageTexture {
    /// The overlay texture this stage binds for a quad of `bitmap`'s `image`.
    pub fn overlay(self, bitmap: u32, image: u16) -> Option<OverlayTexture> {
        match self {
            StageTexture::None | StageTexture::Glyphs => None,
            StageTexture::Widget => Some(OverlayTexture::Bitmap { tag: bitmap, image }),
            StageTexture::Bitmap { tag, image } => Some(OverlayTexture::Bitmap { tag, image }),
        }
    }
}

impl HudShader {
    /// Resolves `shader` from the map's library.
    pub fn resolve(lib: &mut ShaderLibrary, shader: TagId) -> Result<HudShader, HudRefusal> {
        let material = lib.material(shader, None).map_err(|e| HudRefusal::Material(e.to_string()))?;
        let mut all = material.layers.iter().flat_map(|l| &l.passes).flat_map(|p| &p.implementations);
        let count = all.clone().count();
        let (Some(imp), 1) = (all.next(), count) else {
            return Err(HudRefusal::Implementations(count));
        };
        let properties = crate::layer_pass::instance_properties(lib, shader);
        Self::from_implementation(shader, imp.clone(), properties)
    }

    /// [`Self::resolve`]'s checks over an implementation already resolved.
    pub fn from_implementation(
        shader: TagId,
        implementation: ResolvedImplementation,
        properties: Vec<AnimatedProperty>,
    ) -> Result<HudShader, HudRefusal> {
        let imp = &implementation;
        if imp.per_draw_inputs {
            return Err(HudRefusal::EngineInputs);
        }
        if !imp.engine_render_states.is_empty() || !imp.engine_stage_states.is_empty() {
            return Err(HudRefusal::EngineStates);
        }
        if let Some(a) = imp.animated.iter().find(|a| !matches!(a.kind, AnimationKind::PixelConstant)) {
            return Err(HudRefusal::AnimatedState(a.kind));
        }
        let mut stages = [StageTexture::None; 4];
        let mut samplers = [Sampler::default(); 4];
        let mut alpha_kill = 0;
        for (n, texture) in imp.textures.iter().enumerate() {
            stages[n] = match texture.map(|t| t.source) {
                None | Some(TextureSource::None) => StageTexture::None,
                Some(TextureSource::Engine(WIDGET_BITMAP_SOURCE)) => StageTexture::Widget,
                Some(TextureSource::Engine(TEXT_STAGE_SOURCE)) => StageTexture::Glyphs,
                Some(TextureSource::Bitmap { bitmap, image, .. }) => StageTexture::Bitmap {
                    tag: bitmap.0,
                    image: u16::try_from(image).map_err(|_| HudRefusal::Texture { stage: n })?,
                },
                Some(_) => return Err(HudRefusal::Texture { stage: n }),
            };
            if stages[n] == StageTexture::None {
                continue;
            }
            let s = imp.stage_states.get(n).ok_or(HudRefusal::Texture { stage: n })?;
            samplers[n] = Sampler {
                wrap: u32::from(s.address_u) == TextureAddress::Wrap.xbox_value(),
                point: u32::from(s.min_filter) == TextureFilter::Point.xbox_value(),
            };
            if s.alpha_kill & 4 != 0 {
                alpha_kill |= 1u8 << n;
            }
        }
        let state = hud_state(imp)?;
        Ok(HudShader { shader, implementation, properties, stages, samplers, alpha_kill, state })
    }

    /// The combiner words this draw binds: the implementation's pixel
    /// shader definition with its animated constants sampled at `time`
    /// (the render clock) through the HUD's provider `inputs`, and the
    /// D3D default final combiner where the definition programs none
    /// (`gfx::guest_program::overlay_shader_words` reads them).
    pub fn words(&self, time: Option<f64>, inputs: ObjectInputs<'_>) -> Result<[u32; 60], HudRefusal> {
        let sampled;
        let imp = if self.implementation.animated.is_empty() {
            &self.implementation
        } else {
            sampled = sample_with_inputs(&self.implementation, &self.properties, time, inputs, 0)
                .map_err(|r: LayerRefusal| HudRefusal::Animation(r.name()))?;
            &sampled
        };
        Ok(definition_words(&imp.pixel_shader))
    }

    /// Whether any constant animates: a draw's words then depend on its
    /// inputs and the clock, not on the shader alone.
    pub fn animates(&self) -> bool {
        !self.implementation.animated.is_empty()
    }
}

/// A pixel shader definition's 60 words, with the D3D default final
/// combiner where both final-combiner words are zero.
pub fn definition_words(def: &[u8]) -> [u32; 60] {
    let mut words = [0u32; 60];
    for (w, b) in words.iter_mut().zip(def.chunks_exact(4)) {
        *w = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    }
    if words[8] == 0 && words[9] == 0 {
        (words[8], words[9]) = shaders::psh::d3d_default_final_combiner(false, false);
    }
    words
}

/// The overlay state of an implementation's entry render states. The
/// blend must be set by the implementation itself: what an earlier draw
/// left is not modelled.
pub fn hud_state(imp: &ResolvedImplementation) -> Result<HudState, HudRefusal> {
    let table = RenderStateTable::for_build(HALO2_BUILD);
    let mut rs = RenderState::default();
    let mut set = Vec::with_capacity(imp.render_states.len());
    for s in &imp.render_states {
        let slot = u32::from(s.state);
        rs.set_guest(&table, slot, s.value).map_err(|_| HudRefusal::State("value"))?;
        if let Some(t) = RenderStateType::from_guest_index(&table, slot) {
            set.push(t);
        }
    }
    let has = |t| set.contains(&t);
    if !has(RenderStateType::AlphaBlendEnable) {
        return Err(HudRefusal::State("ALPHABLENDENABLE"));
    }
    if rs.blend_enable && !(has(RenderStateType::SrcBlend) && has(RenderStateType::DestBlend)) {
        return Err(HudRefusal::State("SRCBLEND/DESTBLEND"));
    }
    let blend = if rs.blend_enable {
        OverlayBlend::On {
            source: rs.src_blend.xbox_value(),
            dest: rs.dst_blend.xbox_value(),
            equation: rs.blend_op.xbox_value(),
        }
    } else {
        OverlayBlend::Off
    };
    let constant = |f: gfx::state::BlendFactor| {
        use gfx::state::BlendFactor as B;
        matches!(f, B::ConstantColor | B::InvConstantColor | B::ConstantAlpha | B::InvConstantAlpha)
    };
    let blend_constant = if rs.blend_enable && (constant(rs.src_blend) || constant(rs.dst_blend)) {
        let c = gfx::translate::blend_constant(&rs);
        [c.r, c.g, c.b, c.a].map(|v| v as f32)
    } else {
        [0.0; 4]
    };
    let alpha_test = rs.alpha_test_enable.then_some((rs.alpha_func.xbox_value(), rs.alpha_ref));
    Ok(HudState { blend, blend_constant, alpha_test, color_write: rs.color_write.xbox_value() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer_pass::tests::{animated, identity, implementation};
    use gfx::state::{BlendFactor, RenderStateType as R};
    use tags::material::{InputReads, StageTexture as TagStage};
    use tags::shader::{RenderStateValue, StageStates};

    fn slot(t: R) -> u8 {
        let table = RenderStateTable::for_build(HALO2_BUILD);
        (0..=255u8)
            .find(|&s| RenderStateType::from_guest_index(&table, u32::from(s)) == Some(t))
            .expect("a 5849 slot")
    }

    fn blended() -> Vec<RenderStateValue> {
        vec![
            RenderStateValue { state: slot(R::AlphaBlendEnable), value: 1 },
            RenderStateValue { state: slot(R::SrcBlend), value: BlendFactor::SrcAlpha.xbox_value() },
            RenderStateValue { state: slot(R::DestBlend), value: BlendFactor::InvSrcAlpha.xbox_value() },
        ]
    }

    fn widget() -> ResolvedImplementation {
        let mut imp = implementation(0, InputReads::default(), Vec::new());
        imp.render_states = blended();
        imp.textures[0] = Some(TagStage {
            source: TextureSource::Engine(WIDGET_BITMAP_SOURCE),
            size: None,
            stage_size: None,
        });
        imp.stage_states = vec![StageStates {
            address_u: 3,
            address_v: 3,
            address_w: 3,
            mag_filter: 2,
            min_filter: 2,
            mip_filter: 0,
            mip_lod_bias: 0,
            max_mip_level: 0,
            max_anisotropy: 1,
            color_key_op: 0,
            color_sign: 0,
            alpha_kill: 0,
            border_color: 0,
            color_key_color: 0,
            unread: [0; 2],
        }];
        imp
    }

    /// A widget's quad samples its own bitmap through stage 0, blended as
    /// its implementation sets, and draws with its definition's words.
    #[test]
    fn a_still_widget_shader_binds_its_quads_bitmap_and_its_own_blend() {
        let hud = HudShader::from_implementation(TagId(7), widget(), Vec::new()).unwrap();
        assert_eq!(hud.stages[0], StageTexture::Widget);
        assert_eq!(hud.stages[0].overlay(0x40, 3), Some(OverlayTexture::Bitmap { tag: 0x40, image: 3 }));
        assert_eq!(hud.stages[1].overlay(0x40, 3), None);
        assert_eq!(hud.samplers[0], Sampler { wrap: false, point: false }, "clamped, linear");
        assert_eq!(
            hud.state.blend,
            OverlayBlend::On {
                source: BlendFactor::SrcAlpha.xbox_value(),
                dest: BlendFactor::InvSrcAlpha.xbox_value(),
                equation: gfx::state::RenderState::default().blend_op.xbox_value(),
            }
        );
        assert!(!hud.animates());
        let words = hud.words(None, &|_| None).unwrap();
        assert_eq!(words, definition_words(&widget().pixel_shader));
        assert_ne!((words[8], words[9]), (0, 0), "the default final combiner");
    }

    /// What the guest's setup would read and the material does not model
    /// keeps the shader on the guest's setup, by name.
    #[test]
    fn what_the_setup_reads_per_draw_refuses_by_name() {
        let mut imp = widget();
        imp.per_draw_inputs = true;
        assert_eq!(HudShader::from_implementation(TagId(7), imp, Vec::new()).unwrap_err(), HudRefusal::EngineInputs);
        let mut imp = widget();
        imp.render_states.clear();
        assert_eq!(
            HudShader::from_implementation(TagId(7), imp, Vec::new()).unwrap_err(),
            HudRefusal::State("ALPHABLENDENABLE")
        );
        let mut imp = widget();
        imp.textures[1] = Some(TagStage { source: TextureSource::Lightmap, size: None, stage_size: None });
        assert_eq!(
            HudShader::from_implementation(TagId(7), imp, Vec::new()).unwrap_err(),
            HudRefusal::Texture { stage: 1 }
        );
        let mut imp = widget();
        imp.animated = vec![animated(AnimationKind::RenderState, &[(0, 0)])];
        assert_eq!(
            HudShader::from_implementation(TagId(7), imp, Vec::new()).unwrap_err(),
            HudRefusal::AnimatedState(AnimationKind::RenderState)
        );
    }

    /// A text shader's stage 0 is the engine's text stage, which the
    /// string draw binds the glyphs to: the shader resolves, and a widget
    /// quad, which has no glyphs, has no texture for it.
    #[test]
    fn a_text_shaders_stage_is_the_glyphs_and_no_widget_quads() {
        let mut imp = widget();
        imp.textures[0] = Some(TagStage { source: TextureSource::Engine(TEXT_STAGE_SOURCE), size: None, stage_size: None });
        let hud = HudShader::from_implementation(TagId(7), imp, Vec::new()).unwrap();
        assert_eq!(hud.stages[0], StageTexture::Glyphs);
        assert_eq!(hud.stages[0].overlay(0x40, 3), None);
        let mut imp = widget();
        imp.textures[0] = Some(TagStage { source: TextureSource::Engine(14), size: None, stage_size: None });
        assert_eq!(
            HudShader::from_implementation(TagId(7), imp, Vec::new()).unwrap_err(),
            HudRefusal::Texture { stage: 0 },
            "another engine texture"
        );
    }

    /// A meter's constant follows the provider's `hud_input1`: the
    /// property names it (`input`), and the guest's `0x0001b230` takes the
    /// provider's value as the curve's input.
    #[test]
    fn an_animated_constant_follows_the_providers_input() {
        const HUD_INPUT_1: u32 = 0x0a00_05cf;
        let mut imp = widget();
        imp.animated = vec![animated(AnimationKind::PixelConstant, &[(0, 0)])];
        let property = AnimatedProperty { input: HUD_INPUT_1, input_a: 0, input_b: 1f32.to_bits(), function: identity([0.0, 1.0], 0) };
        let hud = HudShader::from_implementation(TagId(7), imp, vec![property]).unwrap();
        assert!(hud.animates());
        let at = |fill: f32| {
            hud.words(Some(3.0), &move |name| Some(if name == HUD_INPUT_1 { fill } else { 1.0 })).unwrap()
        };
        assert_ne!(at(0.25), at(0.75), "the fill reaches the constant");
        assert_eq!(at(0.5), at(0.5));
        assert_eq!(
            hud.words(Some(3.0), &|_| None).unwrap_err(),
            HudRefusal::Animation("object_animation"),
            "an input the provider does not read"
        );
    }
}
