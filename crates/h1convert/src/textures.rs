//! Halo 1's own shaders on the converted structure and models: for each
//! Halo 1 shader, a Halo 2 `shad` rebuilt from its definition
//! ([`crate::shad`]) drawing Halo 1's bitmaps, converted by `h1bitmaps`
//! and added as new tags. Shared by the multiplayer converter and
//! `h1campaign` (both write the structure through
//! [`crate::structure::Structure`]), and by `h1objects` for model shaders
//! ([`ModelTemplate`]).
//!
//! **Which Halo 1 bitmaps.** `senv` (environment shaders, every structure
//! surface on the multiplayer maps but water): base map `+0x88`, primary
//! detail map `+0xb8`, secondary `+0xcc`, micro detail `+0xfc`, bump map
//! `+0x128`. D: on the 13 multiplayer maps every `senv` reference at those
//! offsets is a `bitm` named for its role (179 base maps, 16 primary
//! details, 118 micro details, 144 bump maps), and `editor_core::halo1_art`
//! draws `+0x88` as the base. `swat` (water): base map `+0x4c`. The
//! detail map is the primary one, else the micro detail.
//!
//! **The structure material.** A structure material takes its rebuilt
//! shader through a new material head: the head of the base material
//! [`crate::shaders::pick`] chose (its bytes, shader datum `+0x0c`
//! replaced), with the part kind of the rebuilt shader's stem, so
//! [`crate::structure::Templates`] carries it and `Structure` needs no
//! change.

use std::collections::HashMap;
use std::io::{Read, Seek};

use h1map::{Group, H1Map, TagRef};
use map::build::{Block, TagPayload, TagSet};
use map::{Map, TagId};
use tags::shader::SHAD;

use crate::Result;
use crate::shad;
use crate::structure::Templates;

/// Halo 1 `bitm` format number of a palettized bump map (Halo 2's too).
pub const P8_BUMP: u16 = 17;

/// A Halo 1 shader's bitmaps by role (Halo 1 tag ids).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct H1Maps {
    pub base: Option<h1map::TagId>,
    pub detail: Option<h1map::TagId>,
    pub bump: Option<h1map::TagId>,
}

/// The bitmaps of the Halo 1 shader `name` (any shader group), by role.
/// `None` for a shader this doesn't read.
pub fn h1_maps(h1: &H1Map, name: &str) -> Option<H1Maps> {
    let tag = h1
        .tags()
        .iter()
        .find(|t| t.name == name && h1_maps_of(h1, t).is_some())?;
    h1_maps_of(h1, tag)
}

/// The bitmaps of the Halo 1 shader `tag`, by role. D (every shader on
/// the 13 multiplayer maps): `senv` base `+0x88`, primary detail `+0xb8`
/// (else micro detail `+0xfc`), bump `+0x128`; `swat` base `+0x4c`;
/// `soso` (model shaders) base `+0xa4`, detail `+0xdc`; `schi` first map
/// `+0xd8`; `sotr` first map `+0x88`; `sgla` diffuse `+0xac`; `smet` map
/// `+0x4c`. `None` for any other group.
pub fn h1_maps_of(h1: &H1Map, tag: &h1map::Tag) -> Option<H1Maps> {
    let group = tag.group.to_string();
    if !matches!(
        group.as_str(),
        "senv" | "swat" | "soso" | "schi" | "sotr" | "sgla" | "smet"
    ) {
        return None;
    }
    let d = h1.tag_data(tag).ok()?;
    let at = |o: usize| -> Option<h1map::TagId> {
        let r = TagRef::read(d, o)?;
        (r.group == Group::new(b"bitm") && !r.id.is_none()).then_some(r.id)
    };
    let base = |o: usize| H1Maps {
        base: at(o),
        ..H1Maps::default()
    };
    Some(match group.as_str() {
        "senv" => H1Maps {
            base: at(0x88),
            detail: at(0xb8).or_else(|| at(0xfc)),
            bump: at(0x128),
        },
        "soso" => H1Maps {
            base: at(0xa4),
            detail: at(0xdc),
            bump: None,
        },
        "swat" | "smet" => base(0x4c),
        "schi" => base(0xd8),
        "sotr" => base(0x88),
        _ => base(0xac),
    })
}

/// How many times a Halo 1 shader tiles its detail and bump maps across
/// its base map's texture coordinates (1 where it names none).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct H1Scales {
    /// The detail map's tiling in u and v.
    pub detail: [f32; 2],
    pub bump: f32,
}

