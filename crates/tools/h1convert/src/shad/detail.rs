//! Halo 1's independent detail and micro-detail combiner operations.

use std::io::{Read, Seek};

use map::build::{TagPayload, TagSet};
use map::{FourCC, Map, TagId};
use shaders::psh::PixelShaderDef;
use tags::material::ShaderLibrary;
use tags::shader::TemplatePostprocess;

use super::{Bound, Role, program};
use crate::{Error, Result};

/// The per-shader operation, at senv +0xb0 (detail) and +0xf4 (micro).
/// The Halo 1 XBE selects these RGB input words at 0x0007785d..0x000778dd.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DetailBlend {
    #[default]
    DoubleMultiply,
    Multiply,
    DoubleBiasedAdd,
}

impl DetailBlend {
    pub(crate) fn read(shader: &[u8]) -> [Self; 2] {
        [0xb0, 0xf4].map(|at| match shader.get(at..at + 2) {
            Some([1, 0]) => Self::Multiply,
            Some([2, 0]) => Self::DoubleBiasedAdd,
            _ => Self::DoubleMultiply,
        })
    }

    /// A*B + C*D, writing the sum to r0. The source register is the
    /// detail colour and the destination colour is the surface so far.
    fn inputs(self, surface: u8, detail: u8) -> u32 {
        let one = 0x20;
        u32::from_be_bytes(match self {
            Self::DoubleMultiply => [surface, detail, surface, detail],
            Self::Multiply => [surface, detail, 0, 0],
            Self::DoubleBiasedAdd => [surface, one, detail | 0x40, one],
        })
    }
}

/// Recipes are keyed by the two operations that actually have maps. An
/// absent map has no contribution and consumes no combiner.
pub(crate) fn active(
    modes: [DetailBlend; 2],
    maps: &std::collections::HashMap<Role, Bound>,
) -> [Option<DetailBlend>; 2] {
    [
        maps.contains_key(&Role::Detail).then_some(modes[0]),
        maps.contains_key(&Role::Micro).then_some(modes[1]),
    ]
}

fn pixel(detail: Option<DetailBlend>, micro: Option<DetailBlend>) -> PixelShaderDef {
    let mut p = PixelShaderDef {
        // Final: r0 * c0 + c1 (the stem sets white and black).
        final_inputs_abcd: 0x0c01_0002,
        final_inputs_efg: 0x0000_1c80,
        combiner_count: 0x0001_1100,
        texture_modes: 0x8421, // Four projected 2D textures
        ..PixelShaderDef::default()
    };
    // Preserve the Halo 2 template's base-alpha lerp and output alpha.
    // The register for the RGB result is r0 (0x0c).
    p.alpha_inputs[0] = 0x3819_181a;
    p.alpha_outputs[0] = 0x0c00;
    p.rgb_inputs[0] = if detail.is_some() {
        0x3809_180a
    } else {
        0x0820_0000
    };
    p.rgb_outputs[0] = 0x0c00;
    let mut count = 1;
    if let Some(op) = detail {
        p.rgb_inputs[count] = op.inputs(8, 12);
        p.rgb_outputs[count] = 0x0c00;
        count += 1;
    }
    if let Some(op) = micro {
        p.rgb_inputs[count] = op.inputs(12, 11);
        p.rgb_outputs[count] = 0x0c00;
        count += 1;
    }
    p.combiner_count |= count as u32;
    p
}

/// The light, shadow and fog layer wiring the engine needs for this
/// material class. The colour pass itself is authored from Halo 1's
/// operations. The other layers use Halo 2's existing pass programs.
#[derive(Clone, Debug)]
pub(crate) struct Program {
    graph: TemplatePostprocess,
    color_pass: TagId,
    vertex_shader: TagId,
}

impl Program {
    pub(crate) fn read<R: Read + Seek>(map: &mut Map<R>, stem: TagId) -> Result<Self> {
        let mut read = || -> tags::Result<Self> {
            let mut lib = ShaderLibrary::read(map)?;
            let graph = lib.template(stem)?.compiled()?.clone();
            let color_pass = graph
                .layer_passes(0, 1)
                .filter(|p| p.len() == 1)
                .map(|p| p[0].pass.id)
                .ok_or(tags::Error::Invalid(
                    "terrain template has no single colour pass",
                ))?;
            let pass = lib.pass(color_pass)?.compiled()?;
            let [imp] = pass.implementations.as_slice() else {
                return Err(tags::Error::Invalid(
                    "terrain colour pass is not a single implementation",
                ));
            };
            Ok(Self {
                graph,
                color_pass,
                vertex_shader: imp.vertex_shader.id,
            })
        };
        read().map_err(|e| Error::Textures(format!("terrain detail program: {e}")))
    }

