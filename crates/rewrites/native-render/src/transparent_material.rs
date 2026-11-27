//! A transparent geometry pass's material state, from the tags, as the
//! guest's draw `0x0004d0b0` leaves it in the device at the indexed call
//! (docs/graphics/retire-d3d-r2.md, "R2's geometry arm").
//!
//! The handler `0x00041020` calls the build and the draw per pass of the
//! template's layer 15. The draw's shader begin (`0x0001bbf0`, material
//! apply `0x00018900`) sets, from the pass's [`ResolvedImplementation`],
//! the render states, the stage states of each bound texture, the pixel
//! shader definition and the vertex constants `c[18 + i]`. [`compare`]
//! checks a resolved draw's words against those, field by field, for the
//! game to prove the port; nothing here reads guest memory.
//!
//! What it leaves out, by name in [`Compared::skipped`]: render and stage
//! states an engine input drives (`engine_render`, `engine_stage`), and the
//! definition and constants of an implementation that reads a per-draw
//! input (`combiner`, `constants`). A bound texture is checked only for
//! being bound; the device holds a guest header, the tag a bitmap.

use std::collections::BTreeMap;

use tags::material::ResolvedImplementation;

/// The stage-state slots the tag's sampler record fills
/// (`D3D__TextureState[stage][slot]`).
mod slot {
    pub const ADDRESS_U: usize = 0;
    pub const ADDRESS_V: usize = 1;
    pub const ADDRESS_W: usize = 2;
    pub const MAG_FILTER: usize = 3;
    pub const MIN_FILTER: usize = 4;
    pub const MIP_FILTER: usize = 5;
    pub const MIP_LOD_BIAS: usize = 6;
    pub const MAX_MIP_LEVEL: usize = 7;
    pub const MAX_ANISOTROPY: usize = 8;
    pub const BORDER_COLOR: usize = 29;
}

/// Hardware slot of vertex constant register 0 (`c[-96..95]`, stored 0..191).
pub const CONSTANT_BIAS: usize = 96;

/// Words of the pixel shader definition.
pub const COMBINER_WORDS: usize = 60;

/// A draw's state as the device resolved it.
pub struct DrawWords<'a> {
    /// `raw.render`: the render words by guest slot.
    pub render: &'a [u32],
    /// `raw.stages`: the four stages' words.
    pub stages: [&'a [u32]; 4],
    /// Which stages have a texture bound.
    pub textures: [bool; 4],
    /// The 60-word pixel shader definition, when the draw has one.
    pub combiner: Option<&'a [u32; COMBINER_WORDS]>,
    /// The vertex constants, hardware order (register + 96).
    pub constants: &'a [[f32; 4]],
}

/// What [`compare`] found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Compared {
    /// Where the draw differs: `texture_<n>_unbound`, `render_<slot>`,
    /// `stage<n>_<field>`, `combiner_w<n>`, `combiner_missing`,
    /// `constant_<register>`.
    pub fields: Vec<String>,
    /// What was not compared, and why it was not.
    pub skipped: Vec<&'static str>,
}

