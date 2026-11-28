//! The sky strip's material constants from the plan: the scenario sky's
//! shader resolved at the load (`tags::material::Material`), its layer-15
//! pass evaluated at the render clock (`layer_pass::sample_constants`),
//! and compared with what the guest bound at the strip's issue
//! (docs/graphics/retire-d3d-r5.md, "The sky from the plan").
//!
//! The guest's sky renderer (`0x0004c690`) draws layer 15, implementation
//! zero, of the section part's shader (`scene_extract::layout::sky`). The
//! registers a pass writes from its instance (`Origin::Instance`) and the
//! combiner constants it writes (`c0_written`, `c1_written`) are the
//! material's; the engine's vertex registers and the view's rows stay the
//! draw's.

use tags::material::{Material, Origin, Register, ResolvedImplementation};
use tags::shader::AnimatedProperty;

/// The sky model's shader layer.
pub const SKY_LAYER: usize = 15;

/// A pixel shader definition's combiner constants by dword: stage `n`'s
/// C0 at `10 + n`, C1 at `18 + n`, the final combiner's at 43 and 44. The
/// device's combiner state holds the same dwords in the same slots.
const C0: usize = 10;
const C1: usize = 18;
const FINAL_C0: usize = 43;
const FINAL_C1: usize = 44;

/// The dword of combiner constant `bank` (0 or 1) of stage `n` (8 the
/// final combiner).
fn constant_word(bank: u8, n: usize) -> usize {
    match (bank, n) {
        (0, 8) => FINAL_C0,
        (1, 8) => FINAL_C1,
        (0, n) => C0 + n,
        (_, n) => C1 + n,
    }
}

/// The material's constants of one sky pass, evaluated.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SkyConstants {
    /// The instance's vertex registers, in hardware order, with the
    /// components they write.
    pub vertex: Vec<Register>,
    /// The combiner constants written, `(bank, stage, word)`: bank 0 C0, 1
    /// C1; stage 8 the final combiner.
    pub pixel: Vec<(u8, u8, u32)>,
}

/// The first register the evaluated constants and the bound ones differ
/// in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Difference {
    /// A vertex register, hardware numbering.
    Vertex(u8),
    /// A combiner constant: bank (0 C0, 1 C1) and stage (8 the final).
    Pixel { bank: u8, stage: u8 },
}

impl SkyConstants {
    /// The constants one resolved (or sampled) implementation writes.
    pub fn of(imp: &ResolvedImplementation) -> Self {
        let words: Vec<u32> = imp.pixel_shader.chunks_exact(4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]])).collect();
        let mut pixel = Vec::new();
        for (bank, written) in [(0u8, imp.c0_written), (1u8, imp.c1_written)] {
            for stage in 0..9u8 {
                if written & (1 << stage) != 0 {
                    pixel.push((bank, stage, words[constant_word(bank, usize::from(stage))]));
                }
            }
        }
        let vertex = imp.vertex_constants.iter().filter(|r| r.origin == Origin::Instance && r.written != 0).copied().collect();
        Self { vertex, pixel }
    }

    /// The first register in which the bound constants (`vertex` in
    /// hardware order, `combiner` the device's combiner state dwords)
    /// differ from these, compared bit for bit. `None` when every register
    /// the material writes holds its value.
    pub fn difference(&self, vertex: &[[f32; 4]], combiner: &[u32]) -> Option<Difference> {
        for r in &self.vertex {
            let Some(bound) = vertex.get(usize::from(r.index)) else {
                return Some(Difference::Vertex(r.index));
            };
            if (0..4).any(|k| r.written & (1 << k) != 0 && bound[k].to_bits() != r.value[k].to_bits()) {
                return Some(Difference::Vertex(r.index));
            }
        }
        for &(bank, stage, word) in &self.pixel {
            if combiner.get(constant_word(bank, usize::from(stage))) != Some(&word) {
                return Some(Difference::Pixel { bank, stage });
            }
        }
        None
    }

    /// Writes the evaluated vertex registers over `vertex` (hardware
    /// order) and the combiner constants over `c0` and `c1` (as
    /// `shaders::psh::PixelUniforms` holds them).
    pub fn apply(&self, vertex: &mut [[f32; 4]], c0: &mut [[f32; 4]; 9], c1: &mut [[f32; 4]; 9]) {
        for r in &self.vertex {
            crate::program::put(vertex, r);
        }
        for &(bank, stage, word) in &self.pixel {
            let bank = if bank == 0 { &mut *c0 } else { &mut *c1 };
            bank[usize::from(stage)] = shaders::psh::argb(word);
        }
    }
}

