//! Fixed-function model effects, read from the Xbox tag definitions.
//! The Chicago draw at 0x67c43 binds default image zero for a null 2D
//! stage. Plasma at 0x6d4a0 draws two independently moving volumes.

use h1map::{Group, H1Map, Tag, TagRef};
use map::TagId;
use map::build::Block;
use shaders::psh::PixelShaderDef;
use tags::defs::Val;
use tags::effects::function::{Function, FunctionType};
use tags::shader::{
    ImplementationBindings, IndexRange, ParameterBinding, TagRef as H2Ref, TemplateLod,
    TemplatePass, TemplatePostprocess,
};

use super::{SPAS, encode, pass, stem};
use crate::shad::{self, AnimTree, Bound, Layout, Property, Root};
use crate::sky_shaders::{BLEND, BLEND_ADD, MAPS, SCHI_ANIMATION, SCHI_FUNCTIONS, SCHI_MAP};
use crate::{Error, Result};

pub const PLASMA_VERTEX: &str = r"rasterizer\vertex_shaders_xbox\transparent_generic_offset";

fn word(b: &[u8], at: usize) -> Result<u16> {
    b.get(at..at + 2)
        .map(|v| u16::from_le_bytes([v[0], v[1]]))
        .ok_or(Error::Invalid("truncated shader field"))
}

fn scalar(b: &[u8], at: usize) -> Result<f32> {
    let value = b
        .get(at..at + 4)
        .and_then(|v| v.try_into().ok())
        .map(f32::from_le_bytes);
    value
        .filter(|v| v.is_finite())
        .ok_or(Error::Invalid("invalid shader scalar"))
}

fn block(b: &[u8], at: usize) -> Result<h1map::Block> {
    h1map::Block::read(b, at).ok_or(Error::Invalid("truncated shader block"))
}

/// Resolve the exact default through the cache's rasterizer data. Image
/// zero of this bitmap is opaque black on both campaign small doors.
pub fn chicago_default(map: &H1Map, tag: &Tag) -> Result<Option<h1map::TagId>> {
    if !matches!(&tag.group.0.to_be_bytes(), b"schi" | b"scex") {
        return Ok(None);
    }
    let d = map.tag_data(tag)?;
    let maps = block(d, MAPS)?;
    if maps.count != 1 {
        return Ok(None);
    }
    let stage = map.block(maps, SCHI_MAP.0)?;
    let image = TagRef::read(stage, SCHI_MAP.1).ok_or(Error::Invalid("Chicago stage bitmap"))?;
    if !image.id.is_none() {
        return Ok(None);
    }
    // With no bitmap, only this single, unmodified current/current
    // additive stage has the measured rasterizer-default behavior.
    if image.group != Group::new(b"bitm")
        || word(d, 0x28)? != 0
        || word(d, 0x2a)? != 0
        || word(d, BLEND)? != BLEND_ADD
        || word(d, 0x2e)? != 0
        || block(d, 0x48)?.count != 0
        || block(d, 0x60)?.count != 0
        || scalar(stage, SCHI_MAP.2)? != 1.0
        || scalar(stage, SCHI_MAP.2 + 4)? != 1.0
        || word(stage, 0)? != 0
        || word(stage, SCHI_FUNCTIONS)? != 0
        || word(stage, SCHI_FUNCTIONS + 2)? != 0
        || [0, 16, 32]
            .into_iter()
            .any(|i| stage.get(SCHI_ANIMATION + i..SCHI_ANIMATION + i + 4) != Some(&[0; 4][..]))
    {
        return Err(Error::Invalid("unsupported Chicago null stage"));
    }
    let globals = map
        .find(Group::new(b"matg"), r"globals\globals")
        .ok_or(Error::Invalid("missing rasterizer globals"))?;
    let rasterizer = block(map.tag_data(globals)?, 0x134)?;
    if rasterizer.count != 1 {
        return Err(Error::Invalid("missing rasterizer data"));
    }
    let data = map.read(rasterizer.address, 0xbc)?;
    let default = TagRef::read(data, 0xac)
        .filter(|r| r.group == Group::new(b"bitm") && !r.id.is_none())
        .ok_or(Error::Invalid("missing rasterizer default 2D bitmap"))?;
    Ok(Some(default.id))
}

