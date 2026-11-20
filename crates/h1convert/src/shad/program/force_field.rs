//! The four scrolling samples of the Xbox charger's generic transparent shader.
use h1map::{Group, H1Map, Tag, TagRef};
use map::{TagId, build::Block};
use shaders::psh::{PixelShaderDef, bits::*};
use tags::effects::function::FunctionType;

use super::model_effects::{property, transparent_stem};
use super::{encode, pass_with_inputs};
use crate::shad::{self, AnimTree, Bound, Layout, Root};
use crate::sky_shaders::{MAPS, SLIDE, SOTR_ANIMATION, SOTR_MAP};
use crate::{Error, Result};

mod vertex;
pub use vertex::build as vertex_shader;

pub struct ForceField {
    pub bitmap: h1map::TagId,
    axes: [Axis; 8],
}

#[derive(Clone, Copy, Default)]
struct Axis {
    scale: f32,
    period: f32,
    offset: f32,
}

fn word(d: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(d[at..at + 2].try_into().unwrap())
}

fn scalar(d: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(d[at..at + 4].try_into().unwrap())
}

impl ForceField {
    pub fn read(map: &H1Map, tag: &Tag) -> Result<Option<Self>> {
        if tag.group != Group::new(b"sotr") {
            return Ok(None);
        }
        let d = map.tag_data(tag)?;
        let maps = h1map::Block::read(d, MAPS).ok_or(Error::Invalid("generic shader maps"))?;
        let stages = h1map::Block::read(d, shad::SOTR_STAGES)
            .ok_or(Error::Invalid("generic shader stages"))?;
        if maps.count != 4 || stages.count != 4 {
            return Ok(None);
        }
        // Match the authored program by its first operation. The other four-stage
        // sotr programs on the disc use different inputs.
        let stages = map.block(stages, shad::SOTR_STAGE)?;
        if [0x3c, 0x40, 0x44, 0x48].map(|at| word(stages, at)) != [5, 17, 6, 18] {
            return Ok(None);
        }
        if word(d, 0x28) != 0x400
            || word(d, 0x2a) != 0
            || word(d, 0x2c) != 3
            || word(d, 0x2e) != 2
            || word(d, 0x30) != 0
            || h1map::Block::read(d, 0x48).unwrap().count != 0
        {
            return Err(Error::Invalid(
                "unsupported force field blend, fade or layers",
            ));
        }
        // XBE 0x62d14..0x62e6e: RGB inputs and output destinations.
        for (k, stage) in stages.chunks_exact(shad::SOTR_STAGE).enumerate() {
            let inputs = [
                [5, 17, 6, 18],
                [5, 6, 11, 1],
                [13, 11, 0, 0],
                [13, 11, 0, 0],
            ][k];
            let source = [0, 0, 1, 2][k];
            if (0..4)
                .any(|i| word(stage, 0x3c + 4 * i) != inputs[i] || word(stage, 0x3e + 4 * i) != 0)
                || word(stage, 0) != 0
                || word(stage, 4) != source
                || word(stage, 6) != 0
                || (k >= 2 && (0..8).any(|i| scalar(stage, 0x0c + 4 * i) != 1.0))
                || word(stage, 0x4c) != u16::from(k >= 2)
                || word(stage, 0x54) != u16::from(k < 2)
                || [0x4e, 0x50, 0x52, 0x56]
                    .into_iter()
                    .any(|o| word(stage, o) != 0)
                || (0x58..0x70)
                    .step_by(2)
                    .any(|o| word(stage, o) != u16::from(o == 0x68))
            {
                return Err(Error::Invalid("unsupported force field combiner"));
            }
        }
        let maps = map.block(maps, SOTR_MAP.0)?;
        let image = TagRef::read(maps, SOTR_MAP.1).unwrap();
        let mut axes = [Axis::default(); 8];
        for (k, m) in maps.chunks_exact(SOTR_MAP.0).enumerate() {
            let r = TagRef::read(m, SOTR_MAP.1).unwrap();
            if r.group != Group::new(b"bitm")
                || r.id.is_none()
                || r.id != image.id
                || shad::h1_texture_kind(map, r.id) != Some(h1map::bitmap::ImageKind::D2)
                || word(m, 0) != 0
                || scalar(m, 0x14) != 0.0
                || word(m, 0x4c) != 0
                || word(m, 0x4e) != 0
                || scalar(m, 0x58).rem_euclid(360.0) != 0.0
            {
                return Err(Error::Invalid("unsupported force field texture transform"));
            }
            for axis in 0..2 {
                let a = SOTR_ANIMATION + 16 * axis;
                let function = word(m, a + 2);
                let period = scalar(m, a + 4);
                let phase_seconds = scalar(m, a + 8);
                let amplitude = scalar(m, a + 12);
                let scale = scalar(m, 4 + 4 * axis);
                let base = scalar(m, 0x0c + 4 * axis);
                if word(m, a) != 0
                    || !matches!(function, 0 | SLIDE)
                    || amplitude != 1.0
                    || ![scale, base, period, phase_seconds]
                        .iter()
                        .all(|v| v.is_finite())
                    || period == 0.0
                {
                    return Err(Error::Invalid("unsupported force field scroll"));
                }
                // 0x56ff0..0x5702c adds the phase in seconds before division.
                // A constant-one term translates by a full texture repeat.
                axes[2 * k + axis] = Axis {
                    scale,
                    period: if function == SLIDE { period } else { 0.0 },
                    offset: if function == SLIDE {
                        base + phase_seconds / period
                    } else {
                        base
                    },
                };
            }
        }
        Ok(Some(Self {
            bitmap: image.id,
            axes,
        }))
    }

