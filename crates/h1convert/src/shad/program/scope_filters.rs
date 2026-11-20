//! Layer-15 programs for the Halo 1 zoom post-process. The framebuffer copy
//! and sample transforms use Halo 2's screen-effect render targets (03a's
//! blur egor, 0x0002d140). Colours follow Halo 1's 0x0006cb9d..0x0006cde8.

use super::{pass, pass_with_inputs, scope_template};
use crate::Result;
use map::TagId;
use map::build::Block;
use shaders::psh::PixelShaderDef;
use tags::scope_zoom::Profile;

const ORIGINAL: u8 = 3;
const FRAMEBUFFER: u8 = 5;
const MASK: u8 = 16;
const BLURRED: u8 = 24;
const OPAQUE: [(u8, u32); 5] = [
    (67, 0x0001_0101), // RGB write
    (59, 0),           // no blending
    (60, 0),           // no alpha test
    (143, 0),          // no depth
    (147, 0),          // no cull
];
const RESTORE: [(u8, u32); 3] = [(143, 2), (147, 0x900), (67, 0x0101_0101)];

pub fn template(passes: &[TagId]) -> Result<Block> {
    scope_template(passes)
}

/// An absolute framebuffer-coordinate copy into the window-sized target 1.
pub fn copy(vertex: TagId) -> Result<Block> {
    let mut ps = PixelShaderDef {
        combiner_count: 1,
        texture_modes: 1,
        final_inputs_abcd: 0x0000_000c, // D = r0
        ..PixelShaderDef::default()
    };
    ps.rgb_inputs[0] = 0x0820_0000; // t0 * 1
    ps.rgb_outputs[0] = 0x0000_00c0; // AB -> r0
    pass(vertex, &ps, &[(0xff, FRAMEBUFFER)], 3, 0, &OPAQUE, &RESTORE)
}

pub fn composite(vertex: TagId, profile: Profile, masked: bool) -> Result<Block> {
    let blur = profile.blur.enabled();
    let mut textures = vec![(0xff, ORIGINAL)];
    if blur {
        textures.push((0xff, BLURRED));
    }
    let scene_stages = textures.len();
    if masked {
        textures.push((0xff, MASK));
    }
    let mut pixel_bindings = Vec::new();
    if profile.desaturation.enabled() {
        pixel_bindings.push([0x40, 0, 3, 41]); // stage 1 c0.a from input 41
    }
    if profile.tint.enabled() {
        pixel_bindings.push([0xc0, 0, 3, 42]); // stage 3 c0.a from input 42
    }
    let scalars = pixel_bindings
        .iter()
        .fold(0, |mask, binding| mask | (1u64 << binding[3]));
    let mut vertex_bindings = Vec::new();
    for stage in 0..scene_stages as u16 {
        // Like copy_texaccum_to_fb: c[18+2s].x = texture width,
        // c[19+2s].y = texture height. The mask is in normalized UVs.
        for row in [0x01d0, 0x01f1] {
            let [lo, hi] = (row + 2 * stage + ((stage + 1) << 13)).to_le_bytes();
            vertex_bindings.push([lo, hi, 0, 0xff]);
        }
    }
    let ps = composite_shader(profile, masked);
    pass_with_inputs(
        vertex,
        &ps,
        &textures,
        3,
        0,
        &OPAQUE,
        &RESTORE,
        &pixel_bindings,
        &vertex_bindings,
        scalars,
    )
}