#[derive(Clone, Copy, Debug)]
pub struct Noise {
    pub bitmap: h1map::TagId,
    scale: f32,
    period: f32,
    direction: [f32; 3],
}

#[derive(Debug)]
pub struct Plasma {
    pub noise: [Noise; 2],
    tint: [[f32; 4]; 2],
    input: Option<&'static str>,
}

impl Plasma {
    pub fn read(data: &[u8]) -> Result<Self> {
        let noise = |at| -> Result<Noise> {
            let r = TagRef::read(data, at + 20)
                .filter(|r| r.group == Group::new(b"bitm") && !r.id.is_none())
                .ok_or(Error::Invalid("missing plasma noise bitmap"))?;
            let period = scalar(data, at)?;
            if period == 0.0 {
                return Err(Error::Invalid("zero plasma noise period"));
            }
            Ok(Noise {
                bitmap: r.id,
                period,
                direction: [
                    scalar(data, at + 4)?,
                    scalar(data, at + 8)?,
                    scalar(data, at + 12)?,
                ],
                scale: scalar(data, at + 16)?,
            })
        };
        if word(data, 0x34)? != 0 || scalar(data, 0x38)? != 0.0 || word(data, 0x80)? != 0 {
            return Err(Error::Invalid(
                "unsupported plasma displacement or change colour",
            ));
        }
        let input = match word(data, 0x2c)? {
            0 => None,
            n @ 1..=4 if scalar(data, 0x30)? == 1.0 => {
                Some(["a_out", "b_out", "c_out", "d_out"][usize::from(n - 1)])
            }
            _ => return Err(Error::Invalid("unsupported plasma intensity")),
        };
        let tint = |at| -> Result<[f32; 4]> {
            Ok([
                scalar(data, at + 4)?,
                scalar(data, at + 8)?,
                scalar(data, at + 12)?,
                scalar(data, at)?,
            ])
        };
        // 0x6d867 uploads c[-83] = perpendicular - parallel and
        // c[-82] = parallel, including the brightness in W. Instructions
        // 37 and 38 of the H1 vertex program add c[-83] to dot * c[-82].
        // H2 lerps v11 to v10, so its grazing endpoint is the difference.
        let perpendicular = tint(0x60)?;
        let parallel = tint(0x70)?;
        let grazing = std::array::from_fn(|i| perpendicular[i] - parallel[i]);
        Ok(Self {
            noise: [noise(0xc0)?, noise(0x108)?],
            tint: [perpendicular, grazing],
            input,
        })
    }

    pub fn instance(&self, template: TagId, bitmaps: [Bound; 2]) -> Result<Block> {
        let layout = Layout {
            root: Root::None,
            ..shad::ONE_ADD_ILLUM
        };
        let mut vectors = Vec::new();
        let mut properties = Vec::new();
        for noise in self.noise {
            for (axis, direction) in noise.direction.into_iter().enumerate() {
                let mut row = [0.0; 4];
                row[axis] = noise.scale;
                vectors.push(row);
                // The matrix translation is time * direction / period.
                // Wrapping makes whole texture-coordinate turns equivalent.
                let speed = direction / noise.period;
                properties.push(if speed == 0.0 {
                    property(None, 1.0, FunctionType::Constant, [0.0, 0.0])
                } else {
                    property(
                        None,
                        1.0 / speed.abs(),
                        FunctionType::Transition,
                        [0.0, speed.signum()],
                    )
                });
            }
        }
        vectors.extend(self.tint);
        vectors.push([0.0; 4]); // c[26].w, displacement
        for [_, _, _, brightness] in self.tint {
            properties.push(match self.input {
                Some(input) => {
                    property(Some(input), 1.0, FunctionType::Identity, [0.0, brightness])
                }
                None => property(None, 1.0, FunctionType::Constant, [brightness; 2]),
            });
        }
        let mut record =
            shad::build_record(&layout, template, &bitmaps.map(Some), &[], &vectors, None)?;
        // The retail elite shield likewise has two sampler parameters but
        // one legacy image entry when they both draw image zero of one volume.
        if bitmaps[0].id == bitmaps[1].id {
            let Some(Val::Block(images)) = record.get_mut("bitmap_images") else {
                return Err(Error::Invalid("missing plasma image list"));
            };
            images.truncate(1);
        }
        shad::animate_ranges(&mut record, &ANIMATION, &properties, 1)?;
        encode(shad::shad_def(), &record)
    }
}

