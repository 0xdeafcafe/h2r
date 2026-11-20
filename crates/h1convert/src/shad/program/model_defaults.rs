//! Halo 1's model combiner when the base, detail and cube are unbound.
//! At 0x70433..0x70561 the default images are white, grey and black.
//! The multipurpose map (also white when absent) masks self-illumination
//! with green and object colour with blue (0x6f4df..0x6f58f).

use std::collections::HashMap;
use std::io::{Read, Seek};

use h1map::{Group, TagRef};
use map::build::Block;
use map::{Map, TagId};
use shaders::psh::PixelShaderDef;
use tags::shader::{ImplementationBindings, IndexRange, ParameterBinding, TemplatePass};

use super::{SPAS, encode, pass_with_inputs, stem};
use crate::shad::{
    self, Bm, Bound, Extras, Fixed, Layout, Library, OPAQUE_BITMAPS, Role, TEX_BUMP,
};
use crate::{Error, Result};

/// Halo 2's four-change-colour pass maps R/G/B/A onto these engine vectors.
const OBJECT_RGB: [u8; 4] = [3, 4, 21, 22];
pub const VERTEX: &str = r"rasterizer\vertex_shaders_xbox\stencil_emissive";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelDefaults {
    pub multipurpose: Option<h1map::TagId>,
    emission: [f32; 3],
    emission_slot: Option<usize>,
    diffuse_slot: Option<usize>,
}

impl ModelDefaults {
    /// A null datum must be present in the source. A failed bitmap conversion
    /// or a truncated reference does not select the rasterizer default.
    pub fn read(group: Group, data: &[u8]) -> Result<Option<Self>> {
        if group != Group::new(b"soso") {
            return Ok(None);
        }
        let map = |at| {
            TagRef::read(data, at)
                .filter(|r| {
                    r.group == Group::new(b"bitm") || (r.group == Group::NONE && r.id.is_none())
                })
                .ok_or(Error::Invalid("soso bitmap reference"))
        };
        if !map(0xa4)?.id.is_none()
            || !map(0xdc)?.id.is_none()
            || !map(shad::SOSO_CUBE)?.id.is_none()
        {
            return Ok(None);
        }
        let multipurpose = map(shad::SOSO_MULTIPURPOSE)?.id;
        let word = |at: usize| u16::from_le_bytes([data[at], data[at + 1]]);
        if !multipurpose.is_none() {
            let scalar = |at: usize| f32::from_le_bytes(data[at..at + 4].try_into().unwrap());
            // The emission pass binds the identity UV transform. H1 one
            // may translate by complete repeats or rotate by complete turns.
            let identity =
                [(0xfc, 1.0), (0x10c, 1.0), (0x11c, 360.0)]
                    .into_iter()
                    .all(|(at, wrap)| {
                        word(at) == 0
                            && match word(at + 2) {
                                0 => scalar(at + 12) % wrap == 0.0,
                                1 => true,
                                _ => false,
                            }
                    });
            if !identity
                || [0x9c, 0xa0]
                    .into_iter()
                    .any(|at| !matches!(scalar(at), 0.0 | 1.0))
            {
                return Err(Error::Invalid(
                    "unsupported model illumination mask transform",
                ));
            }
        }
        let slot = |at| shad::halo2_change_colour_slot(word(at));
        let rgb = |at: usize| -> [f32; 3] {
            std::array::from_fn(|k| {
                let o = at + 4 * k;
                f32::from_le_bytes(data[o..o + 4].try_into().unwrap())
            })
        };
        let lower = rgb(0x78);
        let upper = rgb(0x84);
        // 0x705dc..0x7064a: lower + wave(time / period + phase) * (upper - lower).
        // All Xbox campaign instances reduce exactly to a constant before
        // multiplication by the current object's colour at 0x7064f.
        let emission = match word(0x72) {
            0 => upper,
            1 => lower,
            _ if lower == upper => lower,
            _ => return Err(Error::Invalid("time-varying untextured model illumination")),
        };
        Ok(Some(Self {
            multipurpose: (!multipurpose.is_none()).then_some(multipurpose),
            emission,
            emission_slot: slot(0x70),
            diffuse_slot: slot(0x4c),
        }))
    }