    pub fn instance(&self, stem: TagId, image: Bound) -> Result<Block> {
        let mut vectors = Vec::new();
        let mut properties = Vec::new();
        for (i, axis) in self.axes.iter().enumerate() {
            let mut row = [0.0; 4];
            row[i % 2] = axis.scale;
            row[3] = axis.offset;
            vectors.push(row);
            let f = if axis.period == 0.0 {
                property(None, 1.0, FunctionType::Constant, [axis.offset; 2])
            } else {
                property(
                    None,
                    axis.period.abs(),
                    FunctionType::Transition,
                    [axis.offset, axis.offset + axis.period.signum()],
                )
            };
            properties.push(f);
        }
        let layout = Layout {
            root: Root::None,
            ..shad::ONE_ADD_ILLUM
        };
        let mut record = shad::build_record(&layout, stem, &[Some(image)], &[], &vectors, None)?;
        shad::animate_ranges(&mut record, &ANIMATION, &properties, 1)?;
        encode(shad::shad_def(), &record)
    }
}

const ANIMATION: AnimTree = AnimTree {
    layers: 25,
    animated_layer: 15,
    implementation: [(0, 0), (0, 0), (0, 0), (0, 0), (0, 8)],
    bindings: &[
        [0x70, 0, 0, 0],
        [0x71, 0, 0, 1],
        [0x72, 0, 0, 2],
        [0x73, 0, 0, 3],
        [0x74, 0, 0, 4],
        [0x75, 0, 0, 5],
        [0x76, 0, 0, 6],
        [0x77, 0, 0, 7],
    ],
};

pub fn stem(pass: TagId) -> Result<Block> {
    transparent_stem(
        pass,
        1,
        &[0x1b0, 0x1b1, 0x1b2, 0x1b3, 0x1b4, 0x1b5, 0x1b6, 0x1b7],
    )
}

fn pixel() -> PixelShaderDef {
    let mut ps = PixelShaderDef {
        combiner_count: 4 | COUNT_UNIQUE_C0,
        texture_modes: texture_modes([TEX_PROJECT2D; 4]),
        final_inputs_abcd: inputs(REG_V0 | CHAN_ALPHA, REG_R0, REG_ZERO, REG_ZERO),
        ..Default::default()
    };
    ps.rgb_inputs[0] = inputs(REG_T0, REG_T2 | CHAN_ALPHA, REG_T1, REG_T3 | CHAN_ALPHA);
    ps.rgb_inputs[1] = inputs(REG_T0, REG_T1, REG_R0, ONE);
    ps.rgb_inputs[2] = inputs(REG_C0, REG_R0, REG_ZERO, REG_ZERO);
    ps.rgb_inputs[3] = ps.rgb_inputs[2];
    ps.rgb_outputs[0] = outputs(REG_ZERO, REG_ZERO, REG_R0, OUT_IDENTITY);
    ps.rgb_outputs[1] = ps.rgb_outputs[0];
    ps.rgb_outputs[2] = outputs(REG_R0, REG_ZERO, REG_ZERO, OUT_IDENTITY);
    ps.rgb_outputs[3] = ps.rgb_outputs[2];
    ps
}

pub fn pass(vertex: TagId) -> Result<Block> {
    // H1 0x670f8: two-sided, RGB writes, ONE/ONE, read-only depth.
    // All four stages bind sampler parameter zero, with independent UVs.
    pass_with_inputs(
        vertex,
        &pixel(),
        &[(0, 0); 4],
        1,
        2,
        &[
            (67, 0x10101),
            (59, 1),
            (60, 0),
            (62, 1),
            (63, 1),
            (74, 0x8006),
            (64, 0),
            (57, 0x203),
            (147, 0),
        ],
        &[(67, 0x01010101), (147, 0x900)],
        &[[0x80, 0, 4, 3], [0xc0, 0, 4, 4]],
        &[],
        0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaders::psh::{PixelOptions, TextureDim, interp};

    struct Samples([[f32; 4]; 4]);
    impl interp::Sampler for Samples {
        fn sample(&self, stage: usize, dim: TextureDim, _: [f32; 3]) -> [f32; 4] {
            assert_eq!(dim, TextureDim::D2);
            self.0[stage]
        }
    }

    #[test]
    fn masks_two_colour_samples_then_applies_both_object_colours_and_vertex_fade() {
        let samples = Samples([
            [0.1, 0.3, 0.9, 0.0],
            [0.6, 0.2, 0.8, 0.0],
            [1.0, 0.0, 0.0, 0.2],
            [0.0, 1.0, 0.0, 0.75],
        ]);
        let inputs = interp::Inputs {
            d0: [0.0, 0.0, 0.0, 0.6],
            front: true,
            t: [[0.0, 0.0, 0.0, 1.0]; 4],
            ..Default::default()
        };
        for (a, b) in [
            ([0.0; 4], [1.0; 4]),
            ([0.5; 4], [1.0; 4]),
            ([1.0; 4], [1.0, 0.443137, 0.443137, 1.0]),
        ] {
            let mut constants = interp::Uniforms::default();
            constants.c0[2] = a;
            constants.c0[3] = b;
            let out = interp::run(
                &pixel(),
                &PixelOptions::default(),
                &inputs,
                &constants,
                &samples,
            )
            .unwrap();
            for i in 0..3 {
                let x = samples.0[0][i];
                let y = samples.0[1][i];
                let want = ((0.2 * x + 0.75 * y).min(1.0) + x * y).min(1.0) * a[i] * b[i] * 0.6;
                assert!(
                    (want - out[i]).abs() < 1e-6,
                    "channel {i}: {out:?} vs {want}"
                );
            }
        }
    }
}
