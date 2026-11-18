//! The Halo 1 level's own skies (`h1sky`) in place of the base's.
//!
//! Each sky the Halo 1 scenario names (`scnr` skies) is converted with its
//! model, lights and fog by [`h1sky::convert::sky_from_map`] and added as a
//! new tag. The base scenario's skies block (`scnr +0x08`, count; `+0x0c`,
//! pointer; 8-byte tag references) keeps its length: element `k` names
//! Halo 1's sky `k`, or Halo 1's last when the base has more, so no element
//! still names a base sky. Converted clusters all draw sky 0
//! ([`crate::structure`]).
//!
//! A converted sky model's material `k` names a copy of a base sky shader
//! drawing Halo 1's shader `k`'s bitmaps ([`crate::sky_shaders`]), added
//! under the Halo 1 shader's name; a material whose shader doesn't convert
//! names the base sky's own ([`base_sky_shader`]). They are never NONE: the shader draw begin `0x1bbf0` does not skip a NONE shader
//! (`0x1bcb9` only clears its cached words), and the draw `0x16b90` then
//! reads tag slot `0xffff`'s entry as a `shad` and dereferences its
//! compiled postprocess (`0x16bc7`, `0x16bde`): a null read on the first
//! frame that draws the sky.

use std::io::{Read, Seek};

use map::build::{Block, FixupKind, TagPayload, TagSet};
use map::{FourCC, Map, TagId};
use tags::geometry::TagView;
use tags::shader::SHAD;

use crate::sky_shaders::{H1SkyShader, SkyTemplates, read_h1, recipe};
use crate::{Error, Result};

/// `scnr` skies block field (count; pointer at `+4`) and element size.
pub const SKIES: usize = 0x08;
/// Skies element: an 8-byte tag reference.
pub const SKY: usize = 8;

/// One converted sky, ready to add.
#[derive(Debug, Clone)]
pub struct Sky {
    /// The Halo 1 sky's tag path (the new tag's name).
    pub name: String,
    pub sky: h1sky::h1::Sky,
    /// Its render model's path and conversion.
    pub model: Option<(String, h1models::Converted)>,
    /// `sky +0x14`: the least that keeps the sky behind the level
    /// ([`h1sky::convert::model_scale`]).
    pub model_scale: f32,
    /// Halo 1's shader of each model material, where it reads.
    pub shaders: Vec<Option<H1SkyShader>>,
    /// The Halo 1 bitmaps they draw: (Halo 1 id, name, conversion).
    pub bitmaps: Vec<(h1map::TagId, String, h1models::Converted)>,
}

/// The level's skies, and why any didn't convert.
#[derive(Debug, Clone, Default)]
pub struct Skies {
    pub skies: Vec<Sky>,
    pub notes: Vec<String>,
    /// The base's sky shaders the materials copy
    /// ([`crate::sky_shaders::sky_templates`]).
    pub templates: SkyTemplates,
}

/// `mode` materials block (count; pointer at `+4`), element size, and the
/// shader reference's datum in an element.
pub const MODE_MATERIALS: usize = 0x60;
pub const MODE_MATERIAL: usize = 0x20;
pub const MATERIAL_SHADER: usize = 0x08;

/// The `shad` the base's first sky's model draws its first material with:
/// scenario skies (`+0x08`) element 0, its `sky ` model (`+0x00`), that
/// model's materials ([`MODE_MATERIALS`]).
pub fn base_sky_shader<R: Read + Seek>(base: &mut Map<R>) -> Option<TagId> {
    let word = |b: &[u8], o: usize| Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?));
    let scnr = base.scenario()?.clone();
    let bytes = base.read_tag_bytes(scnr.id).ok()?;
    let view = TagView::new(&bytes, scnr.location?.address);
    let skies = view.block(view.at(0, SKIES + 8).ok()?, SKIES, SKY).ok()?;
    let sky = TagId(word(skies.first()?, 4)?);
    let model = TagId(word(&base.read_tag_bytes(sky).ok()?, 4)?);
    let tag = base.tag(model).ok()?.clone();
    if tag.group != FourCC(*b"mode") {
        return None;
    }
    let bytes = base.read_tag_bytes(model).ok()?;
    let view = TagView::new(&bytes, tag.location?.address);
    let root = view.at(0, MODE_MATERIALS + 8).ok()?;
    view.block(root, MODE_MATERIALS, MODE_MATERIAL)
        .ok()?
        .iter()
        .filter_map(|m| word(m, MATERIAL_SHADER + 4).map(TagId))
        .find(|id| {
            base.tag(*id)
                .is_ok_and(|t| t.group == FourCC(*b"shad") && t.location.is_some())
        })
}

