//! What a compiled element takes from the loaded map (docs/game/hud-kit.md,
//! "The model"): for each [`Art`] the bitmap and shader datums and the
//! sequences, and for the texts the shader and string of a retail text
//! widget of the same `nhdt`. A missing template is named; nothing falls
//! back to other art.

use std::io::{Read, Seek};

use anyhow::{Result, bail};
use hud_edit::model::{self, WidgetKind};
use map::{FourCC, Map, TagId, TagQuery};

use hud_edit::Mode;

use crate::hudg::Hudg;
use crate::model::{Art, Hud, HudRef, Origin, PILL_BODY, PILL_END, PillPart};

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

/// Word A of the gate with the frag grenade selected, and with the
/// plasma (V: retail `ui\hud\masterchief` widgets 0 and 1 and the counts
/// `frag_count`, `plasma_count`; `hud_edit::state::HudState::masks`).
pub const FRAG_SELECTED: u16 = 0x04;
pub const PLASMA_SELECTED: u16 = 0x08;
/// The grenade counts' inputs (E `0x0013ad48`: 20 frag, 21 plasma).
pub const FRAG_COUNT_INPUT: u8 = 20;
pub const PLASMA_COUNT_INPUT: u8 = 21;

/// The player's `nhdt` the preview draws, when the map has it (the
/// multiplayer Master Chief's: `hud_edit`'s disc test).
pub const PLAYER_HUD: &str = "ui\\hud\\masterchief";

/// A bitmap and a shader (group and datum), a sequence per screen mode,
/// and the part of the sprite drawn (`[u0, u1, v0, v1]` in fractions of
/// it; `None` the whole sprite).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArtTemplate {
    pub bitmap: TagId,
    pub shader: (FourCC, TagId),
    pub sequences: [i8; 3],
    pub crop: Option<[f32; 4]>,
}

/// A retail text widget's shader and string ID word: a text widget with
/// no shader or no string draws nothing (E `0x0022aa26`, `0x0022aa2f`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextTemplate {
    pub shader: (FourCC, TagId),
    pub string: u32,
}

/// Where the player's grenade row would put a next pill and its count.
///
/// Retail's two pill widgets are one panel (V: `ui\hud\masterchief`
/// widgets 0 and 1, `grenade_bkd` sequences 0 and 1): both at the same
/// offset, each drawing the whole 103 by 19 sprite with both pills in it,
/// its own lit. The counts step 53 from the plasma's to the frag's, and
/// that step is the pills': the frag pill is the panel's last
/// `panel - |step|` pixels (the plasma's the first, the step's way round).
/// So per screen mode the next pill's top left is the frag pill's plus
/// the step, and the next count is the frag count plus the step, each in
/// the anchor of the frag widget it steps from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GrenadeRow {
    pub pill_anchor: u16,
    pub pill: [(i16, i16); 3],
    pub count_anchor: u16,
    pub count: [(i16, i16); 3],
    /// The panel sprite's width, in HUD pixels.
    pub panel: f32,
    /// Where the frag pill starts in it, and its width.
    pub frag: [f32; 2],
}

impl GrenadeRow {
    /// The anchor and offset `origin` names in `mode` (`None` for
    /// [`Origin::Anchor`]).
    pub fn at(&self, origin: Origin, mode: Mode) -> Option<(u16, (i16, i16))> {
        let m = Mode::ALL.iter().position(|x| *x == mode)?;
        match origin {
            Origin::Anchor => None,
            Origin::NextGrenadePill => Some((self.pill_anchor, self.pill[m])),
            Origin::NextGrenadeCount => Some((self.count_anchor, self.count[m])),
        }
    }

    /// The crop of the panel sprite that draws `part` of the frag pill.
    pub fn crop(&self, part: PillPart) -> [f32; 4] {
        let [start, width] = self.frag;
        let (from, to) = match part {
            PillPart::Body => (0.0, f32::from(PILL_BODY)),
            PillPart::End => (width - f32::from(PILL_END), width),
        };
        [
            (start + from) / self.panel,
            (start + to) / self.panel,
            0.0,
            1.0,
        ]
    }
}

/// The templates of one player `nhdt` for one HUD.
#[derive(Clone, Debug, PartialEq)]
pub struct Templates {
    /// The `nhdt` the kit's widgets join.
    pub nhdt: TagId,
    pub text: Result<TextTemplate, String>,
    pub art: Vec<(Art, Result<ArtTemplate, String>)>,
    pub row: Result<GrenadeRow, String>,
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
        if hud.uses_grenade_row()
            && let Err(e) = &self.row
        {
            out.push(format!("the grenade row: {e}"));
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
        let row = grenade_row(map, nhdt);
        let art = hud
            .arts()
            .into_iter()
            .map(|a| {
                let t = art_template(map, nhdt, hudg, row.as_ref(), &a);
                (a, t)
            })
            .collect();
        Templates {
            nhdt,
            text,
            art,
            row,
        }
    }
}

/// A widget's four show words (state bytes `+0x08`..`+0x0f`, E
/// `0x0013ac87`).
fn show_words(w: &model::Widget) -> [u16; 4] {
    let s = w.state();
    std::array::from_fn(|i| u16::from_le_bytes([s[2 * i], s[2 * i + 1]]))
}

/// The first bitmap widget shown on word A `word` alone: a grenade pill.
fn pill(hud: &model::Hud, word: u16) -> Result<&model::Widget, String> {
    hud.widgets
        .iter()
        .find(|w| w.key.kind == WidgetKind::Bitmap && show_words(w) == [word, 0, 0, 0])
        .ok_or_else(|| {
            format!(
                "{} has no bitmap widget shown on word A {word:#x} alone",
                hud.name
            )
        })
}