/// Checks `d` against the pass's implementation.
pub fn compare(imp: &ResolvedImplementation, d: &DrawWords<'_>) -> Compared {
    let mut out = Compared::default();
    for (n, texture) in imp.textures.iter().enumerate() {
        if texture.is_some() && !d.textures[n] {
            out.fields.push(format!("texture_{n}_unbound"));
        }
    }
    if imp.engine_render_states.is_empty() {
        // A state set twice keeps the later value.
        let last: BTreeMap<u8, u32> = imp.render_states.iter().map(|s| (s.state, s.value)).collect();
        for (state, value) in last {
            if d.render.get(usize::from(state)) != Some(&value) {
                out.fields.push(format!("render_{state:#x}"));
            }
        }
    } else {
        out.skipped.push("engine_render");
    }
    if imp.engine_stage_states.is_empty() {
        for (n, texture) in imp.textures.iter().enumerate() {
            let (Some(_), Some(s)) = (texture, imp.stage_states.get(n)) else { continue };
            let fields = [
                ("address_u", slot::ADDRESS_U, u32::from(s.address_u)),
                ("address_v", slot::ADDRESS_V, u32::from(s.address_v)),
                ("address_w", slot::ADDRESS_W, u32::from(s.address_w)),
                ("mag_filter", slot::MAG_FILTER, u32::from(s.mag_filter)),
                ("min_filter", slot::MIN_FILTER, u32::from(s.min_filter)),
                ("mip_filter", slot::MIP_FILTER, u32::from(s.mip_filter)),
                ("mip_lod_bias", slot::MIP_LOD_BIAS, s.mip_lod_bias),
                ("max_mip_level", slot::MAX_MIP_LEVEL, u32::from(s.max_mip_level)),
                ("max_anisotropy", slot::MAX_ANISOTROPY, u32::from(s.max_anisotropy)),
                ("border_color", slot::BORDER_COLOR, s.border_color),
            ];
            for (name, at, value) in fields {
                if d.stages[n].get(at) != Some(&value) {
                    out.fields.push(format!("stage{n}_{name}"));
                }
            }
        }
    } else {
        out.skipped.push("engine_stage");
    }
    if imp.per_draw_inputs || imp.pixel_constants_from_engine {
        out.skipped.push("combiner");
    } else if let Some(combiner) = d.combiner {
        for (n, word) in imp.pixel_shader.chunks_exact(4).enumerate() {
            let word = u32::from_le_bytes([word[0], word[1], word[2], word[3]]);
            if combiner.get(n) != Some(&word) {
                out.fields.push(format!("combiner_w{n}"));
            }
        }
    } else {
        out.fields.push("combiner_missing".to_owned());
    }
    if imp.per_draw_inputs {
        out.skipped.push("constants");
    } else {
        for r in &imp.vertex_constants {
            let at = CONSTANT_BIAS + usize::from(r.index);
            let held = d.constants.get(at);
            let differs = (0..4)
                .filter(|c| r.written >> c & 1 != 0)
                .any(|c| held.map(|v| v[c].to_bits()) != Some(r.value[c].to_bits()));
            if differs {
                out.fields.push(format!("constant_{}", r.index));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine_states::tests::implementation;
    use tags::shader::{ExternalBinding, RenderStateValue};

    const STAGE: [u32; 30] = {
        let mut s = [0; 30];
        s[slot::ADDRESS_U] = 1;
        s[slot::ADDRESS_V] = 1;
        s[slot::ADDRESS_W] = 1;
        s[slot::MAG_FILTER] = 2;
        s[slot::MIN_FILTER] = 2;
        s[slot::MIP_FILTER] = 2;
        s[slot::MAX_ANISOTROPY] = 1;
        s[slot::BORDER_COLOR] = 0x4400_0000;
        s
    };

    /// The implementation's definition as the device holds it.
    fn def(imp: &ResolvedImplementation) -> [u32; COMBINER_WORDS] {
        std::array::from_fn(|n| {
            let w = &imp.pixel_shader[4 * n..4 * n + 4];
            u32::from_le_bytes([w[0], w[1], w[2], w[3]])
        })
    }

    fn words() -> (Vec<u32>, Vec<[f32; 4]>, [u32; COMBINER_WORDS]) {
        let mut render = vec![0; 0x80];
        render[0x3b] = 1;
        render[0x40] = 0;
        (render, vec![[0.0; 4]; 192], def(&imp()))
    }

    fn imp() -> ResolvedImplementation {
        let states = [(0x3b, 1), (0x40, 0)].map(|(state, value)| RenderStateValue { state, value });
        implementation(states.to_vec(), 4)
    }

    fn compared(imp: &ResolvedImplementation, render: &[u32], constants: &[[f32; 4]], combiner: &[u32; COMBINER_WORDS]) -> Compared {
        compare(
            imp,
            &DrawWords {
                render,
                stages: [&STAGE; 4],
                textures: [false; 4],
                combiner: Some(combiner),
                constants,
            },
        )
    }

    #[test]
    fn a_draw_that_holds_the_implementations_state_matches() {
        let (render, constants, combiner) = words();
        let got = compared(&imp(), &render, &constants, &combiner);
        assert_eq!(got, Compared::default());
    }

    #[test]
    fn a_render_word_the_implementation_sets_differs_by_slot_and_the_later_value_wins() {
        let (mut render, constants, combiner) = words();
        render[0x40] = 1;
        assert_eq!(compared(&imp(), &render, &constants, &combiner).fields, ["render_0x40"]);
        let mut twice = imp();
        twice.render_states.insert(0, RenderStateValue { state: 0x40, value: 1 });
        assert!(compared(&twice, &words().0, &constants, &combiner).fields.is_empty(), "set to 1, then to 0");
    }

    #[test]
    fn a_vertex_constant_is_read_at_its_register_plus_the_bias_in_its_written_lanes_only() {
        let (render, mut constants, combiner) = words();
        let mut one = imp();
        one.vertex_constants[0].value = [1.0, 2.0, 3.0, 4.0];
        one.vertex_constants[0].written = 0b0101;
        constants[CONSTANT_BIAS + 18] = [1.0, 9.0, 3.0, 9.0];
        assert!(compared(&one, &render, &constants, &combiner).fields.is_empty());
        constants[CONSTANT_BIAS + 18][2] = 0.0;
        assert_eq!(compared(&one, &render, &constants, &combiner).fields, ["constant_18"]);
    }

    #[test]
    fn what_an_engine_input_drives_is_skipped_by_name() {
        let (render, constants, combiner) = words();
        let mut driven = imp();
        driven.engine_render_states = vec![ExternalBinding { bytes: [0; 4] }];
        driven.engine_stage_states = vec![ExternalBinding { bytes: [0; 4] }];
        driven.per_draw_inputs = true;
        let got = compared(&driven, &render, &constants, &combiner);
        assert!(got.fields.is_empty());
        assert_eq!(got.skipped, ["engine_render", "engine_stage", "combiner", "constants"]);
    }

    #[test]
    fn a_combiner_word_and_a_missing_definition_differ() {
        let (render, constants, mut combiner) = words();
        combiner[3] ^= 7;
        assert_eq!(compared(&imp(), &render, &constants, &combiner).fields, ["combiner_w3"]);
        let got = compare(
            &imp(),
            &DrawWords { render: &render, stages: [&STAGE; 4], textures: [false; 4], combiner: None, constants: &constants },
        );
        assert_eq!(got.fields, ["combiner_missing"]);
    }
}
