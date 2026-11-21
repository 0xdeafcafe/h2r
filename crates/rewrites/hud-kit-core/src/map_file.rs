//! A [`MapView`] over an open map file and its resource files: what the
//! game's runtime resolves the templates from at each game start (the
//! loaded map, through `game::native_map::open`). The reads are those of
//! the HUD editor (`hud_edit::model::read`, `hud_edit::render::sprite_at`
//! and `Art::sprite`), so the runtime's templates are the preview's
//! (`hud-kit`'s disc test compares the two on the disc).

use std::collections::HashMap;
use std::io::{Read, Seek};

use assets::ResourceFiles;
use map::{FourCC, Map, TagId, TagQuery};

use crate::records::{ROOT_BITMAPS, ROOT_SIZE, ROOT_TEXTS, WidgetKind};
use crate::templates::{ArrowRef, MapView, NHDT, PLAYER_HUD, RawHud, RawWidget};

/// `unit +0x1a0`: a unit's HUD interfaces (count, address), 8-byte `nhdt`
/// references, the datum at `+4` (Code `game::hud_swap`).
pub const UNIT_HUD_INTERFACES: usize = 0x1a0;
/// The suffixes of the `nhdt`s the Halo 1 converter writes
/// (`hud_edit::model::HALO1_SUFFIXES`), ordered last.
pub const HALO1_SUFFIXES: [&str; 3] = ["_h1_hud", "_h1_weapon_hud", "_h1_health"];

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

/// An open map and its resources.
pub struct MapFile<'a, R: Read + Seek> {
    pub map: &'a mut Map<R>,
    pub files: &'a mut ResourceFiles,
    bitmaps: HashMap<TagId, Option<tags::Bitmap>>,
}

impl<'a, R: Read + Seek> MapFile<'a, R> {
    pub fn new(map: &'a mut Map<R>, files: &'a mut ResourceFiles) -> Self {
        MapFile {
            map,
            files,
            bitmaps: HashMap::new(),
        }
    }

    fn bitmap(&mut self, id: TagId) -> Result<&tags::Bitmap, String> {
        let map = &mut *self.map;
        self.bitmaps
            .entry(id)
            .or_insert_with(|| match tags::DecodedTag::read(map, id).ok()?.data {
                tags::TagData::Bitmap(b) => Some(*b),
                _ => None,
            })
            .as_ref()
            .ok_or_else(|| format!("{id} is not a readable bitmap"))
    }

    /// The image and the bounds (`[u0, u1, v0, v1]`) sequence `seq` draws
    /// (E `0x0022ae21`; `hud_edit::render::sprite_at`).
    fn sprite_at(&mut self, id: TagId, seq: i8) -> Result<(usize, [f32; 4], [f32; 2]), String> {
        let b = self.bitmap(id)?;
        let pick = usize::try_from(seq).ok().and_then(|s| b.sequences.get(s));
        let (image, bounds) = match pick {
            Some(s) if !s.sprites.is_empty() => {
                let p = &s.sprites[0];
                (usize::try_from(p.image_index).unwrap_or(0), p.bounds)
            }
            Some(s) => (
                usize::try_from(s.wire.first_image).unwrap_or(0),
                [0.0, 1.0, 0.0, 1.0],
            ),
            None => (0, [0.0, 1.0, 0.0, 1.0]),
        };
        let img = b
            .images
            .get(image)
            .ok_or_else(|| format!("image {image} is not in the bitmap"))?;
        let [iw, ih, _] = img.dimensions.map(f32::from);
        let size = [(bounds[1] - bounds[0]) * iw, (bounds[3] - bounds[2]) * ih];
        Ok((image, bounds, size))
    }
}