/// The first text widget of input `input` shown on word A bit `word`: a
/// grenade count (not its `_hidden` twin, which shows the other way).
fn count(hud: &model::Hud, input: u8, word: u16) -> Result<&model::Widget, String> {
    hud.widgets
        .iter()
        .find(|w| {
            w.key.kind == WidgetKind::Text && w.inputs()[0] == input && show_words(w)[0] & word != 0
        })
        .ok_or_else(|| {
            format!(
                "{} has no text widget of input {input} shown on word A {word:#x}",
                hud.name
            )
        })
}

/// The size of the sprite widget `w` draws in mode `m`, read from its
/// bitmap (`hud_edit::render::sprite_at`).
fn sprite_size<R: Read + Seek>(
    map: &mut Map<R>,
    hud: &model::Hud,
    w: &model::Widget,
    m: Mode,
) -> Result<[f32; 2], String> {
    let id = w
        .bitmap()
        .ok_or_else(|| format!("{}'s {} has no bitmap", hud.name, w.name))?;
    let b = match tags::DecodedTag::read(map, id)
        .map_err(|e| e.to_string())?
        .data
    {
        tags::TagData::Bitmap(b) => b,
        _ => return Err(format!("{}'s {} names no bitmap", hud.name, w.name)),
    };
    Ok(hud_edit::render::sprite_at(&b, w.sequence(m))?.size)
}

fn grenade_row<R: Read + Seek>(map: &mut Map<R>, nhdt: TagId) -> Result<GrenadeRow, String> {
    let hud = model::read(map, nhdt).map_err(|e| e.to_string())?;
    let (frag, plasma) = (pill(&hud, FRAG_SELECTED)?, pill(&hud, PLASMA_SELECTED)?);
    let fc = count(&hud, FRAG_COUNT_INPUT, FRAG_SELECTED)?;
    let pc = count(&hud, PLASMA_COUNT_INPUT, PLASMA_SELECTED)?;
    if frag.anchor() != plasma.anchor() || fc.anchor() != pc.anchor() {
        return Err(format!(
            "{}: the grenade pills or counts have two anchors",
            hud.name
        ));
    }
    let mut size = None;
    for m in Mode::ALL {
        if frag.offset(m) != plasma.offset(m) {
            return Err(format!(
                "{}: the grenade pills are not one panel ({} {:?}, {} {:?} {})",
                hud.name,
                frag.name,
                frag.offset(m),
                plasma.name,
                plasma.offset(m),
                m.label()
            ));
        }
        let s = sprite_size(map, &hud, frag, m)?;
        if size.is_some_and(|z| z != s) {
            return Err(format!(
                "{}'s {} is not one size in every screen mode",
                hud.name, frag.name
            ));
        }
        size = Some(s);
    }
    let panel = size.expect("three modes")[0];
    let steps = Mode::ALL.map(|m| {
        let ((fx, fy), (px, py)) = (fc.offset(m), pc.offset(m));
        (fx - px, fy - py)
    });
    let full = f32::from(steps[0].0);
    let width = panel - full.abs();
    if full == 0.0 || width <= f32::from(PILL_BODY + PILL_END) {
        return Err(format!(
            "{}: a step of {full} between the counts does not split the {panel}-wide pills",
            hud.name
        ));
    }
    // The frag pill: the panel's far end the step's way.
    let start = if full > 0.0 { panel - width } else { 0.0 };
    let pill = std::array::from_fn(|i| {
        let (x, y) = frag.offset(Mode::ALL[i]);
        let (sx, sy) = steps[i];
        (x + start as i16 + sx, y + sy)
    });
    let count = std::array::from_fn(|i| {
        let (x, y) = fc.offset(Mode::ALL[i]);
        let (sx, sy) = steps[i];
        (x + sx, y + sy)
    });
    Ok(GrenadeRow {
        pill_anchor: frag.anchor(),
        pill,
        count_anchor: fc.anchor(),
        count,
        panel,
        frag: [start, width],
    })
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
    row: Result<&GrenadeRow, &String>,
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
                crop: None,
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
                crop: None,
            })
        }
        Art::NavpointArrow => {
            let arrow = hudg
                .ok_or("the map has no hudg")?
                .navpoint_arrow
                .ok_or_else(|| {
                    format!(
                        "the hudg has no arrow_assets[{}] with an image and a shader",
                        crate::hudg::NAVPOINT_ARROW
                    )
                })?;
            let seq = i8::try_from(arrow.sequence)
                .map_err(|_| format!("arrow sequence {}", arrow.sequence))?;
            Ok(ArtTemplate {
                bitmap: arrow.image,
                shader: (group_of(map, arrow.material)?, arrow.material),
                sequences: [seq; 3],
                crop: None,
            })
        }
        Art::GrenadePill(part) => {
            let row = row.map_err(|why| format!("the grenade row: {why}"))?;
            let h = model::read(map, nhdt).map_err(|e| e.to_string())?;
            let w = pill(&h, FRAG_SELECTED)?;
            let bitmap = w
                .bitmap()
                .ok_or_else(|| format!("{}'s {} has no bitmap", h.name, w.name))?;
            let shader = w
                .shader()
                .ok_or_else(|| format!("{}'s {} has no shader", h.name, w.name))?;
            Ok(ArtTemplate {
                bitmap,
                shader: (group_of(map, shader)?, shader),
                sequences: Mode::ALL.map(|m| w.sequence(m)),
                crop: Some(row.crop(*part)),
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
                sequences: Mode::ALL.map(|m| w.sequence(m)),
                crop: None,
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
