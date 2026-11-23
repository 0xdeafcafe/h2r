//! Pass availability at a chosen template LOD. The caster chooser uses
//! implementation 1 for model depth and 2 for BSP depth. The RGB receiver
//! uses implementation 3. Other layer-11 implementations write alpha or
//! sample the filtered object mask (docs/graphics/native-shadows.md).
use anyhow::{Context, Result, ensure};
use map::{FourCC, TagId};
use std::collections::HashMap;
use tags::material::ShaderLibrary;
use tags::shader::{RenderStateValue, TemplatePostprocess, TextureBinding, render_state_name};

#[derive(Clone, Copy, Default)]
struct Lod {
    model_depth: bool,
    bsp_depth: bool,
    receiver: bool,
}

#[derive(Clone, Copy, Default)]
pub(super) struct Coverage {
    model_depth: bool,
    bsp_depth: bool,
    receiver: bool,
    model_pair: bool,
    bsp_pair: bool,
    both_every_lod: bool,
}

impl Coverage {
    pub(super) fn include(&mut self, other: Self) {
        self.model_depth |= other.model_depth;
        self.bsp_depth |= other.bsp_depth;
        self.receiver |= other.receiver;
        self.model_pair |= other.model_pair;
        self.bsp_pair |= other.bsp_pair;
        self.both_every_lod |= other.both_every_lod;
    }

    fn from_lods(lods: &[Lod]) -> Self {
        Self {
            model_depth: lods.iter().any(|l| l.model_depth),
            bsp_depth: lods.iter().any(|l| l.bsp_depth),
            receiver: lods.iter().any(|l| l.receiver),
            model_pair: lods.iter().any(|l| l.model_depth && l.receiver),
            bsp_pair: lods.iter().any(|l| l.bsp_depth && l.receiver),
            both_every_lod: !lods.is_empty()
                && lods
                    .iter()
                    .all(|l| l.model_depth && l.bsp_depth && l.receiver),
        }
    }
}

pub(super) fn print(label: &str, templates: &[Coverage], total: usize) {
    let count =
        |predicate: fn(&Coverage) -> bool| templates.iter().filter(|s| predicate(s)).count();
    println!(
        "{label} shadow templates: total={total} unresolved={} model_depth={} bsp_depth={} receiver={} model_pair={} bsp_pair={} both_every_lod={}",
        total - templates.len(),
        count(|s| s.model_depth),
        count(|s| s.bsp_depth),
        count(|s| s.receiver),
        count(|s| s.model_pair),
        count(|s| s.bsp_pair),
        count(|s| s.both_every_lod),
    );
}

pub(super) fn read(
    lib: &mut ShaderLibrary,
    template: &TemplatePostprocess,
    groups: &HashMap<TagId, FourCC>,
) -> Result<Coverage> {
    let mut lods = Vec::with_capacity(template.lods.len());
    for lod in 0..template.lods.len() {
        let mut result = Lod::default();
        for layer in [10, 11] {
            let entry = &template.lods[lod];
            if entry.layer_mask & (1 << layer) == 0 {
                continue;
            }
            ensure!(
                layer < usize::from(entry.layers.count),
                "shadow layer outside LOD range"
            );
            for pass in template
                .layer_passes(lod, layer)
                .context("shadow layer pass range")?
            {
                let pp = lib.pass(pass.pass.id)?.compiled()?;
                ensure!(
                    usize::from(pass.implementations.count) == pp.implementations.len(),
                    "template and pass implementation counts differ"
                );
                for (index, implementation) in pp.implementations.iter().enumerate() {
                    if !matches!((layer, index), (10, 1 | 2) | (11, 3)) {
                        continue;
                    }
                    if implementation.vertex_shader.is_null() {
                        continue;
                    }
                    ensure!(
                        groups.get(&implementation.vertex_shader.id) == Some(&tags::shader::VRTX)
                            && implementation.vertex_shader.group == tags::shader::VRTX,
                        "shadow vertex program does not resolve"
                    );
                    let states = pp
                        .render_states
                        .get(implementation.render_states.indices())
                        .context("shadow implementation render states")?;
                    let textures = pp
                        .textures
                        .get(implementation.textures.indices())
                        .context("shadow implementation textures")?;
                    let (depth, receiver) = pass_kind(states, textures);
                    match (layer, index) {
                        (10, 1) => result.model_depth |= depth,
                        (10, 2) => result.bsp_depth |= depth,
                        (11, 3) => result.receiver |= receiver,
                        _ => {}
                    }
                }
            }
        }
        lods.push(result);
    }
    Ok(Coverage::from_lods(&lods))
}

fn pass_kind(states: &[RenderStateValue], textures: &[TextureBinding]) -> (bool, bool) {
    let state = |name| {
        states
            .iter()
            .rev()
            .find(|s| render_state_name(s.state) == Some(name))
            .map(|s| s.value)
    };
    let depth = state("ZENABLE") == Some(1) && state("ZWRITEENABLE") == Some(1);
    let receiver = textures.iter().rev().find(|t| t.stage == 0)
        .is_some_and(|t| t.slot == 0xff && t.source == 13)
        && state("COLORWRITEENABLE") == Some(0x0001_0101)
        && state("ALPHABLENDENABLE") == Some(1)
        && state("SRCBLEND") == Some(0x306) // DESTCOLOR
        && state("DESTBLEND") == Some(0)
        && state("BLENDOP") == Some(0x8006) // ADD
        && state("ZFUNC") == Some(0x202) // EQUAL
        && state("ZWRITEENABLE") == Some(0);
    (depth, receiver)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_must_coincide_at_one_lod_for_each_geometry_class() {
        let near = Lod {
            model_depth: true,
            bsp_depth: false,
            receiver: true,
        };
        let far = Lod {
            model_depth: false,
            bsp_depth: true,
            receiver: false,
        };
        let c = Coverage::from_lods(&[near, far]);
        assert!(c.model_pair && c.bsp_depth && c.receiver);
        assert!(!c.bsp_pair && !c.both_every_lod);
        assert!(!Coverage::from_lods(&[]).both_every_lod);
    }

    #[test]
    fn mask_and_alpha_only_implementations_do_not_multiply_from_depth() {
        let mut states = Vec::new();
        for (name, value) in [
            ("COLORWRITEENABLE", 0x10101),
            ("ALPHABLENDENABLE", 1),
            ("SRCBLEND", 0x306),
            ("DESTBLEND", 0),
            ("BLENDOP", 0x8006),
            ("ZFUNC", 0x202),
            ("ZWRITEENABLE", 0),
        ] {
            let state = (0..=u8::MAX)
                .find(|&i| render_state_name(i) == Some(name))
                .unwrap();
            states.push(RenderStateValue { state, value });
        }
        let mut binding = TextureBinding {
            slot: 0xff,
            source: 13,
            stage: 0,
            flags: 0,
        };
        assert_eq!(pass_kind(&states, &[binding]), (false, true));
        binding.source = 8;
        assert_eq!(pass_kind(&states, &[binding]), (false, false));
        binding.source = 13;
        states[0].value = 0x100_0000;
        assert_eq!(pass_kind(&states, &[binding]), (false, false));
    }
}
