//! An object definition's render model from the map: the definition's
//! `hlmt` and default variant (`tags::object`), the `mode` geometry
//! (`tags::model`), one section per region (the variant's permutation, the
//! highest LOD) and the nodes' inverse rest frames. Read from the user's
//! own map at run time; nothing is copied.
//!
//! A model is refused (left to the D3D path) when a region's permutation is
//! chosen at run time (the variant lists more than one), when a variant
//! permutation does not name one of the model's, or when the `hlmt` and the
//! `mode` disagree on the node count (object node `i` must be mode node
//! `i`).

use std::io::{Read, Seek};

use map::{Map, TagId};
use tags::geometry::RawSource;
use tags::model::RenderModelGeometry;
use tags::object::Context;

use crate::load::{Error, PART_TRANSPARENT};
use crate::model::{Model, ModelInput, ModelPartInput, ModelSectionInput, NodeMatrix};

/// A model ready to upload, with its materials' shaders.
pub struct LoadedModel {
    pub mode: TagId,
    pub model: Model,
    /// Shader per model-local material (`mode +0x60`, stride `0x20`, shader
    /// reference at `+8`).
    pub shaders: Vec<TagId>,
}

/// A render-model region as the pick sees it: its name and, per
/// permutation, the name and the highest-LOD section.
pub struct RegionSections<'a> {
    pub name: Option<&'a str>,
    pub permutations: Vec<(Option<&'a str>, Option<usize>)>,
}

/// A variant region: its name and the permutation names it may pick.
pub struct VariantRegion<'a> {
    pub name: Option<&'a str>,
    pub permutations: Vec<Option<&'a str>>,
}

/// The section to draw per region: the variant's one permutation when it
/// lists the region, else the region's first. `Err` when a listed region
/// has several permutations (chosen at run time) or names a permutation the
/// render model lacks.
pub fn pick_sections(regions: &[RegionSections<'_>], variant: &[VariantRegion<'_>]) -> Result<Vec<usize>, String> {
    let mut out = Vec::new();
    for r in regions {
        let chosen = match variant.iter().find(|v| v.name.is_some() && v.name == r.name) {
            Some(v) => {
                let [p] = v.permutations.as_slice() else {
                    return Err(format!(
                        "region {} picks among {} permutations at run time",
                        r.name.unwrap_or("?"),
                        v.permutations.len()
                    ));
                };
                r.permutations
                    .iter()
                    .find(|(name, _)| name.is_some() && name == p)
                    .ok_or_else(|| format!("region {}: no permutation {}", r.name.unwrap_or("?"), p.unwrap_or("?")))?
                    .1
            }
            None => r.permutations.first().and_then(|p| p.1),
        };
        if let Some(s) = chosen
            && !out.contains(&s)
        {
            out.push(s);
        }
    }
    Ok(out)
}

/// The inverse rest frame of a mode node: the 13 words from node `+0x28`.
pub fn inverse_bind(words: &[f32]) -> Option<NodeMatrix> {
    let w: &[f32; 13] = words.get(..13)?.try_into().ok()?;
    let m = NodeMatrix::from_words(w);
    m.is_finite().then_some(m)
}

/// Loads the render model of object definition `definition`.
pub fn load_object_model<R: Read + Seek>(
    map: &mut Map<R>,
    ctx: &Context,
    raw: &mut dyn RawSource,
    definition: TagId,
) -> Result<LoadedModel, Error> {
    let object = ctx.resolve_object(definition)?;
    let resolved = object.model.ok_or("the object has no model")?;
    let mode = resolved.render_model.as_ref().ok_or("the model has no render model")?.id;
    let geo = RenderModelGeometry::read(map, mode, raw)?;
    if resolved.node_count as usize != geo.nodes.len() {
        return Err(format!("hlmt has {} nodes, mode {}", resolved.node_count, geo.nodes.len()).into());
    }
    let regions: Vec<RegionSections<'_>> = geo
        .regions
        .iter()
        .map(|r| RegionSections {
            name: r.name.as_deref(),
            permutations: r.permutations.iter().map(|p| (p.name.as_deref(), p.best_section())).collect(),
        })
        .collect();
    let variant: Vec<VariantRegion<'_>> = resolved
        .regions
        .iter()
        .map(|v| VariantRegion {
            name: v.region.as_deref(),
            permutations: v.permutations.iter().map(|p| p.name.as_deref()).collect(),
        })
        .collect();
    let sections = pick_sections(&regions, &variant)?;
    let inverse_binds = geo
        .nodes
        .iter()
        .map(|n| inverse_bind(&n.wire.unknown_028[..]))
        .collect::<Option<Vec<_>>>()
        .ok_or("a node's inverse rest frame is not finite")?;
    let mut input = ModelInput {
        inverse_binds,
        sections: Vec::new(),
    };
    for si in sections {
        let Some(s) = geo.sections.get(si) else { continue };
        let v = &s.vertices;
        input.sections.push(ModelSectionInput {
            positions: v.positions.clone(),
            uvs: v.texcoords.clone(),
            normals: v.normals.clone(),
            node_indices: v.node_indices.clone(),
            node_weights: v.node_weights.clone(),
            node_map: s.node_map.clone(),
            parts: s
                .geometry
                .parts
                .iter()
                .filter(|p| p.is_drawn())
                .map(|p| ModelPartInput {
                    material: p.material,
                    transparent: p.kind == PART_TRANSPARENT,
                    triangles: s.geometry.part_triangles(p),
                })
                .collect(),
        });
    }
    let model = Model::build(&input);
    if model.draws.is_empty() {
        return Err("nothing opaque to draw".into());
    }
    Ok(LoadedModel {
        mode,
        model,
        shaders: geo.materials.iter().map(|m| m.shader.id).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region<'a>(name: &'a str, perms: &[(&'a str, usize)]) -> RegionSections<'a> {
        RegionSections {
            name: Some(name),
            permutations: perms.iter().map(|&(n, s)| (Some(n), Some(s))).collect(),
        }
    }

    #[test]
    fn the_variant_pins_one_permutation_per_region_or_the_first_is_drawn() {
        let regions = [region("body", &[("base", 0), ("damaged", 1)]), region("head", &[("a", 2), ("b", 3)])];
        let variant = [VariantRegion {
            name: Some("body"),
            permutations: vec![Some("damaged")],
        }];
        assert_eq!(pick_sections(&regions, &variant), Ok(vec![1, 2]));
        assert_eq!(pick_sections(&regions, &[]), Ok(vec![0, 2]));
    }

    #[test]
    fn a_run_time_choice_or_a_missing_permutation_refuses_the_model() {
        let regions = [region("head", &[("a", 2), ("b", 3)])];
        let random = [VariantRegion {
            name: Some("head"),
            permutations: vec![Some("a"), Some("b")],
        }];
        assert!(pick_sections(&regions, &random).unwrap_err().contains("run time"));
        let missing = [VariantRegion {
            name: Some("head"),
            permutations: vec![Some("c")],
        }];
        assert!(pick_sections(&regions, &missing).is_err());
    }

    #[test]
    fn inverse_frames_read_from_the_nodes_words() {
        let mut w = [0f32; 14];
        w[..13].copy_from_slice(&[1.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, -1.0, 0.0, 0.0]);
        w[13] = 0.25;
        let m = inverse_bind(&w).unwrap();
        assert_eq!(m.position, [-1.0, 0.0, 0.0]);
        w[3] = f32::NAN;
        assert_eq!(inverse_bind(&w), None);
        assert_eq!(inverse_bind(&w[..12]), None);
    }
}