/// `soso` detail map scale (`+0xd8`, the f32 before the detail map
/// `+0xdc`) and its v multiplier (`+0xec`, after the reference). V (Blood
/// Gulch's `soso`): 16 on the Warthog's hull and chain gun and the
/// Scorpion's hull, 8 on the Ghost's hull and the Scorpion's treads, 10
/// on the cyborg's armour, 12 on the boulders; the v multiplier 1 on
/// every one read. Halo 1 tiles the detail by the scale in u and by the
/// scale times the multiplier in v.
pub const SOSO_DETAIL_SCALE: usize = 0xd8;
pub const SOSO_DETAIL_V_SCALE: usize = 0xec;

/// The tiling of the Halo 1 shader `tag`'s detail and bump maps
/// ([`scales_from`] on its bytes).
pub fn h1_scales_of(h1: &H1Map, tag: &h1map::Tag) -> H1Scales {
    match h1.tag_data(tag) {
        Ok(d) => scales_from(&tag.group.to_string(), d),
        Err(_) => H1Scales {
            detail: [1.0; 2],
            bump: 1.0,
        },
    }
}

/// The tiling of a Halo 1 shader of `group` whose bytes are `d`. `senv`:
/// each map reference's scale is the f32 before it, primary detail
/// `+0xb4`, micro detail `+0xf8`, bump `+0x128 - 4`. V (every `senv` on
/// the disc): 1 wherever the map is null; 1 to 800 where it is set (micro
/// details mostly 16 to 60, bump maps 1 to 100). The detail's is the
/// map's [`h1_maps_of`] takes. `soso`: the detail map's scale
/// ([`SOSO_DETAIL_SCALE`], v times [`SOSO_DETAIL_V_SCALE`]); before
/// converter version 84 a model's detail was tiled once over its base
/// map, so the Warthog's corroded metal (16×) lay over the hull as a
/// few big smears. The base map has none: its coordinates are the
/// surface's. Other groups (and a scale that isn't positive) are 1.
pub fn scales_from(group: &str, d: &[u8]) -> H1Scales {
    let mut out = H1Scales {
        detail: [1.0; 2],
        bump: 1.0,
    };
    let scale = |o: usize| {
        d.get(o..o + 4)
            .map(|b| f32::from_le_bytes(b.try_into().expect("4")))
            .filter(|f| f.is_finite() && *f > 0.0)
            .unwrap_or(1.0)
    };
    let set = |o: usize| TagRef::read(d, o).is_some_and(|r| !r.id.is_none());
    match group {
        "senv" => {
            let detail = if set(0xb8) {
                scale(0xb4)
            } else if set(0xfc) {
                scale(0xf8)
            } else {
                1.0
            };
            out.detail = [detail; 2];
            out.bump = if set(0x128) { scale(0x124) } else { 1.0 };
        }
        "soso" if set(0xdc) => {
            let u = scale(SOSO_DETAIL_SCALE);
            out.detail = [u, u * scale(SOSO_DETAIL_V_SCALE)];
        }
        _ => {}
    }
    out
}

/// The instance vector that scales texture stage `stage`'s coordinates in
/// the implementation `bindings` of the template `tp`: the vector bound to
/// vertex constant `c[18 + stage]` (bank 1, register `stage`) with its x
/// and y. D: on the opaque templates the base's structure shaders use,
/// `c[18 + s]` carries stage `s`'s tiling (Coagulation's `coag_ground`:
/// its base 1×, grass and pebble detail 60×; `blood_cliffs`: base and bump
/// 1×, sandstone detail 12×).
pub fn stage_vector(
    tp: &tags::shader::TemplatePostprocess,
    bindings: tags::shader::ImplementationBindings,
    stage: usize,
) -> Option<usize> {
    tp.bindings
        .get(bindings.vertex_constants.indices())?
        .iter()
        .find(|b| {
            let mode = usize::from((b.word >> 5) & 0x1f);
            usize::from(b.word & 0xf) == stage
                && (b.word >> 4) & 1 == 1
                && tags::material::VECTOR_MODE_MASKS
                    .get(mode)
                    .is_some_and(|m| m & 0b11 == 0b11)
        })
        .map(|b| usize::from(b.source))
}