    pub(crate) fn install(&self, set: &mut TagSet, ops: [Option<DetailBlend>; 2]) -> Result<TagId> {
        let key = |op| match op {
            None => "none",
            Some(DetailBlend::DoubleMultiply) => "double_multiply",
            Some(DetailBlend::Multiply) => "multiply",
            Some(DetailBlend::DoubleBiasedAdd) => "double_biased_add",
        };
        let path = format!("h1\\terrain\\detail_{}_{}", key(ops[0]), key(ops[1]));
        let stem_group = FourCC(*b"stem");
        let pass_group = FourCC(*b"spas");
        if let Some(id) = set.find(stem_group, &path) {
            return Ok(id);
        }

        // The opaque terrain colour layer writes RGBA and depth.
        let pass = program::pass(
            self.vertex_shader,
            &pixel(ops[0], ops[1]),
            &[(0, 0), (1, 0), (2, 0), (3, 0)],
            1,
            2,
            &[(67, 0x0101_0101), (59, 0), (64, 1)],
            &[],
        )?;
        set.add_group(pass_group, None, None);
        let pass_id = set.add_tag(pass_group, &path, TagPayload::Built(pass))?;
        let root = program::stem(&self.graph, |id| {
            if id == self.color_pass { pass_id } else { id }
        })?;
        set.add_group(stem_group, None, None);
        Ok(set.add_tag(stem_group, &path, TagPayload::Built(root))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaders::psh::{PixelOptions, TextureDim, interp};

    struct Texels([[f32; 4]; 4]);

    impl interp::Sampler for Texels {
        fn sample(&self, stage: usize, _: TextureDim, _: [f32; 3]) -> [f32; 4] {
            self.0[stage]
        }
    }

    #[test]
    fn senv_reads_independent_detail_modes() {
        use DetailBlend::*;
        let mut bytes = [0; 0xf6];
        assert_eq!(DetailBlend::read(&bytes), [DoubleMultiply, DoubleMultiply]);
        bytes[0xb0] = 1;
        bytes[0xf4] = 2;
        assert_eq!(DetailBlend::read(&bytes), [Multiply, DoubleBiasedAdd]);
    }

    #[test]
    fn detail_program_matches_the_halo1_combiner_equations() {
        use DetailBlend::*;
        let operations = [
            None,
            Some(DoubleMultiply),
            Some(Multiply),
            Some(DoubleBiasedAdd),
        ];
        let mut uniforms = interp::Uniforms::default();
        uniforms.c0[8] = [1.0; 4];
        let apply = |op, surface: f32, detail: f32| {
            match op {
                None => surface,
                Some(DoubleMultiply) => 2.0 * surface * detail,
                Some(Multiply) => surface * detail,
                Some(DoubleBiasedAdd) => surface + 2.0 * detail - 1.0,
            }
            .clamp(0.0, 1.0)
        };
        for detail in operations {
            for micro in operations {
                let def = pixel(detail, micro);
                for alpha in [0.0, 0.35, 1.0] {
                    let base = [0.8, 0.5, 0.2, alpha];
                    let primary = [0.45, 0.95, 0.05, 0.2];
                    let secondary = [1.0, 0.35, 0.9, 0.4];
                    let fine = [0.1, 0.7, 0.9, 0.6];
                    let sampled = Texels([base, primary, secondary, fine]);
                    let result = interp::run(
                        &def,
                        &PixelOptions::default(),
                        &interp::Inputs::default(),
                        &uniforms,
                        &sampled,
                    )
                    .unwrap();
                    for channel in 0..3 {
                        let blended = primary[channel] * (1.0 - alpha) + secondary[channel] * alpha;
                        let want =
                            apply(micro, apply(detail, base[channel], blended), fine[channel]);
                        assert!(
                            (result[channel] - want).abs() < 0.002,
                            "{detail:?} {micro:?} {channel}: {result:?} vs {want}"
                        );
                    }
                }
            }
        }
    }
}