pub(super) fn property(
    input: Option<&'static str>,
    period: f32,
    kind: FunctionType,
    range: [f32; 2],
) -> Property {
    let payload: &[f32] = match kind {
        FunctionType::Transition => &[0.0, 1.0, 0.0, 1.0],
        FunctionType::Constant => &[1.0, 1.0],
        _ => &[],
    };
    Property {
        input,
        period,
        function: Function {
            kind,
            flags: 0,
            sub_types: [0; 2],
            range,
            unknown_0c: [0; 8],
            payload: payload.iter().flat_map(|v| v.to_le_bytes()).collect(),
        }
        .to_bytes(),
    }
}

// H2 0x17960: mode 3 writes only W, leaving the authored XYZ intact.
// Six translations in c[18..23], two intensity-scaled brightnesses in v10/11.
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
        [0x6a, 0, 0, 6],
        [0x6b, 0, 0, 7],
    ],
};

pub fn plasma_stem(pass: TagId) -> Result<Block> {
    transparent_stem(
        pass,
        2,
        &[0x1b0, 0x1b1, 0x1b2, 0x1b3, 0x1b4, 0x1b5, 0x1aa, 0x1ab, 0x78],
    )
}

/// A layer-15 material binding consecutive local sampler and vector parameters.
pub(super) fn transparent_stem(pass: TagId, samplers: u8, vectors: &[u16]) -> Result<Block> {
    let range = |start, count| IndexRange { start, count };
    let mut layers = vec![IndexRange::default(); 25];
    layers[15] = range(0, 1);
    let mut bindings: Vec<_> = (0..samplers)
        .map(|source| ParameterBinding {
            word: u16::from(source),
            b2: 0,
            source,
        })
        .collect();
    for (source, &word) in vectors.iter().enumerate() {
        bindings.push(ParameterBinding {
            word,
            b2: 0,
            source: source as u8,
        });
    }
    stem(
        &TemplatePostprocess {
            lods: vec![TemplateLod {
                layers: range(0, 25),
                layer_mask: 1 << 15,
                unknown_6: 0,
            }],
            layers,
            passes: vec![TemplatePass {
                pass: H2Ref {
                    group: SPAS,
                    id: pass,
                },
                implementations: range(0, 1),
            }],
            implementations: vec![ImplementationBindings {
                bitmaps: range(0, u16::from(samplers)),
                vertex_constants: range(u16::from(samplers), vectors.len() as u16),
                ..Default::default()
            }],
            bindings,
        },
        |id| id,
    )
}