/// One rebuilt shader: the layout its Halo 1 shader chose and the Halo 1
/// maps it draws.
#[derive(Debug, Clone)]
pub struct ShaderCopy {
    /// The Halo 1 shader it is for (its name is the new tag's).
    pub h1_shader: String,
    /// The base material whose head (and so placement in the structure)
    /// it takes ([`Templates::material_heads`] index).
    pub material: usize,
    /// The stem and parameter layout ([`crate::shad`]).
    pub layout: &'static shad::Layout,
    /// Values over the layout's (tints).
    pub extras: shad::Extras,
    /// (role, index into [`Retexture::bitmaps`], tiling, LOD offset).
    pub maps: Vec<(shad::Role, usize, [f32; 2], f32)>,
    /// What Halo 1 supplies, by role, as (role, Halo 1 bitmap name).
    pub swapped: Vec<(&'static str, String)>,
}

/// Halo 1 bitmaps converted, and the shaders rebuilt to draw them: made
/// by [`plan`], added to the map by [`Retexture::install`].
#[derive(Debug, Clone, Default)]
pub struct Retexture {
    /// (Halo 1 bitmap name, its conversion).
    pub bitmaps: Vec<(String, h1models::Converted)>,
    pub copies: Vec<ShaderCopy>,
    /// Shaders left as the base's, and why.
    pub notes: Vec<String>,
    /// The base's stems and default bitmaps.
    pub library: shad::Library,
}

/// A role's name in reports.
pub fn role_name(r: shad::Role) -> &'static str {
    match r {
        shad::Role::Base => "base",
        shad::Role::Detail => "detail",
        shad::Role::Secondary => "secondary detail",
        shad::Role::Micro => "micro detail",
        shad::Role::Cube => "reflection cube",
        shad::Role::Bump => "bump",
        shad::Role::Mask => "mask",
        shad::Role::Tile => "tile",
        shad::Role::ChangeColour => "change colour",
    }
}

/// Plans Halo 1's shaders for the structure materials `materials` (each:
/// the base material index [`crate::shaders::pick`] chose, whose head the
/// material keeps, and the Halo 1 shader's name), with `base`, the map
/// the set is made from: each Halo 1 shader is rebuilt
/// ([`crate::shad::read_h1`]) and its maps converted once.
pub fn plan<R: Read + Seek>(
    h1: &H1Map,
    base: &mut Map<R>,
    _templates: &Templates,
    materials: &[(usize, String)],
) -> Result<Retexture> {
    let mut out = Retexture {
        library: shad::Library::read(base),
        ..Retexture::default()
    };
    let mut converted: HashMap<h1map::TagId, Option<usize>> = HashMap::new();
    let mut brightest: HashMap<(h1map::TagId, u32), Option<usize>> = HashMap::new();
    let mut done: HashMap<(usize, String), ()> = HashMap::new();
    for (i, name) in materials {
        if done.insert((*i, name.clone()), ()).is_some() {
            continue;
        }
        let Some(s) = h1
            .tags()
            .iter()
            .filter(|t| &t.name == name)
            .find_map(|t| shad::read_h1(h1, t))
        else {
            out.notes
                .push(format!("{name}: not a shader whose maps this reads"));
            continue;
        };
        let Some(chosen) = s.choose(&out.library) else {
            out.notes.push(format!(
                "{name}: the base lacks {} or its default bitmaps",
                s.layout.stem
            ));
            continue;
        };
        let mut maps = Vec::new();
        let mut swapped = Vec::new();
        for (role, m) in &chosen.maps {
            // A plasma's picture with its brightness as alpha (converter
            // version 89, `shad::plasma_shader`), else the bitmap as it is.
            let derived = match (*role, chosen.extras.brightest_mask) {
                (shad::Role::Mask, Some(gain)) => *brightest
                    .entry((m.bitmap, gain.to_bits()))
                    .or_insert_with(|| {
                        let tag = h1.tag(m.bitmap).ok()?;
                        let c = h1bitmaps::mask::convert_brightest(h1, tag, gain).ok()?;
                        out.bitmaps
                            .push((shad::brightness_mask_name(&tag.name, gain), c));
                        Some(out.bitmaps.len() - 1)
                    }),
                _ => None,
            };
            let k = derived.or_else(|| {
                *converted.entry(m.bitmap).or_insert_with(|| {
                    let tag = h1.tag(m.bitmap).ok()?;
                    let (_, c, _) = h1bitmaps::convert_bitmap(h1, tag).ok()?;
                    out.bitmaps.push((tag.name.clone(), c));
                    Some(out.bitmaps.len() - 1)
                })
            });
            let Some(k) = k else { continue };
            maps.push((*role, k, m.scale, shad::h1_lod_offset(h1, m.bitmap)));
            swapped.push((role_name(*role), out.bitmaps[k].0.clone()));
        }
        if maps.is_empty() {
            out.notes
                .push(format!("{name}: none of its bitmaps converts"));
            continue;
        }
        out.copies.push(ShaderCopy {
            h1_shader: name.clone(),
            material: *i,
            layout: chosen.layout,
            extras: chosen.extras.clone(),
            maps,
            swapped,
        });
    }
    Ok(out)
}

