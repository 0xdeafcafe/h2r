//! Halo 1's own textures on the converted structure: for each Halo 1
//! shader, a copy of the Halo 2 shader [`crate::shaders::pick`] chose for it
//! (`shad`), with the bitmaps it draws as base, detail and bump map
//! swapped for Halo 1's own, converted by `h1bitmaps` and added as new
//! tags. Shared by the multiplayer converter and `h1campaign` (both write
//! the structure through [`crate::structure::Structure`]).
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
//! **Which Halo 2 parameter takes it.** The engine binds instance bitmap
//! parameters (`shad` postprocess `+0x04`, stride `0xc`, the datum at `+0`)
//! to texture stages as the template says (`tags::material`). The copy's
//! roles come from the template shader resolved on the base map: the base
//! map is stage 0 of the first pass that samples an instance bitmap there,
//! the detail map stage 1 of that pass, the bump map any parameter whose
//! bitmap is `P8Bump` (Halo 2's bump format; a Halo 1 bump map is swapped
//! in only when it is `P8Bump` too, the same format number 17 in both
//! games). Every parameter holding that role's datum is swapped.
//!
//! **The copy.** A new `shad` built from the template's whole tree (the
//! schema's depth-first copy, [`Relocator::to_built`]) with the swapped
//! parameters' datums replaced, so it points at nothing of the template's
//! and survives a build that moves every tag (`--fresh`). The blocks on
//! the path to the parameters lose their sharing key (they differ from
//! the template's now); every other block keeps it and is placed once.
//! A structure material takes it through
//! a new material head (the base head's bytes, shader datum `+0x0c`
//! replaced), so [`crate::structure::Templates`] carries it and
//! `Structure` needs no change.

use std::collections::HashMap;
use std::io::{Read, Seek};

use h1map::bitmap::Bitmap;
use h1map::{Group, H1Map, TagRef};
use map::build::FixupKind;
use map::build::{Block, TagPayload, TagSet};
use map::{Map, TagId};
use tags::material::{ShaderLibrary, TextureSource};
use tags::schema::{Relocator, Schema};
use tags::shader::SHAD;
use tags::{DecodedTag, PixelFormat, TagData};

use crate::structure::Templates;
use crate::{Error, Result};

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
    pub detail: f32,
    pub bump: f32,
}

/// The tiling of the Halo 1 shader `tag`'s detail and bump maps. `senv`:
/// each map reference's scale is the f32 before it, primary detail
/// `+0xb4`, micro detail `+0xf8`, bump `+0x128 - 4`. V (every `senv` on
/// the disc): 1 wherever the map is null; 1 to 800 where it is set (micro
/// details mostly 16 to 60, bump maps 1 to 100). The detail's is the
/// map's [`h1_maps_of`] takes. The base map has none: its coordinates are
/// the surface's. Other groups (and a scale that isn't positive) are 1.
pub fn h1_scales_of(h1: &H1Map, tag: &h1map::Tag) -> H1Scales {
    let mut out = H1Scales {
        detail: 1.0,
        bump: 1.0,
    };
    if tag.group != Group::new(b"senv") {
        return out;
    }
    let Ok(d) = h1.tag_data(tag) else {
        return out;
    };
    let scale = |o: usize| {
        d.get(o..o + 4)
            .map(|b| f32::from_le_bytes(b.try_into().expect("4")))
            .filter(|f| f.is_finite() && *f > 0.0)
            .unwrap_or(1.0)
    };
    let set = |o: usize| TagRef::read(d, o).is_some_and(|r| !r.id.is_none());
    out.detail = if set(0xb8) {
        scale(0xb4)
    } else if set(0xfc) {
        scale(0xf8)
    } else {
        1.0
    };
    out.bump = if set(0x128) { scale(0x124) } else { 1.0 };
    out
}

/// Which instance bitmaps (datums) a Halo 2 shader draws in each role.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct H2Roles {
    pub base: Option<TagId>,
    pub detail: Option<TagId>,
    pub bump: Option<TagId>,
    /// The instance vector (postprocess `+0x14` index) whose x and y
    /// scale each role's texture coordinates ([`stage_vector`]).
    pub base_vector: Option<usize>,
    pub detail_vector: Option<usize>,
    pub bump_vector: Option<usize>,
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