    /// Layer 7 adds illumination to the white lit surface. Layer 2 runs
    /// after dynamic lights, multiplying the saturated sum by the diffuse
    /// change colour (H2 0x2c034..0x2c05b).
    pub fn passes(self, vertex: Option<TagId>) -> Result<Vec<(usize, Block)>> {
        let vertex = || vertex.ok_or(Error::Invalid("missing model illumination vertex shader"));
        let mut passes = Vec::new();
        if self.emission.iter().any(|&c| c != 0.0) {
            passes.push((7, self.pass(vertex()?, true)?));
        }
        if self.diffuse_slot.is_some() {
            passes.push((2, self.pass(vertex()?, false)?));
        }
        Ok(passes)
    }

    fn pixel(self, emission: bool) -> PixelShaderDef {
        let masked = self.multipurpose.is_some();
        let mut ps = PixelShaderDef {
            combiner_count: 0x11102,
            texture_modes: u32::from(masked),
            final_inputs_abcd: 0x0c,
            ..PixelShaderDef::default()
        };
        if emission {
            // r0 = green (dot with the green unit vector), or one.
            ps.constant0[0] = 0x0000_ff00;
            ps.rgb_inputs[0] = if masked { 0x0801_0000 } else { 0x2020_0000 };
            ps.rgb_outputs[0] = if masked { 0x20c0 } else { 0xc0 };
            // Multiply by the interpolated authored RGB and the object's
            // independent illumination change colour (or white).
            ps.constant0[1] = shad::rgb_color(self.emission);
            ps.constant1[1] = 0x00ff_ffff;
            ps.rgb_inputs[1] = 0x0102_0000;
            ps.rgb_outputs[1] = 0xd0;
            ps.final_inputs_abcd = 0x0c0d_0000;
        } else {
            // r0.a = blue (or one), then lerp(white, object RGB, blue).
            ps.alpha_inputs[0] = if masked { 0x0820_0000 } else { 0x2020_0000 };
            ps.alpha_outputs[0] = 0xc0;
            ps.rgb_inputs[1] = 0x3c20_1c01;
            ps.rgb_outputs[1] = 0xc00;
        }
        ps
    }

    fn pass(self, vertex: TagId, emission: bool) -> Result<Block> {
        let mut bindings = Vec::new();
        let slot = if emission {
            self.emission_slot
        } else {
            self.diffuse_slot
        };
        if let Some(slot) = slot {
            // Stage 1 c1 (emission) or c0 (diffuse), all three RGB lanes.
            bindings.push([0x40, if emission { 4 } else { 0 }, 4, OBJECT_RGB[slot]]);
        }
        let textures: &[(u8, u8)] = if self.multipurpose.is_some() {
            &[(0, 0)]
        } else {
            &[]
        };
        pass_with_inputs(
            vertex,
            &self.pixel(emission),
            textures,
            [1; 4],
            2,
            &[
                (67, 0x010101),
                (59, 1),
                (60, 0),
                (57, 0x202),
                (64, 0),
                (62, if emission { 1 } else { 0 }),
                (63, if emission { 1 } else { 0x300 }),
                (74, 0x8006),
            ],
            &[(57, 0x203)],
            &bindings,
            &[],
            0,
        )
    }
}

impl Library {
    pub(crate) fn load_model_lighting<R: Read + Seek>(&mut self, map: &mut Map<R>) {
        // Only a shader with extra passes requires this graph. Other model
        // materials can still use the base when its compiled graph is missing.
        self.model_lighting = self.read_model_lighting(map).ok().map(std::sync::Arc::new);
    }

    fn read_model_lighting<R: Read + Seek>(
        &self,
        map: &mut Map<R>,
    ) -> Result<tags::shader::TemplatePostprocess> {
        let id = self
            .stem(&TEX_BUMP)
            .ok_or(Error::Invalid("missing model lighting stem"))?;
        let mut lib =
            tags::material::ShaderLibrary::read(map).map_err(|e| Error::Textures(e.to_string()))?;
        let graph = lib
            .template(id)
            .and_then(|s| s.compiled())
            .map_err(|e| Error::Textures(e.to_string()))?;
        graph
            .validate()
            .map_err(|e| Error::Textures(e.to_string()))?;
        Ok(graph.clone())
    }

