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
//! Every authored sky material requires a rebuilt shader before installation.

use std::io::{Read, Seek};

use map::build::{Block, FixupKind, TagPayload, TagSet};
use map::{FourCC, Map, TagId};
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
    source_map: String,
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
    /// Opaque copies of the pictures drawn without their own alpha
    /// ([`H1SkyShader::opaque_picture`]), named [`opaque_name`].
    pub opaque_bitmaps: Vec<(h1map::TagId, String, h1models::Converted)>,
    /// Per sky light, its lens flare ([`crate::lens`]), where it has one
    /// that reads.
    pub flares: Vec<Option<crate::lens::Flare>>,
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

/// The name of a bitmap's opaque copy ([`Sky::opaque_bitmaps`]).
pub fn opaque_name(name: &str) -> String {
    format!("{name} opaque")
}

/// `mode` materials block (count; pointer at `+4`), element size, and the
/// shader reference's datum in an element.
pub const MODE_MATERIALS: usize = 0x60;
pub const MODE_MATERIAL: usize = 0x20;
pub const MATERIAL_SHADER: usize = 0x08;

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
        let shaders = f
            .shaders
            .iter()
            .map(|&id| {
                let shader = h1.tag(id)?;
                read_h1(h1, id).map(Some).ok_or_else(|| {
                    Error::no_rebuild(
                        &h1.header().name,
                        &shader.name,
                        format!("{} sky material", shader.group),
                    )
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let mut bitmaps = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for m in shaders.iter().flatten().flat_map(|s| &s.maps) {
            if !seen.insert(m.bitmap) {
                continue;
            }
            let t = h1.tag(m.bitmap)?;
            let (_, c, _) = h1bitmaps::convert_bitmap(h1, t)
                .map_err(|e| Error::no_rebuild(&h1.header().name, &t.name, e))?;
            bitmaps.push((m.bitmap, t.name.clone(), c));
        }
        let mut opaque_bitmaps = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for s in shaders.iter().flatten() {
            let Some(k) = s.opaque_picture() else {
                continue;
            };
            let id = s.maps[k].bitmap;
            if !seen.insert(id) {
                continue;
            }
            let t = h1.tag(id)?;
            let (_, c, _) = h1bitmaps::convert::convert_bitmap_opaque(h1, t)
                .map_err(|e| Error::no_rebuild(&h1.header().name, &t.name, e))?;
            opaque_bitmaps.push((id, opaque_name(&t.name), c));
        }
        let flares = f
            .sky
            .lights
            .iter()
            .map(|l| {
                (!l.lens_flare.id.is_none())
                    .then(|| {
                        let tag = h1.tag(l.lens_flare.id)?;
                        crate::lens::Flare::read(h1, tag.id)
                            .map_err(|e| Error::no_rebuild(&h1.header().name, &tag.name, e))
                    })
                    .transpose()
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            source_map: h1.header().name.clone(),
            name: tag.name.clone(),
            flares,
            sky: f.sky,
            model: f.model,
            model_scale: f.model_scale,
            shaders,
            bitmaps,
            opaque_bitmaps,
        })
    }

    /// Adds the sky to `set`: its bitmaps, a shader copy per material
    /// (`templates`), the model and the sky; returns the sky's datum. A
    /// missing material fails conversion with its source path.
    pub fn install(&self, set: &mut TagSet, templates: &SkyTemplates) -> Result<TagId> {
        let err = |e: &dyn std::fmt::Display| Error::Sky(format!("{}: {e}", self.name));
        let model = match &self.model {
            Some((n, c)) => {
                let mut c = c.clone();
                // Only the bitmaps a shader copy draws
                // ([`SkyTemplates::drawn`]): a dome's star detail has no
                // stage, and nothing would name it. A picture drawn
                // through its opaque copy
                // (`sky_shaders::H1SkyShader::opaque_picture`)
                // is drawn as that copy, not itself.
                let drawn: std::collections::HashSet<h1map::TagId> = self
                    .shaders
                    .iter()
                    .flatten()
                    .filter(|s| set.converted(SHAD, &s.name).is_none())
                    .filter_map(|s| Some((s, recipe(s)?)))
                    .flat_map(|(s, r)| {
                        let picture = s.opaque_picture();
                        templates
                            .drawn(r)
                            .into_iter()
                            .filter(move |k| Some(*k) != picture)
                            .filter_map(|k| s.maps.get(k).map(|m| m.bitmap))
                    })
                    .collect();
                let mut ids = std::collections::HashMap::new();
                for (h1id, name, b) in self.bitmaps.iter().filter(|b| drawn.contains(&b.0)) {
                    let id = match set.converted(b.group, name) {
                        Some(id) => id,
                        None => b.clone().install(set, name).map_err(|e| err(&e))?,
                    };
                    ids.insert(*h1id, id);
                }
                let mut opaque = std::collections::HashMap::new();
                for (h1id, name, b) in &self.opaque_bitmaps {
                    let id = match set.converted(b.group, name) {
                        Some(id) => id,
                        None => b.clone().install(set, name).map_err(|e| err(&e))?,
                    };
                    opaque.insert(*h1id, id);
                }
                set.add_group(SHAD, None, None);
                for (k, s) in self.shaders.iter().enumerate() {
                    let copy = match s {
                        Some(s) => match set.converted(SHAD, &s.name) {
                            Some(id) => Some(id),
                            None => {
                                let picture = s.opaque_picture();
                                let maps: Vec<_> = s
                                    .maps
                                    .iter()
                                    .enumerate()
                                    .map(|(j, m)| {
                                        let id = if Some(j) == picture {
                                            opaque.get(&m.bitmap)
                                        } else {
                                            ids.get(&m.bitmap)
                                        };
                                        id.map(|&id| (id, m.scale, m.lod_offset))
                                    })
                                    .collect();
                                match recipe(s).and_then(|r| templates.copy(r, &maps, r.scroll(s))) {
                                    Some(tree) => {
                                        let id =
                                            set.add_tag(SHAD, &s.name, TagPayload::Built(tree))?;
                                        set.record_conversion(SHAD, &s.name, id);
                                        Some(id)
                                    }
                                    None => None,
                                }
                            }
                        },
                        None => None,
                    };
                    let id = copy.ok_or_else(|| {
                        Error::no_rebuild(
                            &self.source_map,
                            s.as_ref().map_or(n, |s| &s.name),
                            format!(
                                "sky {} material {k} lacks its shader program or bitmap",
                                self.name
                            ),
                        )
                    })?;
                    if !set_material_shader(&mut c.root, k, id) {
                        return Err(Error::no_rebuild(
                            &self.source_map,
                            n,
                            format!("missing material {k}"),
                        ));
                    }
                }
                Some((n.as_str(), c))
            }
            None => None,
        };
        // Each light's lens flare (`sky` light `+0x14`; E `0x0003eec0`
        // submits it), with its bitmap; one that doesn't install stays null.
        let flares: Vec<Option<TagId>> = self
            .flares
            .iter()
            .map(|f| f.as_ref().map(|f| f.install(set)).transpose())
            .collect::<Result<_>>()?;
        h1sky::convert::install_sky_with_flares(
            set,
            &self.sky,
            model,
            &self.name,
            self.model_scale,
            &flares,
        )
        .map_err(|e| err(&e))
    }
}

/// Converts every sky the Halo 1 scenario names.
pub fn read(h1: &h1map::H1Map) -> Result<Skies> {
    let mut out = Skies::default();
    for r in h1.multiplayer()?.skies {
        if !r.id.is_none() {
            out.skies.push(Sky::from_map(h1, h1.tag(r.id)?)?);
        }
    }
    Ok(out)
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
    let word = |b: &[u8], at: usize| -> Result<u32> {
        b.get(at..at + 4)
            .map(|b| u32::from_le_bytes(b.try_into().expect("4")))
            .ok_or(Error::Invalid("scenario skies outside the tag"))
    };
    let count = word(scnr, SKIES)? as usize;
    if ids.len() > count {
        return Err(Error::Invalid(
            "source sky palette exceeds pinned scenario capacity",
        ));
    }
    if ids.is_empty() {
        scnr[SKIES..SKIES + 4].fill(0);
        return Ok(0);
    }
    let start = word(scnr, SKIES + 4)?.wrapping_sub(address) as usize;
    if start + count * SKY > scnr.len() {
        return Err(Error::Invalid("scenario skies outside the tag"));
    }
    scnr[SKIES..SKIES + 4].copy_from_slice(&(ids.len() as u32).to_le_bytes());
    for (k, id) in ids.iter().enumerate() {
        let at = start + k * SKY;
        scnr[at..at + 4].copy_from_slice(&FourCC(*b"sky ").as_u32().to_le_bytes());
        scnr[at + 4..at + 8].copy_from_slice(&id.0.to_le_bytes());
    }
    Ok(ids.len())
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
        assert_eq!(patch_scenario(&mut b, address, &ids).unwrap(), 2);
        let datum =
            |k: usize| u32::from_le_bytes(b[0x24 + 8 * k..0x28 + 8 * k].try_into().unwrap());
        assert_eq!(
            [datum(0), datum(1)],
            [ids[0].0, ids[1].0]
        );
        assert_eq!(b[0x20..0x24], FourCC(*b"sky ").as_u32().to_le_bytes());
        assert_eq!(patch_scenario(&mut b, address, &[]).unwrap(), 0);
        assert_eq!(&b[SKIES..SKIES + 4], &0u32.to_le_bytes());
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