impl Retexture {
    /// Adds the bitmaps and rebuilt shaders to `set` (after anything whose
    /// datums were predicted) and returns `templates` with a material head
    /// per shader (the base head's bytes, shader datum `+0x0c` replaced;
    /// the part kind the shader's stem draws as,
    /// [`crate::structure::part_kind`]), and for each of `materials` the
    /// head it now uses. A bitmap or shader whose name the set already
    /// holds is reused, so the BSPs of one level share them.
    pub fn install(
        &self,
        set: &mut TagSet,
        templates: &Templates,
        materials: &[(usize, String)],
    ) -> Result<(Templates, Vec<usize>)> {
        let mut ids = Vec::new();
        for (name, c) in &self.bitmaps {
            let have = set.find(c.group, name);
            ids.push(match have {
                Some(id) => id,
                None => c.clone().install(set, name)?,
            });
        }
        set.add_group(SHAD, None, None);
        let mut out = templates.clone();
        let mut heads: HashMap<(usize, &str), usize> = HashMap::new();
        for c in &self.copies {
            let id = match set.find(SHAD, &c.h1_shader) {
                Some(id) => id,
                None => {
                    let maps: HashMap<shad::Role, shad::Bound> = c
                        .maps
                        .iter()
                        .map(|&(r, k, scale, lod_offset)| {
                            (
                                r,
                                shad::Bound {
                                    id: ids[k],
                                    scale,
                                    lod_offset,
                                },
                            )
                        })
                        .collect();
                    let tree = self.library.build_with(c.layout, &maps, &c.extras)?;
                    set.add_tag(SHAD, &c.h1_shader, TagPayload::Built(tree))?
                }
            };
            let mut head = templates.material_heads[c.material];
            head[0x8..0xc].copy_from_slice(&SHAD.as_u32().to_le_bytes());
            head[0xc..0x10].copy_from_slice(&id.0.to_le_bytes());
            out.material_heads.push(head);
            out.part_kinds
                .push(crate::structure::part_kind(c.layout.stem));
            heads.insert(
                (c.material, c.h1_shader.as_str()),
                out.material_heads.len() - 1,
            );
        }
        let used = materials
            .iter()
            .map(|(i, n)| heads.get(&(*i, n.as_str())).copied().unwrap_or(*i))
            .collect();
        Ok((out, used))
    }
}

/// Converted object models' shaders: [`shad::TEX_BUMP`] rebuilt from each
/// Halo 1 model shader's base and detail maps ([`ModelTemplate::copy`]),
/// and a base shader a model shader with no base map names instead.
#[derive(Debug, Clone)]
pub struct ModelTemplate {
    /// A base `objects\` shader drawn with [`MODEL_TEMPLATE`]: what a
    /// material whose Halo 1 shader has no base map names (never NONE).
    pub shader: TagId,
    pub name: String,
    pub library: shad::Library,
}

/// The stem converted model shaders draw with.
pub const MODEL_TEMPLATE: &str = shad::TEX_BUMP.stem;

/// The base's stems for model shaders, and the first `objects\` shader
/// (by name) drawn with [`MODEL_TEMPLATE`] as the stand-in. `None` when
/// the base lacks either.
pub fn model_template<R: Read + Seek>(base: &mut Map<R>) -> Result<Option<ModelTemplate>> {
    let library = shad::Library::read(base);
    if !library.has(&shad::TEX_BUMP) {
        return Ok(None);
    }
    let mut candidates: Vec<(String, TagId)> = base
        .tags()
        .iter()
        .filter(|t| t.group == SHAD && t.location.is_some())
        .filter_map(|t| Some((t.name.clone()?, t.id)))
        .filter(|(n, _)| n.starts_with("objects\\"))
        .collect();
    candidates.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, id) in candidates {
        let Ok((template, _)) = scenario_edit::scene::shader_bitmaps(base, id) else {
            continue;
        };
        if template == MODEL_TEMPLATE {
            return Ok(Some(ModelTemplate {
                shader: id,
                name,
                library,
            }));
        }
    }
    Ok(None)
}