/// Pass `pass` of `material`'s layer 15 at the render clock `time`,
/// implementation zero: its constants, or why not (the strip's
/// `scene_kept_sky_<cause>`): `layer` (no layer-15 pass list at lod
/// 0), `pass` (no such pass or implementation), `animation` (an
/// animated register the sampler refuses).
pub fn evaluate(
    material: &Material, properties: &[AnimatedProperty], pass: u32, time: Option<f64>,
) -> Result<SkyConstants, &'static str> {
    let layer = material.layers.iter().find(|l| l.lod == 0 && l.layer == SKY_LAYER).ok_or("layer")?;
    let index = usize::try_from(pass).map_err(|_| "pass")?;
    let imp = layer.passes.get(index).and_then(|p| p.implementations.first()).ok_or("pass")?;
    if imp.animated.is_empty() {
        return Ok(SkyConstants::of(imp));
    }
    let sampled = crate::layer_pass::sample_constants(imp, properties, time, false, index).map_err(|_| "animation")?;
    Ok(SkyConstants::of(&sampled))
}

/// A sky pass's program and textures from the material, as the load made
/// them (`native_sky_glue::load`, through `decorators_load::decorator_look`
/// at layer 15): the classification-1 program's instruction words and the
/// stages the pass samples, each a bitmap image the load decoded.
#[derive(Clone, Debug, PartialEq)]
pub struct SkyLook {
    pub words: Vec<u32>,
    pub stages: [crate::look::Stage; 4],
}

impl SkyLook {
    /// `None` when the program does not parse.
    pub fn new(microcode: &[u8], stages: [crate::look::Stage; 4]) -> Option<Self> {
        let program = shaders::vsh::Program::parse(microcode).ok()?;
        Some(Self { words: program.instructions.iter().flat_map(|i| i.words).collect(), stages })
    }
}

/// A sky model section's geometry as the load read it from the map
/// (`tags::model::RenderModelGeometry`, the section's raw block): each
/// vertex stream's stride and bytes, in descriptor order, and each part's
/// index run with its vertex window cut at the load, every window and
/// index list a snapshot of its own.
#[derive(Clone, Debug, Default)]
pub struct SkySection {
    pub streams: Vec<(u32, Vec<u8>)>,
    pub parts: Vec<SkyPart>,
}

/// One part of a [`SkySection`]: its index run.
#[derive(Clone, Debug)]
pub struct SkyPart {
    pub run: Vec<u16>,
}

impl SkySection {
    pub fn of(section: &tags::geometry::Section) -> Self {
        let streams = section.vertex_buffers.iter().map(|v| (u32::from(v.stride), section.data(v).to_vec())).collect();
        let runs = section.parts.iter().map(|p| {
            let first = usize::from(p.first_index);
            section.indices.get(first..first + usize::from(p.index_count)).map_or_else(Vec::new, <[u16]>::to_vec)
        }).collect();
        Self::new(streams, runs)
    }