fn composite_shader(profile: Profile, masked: bool) -> PixelShaderDef {
    let blur = profile.blur.enabled();
    let scene_stages = if blur { 2 } else { 1 };
    let mask = 8 + scene_stages;
    let mut ps = PixelShaderDef {
        texture_modes: [0, 1, 0x21, 0x421][scene_stages + masked as usize],
        combiner_count: 0x11004, // four stages, each with its own constants
        final_inputs_abcd: 0x0000_000c,
        ..PixelShaderDef::default()
    };
    let inputs = u32::from_be_bytes;
    if blur && masked {
        // Blend the blurred image over the untouched scene by mask alpha.
        ps.rgb_inputs[0] = inputs([9, 0x10 | mask as u8, 8, 0x30 | mask as u8]);
        ps.rgb_outputs[0] = 0x0c00; // sum -> r0
    } else {
        ps.rgb_inputs[0] = inputs([if blur { 9 } else { 8 }, 0x20, 0, 0]);
        ps.rgb_outputs[0] = 0x00c0; // AB -> r0
    }

    if !profile.desaturation.enabled() && !profile.tint.enabled() {
        ps.combiner_count = 1;
        return ps;
    }

    // One-third of each channel, as in Halo 1's desaturation combiner.
    ps.constant0[1] = 0x0055_5555;
    ps.rgb_inputs[1] = inputs([0x0c, 1, 0, 0]);
    ps.rgb_outputs[1] = 0x20d0; // dot(AB) -> r1
    let coverage = |flag: bool| {
        if masked && flag {
            0x60 | mask as u8
        } else {
            0x20
        }
    };

    ps.alpha_inputs[1] = inputs([0x11, coverage(profile.desaturation.flags & 4 != 0), 0, 0]);
    ps.alpha_outputs[1] = 0x00c0; // desaturation factor -> r0.a
    ps.rgb_inputs[2] = inputs([0x0d, 0x1c, 0x0c, 0x3c]);
    ps.rgb_outputs[2] = 0x0c00; // lerp original to grayscale

    if profile.tint.enabled() {
        ps.alpha_inputs[3] = inputs([0x11, coverage(profile.tint.flags & 8 != 0), 0, 0]);
        ps.alpha_outputs[3] = 0x00d0; // tint factor -> r1.a
        let [r, g, b] = profile
            .tint_rgb
            .map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
        ps.final_constant0 = inputs([0, r, g, b]);
        ps.final_inputs_efg = inputs([1, 0x0d, 0, 0]); // EF = tint.rgb * grayscale
        ps.final_inputs_abcd = if profile.tint.flags & 4 != 0 {
            inputs([0x1d, 0x0f, 0, 0x0c]) // add tint
        } else {
            inputs([0x1d, 0x0f, 0x0c, 0]) // blend to tint
        };
    }
    ps
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaders::psh::{PixelOptions, TextureDim, interp};
    use tags::scope_zoom::{Blur, Filter};

    struct Textures([[f32; 4]; 3]);

    impl interp::Sampler for Textures {
        fn sample(&self, stage: usize, _: TextureDim, _: [f32; 3]) -> [f32; 4] {
            self.0[stage]
        }
    }

    fn sample(
        profile: Profile,
        has_mask: bool,
        textures: [[f32; 4]; 3],
        desat: f32,
        tint: f32,
    ) -> [f32; 3] {
        let mut uniforms = interp::Uniforms::default();
        uniforms.c0[1] = [1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0, desat];
        uniforms.c0[3][3] = tint;
        let [r, g, b] = profile.tint_rgb;
        uniforms.c0[8] = [r, g, b, 0.0];
        let [r, g, b, _] = interp::run(
            &composite_shader(profile, has_mask),
            &PixelOptions::default(),
            &interp::Inputs::default(),
            &uniforms,
            &Textures(textures),
        )
        .unwrap();
        [r, g, b]
    }

    fn near(got: [f32; 3], want: [f32; 3]) {
        for (got, want) in got.into_iter().zip(want) {
            assert!((got - want).abs() < 1.0e-5, "{got} != {want}");
        }
    }

    #[test]
    fn blur_coverage_uses_mask_alpha() {
        let profile = Profile {
            blur: Blur {
                radius: [0.0, 11.0],
                ..Blur::default()
            },
            ..Profile::default()
        };
        let textures = |alpha| {
            [
                [0.6, 0.2, 0.4, 1.0],
                [0.2, 0.6, 0.4, 1.0],
                [0.5, 0.5, 0.5, alpha],
            ]
        };
        near(
            sample(profile, true, textures(0.0), 0.0, 0.0),
            [0.6, 0.2, 0.4],
        );
        near(
            sample(profile, true, textures(0.25), 0.0, 0.0),
            [0.5, 0.3, 0.4],
        );
        near(
            sample(profile, true, textures(1.0), 0.0, 0.0),
            [0.2, 0.6, 0.4],
        );
    }

    #[test]
    fn night_vision_desaturates_then_adds_green_through_the_scope() {
        let profile = Profile {
            desaturation: Filter {
                flags: 3,
                channel: 1,
                strength: 1.0,
            },
            tint: Filter {
                flags: 15,
                channel: 1,
                strength: 1.0,
            },
            tint_rgb: [0.0, 1.0, 0.0],
            ..Profile::default()
        };
        let textures = |luminance| {
            [
                [0.6, 0.3, 0.0, 1.0],
                [luminance, luminance, luminance, 1.0],
                [0.0; 4],
            ]
        };
        near(
            sample(profile, true, textures(0.0), 0.0, 0.0),
            [0.6, 0.3, 0.0],
        );
        near(
            sample(profile, true, textures(0.0), 1.0, 1.0),
            [0.3, 0.6, 0.3],
        );
        near(
            sample(profile, true, textures(0.5), 1.0, 1.0),
            [0.3, 0.3, 0.3],
        );
        near(
            sample(profile, true, textures(0.25), 0.0, 1.0),
            [0.6, 0.45, 0.0],
        );

        let profile = Profile {
            tint: Filter {
                flags: 0,
                ..profile.tint
            },
            ..profile
        };
        near(
            sample(profile, false, textures(0.0), 1.0, 1.0),
            [0.0, 0.3, 0.0],
        );
    }
}