impl<R: Read + Seek> MapView for MapFile<'_, R> {
    fn nhdt(&mut self, id: TagId) -> Result<RawHud, String> {
        let tag = self.map.tag(id).map_err(|e| e.to_string())?.clone();
        if tag.group != NHDT {
            return Err(format!("{id} is a {} tag, not an nhdt", tag.group));
        }
        let name = tag.name.clone().unwrap_or_else(|| format!("{id}"));
        let root = self.map.read_tag_bytes(id).map_err(|e| e.to_string())?;
        if root.len() < ROOT_SIZE {
            return Err(format!("{name}: the nhdt root is {} bytes", root.len()));
        }
        let mut widgets = Vec::new();
        for kind in [WidgetKind::Bitmap, WidgetKind::Text] {
            let at = kind.root_field() as usize;
            let (count, address) = (u32_at(&root, at), u32_at(&root, at + 4));
            if count == 0 {
                continue;
            }
            if count > 1024 {
                return Err(format!("{name}: {count} {kind:?} widgets"));
            }
            let size = kind.size() as u32;
            let bytes = self
                .map
                .file_offset(address, count * size)
                .and_then(|o| self.map.read_file_range(o, u64::from(count * size)))
                .map_err(|e| format!("{name}: {kind:?} widgets at {address:#x}: {e}"))?;
            for (i, w) in bytes.chunks_exact(size as usize).enumerate() {
                let label = tags::StringId(u32_at(w, 0))
                    .resolve(self.map.strings())
                    .ok()
                    .flatten()
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("{kind:?} {i}"));
                widgets.push(RawWidget {
                    kind,
                    name: label,
                    bytes: w.to_vec(),
                });
            }
        }
        Ok(RawHud { name, widgets })
    }

    fn sprite_size(&mut self, id: TagId, seq: i8) -> Result<[f32; 2], String> {
        Ok(self.sprite_at(id, seq)?.2)
    }

    fn sprite_blues(&mut self, id: TagId, seq: i8) -> Result<Vec<f32>, String> {
        let (image, bounds, size) = self.sprite_at(id, seq)?;
        let img = self.bitmap(id)?.images[image].clone();
        let d = assets::decode_image(&img, &mut *self.files).map_err(|e| e.to_string())?;
        let l = d
            .levels
            .into_iter()
            .find(|l| l.face == 0 && l.level == 0)
            .ok_or("no top level")?;
        let (w, h) = (l.width, l.height);
        if w == 0 || h == 0 || l.rgba.len() < (w * h * 4) as usize {
            return Err(format!("{id}: a short image"));
        }
        let px = size.map(|v| v.round().max(0.0) as u32);
        let x0 = (bounds[0] * w as f32).round() as u32;
        let y0 = (bounds[2] * h as f32).round() as u32;
        let y = (y0 + px[1] / 2).min(h - 1);
        Ok((0..px[0])
            .map(|x| {
                let sx = (x0 + x).min(w - 1);
                f32::from(l.rgba[((y * w + sx) * 4 + 2) as usize]) / 255.0
            })
            .collect())
    }

    fn named(&mut self, name: &str, group: Option<FourCC>) -> Result<TagId, String> {
        let q = match group {
            Some(g) => TagQuery::NameInGroup(name, g),
            None => TagQuery::Name(name),
        };
        self.map
            .tag(q)
            .ok()
            .filter(|t| t.location.is_some())
            .map(|t| t.id)
            .ok_or_else(|| format!("no tag {name} in the map"))
    }

    fn group_of(&mut self, id: TagId) -> Result<FourCC, String> {
        self.map
            .tag(id)
            .map(|t| t.group)
            .map_err(|_| format!("{id} is not in the map"))
    }

    fn arrows(&mut self) -> [Result<ArrowRef, String>; 2] {
        let none =
            || Err("the game draws its own navpoints: no arrow is resolved for them".to_owned());
        [none(), none()]
    }
}

/// The `nhdt`s a biped's HUD interfaces name, once each: [`PLAYER_HUD`]
/// first when the map has it, then the others by name, the Halo 1
/// converter's last (`hud_edit::model::list`'s order).
pub fn player_huds<R: Read + Seek>(map: &mut Map<R>) -> Vec<TagId> {
    let bipeds: Vec<TagId> = map
        .tags()
        .iter()
        .filter(|t| t.location.is_some() && t.group.0 == *b"bipd")
        .map(|t| t.id)
        .collect();
    let mut out: Vec<(bool, bool, String, TagId)> = Vec::new();
    for b in bipeds {
        let Ok(root) = map.read_tag_bytes(b) else {
            continue;
        };
        if root.len() < UNIT_HUD_INTERFACES + 8 {
            continue;
        }
        let count = u32_at(&root, UNIT_HUD_INTERFACES);
        let address = u32_at(&root, UNIT_HUD_INTERFACES + 4);
        if count == 0 || count >= 64 {
            continue;
        }
        let Ok(block) = map
            .file_offset(address, count * 8)
            .and_then(|o| map.read_file_range(o, u64::from(count) * 8))
        else {
            continue;
        };
        for r in block.chunks_exact(8) {
            let id = TagId(u32_at(r, 4));
            let Ok(t) = map.tag(id) else { continue };
            if t.group != NHDT || t.location.is_none() || out.iter().any(|o| o.3 == id) {
                continue;
            }
            let name = t.name.clone().unwrap_or_default();
            let halo1 = HALO1_SUFFIXES.iter().any(|s| name.ends_with(s));
            out.push((name != PLAYER_HUD, halo1, name, id));
        }
    }
    out.sort_by(|a, b| (a.0, a.1, &a.2).cmp(&(b.0, b.1, &b.2)));
    out.into_iter().map(|o| o.3).collect()
}