fn plasma_pixel() -> PixelShaderDef {
    use shaders::psh::bits::*;

    // H1 0x6d8ed..0x6d99f. H2 exports the facing tint in v0 and fog
    // visibility in v1.a. The H1 vertex program swaps those two outputs.
    let a = REG_R0 | CHAN_ALPHA;
    let core = REG_R1 | CHAN_ALPHA;
    let mut ps = PixelShaderDef {
        texture_modes: texture_modes([TEX_PROJECT3D, TEX_PROJECT3D, TEX_NONE, TEX_NONE]),
        combiner_count: 4 | COUNT_MUX_MSB,
        final_inputs_abcd: inputs(REG_R0, REG_EF_PROD, REG_ZERO, REG_ZERO),
        final_inputs_efg: final_efg(a, a, REG_V0 | CHAN_ALPHA, 0),
        ..Default::default()
    };

    // Keep the biased luminance and alpha differences in r0.a and r0.rgb.
    ps.alpha_inputs[0] = inputs(REG_T0, ONE, REG_T1 | MAP_HALFBIAS_NEGATE, ONE);
    ps.rgb_inputs[0] = inputs(
        REG_T1 | CHAN_ALPHA,
        ONE,
        REG_T0 | CHAN_ALPHA | MAP_HALFBIAS_NEGATE,
        ONE,
    );
    ps.alpha_outputs[0] = outputs(REG_ZERO, REG_ZERO, REG_R0, OUT_IDENTITY);
    ps.rgb_outputs[0] = ps.alpha_outputs[0];

    // Select the squared difference by the high bit of r0.a, then scale by four.
    ps.alpha_inputs[1] = inputs(a, a, REG_R0, REG_R0);
    ps.alpha_outputs[1] = outputs(REG_ZERO, REG_ZERO, REG_R0, OUT_AB_CD_MUX | OUT_SHIFTLEFT_2);

    // Only the upper branch contributes a core, (2 * r0.a - 1) squared.
    let expanded = a | MAP_EXPAND_NORMAL;
    ps.alpha_inputs[2] = inputs(REG_ZERO, REG_ZERO, expanded, expanded);
    ps.alpha_outputs[2] = outputs(REG_ZERO, REG_ZERO, REG_R1, OUT_AB_CD_MUX);

    // Colour the edge with the vertex tint and add half the white core.
    // The final combiner modulates this RGB by r0.a squared.
    ps.rgb_inputs[3] = inputs(a, REG_V0, core, REG_ZERO | MAP_HALFBIAS_NEGATE);
    ps.rgb_outputs[3] = ps.rgb_outputs[0];
    ps.alpha_inputs[3] = inputs(REG_V0 | CHAN_ALPHA, REG_V1 | CHAN_ALPHA, REG_ZERO, REG_ZERO);
    ps.alpha_outputs[3] = outputs(REG_V0, REG_ZERO, REG_ZERO, OUT_IDENTITY);
    ps
}

