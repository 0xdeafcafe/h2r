//! Colour layer for maps without the native two-input sky template.

use map::build::{Block, TagPayload, TagSet};
use map::{FourCC, TagId};
use shaders::psh::PixelShaderDef;
use tags::shader::{
    ImplementationBindings, IndexRange, ParameterBinding, TagRef, TemplateLod, TemplatePass,
    TemplatePostprocess,
};

use super::{SPAS, pass_with_inputs, stem};
use crate::{Error, Result};

const PATH: &str = "h1\\sky\\two_alpha_clouds";
const VERTEX: &str = "rasterizer\\vertex_shaders_xbox\\transparent_generic_reflection";

pub(in crate::shad) fn install(set: &mut TagSet) -> Result<TagId> {
    let template = FourCC(*b"stem");
    if let Some(id) = set.find(template, PATH) {
        return Ok(id);
    }
    let vertex = set
        .find(FourCC(*b"vrtx"), VERTEX)
        .ok_or(Error::Invalid("the base lacks the cloud vertex program"))?;
    let root = cloud_pass(vertex)?;
    set.add_group(SPAS, None, None);
    let pass = set.add_tag(SPAS, PATH, TagPayload::Built(root))?;
    let root = cloud_template(pass)?;
    set.add_group(template, None, None);
    Ok(set.add_tag(template, PATH, TagPayload::Built(root))?)
}

/// Both `clouds_simple_transparent` passes on Coagulation and Beaver Creek
/// multiply mask (stage 1) and tile (stage 2) in RGB and alpha.
fn pixel() -> PixelShaderDef {
    let mut p = PixelShaderDef {
        final_inputs_abcd: 0x090a_0000,
        final_inputs_efg: 0x0000_1c80,
        combiner_count: 0x0001_1101,
        texture_modes: 0x420,
        ..PixelShaderDef::default()
    };
    p.alpha_inputs[0] = 0x191a_0000;
    p.alpha_outputs[0] = 0xc0;
    p
}

fn cloud_pass(vertex: TagId) -> Result<Block> {
    // RGB writes, source alpha blend, no depth writes, no culling.
    // Stage 0 is unbound. Stages 1 and 2 wrap both authored UVs.
    pass_with_inputs(
        vertex,
        &pixel(),
        &[(0xff, 0), (0, 0), (1, 0)],
        [3, 1, 1, 1],
        2,
        &[
            (67, 0x0101_0101),
            (59, 1),
            (62, 0x302),
            (63, 0x303),
            (74, 0x8006),
            (64, 0),
            (147, 0),
        ],
        &[(147, 0x900)],
        &[],
        &[],
        0,
    )
}

fn cloud_template(pass: TagId) -> Result<Block> {
    let range = |start, count| IndexRange { start, count };
    let mut layers = vec![range(0, 0); 25];
    layers[15] = range(0, 1);
    layers[16..].fill(range(1, 0));
    let graph = TemplatePostprocess {
        lods: vec![TemplateLod {
            layers: range(0, 25),
            layer_mask: 1 << 15,
            unknown_6: 0,
        }],
        layers,
        passes: vec![TemplatePass {
            pass: TagRef {
                group: SPAS,
                id: pass,
            },
            implementations: range(0, 1),
        }],
        implementations: vec![ImplementationBindings {
            bitmaps: range(0, 2),
            pixel_constants: range(2, 0),
            vertex_constants: range(2, 4),
        }],
        // The mask uses slot 0 and c[20..22]. The tile uses slot 1 and
        // c[22..24], the same destinations as the animated cloud tree.
        bindings: [
            (0, 0, 1),
            (1, 0, 0),
            (0x05d4, 0x05, 0),
            (0x05f5, 0x05, 1),
            (0x01d2, 0, 2),
            (0x01f3, 0, 3),
        ]
        .map(|(word, b2, source)| ParameterBinding { word, b2, source })
        .to_vec(),
    };
    stem(&graph, |id| id)
}

#[cfg(test)]
mod tests {
    use shaders::psh::{PixelOptions, TextureDim, interp};

    use super::*;

    struct Cloud {
        mask: [f32; 4],
        tile: [f32; 4],
    }

    impl interp::Sampler for Cloud {
        fn sample(&self, stage: usize, _: TextureDim, _: [f32; 3]) -> [f32; 4] {
            match stage {
                1 => self.mask,
                2 => self.tile,
                _ => panic!("cloud pass sampled stage {stage}"),
            }
        }
    }

    #[test]
    fn the_shared_program_multiplies_mask_and_tile() {
        let tile = [0.8, 0.6, 0.2, 0.5];
        for mask_alpha in [0.0, 0.25, 1.0] {
            let cloud = Cloud {
                mask: [1.0, 1.0, 1.0, mask_alpha],
                tile,
            };
            let color = interp::run(
                &pixel(),
                &PixelOptions::default(),
                &interp::Inputs::default(),
                &interp::Uniforms::default(),
                &cloud,
            )
            .unwrap();
            assert_eq!(color, [tile[0], tile[1], tile[2], mask_alpha * tile[3]]);
        }
    }
}