/// The roles of `shader`'s bitmaps, from its material on `map`.
pub fn h2_roles<R: Read + Seek>(
    lib: &mut ShaderLibrary,
    map: &mut Map<R>,
    shader: TagId,
) -> Result<H2Roles> {
    let material = lib.material(shader, None).map_err(tags_err)?;
    let mut roles = H2Roles::default();
    let bitmap = |t: Option<tags::material::StageTexture>| match t.map(|t| t.source) {
        Some(TextureSource::Bitmap { bitmap, .. }) if bitmap != TagId::NULL => Some(bitmap),
        _ => None,
    };
    let mut found = None;
    'find: for layer in material.layers.iter().filter(|l| l.lod == 0) {
        for pass in &layer.passes {
            let Some(imp) = pass.implementations.first() else {
                continue;
            };
            if let Some(b) = bitmap(imp.textures[0]) {
                roles.base = Some(b);
                roles.detail = bitmap(imp.textures[1]).filter(|&d| d != b);
                found = Some((layer.lod, layer.layer, pass.index, imp.textures));
                break 'find;
            }
        }
    }
    let shad = lib.shader(shader).map_err(tags_err)?;
    for p in &shad.compiled().map_err(tags_err)?.bitmaps {
        if p.bitmap == TagId::NULL || Some(p.bitmap) == roles.base || Some(p.bitmap) == roles.detail
        {
            continue;
        }
        if let Ok(TagData::Bitmap(bm)) = DecodedTag::read(map, p.bitmap).map(|d| d.data)
            && bm
                .images
                .first()
                .is_some_and(|i| i.format == PixelFormat::P8Bump)
        {
            roles.bump = Some(p.bitmap);
            break;
        }
    }
    // Each role's tiling vector, from the pass the roles came from.
    if let Some((lod, layer, pi, textures)) = found {
        let template = shad.template.id;
        let tp = lib.template(template).map_err(tags_err)?;
        let tp = tp.compiled().map_err(tags_err)?;
        let ib = tp
            .layer_passes(lod, layer)
            .and_then(|l| l.get(pi))
            .and_then(|p| tp.implementations.get(usize::from(p.implementations.start)))
            .copied();
        let stage_of = |b: Option<TagId>| {
            textures
                .iter()
                .position(|t| bitmap(*t).is_some_and(|x| Some(x) == b))
        };
        if let Some(ib) = ib {
            let vector = |b| stage_of(b).and_then(|s| stage_vector(tp, ib, s));
            roles.base_vector = vector(roles.base);
            roles.detail_vector = vector(roles.detail);
            roles.bump_vector = vector(roles.bump);
        }
    }
    // A role whose bitmap is a bump map is no picture.
    for r in [&mut roles.base, &mut roles.detail] {
        if let Some(b) = *r
            && let Ok(TagData::Bitmap(bm)) = DecodedTag::read(map, b).map(|d| d.data)
            && bm
                .images
                .first()
                .is_some_and(|i| i.format == PixelFormat::P8Bump)
        {
            *r = None;
        }
    }
    Ok(roles)
}

