//! The templates on the preview's map (`hud_kit_core::templates`, which
//! names what each element takes and resolves it): read through the HUD
//! editor's model and renderer, with the `hudg`'s arrows. The game's
//! runtime resolves the same templates through
//! `hud_kit_core::map_file::MapFile`; the disc test compares the two.

use std::io::{Read, Seek};

use anyhow::{Result, bail};
use hud_edit::model::{self, WidgetKind as EditKind};
use hud_edit::render::Art as Pictures;
use map::{FourCC, Map, TagId, TagQuery};

use hud_kit_core::WidgetKind;
pub use hud_kit_core::templates::{
    ArrowRef, ArtTemplate, BITM, FRAG_COUNT_INPUT, FRAG_SELECTED, GrenadeRow, MapView,
    PLASMA_COUNT_INPUT, PLASMA_SELECTED, PLAYER_HUD, RawHud, RawWidget, STRIP_BITMAP,
    STRIP_SEQUENCE, STRIP_SHADER, TEXT_TEMPLATE_INPUT, Templates, TextTemplate, end_crop, ramp,
};

use crate::hudg::{self, Hudg};
use crate::model::Hud;

/// The preview's map, as the resolver asks it.
pub struct EditView<'a, R: Read + Seek> {
    pub map: &'a mut Map<R>,
    pub art: &'a mut Pictures,
    pub hudg: Option<&'a Hudg>,
}

impl<R: Read + Seek> MapView for EditView<'_, R> {
    fn nhdt(&mut self, id: TagId) -> Result<RawHud, String> {
        let h = model::read(self.map, id).map_err(|e| e.to_string())?;
        Ok(RawHud {
            name: h.name,
            widgets: h
                .widgets
                .into_iter()
                .map(|w| RawWidget {
                    kind: match w.key.kind {
                        EditKind::Bitmap => WidgetKind::Bitmap,
                        EditKind::Text => WidgetKind::Text,
                    },
                    name: w.name,
                    bytes: w.bytes,
                })
                .collect(),
        })
    }

    fn sprite_size(&mut self, id: TagId, seq: i8) -> Result<[f32; 2], String> {
        let b = match tags::DecodedTag::read(self.map, id)
            .map_err(|e| e.to_string())?
            .data
        {
            tags::TagData::Bitmap(b) => b,
            _ => return Err(format!("{id} names no bitmap")),
        };
        Ok(hud_edit::render::sprite_at(&b, seq)?.size)
    }

    fn sprite_blues(&mut self, id: TagId, seq: i8) -> Result<Vec<f32>, String> {
        let s = self.art.sprite(self.map, id, seq)?;
        let rgba = s
            .rgba
            .ok_or_else(|| s.note.unwrap_or_else(|| "no texels".into()))?;
        let [w, h] = s.px;
        let y = h / 2;
        Ok((0..w)
            .map(|x| f32::from(rgba[((y * w + x) * 4 + 2) as usize]) / 255.0)
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
        let arrow = |a: Option<hudg::Arrow>, which: usize| match (self.hudg, a) {
            (None, _) => Err("the map has no hudg".to_owned()),
            (Some(_), None) => Err(format!(
                "the hudg has no arrow_assets[{which}] with an image and a shader"
            )),
            (Some(_), Some(a)) => Ok(ArrowRef {
                image: a.image,
                material: a.material,
                sequence: a.sequence,
            }),
        };
        let h = self.hudg;
        [
            arrow(h.and_then(|h| h.arrow), 0),
            arrow(h.and_then(|h| h.navpoint_arrow), hudg::NAVPOINT_ARROW),
        ]
    }
}

/// Resolves every template `hud` needs on `map`, for the player `nhdt`
/// `nhdt` (`art` decodes the strip's texels).
pub fn resolve<R: Read + Seek>(
    map: &mut Map<R>,
    art: &mut Pictures,
    nhdt: TagId,
    hudg: Option<&Hudg>,
    hud: &Hud,
) -> Templates {
    let mut view = EditView { map, art, hudg };
    hud_kit_core::templates::resolve(&mut view, nhdt, hud)
}

/// The `nhdt`s a biped's HUD interfaces name (`unit +0x1a0`), the ones
/// the kit's widgets join: [`PLAYER_HUD`] first when the map has it, then
/// the others by name, the Halo 1 converter's last.
pub fn player_huds<R: Read + Seek>(map: &mut Map<R>) -> Result<Vec<TagId>> {
    let mut out: Vec<_> = model::list(map)?
        .into_iter()
        .filter(|e| {
            e.owners
                .iter()
                .any(|o| o.group.0 == *b"bipd" && o.at.starts_with("unit +0x1a0"))
        })
        .collect();
    out.sort_by_key(|e| (e.name != PLAYER_HUD, e.halo1, e.name.clone()));
    if out.is_empty() {
        bail!("no biped names an nhdt in its HUD interfaces");
    }
    Ok(out.into_iter().map(|e| e.id).collect())
}