    pub fn model_default_stem(&self, passes: &[(usize, TagId)], masked: bool) -> Result<Block> {
        let mut graph = self
            .model_lighting
            .as_deref()
            .ok_or(Error::Invalid("missing model lighting graph"))?
            .clone();
        // The lighting, shadow and fog programs and all their bindings use
        // the tex_bump parameters. The extra pass samples bitmap 5, with
        // the identity transform of vector 1 in c[18] and c[19].
        // Child-range starts are nine bits. A range that wraps here would
        // silently bind an unrelated native pass or bitmap.
        if graph.implementations.len() >= 512
            || graph.passes.len() + passes.len() > 512
            || (masked && graph.bindings.len() + 3 > 512)
        {
            return Err(Error::Invalid("model lighting graph exceeds packed ranges"));
        }
        let bindings = graph.bindings.len() as u16;
        let imp = graph.implementations.len() as u16;
        let mut implementation = ImplementationBindings::default();
        if masked {
            graph.bindings.extend([
                ParameterBinding {
                    word: 0,
                    b2: 0,
                    source: 5,
                },
                ParameterBinding {
                    word: 0x1b0,
                    b2: 0,
                    source: 1,
                },
                ParameterBinding {
                    word: 0x1b1,
                    b2: 0,
                    source: 1,
                },
            ]);
            implementation.bitmaps = IndexRange {
                start: bindings,
                count: 1,
            };
            implementation.vertex_constants = IndexRange {
                start: bindings + 1,
                count: 2,
            };
        }
        graph.implementations.push(implementation);
        for &(layer, id) in passes {
            if layer >= 32
                || graph
                    .lods
                    .iter()
                    .any(|lod| lod.layer_mask & (1 << layer) != 0)
            {
                return Err(Error::Invalid(
                    "model lighting layer already in use or out of range",
                ));
            }
            let start = graph.passes.len() as u16;
            graph.passes.push(TemplatePass {
                pass: tags::shader::TagRef { group: SPAS, id },
                implementations: IndexRange {
                    start: imp,
                    count: 1,
                },
            });
            for lod in &mut graph.lods {
                let index = lod
                    .layers
                    .indices()
                    .nth(layer)
                    .ok_or(Error::Invalid("model lighting layer missing"))?;
                graph.layers[index] = IndexRange { start, count: 1 };
                lod.layer_mask |= 1 << layer;
            }
        }
        stem(&graph, |id| id)
    }

