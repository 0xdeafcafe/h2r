//! Halo 1 textureless surfaces and their layer-15 programs.

use super::*;
use tags::shader::{ImplementationBindings, ParameterBinding, TagRef, TemplateLod, TemplatePass};

pub const CLEAR: Layout = Layout {
    stem: r"h1\transparent\glass_background_tint",
    word_14: 5,
    ints: [0, 0],
    bitmaps: &[],
    colors: &[],
    vectors: &[],
    root: Root::None,
    animation: None,
    tint: None,
};

pub const GLASS_VERTEX: &str = r"rasterizer\vertex_shaders_xbox\transparent_generic_reflection";
pub const PARTICLE_VERTEX: &str = r"rasterizer\vertex_shaders_xbox\particle";
pub const PARTICLE_MAX: &str = r"h1\transparent\particle_maximum";

/// One transparent pass, with the instance inputs consumed by its vertex and pixel programs.
fn transparent_graph(
    pass: TagId,
    implementation: ImplementationBindings,
    bindings: Vec<ParameterBinding>,
) -> TemplatePostprocess {
    let one = IndexRange { start: 0, count: 1 };
    let mut layers = vec![IndexRange::default(); 25];
    layers[15] = one;
    TemplatePostprocess {
        lods: vec![TemplateLod {
            layers: IndexRange {
                start: 0,
                count: 25,
            },
            layer_mask: 1 << 15,
            unknown_6: 0,
        }],
        layers,
        passes: vec![TemplatePass {
            pass: TagRef {
                group: SPAS,
                id: pass,
            },
            implementations: one,
        }],
        implementations: vec![implementation],
        bindings,
    }
}

pub fn clear_template(pass_id: TagId) -> Result<Block> {
    stem(
        &transparent_graph(pass_id, ImplementationBindings::default(), vec![]),
        |id| id,
    )
}

pub fn clear_instance(stem_id: TagId) -> Result<Block> {
    encode(
        super::super::shad_def(),
        &build_record(&CLEAR, stem_id, &[], &[], &[], None)?,
    )
}

fn clear_pixel() -> PixelShaderDef {
    // H1 0x68383..0x683db: RGB only, ZERO / SRC_COLOR, ADD. With no
    // texture the sampler is white; white background tint leaves RGB white
    // at every view angle. Reflection and diffuse layers are absent.
    PixelShaderDef {
        combiner_count: 1,
        final_inputs_abcd: 0x20,  // D = unsigned-invert(zero)
        final_inputs_efg: 0x2080, // G = one
        ..Default::default()
    }
}

pub fn clear_pass(vertex: TagId) -> Result<Block> {
    pass(
        vertex,
        &clear_pixel(),
        &[],
        1,
        0,
        &[
            (67, 0x10101),
            (59, 1),
            (62, 0),
            (63, 0x300),
            (74, 0x8006),
            (64, 0),
            (60, 1),
            (61, 0),
            (58, 0x204),
        ],
        &[(60, 0)],
    )
}

/// H1 particle secondary-map coordinate selector (part +0x10c).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParticleCoordinates {
    Primary,
    Sprite,
    Screen,
}

impl ParticleCoordinates {
    pub fn stem(self) -> &'static str {
        match self {
            Self::Primary => PARTICLE_MAX,
            Self::Sprite => r"h1\transparent\particle_maximum_sprite_mask",
            Self::Screen => r"h1\transparent\particle_maximum_screen_mask",
        }
    }
}

pub fn maximum_graph(pass: TagId, coordinates: ParticleCoordinates) -> TemplatePostprocess {
    let count = if coordinates == ParticleCoordinates::Primary {
        1
    } else {
        2
    };
    let binding = |word, source| ParameterBinding {
        word,
        b2: 0,
        source,
    };
    let mut bindings = vec![binding(0, 0)];
    if count == 2 {
        bindings.push(binding(1, 1));
    }
    bindings.extend([binding(0x01b0, 0), binding(0x1c6f, 1)]);
    if count == 2 {
        bindings.push(binding(0x01b1, 2));
    }
    transparent_graph(
        pass,
        ImplementationBindings {
            bitmaps: IndexRange { start: 0, count },
            vertex_constants: IndexRange {
                start: count.into(),
                count: count + 1,
            },
            ..Default::default()
        },
        bindings,
    )
}

pub fn maximum_template(graph: &TemplatePostprocess) -> Result<Block> {
    stem(graph, |id| id)
}

