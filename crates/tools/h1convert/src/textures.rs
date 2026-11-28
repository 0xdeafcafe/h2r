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
//! Structure render and collision records use the source palette order.
//! Each material points at its rebuilt shader and uses its program's part kind.

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
/// ([`scales_from`] on its bytes). A shader whose bytes do not read is an
/// error, not a scale of one.
pub fn h1_scales_of(h1: &H1Map, tag: &h1map::Tag) -> std::result::Result<H1Scales, h1map::Error> {
    Ok(scales_from(&tag.group.to_string(), h1.tag_data(tag)?))
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
    pub bitmaps: Vec<(String, h1_assets::models::Converted)>,
    pub copies: Vec<ShaderCopy>,
    /// Source shader name and translated globals index per collision material.
    pub collision: Vec<(Option<String>, u16)>,
    /// Informational conversion notes.
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

/// Rebuild the shaders at their source material indices, converting each bitmap once.
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
    let mut converted = HashMap::<(h1map::TagId, Option<u32>), usize>::new();
    let mut done: HashMap<(usize, String), ()> = HashMap::new();
    for (i, name) in materials {
        if done.insert((*i, name.clone()), ()).is_some() {
            continue;
        }
        let fail = |why: &str| crate::Error::no_rebuild(&h1.header().name, name, why);
        let mut candidates = h1
            .tags()
            .iter()
            .filter(|t| &t.name == name && shad::H1_GROUPS.contains(&t.group.to_string().as_str()));
        let tag = candidates
            .next()
            .ok_or_else(|| fail("missing source shader"))?;
        if candidates.next().is_some() {
            return Err(fail("ambiguous source shader path"));
        }
        if crate::sky_shaders::draws_nothing(h1, tag) {
            out.notes.push(no_draw_note(name));
            continue;
        }
        let s = shad::read_h1(h1, tag).ok_or_else(|| {
            fail(&format!("unsupported shader definition (group {})", tag.group))
        })?;
        if !out.library.has(s.layout) {
            return Err(fail(s.layout.stem));
        }
        let chosen = &s;
        if shad::uses_detail_program(chosen.layout, &chosen.extras) {
            out.library.load_detail_program(base)?;
        }
        let mut maps = Vec::new();
        let mut swapped = Vec::new();
        for (role, m) in &chosen.maps {
            // A plasma's picture with its brightness as alpha (converter
            // version 90, `shad::plasma_shader`), else the bitmap as it is.
            let gain = (*role == shad::Role::Mask)
                .then_some(chosen.extras.brightest_mask)
                .flatten();
            let key = (m.bitmap, gain.map(f32::to_bits));
            let k = match converted.get(&key) {
                Some(&k) => k,
                None => {
                    let bitmap = h1.tag(m.bitmap)?;
                    let fail = |why: String| {
                        crate::Error::no_rebuild(&h1.header().name, &bitmap.name, why)
                    };
                    let (name, image) = if let Some(gain) = gain {
                        let image = h1_assets::bitmaps::mask::convert_brightest(h1, bitmap, gain)
                            .map_err(|e| fail(e.to_string()))?;
                        (shad::brightness_mask_name(&bitmap.name, gain), image)
                    } else {
                        let (_, image, _) = h1_assets::bitmaps::convert_bitmap(h1, bitmap)
                            .map_err(|e| fail(e.to_string()))?;
                        (bitmap.name.clone(), image)
                    };
                    let k = out.bitmaps.len();
                    out.bitmaps.push((name, image));
                    converted.insert(key, k);
                    k
                }
            };
            maps.push((*role, k, m.scale, shad::h1_lod_offset(h1, m.bitmap)));
            swapped.push((role_name(*role), out.bitmaps[k].0.clone()));
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

/// The report line for a shader Halo 1 draws as nothing.
pub fn no_draw_note(shader: &str) -> String {
    format!("{shader}: Halo 1 draws nothing (no stage has a map); its parts are left out")
}

/// Removes the parts of `parts` whose shader Halo 1 draws as nothing
/// (`no_draw`, [`crate::sky_shaders::draws_nothing`]); returns the names of
/// those shaders, sorted, once each, for the report ([`no_draw_note`]).
pub fn drop_no_draw_parts<T>(
    parts: &mut Vec<T>,
    shader: impl Fn(&T) -> &str,
    mut no_draw: impl FnMut(&str) -> bool,
) -> Vec<String> {
    let mut seen: std::collections::BTreeMap<String, bool> = std::collections::BTreeMap::new();
    parts.retain(|p| {
        let name = shader(p);
        let drops = match seen.get(name) {
            Some(&d) => d,
            None => {
                let d = no_draw(name);
                seen.insert(name.to_owned(), d);
                d
            }
        };
        !drops
    });
    seen.into_iter().filter_map(|(n, d)| d.then_some(n)).collect()
}

/// [`drop_no_draw_parts`] for a map's shaders: those Halo 1 draws as nothing.
/// A name no shader tag has is left for [`plan`]'s named error.
pub fn drop_no_draw_meshes(h1: &H1Map, parts: &mut Vec<crate::structure::Mesh>) -> Vec<String> {
    drop_no_draw_parts(
        parts,
        |m| m.shader.as_str(),
        |name| {
            h1.tags()
                .iter()
                .filter(|t| t.name == name && shad::H1_GROUPS.contains(&t.group.to_string().as_str()))
                .any(|t| crate::sky_shaders::draws_nothing(h1, t))
        },
    )
}

/// Plan render and collision shaders together so a collision-only shader also gets a datum.
pub fn plan_bsp<R: Read + Seek>(
    h1: &H1Map,
    base: &mut Map<R>,
    materials: &[(usize, String)],
    collision: &[TagRef],
) -> Result<Retexture> {
    let meta =
        tags::object::Metadata::read(base).map_err(|e| crate::Error::Textures(e.to_string()))?;
    let globals = crate::materials::global_materials(base, &meta).map_err(crate::Error::Textures)?;
    let mut all = materials.to_vec();
    let mut palette = Vec::with_capacity(collision.len());
    for r in collision {
        let tag = h1.tag(r.id)?;
        let fail = |why: &str| crate::Error::no_rebuild(&h1.header().name, &tag.name, why);
        let bytes = h1.tag_data(tag)?;
        let material = bytes
            .get(0x22..0x24)
            .ok_or_else(|| fail("short source shader header"))?;
        let index = usize::from(u16::from_le_bytes([material[0], material[1]]));
        let (_, global) = crate::materials::MATERIALS
            .get(index)
            .ok_or_else(|| fail("unknown source material type"))?;
        let index = globals
            .iter()
            .position(|n| n.as_str() == *global)
            .and_then(|i| u16::try_from(i).ok())
            .ok_or_else(|| fail(global))?;
        if crate::sky_shaders::draws_nothing(h1, tag) {
            // Collision keeps its material; there is no shader to draw.
            palette.push((None, index));
            continue;
        }
        palette.push((Some(tag.name.clone()), index));
        if !all.iter().any(|(_, name)| name == &tag.name) {
            all.push((all.len(), tag.name.clone()));
        }
    }
    let mut textures = plan(h1, base, &Templates::default(), &all)?;
    textures.collision = palette;
    Ok(textures)
}

impl Retexture {
    /// Install the source images and shaders and build both material palettes.
    /// Multiple BSPs may share a shader already rebuilt for this output map.
    pub fn install(
        &self,
        set: &mut TagSet,
        templates: &Templates,
        materials: &[(usize, String)],
    ) -> Result<(Templates, Vec<usize>)> {
        let mut ids = Vec::new();
        for (name, c) in &self.bitmaps {
            let have = set.converted(c.group, name);
            ids.push(match have {
                Some(id) => id,
                None => c.clone().install(set, name)?,
            });
        }
        set.add_group(SHAD, None, None);
        let mut out = templates.clone();
        let mut heads: HashMap<(usize, &str), usize> = HashMap::new();
        for c in &self.copies {
            let id = match set.converted(SHAD, &c.h1_shader) {
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
                    let tree = self.library.build_surface(set, c.layout, &maps, &c.extras)?;
                    let id = set.add_tag(SHAD, &c.h1_shader, TagPayload::Built(tree))?;
                    set.record_conversion(SHAD, &c.h1_shader, id);
                    id
                }
            };
            let mut head = [0u8; 16];
            head[4..8].copy_from_slice(&TagId::NULL.0.to_le_bytes());
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
        for (name, global) in &self.collision {
            let id = match name {
                Some(name) => set.converted(SHAD, name).ok_or_else(|| {
                    crate::Error::no_rebuild(&set.name, name, "missing collision shader")
                })?,
                None => TagId::NULL, // Object collision materials have a substance and no shader.
            };
            let mut row = [0u8; crate::structure::COLLISION_MATERIAL];
            row[4..8].copy_from_slice(&TagId::NULL.0.to_le_bytes());
            row[8..10].copy_from_slice(&global.to_le_bytes());
            row[10..12].copy_from_slice(&(-1i16).to_le_bytes());
            row[12..16].copy_from_slice(&SHAD.as_u32().to_le_bytes());
            row[16..20].copy_from_slice(&id.0.to_le_bytes());
            out.collision_materials.push(row);
        }
        let used = materials
            .iter()
            .map(|(i, n)| {
                heads.get(&(*i, n.as_str())).copied().ok_or_else(|| {
                    crate::Error::no_rebuild(&set.name, n, "missing planned structure material")
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok((out, used))
    }
}

/// The compiled templates and rasterizer resources used to build model materials.
#[derive(Debug, Clone)]
pub struct ModelTemplate {
    pub library: shad::Library,
}

pub const MODEL_TEMPLATE: &str = shad::TEX_BUMP.stem;

pub fn model_template<R: Read + Seek>(base: &mut Map<R>) -> ModelTemplate {
    let mut library = shad::Library::read(base);
    library.load_model_lighting(base);
    ModelTemplate { library }
}

impl ModelTemplate {
    /// A [`shad::TEX_BUMP`] drawing `base` as its base map and `detail`
    /// (else the default detail) as its detail map, tiled `detail_scale`
    /// times (u, v) over the base, its bump map flat.
    /// The datums are [`map::build::FixupKind::TagId`] fixups, so a datum
    /// not yet installed (an `h1objects` tree's local one) is filled in
    /// later.
    pub fn build_opaque(
        &self,
        base: TagId,
        detail: Option<TagId>,
        detail_scale: [f32; 2],
    ) -> Result<Block> {
        let bound = |id, scale| shad::Bound {
            id,
            scale,
            lod_offset: 0.0,
        };
        let mut maps = HashMap::new();
        maps.insert(shad::Role::Base, bound(base, [1.0, 1.0]));
        if let Some(d) = detail {
            maps.insert(shad::Role::Detail, bound(d, detail_scale));
        }
        self.library.build(&shad::TEX_BUMP, &maps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tags::shader::{ImplementationBindings, IndexRange, ParameterBinding, TemplatePostprocess};

    #[test]
    fn a_shader_that_draws_nothing_drops_its_parts() {
        use crate::sky_shaders::maps_draw_nothing;
        // Two elements of 0x14 bytes, bitmap reference at 4 (16 bytes: group,
        // name address, name length, id): the id is the last four bytes.
        let element = |id: u32| {
            let mut e = [0u8; 0x14];
            e[4..8].copy_from_slice(&Group::new(b"bitm").0.to_le_bytes());
            e[16..20].copy_from_slice(&id.to_le_bytes());
            e
        };
        let null = element(u32::MAX);
        let lit = element(7);
        assert!(maps_draw_nothing(&[], 0x14, 4), "no stages");
        assert!(maps_draw_nothing(&[null, null].concat(), 0x14, 4));
        assert!(!maps_draw_nothing(&[null, lit].concat(), 0x14, 4));
        assert!(!maps_draw_nothing(&null[..8], 0x14, 4), "a short element is an error");

        let mut parts = vec!["glow", "wall", "glow", "floor"];
        let dropped = drop_no_draw_parts(&mut parts, |p| p, |n| n == "glow");
        assert_eq!(parts, ["wall", "floor"]);
        assert_eq!(dropped, ["glow"]);
        assert!(no_draw_note("glow").starts_with("glow:"));
    }

    #[test]
    fn native_shader_names_cannot_fill_a_missing_source_material() {
        let mut set = TagSet::new(map::CacheType::Multiplayer, "source_map", "scenario");
        set.add_group(SHAD, None, None);
        set.add_tag(SHAD, "stone", TagPayload::Empty).unwrap();
        let textures = Retexture {
            collision: vec![(Some("stone".into()), 5)],
            ..Default::default()
        };
        let error = textures
            .install(&mut set, &Templates::default(), &[])
            .unwrap_err();
        assert!(
            matches!(error, crate::Error::NoRebuild { map, tag, .. } if map == "source_map" && tag == "stone")
        );
    }

    #[test]
    fn collision_materials_keep_their_own_global_indices_and_shader_datums() {
        let mut set = TagSet::new(map::CacheType::Multiplayer, "source", "scenario");
        set.add_group(SHAD, None, None);
        let mut textures = Retexture::default();
        let mut expected = Vec::new();
        for (name, material) in [("snow", 8u16), ("ice", 2u16)] {
            let shader = set.add_tag(SHAD, name, TagPayload::Empty).unwrap();
            set.record_conversion(SHAD, name, shader);
            textures.collision.push((Some(name.into()), material));
            expected.push((material, shader));
        }
        let (templates, _) = textures
            .install(&mut set, &Templates::default(), &[])
            .unwrap();
        for (row, (material, shader)) in templates.collision_materials.iter().zip(expected) {
            assert_eq!(&row[8..10], &material.to_le_bytes());
            assert_eq!(&row[16..20], &shader.0.to_le_bytes());
        }
    }

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
