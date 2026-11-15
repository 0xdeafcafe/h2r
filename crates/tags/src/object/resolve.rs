//! Object tag -> `hlmt` -> render/collision/physics/animation tags, and the
//! default variant's region and permutation choices.
use super::{Context, DecodedTag, ObjectHeader, Record, Value, is_object_group};
use crate::wire::{StringId, TagReference};
use crate::{Error, Result};
use map::{FourCC, TagId};

/// A resolved reference: the tag it names, if any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Linked {
    pub id: TagId,
    pub group: FourCC,
    pub name: Option<String>,
}

/// Which variant an object starts with, following the engine's lookup
/// `0x0016d180`: a zero name selects variant 0 when the model has any;
/// otherwise the first variant whose name matches exactly, else none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VariantChoice {
    /// `default_variant` is zero; the engine takes variant 0.
    First {
        index: usize,
        name: Option<String>,
    },
    Named {
        index: usize,
        name: Option<String>,
    },
    /// Named but not present: the engine's lookup returns -1.
    Missing(StringId),
    /// The model has no variants.
    None,
}
impl VariantChoice {
    pub fn index(&self) -> Option<usize> {
        match self {
            Self::First { index, .. } | Self::Named { index, .. } => Some(*index),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PermutationChoice {
    pub name: Option<String>,
    pub probability: f32,
    /// Index into the hlmt region's permutation list, when the name is there.
    pub model_permutation: Option<usize>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct RegionChoice {
    pub region: Option<String>,
    /// Index into hlmt regions (`+0x70`).
    pub model_region: i16,
    pub permutations: Vec<PermutationChoice>,
}

#[derive(Clone, Debug)]
pub struct ResolvedModel {
    pub id: TagId,
    pub name: Option<String>,
    pub tag: DecodedTag,
    pub render_model: Option<Linked>,
    pub collision_model: Option<Linked>,
    pub physics_model: Option<Linked>,
    pub animation: Option<Linked>,
    pub variant: VariantChoice,
    /// The chosen variant's regions, in its own order.
    pub regions: Vec<RegionChoice>,
    pub variant_count: usize,
    pub region_count: usize,
    pub node_count: u32,
}

#[derive(Clone, Debug)]
pub struct ResolvedObject {
    pub tag: DecodedTag,
    pub header: ObjectHeader,
    pub model: Option<ResolvedModel>,
}

impl Context {
    fn link(&self, r: Option<TagReference>) -> Option<Linked> {
        let r = r?;
        if r.id == TagId::NULL {
            return None;
        }
        let t = self.catalog.get(r.id)?;
        Some(Linked {
            id: r.id,
            group: t.group,
            name: t.name.clone(),
        })
    }

    /// Decode an object tag and follow its model chain.
    pub fn resolve_object(&self, id: TagId) -> Result<ResolvedObject> {
        let tag = self.decode(id)?;
        if !is_object_group(tag.group) {
            return Err(Error::UnsupportedGroup(tag.group));
        }
        let header = ObjectHeader::from_record(&tag.layers[0])?;
        let model = if header.model.id == TagId::NULL {
            None
        } else {
            Some(self.resolve_model(header.model.id, header.default_variant)?)
        };
        Ok(ResolvedObject { tag, header, model })
    }

    /// Decode an `hlmt` and select `variant` as the engine would.
    pub fn resolve_model(&self, id: TagId, variant: StringId) -> Result<ResolvedModel> {
        let tag = self.decode(id)?;
        if tag.group != FourCC(*b"hlmt") {
            return Err(Error::UnsupportedGroup(tag.group));
        }
        let root = &tag.layers[0];
        check_model(root)?;
        let variants = root.elements("variants");
        let choice = if variants.is_empty() {
            VariantChoice::None
        } else if variant == StringId(0) {
            VariantChoice::First {
                index: 0,
                name: variants[0].sid("name").and_then(|s| s.1.map(str::to_owned)),
            }
        } else {
            match variants
                .iter()
                .position(|v| v.sid("name").map(|s| s.0) == Some(variant))
            {
                Some(index) => VariantChoice::Named {
                    index,
                    name: variants[index]
                        .sid("name")
                        .and_then(|s| s.1.map(str::to_owned)),
                },
                None => VariantChoice::Missing(variant),
            }
        };
        let model_regions = root.elements("regions");
        let regions = choice
            .index()
            .map(|i| {
                variants[i]
                    .elements("regions")
                    .iter()
                    .map(|vr| region_choice(vr, model_regions))
                    .collect()
            })
            .unwrap_or_default();
        Ok(ResolvedModel {
            id,
            name: tag.name.clone(),
            render_model: self.link(root.tag_ref("render_model")),
            collision_model: self.link(root.tag_ref("collision_model")),
            physics_model: self.link(root.tag_ref("physics_model")),
            animation: self.link(root.tag_ref("animation")),
            variant: choice,
            regions,
            variant_count: variants.len(),
            region_count: model_regions.len(),
            node_count: root.block("nodes").map_or(0, |b| b.count),
            tag,
        })
    }
}

fn i16_of(r: &Record, n: &str) -> i16 {
    match r.get(n) {
        Some(Value::I16(v)) => *v,
        Some(Value::U8(v)) => i16::from(*v),
        _ => -1,
    }
}
fn name_of(r: &Record, n: &str) -> Option<StringId> {
    r.sid(n).map(|s| s.0)
}

fn region_choice(vr: &Record, model_regions: &[Record]) -> RegionChoice {
    let model_region = i16_of(vr, "model_region");
    let model_perms = usize::try_from(model_region)
        .ok()
        .and_then(|i| model_regions.get(i))
        .map(|r| r.elements("permutations"))
        .unwrap_or(&[]);
    RegionChoice {
        region: vr.sid("region").and_then(|s| s.1.map(str::to_owned)),
        model_region,
        permutations: vr
            .elements("permutations")
            .iter()
            .map(|p| PermutationChoice {
                name: p.sid("permutation").and_then(|s| s.1.map(str::to_owned)),
                probability: match p.get("probability") {
                    Some(Value::F32(v)) => *v,
                    _ => 0.0,
                },
                model_permutation: model_perms
                    .iter()
                    .position(|mp| name_of(mp, "name") == name_of(p, "permutation")),
            })
            .collect(),
    }
}

/// Cross-block consistency of an `hlmt` (V on every disc model):
/// each variant region names the hlmt region at its `model_region` index,
/// and the variant's region map sends that region back to it.
pub fn check_model(root: &Record) -> Result<()> {
    let regions = root.elements("regions");
    for v in root.elements("variants") {
        let map = match v.get("region_map") {
            Some(Value::Bytes(b)) => b.clone(),
            _ => return Err(Error::Invalid("variant without region map")),
        };
        let vregions = v.elements("regions");
        for (vi, vr) in vregions.iter().enumerate() {
            let mi = i16_of(vr, "model_region");
            let mr = usize::try_from(mi)
                .ok()
                .and_then(|i| regions.get(i))
                .ok_or(Error::Invalid("variant region index outside hlmt regions"))?;
            if name_of(mr, "name") != name_of(vr, "region") {
                return Err(Error::Invalid(
                    "variant region name differs from its hlmt region",
                ));
            }
            if map.get(mi as usize).map(|b| *b as i8 as isize) != Some(vi as isize) {
                return Err(Error::Invalid(
                    "variant region map does not point back at the region",
                ));
            }
        }
        for (i, b) in map.iter().enumerate() {
            let b = *b as i8;
            if b != -1 && (i >= regions.len() || b as usize >= vregions.len()) {
                return Err(Error::Invalid("variant region map entry out of range"));
            }
        }
    }
    Ok(())
}