pub fn plasma_pass(vertex: TagId) -> Result<Block> {
    pass(
        vertex,
        &plasma_pixel(),
        &[(0, 0), (1, 0)],
        1,
        2,
        &[
            (67, 0x10101),
            (59, 1),
            (62, 0x302),
            (63, 1),
            (74, 0x8006),
            (60, 0),
            (64, 0),
            (57, 0x203),
        ],
        &[(67, 0x0101_0101)],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaders::psh::{PixelOptions, TextureDim, interp};

    struct Samples([[f32; 2]; 2]);

    impl interp::Sampler for Samples {
        fn sample(&self, stage: usize, dim: TextureDim, _: [f32; 3]) -> [f32; 4] {
            assert_eq!(dim, TextureDim::D3);
            let [y, a] = self.0[stage];
            [y, y, y, a]
        }
    }

    #[test]
    fn two_noise_channels_select_the_edge_and_core_independently() {
        let tint = [0.2, 0.35, 0.45, 0.7];
        let inputs = interp::Inputs {
            d0: tint,
            d1: [0.0, 0.0, 0.0, 0.4],
            t: [[0.1, 0.2, 0.3, 1.0]; 4],
            ..Default::default()
        };
        for samples in [
            [[0.1f32, 0.8], [0.4, 0.9]],
            [[0.7, 0.9], [0.4, 0.55]],
            [[0.7, 0.4], [0.4, 0.35]],
            [[0.7, 0.1], [0.4, 0.85]],
        ] {
            let [[y0, a0], [y1, a1]] = samples;
            let y = (0.5 + y0 - y1).clamp(0.0, 1.0);
            let a = (0.5 + a1 - a0).clamp(0.0, 1.0);
            let s = (4.0 * (if y >= 0.5 { a } else { y }).powi(2)).min(1.0);
            let core = (2.0 * s - 1.0).max(0.0).powi(2);
            let result = interp::run(
                &plasma_pixel(),
                &PixelOptions::default(),
                &inputs,
                &interp::Uniforms::default(),
                &Samples(samples),
            )
            .unwrap();
            for k in 0..3 {
                let expected = (s * tint[k] + 0.5 * core).min(1.0) * s * s;
                assert!(
                    (result[k] - expected).abs() < 1e-5,
                    "{samples:?} channel {k}"
                );
            }
            assert!((result[3] - 0.28).abs() < 1e-5);
        }
    }

    #[test]
    fn a_null_chicago_stage_resolves_the_rasterizer_reference() {
        use h1map::testing::{BASE, SCNR_ID, Spec, build_with_extra_tags, group, put};

        let tag = |group, name, size| Spec {
            group,
            name,
            parent: None,
            data: Some(vec![0; size]),
        };
        let mut built = build_with_extra_tags(
            &Default::default(),
            vec![
                tag(b"schi", "door", 0x6c + SCHI_MAP.0),
                tag(b"matg", r"globals\globals", 0x148 + 0xbc),
                tag(b"bitm", "custom rasterizer default", 0x70),
            ],
        );
        let first = built.data_at.len() - 3;
        let shad = built.data_at[first].unwrap();
        let globals = built.data_at[first + 1].unwrap();
        let bitmap = SCNR_ID + first as u32 + 2;
        let addr = |at: usize| BASE + (at - h1map::HEADER_LEN) as u32;
        let b = &mut built.file;
        let stage = shad + 0x6c;
        put(b, shad + BLEND, u32::from(BLEND_ADD));
        put(b, shad + MAPS, 1);
        put(b, shad + MAPS + 4, addr(stage));
        put(b, stage + SCHI_MAP.2, 1f32.to_bits());
        put(b, stage + SCHI_MAP.2 + 4, 1f32.to_bits());
        put(b, stage + SCHI_MAP.1, group(b"bitm"));
        put(b, stage + SCHI_MAP.1 + 12, u32::MAX);
        put(b, globals + 0x134, 1);
        put(b, globals + 0x138, addr(globals + 0x148));
        put(b, globals + 0x148 + 0xac, group(b"bitm"));
        put(b, globals + 0x148 + 0xb8, bitmap);
        for (function, supported) in [(0, true), (1, false)] {
            put(b, stage + SCHI_FUNCTIONS, function);
            let map = H1Map::from_decompressed(b.clone()).unwrap();
            let source = map.find(Group::new(b"schi"), "door").unwrap();
            let resolved = chicago_default(&map, source);
            if supported {
                assert_eq!(resolved.unwrap(), Some(h1map::TagId(bitmap)));
            } else {
                assert!(resolved.is_err());
            }
        }
    }

    #[test]
    fn rejects_incomplete_and_unhandled_plasma_definitions() {
        assert!(Plasma::read(&[0; 8]).is_err());
        let mut data = [0u8; 0x130];
        for at in [0xc0, 0x108] {
            data[at..at + 4].copy_from_slice(&2f32.to_le_bytes());
            data[at + 20..at + 24].copy_from_slice(&Group::new(b"bitm").0.to_le_bytes());
            data[at + 32..at + 36].copy_from_slice(&17u32.to_le_bytes());
        }
        assert_eq!(
            Plasma::read(&data).unwrap().noise[1].bitmap,
            h1map::TagId(17)
        );
        data[0x128..0x12c].fill(0xff);
        assert!(Plasma::read(&data).is_err());
        data[0x128..0x12c].fill(0);
        data[0x80] = 1;
        assert!(Plasma::read(&data).is_err());
    }
}