    /// The section of these streams (stride, bytes) and part index runs.
    pub fn new(streams: Vec<(u32, Vec<u8>)>, runs: Vec<Vec<u16>>) -> Self {
        let parts = runs.into_iter().map(|run| SkyPart { run }).collect();
        Self { streams, parts }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer_pass::tests::{animated, identity, implementation, material, own_inputs, timed};
    use tags::material::{AnimationKind, InputReads};

    /// A sky pass writing instance register 20, stage 1's C0 and the final
    /// C1; register 18 is the engine's.
    fn sky_pass() -> ResolvedImplementation {
        let mut imp = implementation(0, InputReads::default(), Vec::new());
        imp.vertex_constants.push(Register { index: 20, value: [1.0, 2.0, 0.0, 0.0], written: 0b0011, origin: Origin::Instance });
        let mut def = shaders::PixelShaderDef::default();
        def.constant0[1] = 0xff10_2030;
        def.final_constant1 = 0x8000_0000;
        imp.pixel_shader = def.to_bytes();
        imp.c0_written = 1 << 1;
        imp.c1_written = 1 << 8;
        imp
    }

    fn bound(constants: &SkyConstants) -> (Vec<[f32; 4]>, Vec<u32>) {
        let mut vertex = vec![[9.0; 4]; 192];
        let (mut c0, mut c1) = ([[0.0; 4]; 9], [[0.0; 4]; 9]);
        constants.apply(&mut vertex, &mut c0, &mut c1);
        let mut combiner = vec![0u32; 57];
        combiner[C0 + 1] = 0xff10_2030;
        combiner[FINAL_C1] = 0x8000_0000;
        (vertex, combiner)
    }

    #[test]
    fn the_evaluated_constants_match_what_the_guest_bound_and_each_difference_is_named() {
        let m = material(SKY_LAYER, vec![vec![sky_pass()]], own_inputs());
        let constants = evaluate(&m, &[], 0, Some(1.0)).unwrap();
        assert_eq!(constants.vertex.iter().map(|r| r.index).collect::<Vec<_>>(), vec![20], "the engine's register 18 is the draw's");
        assert_eq!(constants.pixel, vec![(0, 1, 0xff10_2030), (1, 8, 0x8000_0000)]);
        let (mut vertex, mut combiner) = bound(&constants);
        assert_eq!(vertex[20], [1.0, 2.0, 9.0, 9.0], "only the written components");
        assert_eq!(constants.difference(&vertex, &combiner), None);
        vertex[18] = [5.0; 4];
        vertex[20][2] = 7.0;
        assert_eq!(constants.difference(&vertex, &combiner), None, "unwritten components and engine registers are not compared");
        combiner[FINAL_C1] = 0x8000_0001;
        assert_eq!(constants.difference(&vertex, &combiner), Some(Difference::Pixel { bank: 1, stage: 8 }));
        vertex[20][1] = 2.5;
        assert_eq!(constants.difference(&vertex, &combiner), Some(Difference::Vertex(20)));
        assert_eq!(constants.difference(&vertex[..20], &combiner), Some(Difference::Vertex(20)), "a register the draw lacks");
    }

    #[test]
    fn an_animated_sky_pass_is_sampled_at_the_render_clock() {
        let mut slot = animated(AnimationKind::Bitmap, &[(0, 3), (1, 6)]);
        slot.binding.target = 3;
        let mut vertex = animated(AnimationKind::VertexConstant, &[(9, 0)]);
        let word: u16 = 4 | 16 | (20 << 5) | (3 << 10);
        [vertex.binding.target, vertex.binding.b1] = word.to_le_bytes();
        let props = [timed(2., identity([3., 5.], 0)), timed(2., identity([8., 12.], 0))];
        let m = material(SKY_LAYER, vec![vec![implementation(0, InputReads::default(), vec![slot, vertex])]], own_inputs());
        let constants = evaluate(&m, &props, 0, Some(1.0)).unwrap();
        let r = constants.vertex.iter().find(|r| r.index == 22).unwrap();
        assert_eq!((r.value, r.written), ([0., 0., 4., 10.], 15));
        assert_eq!(evaluate(&m, &props, 0, None), Err("animation"), "no clock");
    }

    #[test]
    fn a_sky_without_the_pass_names_what_is_missing() {
        let m = material(SKY_LAYER, vec![vec![sky_pass()]], own_inputs());
        assert_eq!(evaluate(&m, &[], 1, Some(0.0)), Err("pass"));
        let other = material(3, vec![vec![sky_pass()]], own_inputs());
        assert_eq!(evaluate(&other, &[], 0, Some(0.0)), Err("layer"));
        let empty = material(SKY_LAYER, vec![Vec::new()], own_inputs());
        assert_eq!(evaluate(&empty, &[], 0, Some(0.0)), Err("pass"));
    }
}