/// Points material `k` of the converted `mode` tree `root` at `shader`.
/// False when it has no such material.
pub fn set_material_shader(root: &mut Block, k: usize, shader: TagId) -> bool {
    let Some(materials) = root.fixups.iter_mut().find_map(|f| match &mut f.kind {
        FixupKind::Block { child, .. } if f.offset == MODE_MATERIALS as u32 => Some(child),
        _ => None,
    }) else {
        return false;
    };
    let at = (k * MODE_MATERIAL + MATERIAL_SHADER) as u32;
    materials.fixups.iter_mut().any(|f| match &mut f.kind {
        FixupKind::TagRef(id) if f.offset == at => {
            *id = shader;
            true
        }
        _ => false,
    })
}

impl Sky {
    /// Reads and converts the Halo 1 sky `tag`: its model, and its
    /// model's shaders and their bitmaps.
    pub fn from_map(h1: &h1map::H1Map, tag: &h1map::Tag) -> Result<Self> {
        let f = h1sky::convert::sky_from_map(h1, tag)
            .map_err(|e| Error::Sky(format!("{}: {e}", tag.name)))?;
        let shaders: Vec<Option<H1SkyShader>> =
            f.shaders.iter().map(|&id| read_h1(h1, id)).collect();
        let mut bitmaps = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for m in shaders.iter().flatten().flat_map(|s| &s.maps) {
            if !seen.insert(m.bitmap) {
                continue;
            }
            let Ok(t) = h1.tag(m.bitmap) else { continue };
            if let Ok((_, c, _)) = h1bitmaps::convert_bitmap(h1, t) {
                bitmaps.push((m.bitmap, t.name.clone(), c));
            }
        }
        Ok(Self {
            name: tag.name.clone(),
            sky: f.sky,
            model: f.model,
            model_scale: f.model_scale,
            shaders,
            bitmaps,
        })
    }

    /// Adds the sky to `set`: its bitmaps, a shader copy per material
    /// (`templates`), the model and the sky; returns the sky's datum. A
    /// material with no copy names [`SkyTemplates::fallback`]; with
    /// neither the sky goes in without its model.
    pub fn install(&self, set: &mut TagSet, templates: &SkyTemplates) -> Result<TagId> {
        let err = |e: &dyn std::fmt::Display| Error::Sky(format!("{}: {e}", self.name));
        let model = match &self.model {
            Some((n, c)) => {
                let mut c = c.clone();
                let mut ids = std::collections::HashMap::new();
                for (h1id, name, b) in &self.bitmaps {
                    let id = match set.find(b.group, name) {
                        Some(id) => id,
                        None => b.clone().install(set, name).map_err(|e| err(&e))?,
                    };
                    ids.insert(*h1id, id);
                }
                set.add_group(SHAD, None, None);
                let mut all = true;
                for (k, s) in self.shaders.iter().enumerate() {
                    let copy = match s {
                        Some(s) => match set.find(SHAD, &s.name) {
                            Some(id) => Some(id),
                            None => {
                                let maps: Vec<_> = s
                                    .maps
                                    .iter()
                                    .map(|m| ids.get(&m.bitmap).map(|&id| (id, m.scale)))
                                    .collect();
                                match recipe(s).and_then(|r| templates.copy(r, &maps)) {
                                    Some(tree) => Some(
                                        set.add_tag(SHAD, &s.name, TagPayload::Built(tree))
                                            .map_err(|e| err(&e))?,
                                    ),
                                    None => None,
                                }
                            }
                        },
                        None => None,
                    };
                    match copy.or(templates.fallback) {
                        Some(id) => {
                            set_material_shader(&mut c.root, k, id);
                        }
                        None => all = false,
                    }
                }
                all.then_some((n.as_str(), c))
            }
            None => None,
        };
        h1sky::convert::install_sky(set, &self.sky, model, &self.name, self.model_scale)
            .map_err(|e| err(&e))
    }
}

/// Converts every sky the Halo 1 scenario names.
pub fn read(h1: &h1map::H1Map) -> Skies {
    let mut out = Skies::default();
    let refs = match h1.multiplayer() {
        Ok(mp) => mp.skies,
        Err(e) => {
            out.notes
                .push(format!("the scenario's skies don't read: {e}"));
            return out;
        }
    };
    for r in refs {
        let Ok(tag) = h1.tag(r.id) else { continue };
        match Sky::from_map(h1, tag) {
            Ok(sky) => out.skies.push(sky),
            Err(e) => out.notes.push(e.to_string()),
        }
    }
    out
}

