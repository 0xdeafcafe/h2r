//! What a compiled element takes from the loaded map (docs/game/hud-kit.md,
//! "The model"): for each [`Art`] the bitmap and shader datums and the
//! sequences, and for the texts the shader and string of a retail text
//! widget of the same `nhdt`. A missing template is named; nothing falls
//! back to other art.

use std::io::{Read, Seek};

use anyhow::{Result, bail};
use hud_edit::model::{self, WidgetKind};
use map::{FourCC, Map, TagId, TagQuery};

use crate::hudg::Hudg;
use crate::model::{Art, Hud, HudRef};

pub const BITM: FourCC = FourCC(*b"bitm");

/// The energy sword's battery strip: its bitmap and shader, as retail
/// `ui\hud\plasma_sword`'s battery meter names them, 92 by 8, filled where
/// the texture's blue is below `hud_input1` (docs/halo1/health.md; the
/// same pair as `h1objects::health_hud::STRIP_BITMAP` and `STRIP_SHADER`).
pub const STRIP_BITMAP: &str = "ui\\hud\\bitmaps\\new_hud\\meters\\disintegrator_meter";
pub const STRIP_SHADER: &str = "ui\\hud\\shaders\\ammo_meter";
/// The strip's sequence (`health_hud` draws sequence 0 in every mode).
pub const STRIP_SEQUENCE: i8 = 0;

/// The text template: the first text widget of the `nhdt` whose first
/// input is the frag grenade count (E `0x0013ad48`, input 20).
pub const TEXT_TEMPLATE_INPUT: u8 = 20;

/// The player's `nhdt` the preview draws, when the map has it (the
/// multiplayer Master Chief's: `hud_edit`'s disc test).
pub const PLAYER_HUD: &str = "ui\\hud\\masterchief";

/// A bitmap and a shader (group and datum), and a sequence per screen mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArtTemplate {
    pub bitmap: TagId,
    pub shader: (FourCC, TagId),
    pub sequences: [i8; 3],
}

/// A retail text widget's shader and string ID word: a text widget with
/// no shader or no string draws nothing (E `0x0022aa26`, `0x0022aa2f`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextTemplate {
    pub shader: (FourCC, TagId),
    pub string: u32,
}

/// The templates of one player `nhdt` for one HUD.
#[derive(Clone, Debug, PartialEq)]
pub struct Templates {
    /// The `nhdt` the kit's widgets join.
    pub nhdt: TagId,
    pub text: Result<TextTemplate, String>,
    pub art: Vec<(Art, Result<ArtTemplate, String>)>,
}

impl Templates {
    /// `a`'s template, or why there is none.
    pub fn art(&self, a: &Art) -> Result<&ArtTemplate, String> {
        match self.art.iter().find(|(x, _)| x == a) {
            Some((_, Ok(t))) => Ok(t),
            Some((_, Err(e))) => Err(e.clone()),
            None => Err(format!("{a:?} was not resolved")),
        }
    }

    /// What is missing, one line each.
    pub fn missing(&self, hud: &Hud) -> Vec<String> {
        let mut out = Vec::new();
        if hud.has_text()
            && let Err(e) = &self.text
        {
            out.push(format!("the text template: {e}"));
        }
        for a in hud.arts() {
            if let Err(e) = self.art(&a) {
                out.push(format!("{a:?}: {e}"));
            }
        }
        out
    }

    /// Resolves every template `hud` needs on `map`, for the player
    /// `nhdt` `nhdt`.
    pub fn resolve<R: Read + Seek>(
        map: &mut Map<R>,
        nhdt: TagId,
        hudg: Option<&Hudg>,
        hud: &Hud,
    ) -> Templates {
        let text = text_template(map, nhdt);
        let art = hud
            .arts()
            .into_iter()
            .map(|a| {
                let t = art_template(map, nhdt, hudg, &a);
                (a, t)
            })
            .collect();
        Templates { nhdt, text, art }
    }
}

fn group_of<R: Read + Seek>(map: &Map<R>, id: TagId) -> Result<FourCC, String> {
    map.tag(id)
        .map(|t| t.group)
        .map_err(|_| format!("{id} is not in the map"))
}

fn named<R: Read + Seek>(map: &Map<R>, name: &str, group: Option<FourCC>) -> Result<TagId, String> {
    let q = match group {
        Some(g) => TagQuery::NameInGroup(name, g),
        None => TagQuery::Name(name),
    };
    map.tag(q)
        .ok()
        .filter(|t| t.location.is_some())
        .map(|t| t.id)
        .ok_or_else(|| format!("no tag {name} in the map"))
}

fn text_template<R: Read + Seek>(map: &mut Map<R>, nhdt: TagId) -> Result<TextTemplate, String> {
    let hud = model::read(map, nhdt).map_err(|e| e.to_string())?;
    let w = hud
        .widgets
        .iter()
        .find(|w| w.key.kind == WidgetKind::Text && w.inputs()[0] == TEXT_TEMPLATE_INPUT)
        .ok_or_else(|| {
            format!(
                "{} has no text widget of input {TEXT_TEMPLATE_INPUT}",
                hud.name
            )
        })?;
    let shader = w
        .shader()
        .ok_or_else(|| format!("{}'s {} has no shader", hud.name, w.name))?;
    if w.string() == 0 {
        return Err(format!("{}'s {} has no string", hud.name, w.name));
    }
    Ok(TextTemplate {
        shader: (group_of(map, shader)?, shader),
        string: w.string(),
    })
}

fn art_template<R: Read + Seek>(
    map: &mut Map<R>,
    nhdt: TagId,
    hudg: Option<&Hudg>,
    a: &Art,
) -> Result<ArtTemplate, String> {
    match a {
        Art::StripMeter => {
            let bitmap = named(map, STRIP_BITMAP, Some(BITM))?;
            let shader = named(map, STRIP_SHADER, None)?;
            Ok(ArtTemplate {
                bitmap,
                shader: (group_of(map, shader)?, shader),
                sequences: [STRIP_SEQUENCE; 3],
            })
        }
        Art::WaypointArrow => {
            let arrow = hudg
                .ok_or("the map has no hudg")?
                .arrow
                .ok_or("the hudg has no arrow_assets[0] with an image and a shader")?;
            let seq = i8::try_from(arrow.sequence)
                .map_err(|_| format!("arrow sequence {}", arrow.sequence))?;
            Ok(ArtTemplate {
                bitmap: arrow.image,
                shader: (group_of(map, arrow.material)?, arrow.material),
                sequences: [seq; 3],
            })
        }
        Art::Widget { hud, name } => {
            let id = match hud {
                HudRef::Player => nhdt,
                HudRef::Named(n) => named(map, n, Some(model::NHDT))?,
            };
            let h = model::read(map, id).map_err(|e| e.to_string())?;
            let w = h
                .widgets
                .iter()
                .find(|w| w.key.kind == WidgetKind::Bitmap && w.name == *name)
                .ok_or_else(|| format!("{} has no bitmap widget {name}", h.name))?;
            let bitmap = w
                .bitmap()
                .ok_or_else(|| format!("{}'s {name} has no bitmap", h.name))?;
            let shader = w
                .shader()
                .ok_or_else(|| format!("{}'s {name} has no shader", h.name))?;
            Ok(ArtTemplate {
                bitmap,
                shader: (group_of(map, shader)?, shader),
                sequences: hud_edit::Mode::ALL.map(|m| w.sequence(m)),
            })
        }
    }
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