/// One shader copy: the template's tree and which parameters change.
#[derive(Debug, Clone)]
pub struct ShaderCopy {
    /// The Halo 1 shader it is for (its name is the copy's).
    pub h1_shader: String,
    /// The base material it copies ([`Templates::material_heads`] index).
    pub material: usize,
    /// The base `shad` it copies, and its name.
    pub template: TagId,
    pub template_name: String,
    /// The template's tree ([`Relocator::to_built`]).
    tree: Block,
    /// (parameter index, index into [`Retexture::bitmaps`]).
    swaps: Vec<(usize, usize)>,
    /// What was swapped, by role, as (role, Halo 1 bitmap name).
    pub swapped: Vec<(&'static str, String)>,
    /// (instance vector, x and y scale): each swapped role's tiling, Halo
    /// 1's ([`H1Scales`]; 1 for the base map).
    pub scales: Vec<(usize, [f32; 2])>,
}

/// Halo 1 bitmaps converted, and the shader copies naming them: made by
/// [`plan`], added to the map by [`Retexture::install`].
#[derive(Debug, Clone, Default)]
pub struct Retexture {
    /// (Halo 1 bitmap name, its conversion).
    pub bitmaps: Vec<(String, h1models::Converted)>,
    pub copies: Vec<ShaderCopy>,
    /// Shaders left as the base's, and why.
    pub notes: Vec<String>,
}

/// Plans Halo 1's textures for the structure materials `materials`
/// (each: the base material index [`crate::shaders::pick`] chose, the
/// Halo 1 shader's name), with `base`, the map the set is made from.
pub fn plan<R: Read + Seek>(
    h1: &H1Map,
    base: &mut Map<R>,
    templates: &Templates,
    materials: &[(usize, String)],
) -> Result<Retexture> {
    let mut lib = ShaderLibrary::read(base).map_err(tags_err)?;
    let mut out = Retexture::default();
    let mut converted: HashMap<h1map::TagId, Option<usize>> = HashMap::new();
    let mut roles: HashMap<TagId, Option<H2Roles>> = HashMap::new();
    let mut done: HashMap<(usize, String), ()> = HashMap::new();
    let mut relocator: Option<Relocator> = None;
    for (i, name) in materials {
        if done.insert((*i, name.clone()), ()).is_some() {
            continue;
        }
        let scales = h1
            .tags()
            .iter()
            .find(|t| &t.name == name && h1_maps_of(h1, t).is_some())
            .map(|t| h1_scales_of(h1, t));
        let Some(maps) = h1_maps(h1, name) else {
            out.notes
                .push(format!("{name}: not a shader whose maps this reads"));
            continue;
        };
        let shader = templates.shader(*i);
        let r = match roles.get(&shader) {
            Some(r) => *r,
            None => {
                let r = h2_roles(&mut lib, base, shader).ok();
                roles.insert(shader, r);
                r
            }
        };
        let Some(r) = r else {
            out.notes
                .push(format!("{name}: its Halo 2 shader doesn't resolve"));
            continue;
        };
        let mut convert = |id: Option<h1map::TagId>, bump: bool| -> Option<usize> {
            let id = id?;
            if let Some(k) = converted.get(&id) {
                return *k;
            }
            let k = (|| {
                let tag = h1.tag(id).ok()?;
                if bump {
                    let bm = Bitmap::read(h1, tag).ok()?;
                    if bm.images.first()?.format.raw() != P8_BUMP {
                        return None;
                    }
                }
                let (_, c, _) = h1bitmaps::convert_bitmap(h1, tag).ok()?;
                out.bitmaps.push((tag.name.clone(), c));
                Some(out.bitmaps.len() - 1)
            })();
            converted.insert(id, k);
            k
        };
        let pairs = [
            ("base", r.base, convert(maps.base, false), maps.base),
            ("detail", r.detail, convert(maps.detail, false), maps.detail),
            ("bump", r.bump, convert(maps.bump, true), maps.bump),
        ];
        let h1_scales = scales.unwrap_or(H1Scales {
            detail: 1.0,
            bump: 1.0,
        });
        let tiling = |role: &str| match role {
            "base" => (r.base_vector, 1.0),
            "detail" => (r.detail_vector, h1_scales.detail),
            _ => (r.bump_vector, h1_scales.bump),
        };
        let template_name = base
            .tag(shader)
            .ok()
            .and_then(|t| t.name.clone())
            .unwrap_or_default();
        if relocator.is_none() {
            relocator = Some(Relocator::new(Schema::builtin(), base).map_err(tags_err)?);
        }
        let rel = relocator.as_ref().expect("made");
        let mut tree = match rel.to_built(shader) {
            Ok(t) => t,
            Err(e) => {
                out.notes
                    .push(format!("{name}: {template_name} doesn't copy ({e:?})"));
                continue;
            }
        };
        let Some(params) = params_mut(&mut tree) else {
            out.notes
                .push(format!("{name}: {template_name} has no bitmap parameters"));
            continue;
        };
        let params = params.bytes.clone();
        let n = params.len() / 0xc;
        let word = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
        let mut swaps = Vec::new();
        let mut swapped = Vec::new();
        let mut scales_out = Vec::new();
        for (role, h2, k, h1id) in pairs {
            let (Some(h2), Some(k)) = (h2, k) else {
                continue;
            };
            let mut any = false;
            for p in 0..n {
                if TagId(word(&params, p * 0xc)) == h2 {
                    swaps.push((p, k));
                    any = true;
                }
            }
            if any {
                let h1name = h1id
                    .and_then(|id| h1.tag(id).ok())
                    .map(|t| t.name.clone())
                    .unwrap_or_default();
                swapped.push((role, h1name));
                if let (Some(v), f) = tiling(role) {
                    scales_out.push((v, [f, f]));
                }
            }
        }
        if swaps.is_empty() {
            out.notes
                .push(format!("{name}: no Halo 1 bitmap fits {template_name}'s"));
            continue;
        }
        out.copies.push(ShaderCopy {
            h1_shader: name.clone(),
            material: *i,
            template: shader,
            template_name,
            tree,
            swaps,
            swapped,
            scales: scales_out,
        });
    }
    Ok(out)
}

impl Retexture {
    /// Adds the bitmaps and shader copies to `set` (after anything whose
    /// datums were predicted) and returns `templates` with a material head
    /// per copy, and for each of `materials` the head it now uses. A bitmap
    /// or shader copy whose name the set already holds is reused, so the
    /// BSPs of one level share their textures.
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
            let mut root = c.tree.clone();
            let params = params_mut(&mut root).ok_or(Error::Invalid("shader copy"))?;
            for &(p, k) in &c.swaps {
                params.bytes[p * 0xc..p * 0xc + 4].copy_from_slice(&ids[k].0.to_le_bytes());
            }
            set_tiling(&mut root, &c.scales);
            let id = match set.find(SHAD, &c.h1_shader) {
                Some(id) => id,
                None => set.add_tag(SHAD, &c.h1_shader, TagPayload::Built(root))?,
            };
            let mut head = templates.material_heads[c.material];
            head[0x8..0xc].copy_from_slice(&SHAD.as_u32().to_le_bytes());
            head[0xc..0x10].copy_from_slice(&id.0.to_le_bytes());
            out.material_heads.push(head);
            let kind = templates.part_kinds.get(c.material).copied();
            out.part_kinds
                .push(kind.unwrap_or(crate::structure::PART_KIND));
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

/// The Halo 2 shader converted object models copy ([`model_template`]):
/// its tree, its roles, and the base's plain bitmaps for a role Halo 1
/// leaves empty.
#[derive(Debug, Clone)]
pub struct ModelTemplate {
    pub shader: TagId,
    pub name: String,
    pub roles: H2Roles,
    /// `shaders\default_bitmaps\bitmaps\default_vector` (a flat bump).
    pub flat_bump: Option<TagId>,
    /// `shaders\default_bitmaps\bitmaps\default_detail` (a plain detail).
    pub plain_detail: Option<TagId>,
    tree: Block,
}

/// The template [`model_template`] looks for: an opaque bumped shader,
/// the commonest on the base maps' objects.
pub const MODEL_TEMPLATE: &str = "shaders\\shader_templates\\opaque\\tex_bump";

/// The first `objects\` shader (by name) of `base` drawn with
/// [`MODEL_TEMPLATE`] whose base map role resolves and whose tree copies.
pub fn model_template<R: Read + Seek>(base: &mut Map<R>) -> Result<Option<ModelTemplate>> {
    let mut candidates: Vec<(String, TagId)> = base
        .tags()
        .iter()
        .filter(|t| t.group == SHAD && t.location.is_some())
        .filter_map(|t| Some((t.name.clone()?, t.id)))
        .filter(|(n, _)| n.starts_with("objects\\"))
        .collect();
    candidates.sort_by(|a, b| a.0.cmp(&b.0));
    let find = |base: &Map<R>, name: &str| {
        base.tags()
            .iter()
            .find(|t| t.group == map::FourCC(*b"bitm") && t.name.as_deref() == Some(name))
            .map(|t| t.id)
    };
    let flat_bump = find(base, "shaders\\default_bitmaps\\bitmaps\\default_vector");
    let plain_detail = find(base, "shaders\\default_bitmaps\\bitmaps\\default_detail");
    let mut lib = ShaderLibrary::read(base).map_err(tags_err)?;
    let mut rel: Option<Relocator> = None;
    for (name, id) in candidates {
        let Ok((template, _)) = scenario_edit::scene::shader_bitmaps(base, id) else {
            continue;
        };
        if template != MODEL_TEMPLATE {
            continue;
        }
        let Ok(roles) = h2_roles(&mut lib, base, id) else {
            continue;
        };
        if roles.base.is_none() {
            continue;
        }
        if rel.is_none() {
            rel = Some(Relocator::new(Schema::builtin(), base).map_err(tags_err)?);
        }
        let Ok(mut tree) = rel.as_ref().expect("made").to_built(id) else {
            continue;
        };
        if params_mut(&mut tree).is_none() {
            continue;
        }
        return Ok(Some(ModelTemplate {
            shader: id,
            name,
            roles,
            flat_bump,
            plain_detail,
            tree,
        }));
    }
    Ok(None)
}

impl ModelTemplate {
    /// A copy drawing `base` as its base map and `detail` (else the plain
    /// detail) as its detail map, its bump map flat. Returns the tree and
    /// the roles swapped. The datums are [`FixupKind::TagId`] fixups.
    pub fn copy(&self, base: TagId, detail: Option<TagId>) -> (Block, Vec<&'static str>) {
        let mut tree = self.tree.clone();
        let params = params_mut(&mut tree).expect("checked in model_template");
        let mut swapped = Vec::new();
        let n = params.bytes.len() / 0xc;
        for (role, from, to) in [
            ("base", self.roles.base, Some(base)),
            ("detail", self.roles.detail, detail.or(self.plain_detail)),
            ("bump", self.roles.bump, self.flat_bump),
        ] {
            let (Some(from), Some(to)) = (from, to) else {
                continue;
            };
            let mut any = false;
            for p in 0..n {
                let at = p * 0xc;
                if params.bytes[at..at + 4] == from.0.to_le_bytes() {
                    // A fixup, so a datum not yet installed (an
                    // `h1objects` tree's local one) is filled in later.
                    params.fixup(at as u32, FixupKind::TagId(to));
                    any = true;
                }
            }
            if any {
                swapped.push(role);
            }
        }
        (tree, swapped)
    }
}

/// Writes each (vector, scale) into the x and y of that instance vector
/// (postprocess `+0x14`, 16 bytes each) of the `shad` tree `root`, its z
/// and w kept.
pub fn set_tiling(root: &mut Block, scales: &[(usize, [f32; 2])]) {
    if scales.is_empty() {
        return;
    }
    root.share = None;
    let Some(pp) = block_child(root, 0x20) else {
        return;
    };
    pp.share = None;
    let Some(vectors) = block_child(pp, 0x14) else {
        return;
    };
    vectors.share = None;
    for &(v, [x, y]) in scales {
        if let Some(dst) = vectors.bytes.get_mut(v * 0x10..v * 0x10 + 8) {
            dst[..4].copy_from_slice(&x.to_le_bytes());
            dst[4..].copy_from_slice(&y.to_le_bytes());
        }
    }
}

/// The bitmap parameter block of a `shad` tree (root `+0x20` postprocess,
/// its first element's `+0x04` parameters), with the sharing key cleared
/// on it and on the blocks above it.
fn params_mut(root: &mut Block) -> Option<&mut Block> {
    root.share = None;
    let pp = block_child(root, 0x20)?;
    pp.share = None;
    let params = block_child(pp, 0x04)?;
    params.share = None;
    Some(params)
}

/// The child block at `at` (a non-empty tag block field) of `b`.
pub(crate) fn block_child(b: &mut Block, at: u32) -> Option<&mut Block> {
    b.fixups.iter_mut().find_map(|f| match &mut f.kind {
        FixupKind::Block { count, child } if f.offset == at && *count > 0 => Some(child),
        _ => None,
    })
}

fn tags_err(e: tags::Error) -> Error {
    Error::Textures(e.to_string())
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

    #[test]
    fn tiling_sets_x_and_y_and_keeps_z_and_w() {
        let mut vectors = Block::zeroed(3 * 0x10);
        for (k, f) in vectors.bytes.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            *f = (k as f32).to_le_bytes();
        }
        vectors.share = Some(7);
        let mut pp = Block::zeroed(0x7c);
        pp.block(0x14, 3, vectors);
        let mut root = Block::zeroed(0x54);
        root.block(0x20, 1, pp);
        set_tiling(&mut root, &[(1, [16.0, 8.0])]);
        let pp = block_child(&mut root, 0x20).unwrap();
        let v = block_child(pp, 0x14).unwrap();
        assert_eq!(v.share, None);
        let f: Vec<f32> = v
            .bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        assert_eq!(&f[4..8], [16.0, 8.0, 6.0, 7.0]);
        assert_eq!(&f[..4], [0.0, 1.0, 2.0, 3.0]);
    }
}