impl Skies {
    /// Adds each sky ([`Sky::install`]) to `set`; returns their datums.
    pub fn install(&self, set: &mut TagSet) -> Result<Vec<TagId>> {
        self.skies
            .iter()
            .map(|s| s.install(set, &self.templates))
            .collect()
    }
}

/// Points every element of the skies block of a scenario payload (loaded
/// at `address`) at `ids` (module docs). Returns the elements rewritten.
pub fn patch_scenario(scnr: &mut [u8], address: u32, ids: &[TagId]) -> Result<usize> {
    let Some(&last) = ids.last() else {
        return Ok(0);
    };
    let word = |b: &[u8], at: usize| -> Result<u32> {
        b.get(at..at + 4)
            .map(|b| u32::from_le_bytes(b.try_into().expect("4")))
            .ok_or(Error::Invalid("scenario skies outside the tag"))
    };
    let count = word(scnr, SKIES)? as usize;
    if count == 0 {
        return Ok(0);
    }
    let start = word(scnr, SKIES + 4)?.wrapping_sub(address) as usize;
    if start + count * SKY > scnr.len() {
        return Err(Error::Invalid("scenario skies outside the tag"));
    }
    for k in 0..count {
        let id = ids.get(k).copied().unwrap_or(last);
        let at = start + k * SKY;
        scnr[at..at + 4].copy_from_slice(&FourCC(*b"sky ").as_u32().to_le_bytes());
        scnr[at + 4..at + 8].copy_from_slice(&id.0.to_le_bytes());
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sky_element_names_a_halo1_sky() {
        let address: u32 = 0x1000;
        let mut b = vec![0u8; 0x40];
        b[SKIES..SKIES + 4].copy_from_slice(&3u32.to_le_bytes());
        b[SKIES + 4..SKIES + 8].copy_from_slice(&(address + 0x20).to_le_bytes());
        let ids = [TagId(0xe1000010), TagId(0xe1010011)];
        assert_eq!(patch_scenario(&mut b, address, &ids).unwrap(), 3);
        let datum =
            |k: usize| u32::from_le_bytes(b[0x24 + 8 * k..0x28 + 8 * k].try_into().unwrap());
        assert_eq!(
            [datum(0), datum(1), datum(2)],
            [ids[0].0, ids[1].0, ids[1].0]
        );
        assert_eq!(b[0x20..0x24], FourCC(*b"sky ").as_u32().to_le_bytes());
        assert_eq!(patch_scenario(&mut b, address, &[]).unwrap(), 0);
    }

    /// A converted model's materials, as `h1models::mode` lays them out:
    /// each a null `+0x00` reference and its shader at `+0x08`.
    #[test]
    fn every_sky_material_names_the_shader() {
        let mut materials = Block::zeroed(MODE_MATERIAL * 3);
        for k in 0..3 {
            let at = (k * MODE_MATERIAL) as u32;
            materials.tag_ref(at, TagId::NULL);
            materials.tag_ref(at + MATERIAL_SHADER as u32, TagId::NULL);
        }
        let mut root = Block::zeroed(0x100);
        root.tag_ref(0x58, TagId::NULL);
        root.block(MODE_MATERIALS as u32, 3, materials);
        let shader = TagId(0xe000_0042);
        for k in 0..3 {
            assert!(set_material_shader(&mut root, k, shader));
        }
        assert!(!set_material_shader(&mut root, 3, shader));
        let FixupKind::Block { child, .. } = &root.fixups[1].kind else {
            panic!("materials");
        };
        let refs: Vec<(u32, TagId)> = child
            .fixups
            .iter()
            .filter_map(|f| match f.kind {
                FixupKind::TagRef(id) => Some((f.offset, id)),
                _ => None,
            })
            .collect();
        for (offset, id) in refs {
            let want = if offset as usize % MODE_MATERIAL == MATERIAL_SHADER {
                shader
            } else {
                TagId::NULL
            };
            assert_eq!(id, want, "+{offset:#x}");
        }
        // The other references are left alone.
        assert_eq!(root.fixups[0].kind, FixupKind::TagRef(TagId::NULL));
        // No materials block: nothing to do.
        assert!(!set_material_shader(&mut Block::zeroed(0x100), 0, shader));
    }
}
