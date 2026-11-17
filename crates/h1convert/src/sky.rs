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
//! A converted sky model's materials name the shader of the base sky's
//! model ([`base_sky_shader`]) until Halo 1's sky shaders convert. They are
//! never NONE: the shader draw begin `0x1bbf0` does not skip a NONE shader
//! (`0x1bcb9` only clears its cached words), and the draw `0x16b90` then
//! reads tag slot `0xffff`'s entry as a `shad` and dereferences its
//! compiled postprocess (`0x16bc7`, `0x16bde`): a null read on the first
//! frame that draws the sky.

use std::io::{Read, Seek};

use map::build::{Block, FixupKind, TagSet};
use map::{FourCC, Map, TagId};
use tags::geometry::TagView;

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
}

/// The level's skies, and why any didn't convert.
#[derive(Debug, Clone, Default)]
pub struct Skies {
    pub skies: Vec<Sky>,
    pub notes: Vec<String>,
    /// The shader the sky models' materials name ([`base_sky_shader`]).
    /// Without one the models are not installed.
    pub shader: Option<TagId>,
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

/// Points every material of the converted `mode` tree `root` at `shader`.
/// Returns how many it changed.
pub fn set_material_shaders(root: &mut Block, shader: TagId) -> usize {
    let Some(materials) = root.fixups.iter_mut().find_map(|f| match &mut f.kind {
        FixupKind::Block { child, .. } if f.offset == MODE_MATERIALS as u32 => Some(child),
        _ => None,
    }) else {
        return 0;
    };
    let mut n = 0;
    for f in &mut materials.fixups {
        if f.offset as usize % MODE_MATERIAL == MATERIAL_SHADER
            && let FixupKind::TagRef(id) = &mut f.kind
        {
            *id = shader;
            n += 1;
        }
    }
    n
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
        match h1sky::convert::sky_from_map(h1, tag) {
            Ok((sky, model)) => out.skies.push(Sky {
                name: tag.name.clone(),
                sky,
                model,
            }),
            Err(e) => out.notes.push(format!("{}: {e}", tag.name)),
        }
    }
    out
}

impl Skies {
    /// Adds each sky (and its model, its materials naming [`Self::shader`])
    /// to `set`; returns their datums. With no shader a sky goes in
    /// without its model.
    pub fn install(&self, set: &mut TagSet) -> Result<Vec<TagId>> {
        self.skies
            .iter()
            .map(|s| {
                let model = match (&s.model, self.shader) {
                    (Some((n, c)), Some(shader)) => {
                        let mut c = c.clone();
                        set_material_shaders(&mut c.root, shader);
                        Some((n.as_str(), c))
                    }
                    _ => None,
                };
                h1sky::convert::install_sky(set, &s.sky, model, &s.name)
                    .map_err(|e| Error::Sky(format!("{}: {e}", s.name)))
            })
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
        assert_eq!(set_material_shaders(&mut root, shader), 3);
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
        assert_eq!(set_material_shaders(&mut Block::zeroed(0x100), shader), 0);
    }
}