fn maximum_pixel(mask: bool) -> PixelShaderDef {
    let mut pixel = PixelShaderDef {
        combiner_count: if mask { 3 } else { 2 },
        texture_modes: if mask { 0x21 } else { 1 },
        final_inputs_abcd: 0x0c,
        final_inputs_efg: 0x1c80,
        ..Default::default()
    };
    // 0x66e45..0x67060: primary * vertex tint; when present, multiply
    // by the secondary map. H1 keeps sampled alpha in r0.a and puts
    // fog times particle opacity in v0.a; only the latter multiplies RGB.
    // The H2 billboard vertex program supplies that product in v1.a.
    pixel.rgb_inputs[0] = 0x0805_0000;
    pixel.alpha_inputs[0] = 0x1820_0000;
    pixel.rgb_outputs[0] = 0xc0;
    pixel.alpha_outputs[0] = 0xc0;
    if mask {
        pixel.rgb_inputs[1] = 0x0c09_0000;
        pixel.alpha_inputs[1] = 0x1c19_0000;
        pixel.rgb_outputs[1] = 0xc0;
        pixel.alpha_outputs[1] = 0xc0;
    }
    let last = if mask { 2 } else { 1 };
    pixel.rgb_inputs[last] = 0x0c15_0000;
    pixel.rgb_outputs[last] = 0xc0;
    pixel
}

pub fn maximum_pass(vertex: TagId, coordinates: ParticleCoordinates) -> Result<Block> {
    let mask = coordinates != ParticleCoordinates::Primary;
    let textures: &[(u8, u8)] = if mask { &[(0, 0), (1, 0)] } else { &[(0, 0)] };
    // E 0x7f4e0, table entry 6: ONE, ONE, MAX. H1 writes RGB only.
    pass(
        vertex,
        &maximum_pixel(mask),
        textures,
        1,
        2,
        &[
            (67, 0x10101),
            (59, 1),
            (62, 1),
            (63, 1),
            (74, 0x8008),
            (64, 0),
            (60, 0),
            (147, 0),
        ],
        &[(147, 0x900), (74, 0x8006)],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaders::psh::{PixelOptions, TextureDim, interp};

    #[test]
    fn white_glass_preserves_destination_colour() {
        let rgba = interp::run(
            &clear_pixel(),
            &PixelOptions::default(),
            &interp::Inputs::default(),
            &interp::Uniforms::default(),
            &interp::NoTextures,
        )
        .unwrap();
        assert_eq!(rgba, [1.; 4]);
        let scene = [0.25, 0.125, 0.5];
        assert_eq!(
            std::array::from_fn::<_, 3, _>(|i| scene[i] * rgba[i]),
            scene
        );
    }

    struct Textures;
    impl interp::Sampler for Textures {
        fn sample(&self, stage: usize, _: TextureDim, _: [f32; 3]) -> [f32; 4] {
            [[1., 0.5, 0.25, 0.], [0.5, 1., 0.25, 0.75]][stage]
        }
    }

    #[test]
    fn maximum_draw_uses_particle_opacity_independently_of_sampled_alpha() {
        for coordinates in [
            ParticleCoordinates::Primary,
            ParticleCoordinates::Sprite,
            ParticleCoordinates::Screen,
        ] {
            let mask = coordinates != ParticleCoordinates::Primary;
            let rgba = interp::run(
                &maximum_pixel(mask),
                &PixelOptions::default(),
                &interp::Inputs {
                    d1: [1., 0.5, 1., 0.5],
                    t: [[0., 0., 0., 1.]; 4],
                    front: true,
                    ..Default::default()
                },
                &interp::Uniforms::default(),
                &Textures,
            )
            .unwrap();
            let expected = if mask {
                [0.25, 0.125, 0.03125, 0.]
            } else {
                [0.5, 0.125, 0.125, 0.]
            };
            assert_eq!(rgba, expected);
            let pass = maximum_pass(TagId(1), coordinates).unwrap();
            let r = defs::from_built(def(b"spas"), &pass).unwrap();
            let Some(Val::Block(states)) = r.at("postprocess[0].render_states") else {
                panic!()
            };
            for (state, value) in [(67, 0x10101), (62, 1), (63, 1), (74, 0x8008)] {
                assert!(
                    states.iter().any(|s| s.at("state") == Some(&int(state))
                        && s.at("value") == Some(&int(value)))
                );
            }
        }
    }
}