    pub fn model_default_instance(&self, stem: TagId, mask: Option<Bound>) -> Result<Block> {
        const MASKED: Layout = Layout {
            bitmaps: &[
                OPAQUE_BITMAPS[0],
                OPAQUE_BITMAPS[1],
                OPAQUE_BITMAPS[2],
                OPAQUE_BITMAPS[3],
                OPAQUE_BITMAPS[4],
                Bm::Role(Role::Mask, Fixed::ColorWhite),
            ],
            ..TEX_BUMP
        };
        let maps = mask
            .map(|m| (Role::Mask, m))
            .into_iter()
            .collect::<HashMap<_, _>>();
        let layout = if mask.is_some() { &MASKED } else { &TEX_BUMP };
        let rec = self.record_on_stem(layout, &maps, &Extras::default(), stem)?;
        encode(shad::shad_def(), &rec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaders::psh::{PixelOptions, TextureDim, interp};

    fn source() -> Vec<u8> {
        let mut bytes = vec![0; shad::SOSO_CUBE + 16];
        for offset in [0xa4, shad::SOSO_MULTIPURPOSE, 0xdc, shad::SOSO_CUBE] {
            bytes[offset..offset + 4].copy_from_slice(&Group::new(b"bitm").0.to_le_bytes());
            bytes[offset + 12..offset + 16].fill(0xff);
        }
        bytes
    }

    fn read(bytes: &[u8]) -> ModelDefaults {
        ModelDefaults::read(Group::new(b"soso"), bytes)
            .unwrap()
            .unwrap()
    }

    #[test]
    fn only_explicit_null_inputs_use_the_default_images() {
        let bytes = source();
        let soso = Group::new(b"soso");
        assert_eq!(read(&bytes).multipurpose, None);
        assert!(read(&bytes).passes(None).unwrap().is_empty());
        assert!(
            ModelDefaults::read(Group::new(b"schi"), &[])
                .unwrap()
                .is_none()
        );
        assert!(ModelDefaults::read(soso, &bytes[..shad::SOSO_CUBE + 12]).is_err());
        for offset in [0xa4, 0xdc, shad::SOSO_CUBE] {
            let mut authored = bytes.clone();
            authored[offset + 12..offset + 16].copy_from_slice(&123u32.to_le_bytes());
            assert!(ModelDefaults::read(soso, &authored).unwrap().is_none());
        }
        let mut masked = bytes;
        masked[0xc8..0xcc].copy_from_slice(&123u32.to_le_bytes());
        assert_eq!(read(&masked).multipurpose, Some(h1map::TagId(123)));
        masked[0x10e] = 6;
        assert!(ModelDefaults::read(soso, &masked).is_err());
    }

    #[test]
    fn illumination_endpoints_and_object_colours_are_independent() {
        let mut bytes = source();
        bytes[0x4c] = 2;
        bytes[0x70] = 1;
        bytes[0x78..0x7c].copy_from_slice(&1.0f32.to_le_bytes());
        bytes[0x88..0x8c].copy_from_slice(&0.5f32.to_le_bytes());
        let s = read(&bytes);
        assert_eq!(s.emission, [0.0, 0.5, 0.0]);
        assert_eq!((s.emission_slot, s.diffuse_slot), (Some(0), Some(1)));
        assert!(s.passes(None).is_err());
        bytes[0x72] = 1;
        assert_eq!(read(&bytes).emission, [1.0, 0.0, 0.0]);
        bytes[0x72] = 3;
        bytes.copy_within(0x78..0x84, 0x84);
        assert_eq!(read(&bytes).emission, [1.0, 0.0, 0.0]);
    }

    struct Mask;
    impl interp::Sampler for Mask {
        fn sample(&self, stage: usize, _: TextureDim, _: [f32; 3]) -> [f32; 4] {
            assert_eq!(stage, 0);
            [0.6, 0.25, 0.75, 0.9]
        }
    }

    #[test]
    fn green_masks_emission_and_blue_masks_diffuse_object_colour() {
        let emission = [1.0, 0.8, 0.2];
        let diffuse_color = [0.1, 0.4, 0.3, 1.0];
        let emission_color = [0.5, 0.7, 0.9, 1.0];
        for (mask, green, blue) in [(None, 1.0, 1.0), (Some(h1map::TagId(1)), 0.25, 0.75)] {
            let s = ModelDefaults {
                multipurpose: mask,
                emission,
                emission_slot: Some(0),
                diffuse_slot: Some(1),
            };
            for additive in [true, false] {
                let pixel = s.pixel(additive);
                let unpack = |c: u32| c.to_be_bytes().map(|b| f32::from(b) / 255.0);
                let mut u = interp::Uniforms::default();
                for (i, &constant) in pixel.constant0.iter().enumerate() {
                    let [a, r, g, b] = unpack(constant);
                    u.c0[i] = [r, g, b, a];
                }
                if additive {
                    u.c1[1] = emission_color;
                } else {
                    u.c0[1] = diffuse_color;
                }
                let result = interp::run(
                    &pixel,
                    &PixelOptions::default(),
                    &interp::Inputs::default(),
                    &u,
                    &Mask,
                )
                .unwrap();
                for k in 0..3 {
                    let expected = if additive {
                        green * emission[k] * emission_color[k]
                    } else {
                        1.0 - blue + blue * diffuse_color[k]
                    };
                    assert!(
                        (result[k] - expected).abs() < 0.0001,
                        "{mask:?} {additive}: {result:?} vs {expected}"
                    );
                }
            }
        }
    }
}