impl ModelTemplate {
    /// A [`shad::TEX_BUMP`] drawing `base` as its base map and `detail`
    /// (else the default detail) as its detail map, tiled `detail_scale`
    /// times (u, v) over the base, its bump map flat.
    /// The datums are [`map::build::FixupKind::TagId`] fixups, so a datum
    /// not yet installed (an `h1objects` tree's local one) is filled in
    /// later. Returns the tree and the roles Halo 1 supplied.
    pub fn copy(
        &self,
        base: TagId,
        detail: Option<TagId>,
        detail_scale: [f32; 2],
    ) -> (Block, Vec<&'static str>) {
        let bound = |id, scale| shad::Bound {
            id,
            scale,
            lod_offset: 0.0,
        };
        let mut maps = HashMap::new();
        maps.insert(shad::Role::Base, bound(base, [1.0, 1.0]));
        let mut roles = vec!["base"];
        if let Some(d) = detail {
            maps.insert(shad::Role::Detail, bound(d, detail_scale));
            roles.push("detail");
        }
        let tree = self
            .library
            .build(&shad::TEX_BUMP, &maps)
            .expect("model_template checked the stem and defaults");
        (tree, roles)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tags::shader::{ImplementationBindings, IndexRange, ParameterBinding, TemplatePostprocess};

    /// A vector binding: register `reg`, `bank`, component `mode`, from
    /// instance vector `source`.
    fn vector(reg: u16, bank: u16, mode: u16, source: u8) -> ParameterBinding {
        ParameterBinding {
            word: reg | bank << 4 | mode << 5,
            b2: 0,
            source,
        }
    }

    #[test]
    fn a_stage_is_tiled_by_the_vector_bound_to_its_constant_x_and_y() {
        let tp = TemplatePostprocess {
            lods: Vec::new(),
            layers: Vec::new(),
            passes: Vec::new(),
            implementations: Vec::new(),
            bindings: vec![
                // Another implementation's.
                vector(0, 1, 13, 9),
                // c[18] from vector 3 (all of it), c[19] from vector 2;
                // vector 5 fills c[20]'s z and w only (mode 10), vector 4
                // its x and y (mode 11); vector 7 a vertex default.
                vector(0, 1, 13, 3),
                vector(1, 1, 13, 2),
                vector(2, 1, 10, 5),
                vector(2, 1, 11, 4),
                vector(3, 0, 13, 7),
            ],
        };
        let ib = ImplementationBindings {
            vertex_constants: IndexRange { start: 1, count: 5 },
            ..Default::default()
        };
        let got: Vec<_> = (0..4).map(|s| stage_vector(&tp, ib, s)).collect();
        assert_eq!(got, [Some(3), Some(2), Some(4), None]);
    }

    /// A `soso`'s detail is tiled by its own scale (invented bytes): u the
    /// scale, v the scale times its multiplier; 1 without a detail map,
    /// and for a scale that isn't positive.
    #[test]
    fn a_model_shader_tiles_its_detail_by_its_scale() {
        let mut d = vec![0u8; 0x1b8];
        let put = |d: &mut Vec<u8>, at: usize, v: u32| d[at..at + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut d, SOSO_DETAIL_SCALE, 16f32.to_bits());
        put(&mut d, SOSO_DETAIL_V_SCALE, 0.5f32.to_bits());
        // No detail map (a null reference): 1.
        put(&mut d, 0xdc + 12, u32::MAX);
        assert_eq!(scales_from("soso", &d).detail, [1.0, 1.0]);
        put(&mut d, 0xdc, u32::from_le_bytes(*b"mtib"));
        put(&mut d, 0xdc + 12, 0xe1ad_0039);
        assert_eq!(scales_from("soso", &d).detail, [16.0, 8.0]);
        put(&mut d, SOSO_DETAIL_SCALE, (-2f32).to_bits());
        put(&mut d, SOSO_DETAIL_V_SCALE, 0);
        assert_eq!(scales_from("soso", &d).detail, [1.0, 1.0]);
        // An `senv` reads its own offsets, not these.
        assert_eq!(scales_from("senv", &d).detail, [1.0, 1.0]);
    }
}
