//! What a compiled element takes from the loaded map (docs/game/hud-kit.md,
//! "The model"): for each [`Art`] the bitmap and shader datums and the
//! sequences, and for the texts the shader and string of a retail text
//! widget of the same `nhdt`. A missing template is named; nothing falls
//! back to other art.
//!
//! The map is read through a [`MapView`]: the preview's over the HUD
//! editor's reader (`hud-kit`), the game's over the loaded map file
//! ([`crate::map_file::MapFile`]). Both answer the same questions, so
//! both resolve the same templates.

use map::{FourCC, TagId};

use crate::Mode;
use crate::model::{Art, Hud, HudRef, Origin, PILL_BODY, PILL_END, PillPart};
use crate::records::{WidgetKind, bitmap as bf, text as tf};

pub const BITM: FourCC = FourCC(*b"bitm");
pub const NHDT: FourCC = FourCC(*b"nhdt");

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
    /// For a cropped meter ([`Art::StripMeterEnd`]): the `hud_input1` that
    /// lights none of the crop and the one that lights all of it. The
    /// meter shader lights the texels whose blue is below `hud_input1`, so
    /// the first is the blue of the crop's first column and the second the
    /// blue one column past its last, read from the strip's texels
    /// ([`ramp`]).
    pub ramp: Option<[f32; 2]>,
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
    /// The step from the plasma count to the frag count, which is the
    /// pills' step too, per screen mode.
    pub step: [(i16, i16); 3],
}

impl GrenadeRow {
    /// The anchor and offset `origin` names in `mode` (`None` for
    /// [`Origin::Anchor`]).
    pub fn at(&self, origin: Origin, mode: Mode) -> Option<(u16, (i16, i16))> {
        let m = mode.index();
        match origin {
            Origin::Anchor => None,
            Origin::NextGrenadePill => Some((self.pill_anchor, self.pill[m])),
            Origin::NextGrenadeCount => Some((self.count_anchor, self.count[m])),
        }
    }

    /// The row as a monitor's HUD sees it, with the retail pills gated off
    /// (`Describe::monitor`): the next pill and count are the row's first
    /// place, the plasma pill's and count's, the frag's less one step.
    pub fn first(&self) -> GrenadeRow {
        let back = |at: [(i16, i16); 3]| {
            std::array::from_fn(|i| {
                let ((x, y), (sx, sy)) = (at[i], self.step[i]);
                (x - 2 * sx, y - 2 * sy)
            })
        };
        GrenadeRow {
            pill: back(self.pill),
            count: back(self.count),
            ..*self
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

/// The crop of a sprite `width` pixels wide that keeps its last `n`
/// columns ([`Art::StripMeterEnd`]).
pub fn end_crop(width: f32, n: u8) -> Result<[f32; 4], String> {
    let n = f32::from(n);
    if width <= 0.0 || n <= 0.0 || n > width {
        return Err(format!("the last {n} columns of a {width}-wide strip"));
    }
    Ok([(width - n) / width, 1.0, 0.0, 1.0])
}

/// A cropped meter's [`ArtTemplate::ramp`] from the blue of each of the
/// crop's columns, left to right (0 to 1): the first column's blue, and
/// one column past the last on the same slope. A crop whose blue does not
/// rise is named.
pub fn ramp(blues: &[f32]) -> Result<[f32; 2], String> {
    let (Some(first), Some(last)) = (blues.first(), blues.last()) else {
        return Err("the crop has no columns".into());
    };
    if blues.len() < 2 || last <= first {
        return Err(format!(
            "the crop's blue does not rise ({first} to {last} over {} columns)",
            blues.len()
        ));
    }
    let step = (last - first) / (blues.len() - 1) as f32;
    Ok([*first, last + step])
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
    /// The templates of a monitor's HUD (`Describe::monitor`): the grenade
    /// row's origins are its first place ([`GrenadeRow::first`]), since the
    /// retail pills are not drawn there. The pills' art is unchanged.
    pub fn for_monitor(mut self) -> Self {
        self.row = self.row.map(|r| r.first());
        self
    }

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
        self.missing_of(hud, &hud.arts())
    }

    /// What the widgets alone need and is missing: what the runtime
    /// checks (the navpoint's arrow is the game's).
    pub fn missing_widgets(&self, hud: &Hud) -> Vec<String> {
        self.missing_of(hud, &hud.widget_arts())
    }

    fn missing_of(&self, hud: &Hud, arts: &[Art]) -> Vec<String> {
        let mut out = Vec::new();
        if hud.has_text()
            && let Err(e) = &self.text
        {
            out.push(format!("the text template: {e}"));
        }
        for a in arts {
            if let Err(e) = self.art(a) {
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
}

/// One retail widget record, as the map holds it.
#[derive(Clone, Debug, PartialEq)]
pub struct RawWidget {
    pub kind: WidgetKind,
    /// Its name, for messages.
    pub name: String,
    pub bytes: Vec<u8>,
}

impl RawWidget {
    fn u16(&self, at: usize) -> u16 {
        u16::from_le_bytes([self.bytes[at], self.bytes[at + 1]])
    }
    fn u32(&self, at: usize) -> u32 {
        u32::from_le_bytes(self.bytes[at..at + 4].try_into().unwrap())
    }
    /// The four show words (E `0x0013ac87`).
    pub fn show_words(&self) -> [u16; 4] {
        std::array::from_fn(|i| self.u16(bf::STATE + 2 * i))
    }
    pub fn inputs(&self) -> [u8; 4] {
        self.bytes[bf::INPUTS..bf::INPUTS + 4].try_into().unwrap()
    }
    pub fn anchor(&self) -> u16 {
        self.u16(bf::ANCHOR)
    }
    pub fn offset(&self, m: Mode) -> (i16, i16) {
        let at = match self.kind {
            WidgetKind::Bitmap => bf::OFFSETS[m.index()],
            WidgetKind::Text => tf::OFFSETS[m.index()],
        };
        (self.u16(at) as i16, self.u16(at + 2) as i16)
    }
    /// A tag reference's datum, `None` when null.
    fn datum(&self, at: usize) -> Option<TagId> {
        Some(self.u32(at + 4)).filter(|d| *d != u32::MAX).map(TagId)
    }
    pub fn bitmap(&self) -> Option<TagId> {
        (self.kind == WidgetKind::Bitmap)
            .then(|| self.datum(bf::BITMAP))
            .flatten()
    }
    pub fn shader(&self) -> Option<TagId> {
        match self.kind {
            WidgetKind::Bitmap => self.datum(bf::SHADER),
            WidgetKind::Text => self.datum(tf::SHADER),
        }
    }
    pub fn sequence(&self, m: Mode) -> i8 {
        self.bytes[bf::SEQUENCES + m.index()] as i8
    }
    pub fn string(&self) -> u32 {
        self.u32(tf::STRING)
    }
}

/// One `nhdt` of the map: its name and its widgets, bitmaps first.
#[derive(Clone, Debug, PartialEq)]
pub struct RawHud {
    pub name: String,
    pub widgets: Vec<RawWidget>,
}

/// An arrow of the `hudg` (`arrow_assets`): image, material, sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArrowRef {
    pub image: TagId,
    pub material: TagId,
    pub sequence: i16,
}

/// The map, as the resolver asks it.
pub trait MapView {
    /// The `nhdt` `id`'s widgets.
    fn nhdt(&mut self, id: TagId) -> Result<RawHud, String>;
    /// The size in HUD pixels of the sprite bitmap `id` draws at sequence
    /// `seq` (E `0x0022ae21`: the sequence's first sprite, else its first
    /// image whole, else image 0).
    fn sprite_size(&mut self, id: TagId, seq: i8) -> Result<[f32; 2], String>;
    /// The blue (0 to 1) of each column of that sprite, along its middle
    /// row.
    fn sprite_blues(&mut self, id: TagId, seq: i8) -> Result<Vec<f32>, String>;
    /// The loaded tag named `name` (in group `group`).
    fn named(&mut self, name: &str, group: Option<FourCC>) -> Result<TagId, String>;
    /// Tag `id`'s group.
    fn group_of(&mut self, id: TagId) -> Result<FourCC, String>;
    /// The `hudg`'s waypoint arrow (`arrow_assets[0]`) and the navpoints'
    /// arrow (`arrow_assets[1]`), for the preview; the runtime has none
    /// to give (the game draws its navpoints).
    fn arrows(&mut self) -> [Result<ArrowRef, String>; 2];
}

/// The first bitmap widget shown on word A `word` alone: a grenade pill.
fn pill(hud: &RawHud, word: u16) -> Result<&RawWidget, String> {
    hud.widgets
        .iter()
        .find(|w| w.kind == WidgetKind::Bitmap && w.show_words() == [word, 0, 0, 0])
        .ok_or_else(|| {
            format!(
                "{} has no bitmap widget shown on word A {word:#x} alone",
                hud.name
            )
        })
}

/// The first text widget of input `input` shown on word A bit `word`: a
/// grenade count (not its `_hidden` twin, which shows the other way).
fn count(hud: &RawHud, input: u8, word: u16) -> Result<&RawWidget, String> {
    hud.widgets
        .iter()
        .find(|w| {
            w.kind == WidgetKind::Text && w.inputs()[0] == input && w.show_words()[0] & word != 0
        })
        .ok_or_else(|| {
            format!(
                "{} has no text widget of input {input} shown on word A {word:#x}",
                hud.name
            )
        })
}

fn sprite_size(
    map: &mut dyn MapView,
    hud: &RawHud,
    w: &RawWidget,
    m: Mode,
) -> Result<[f32; 2], String> {
    let id = w
        .bitmap()
        .ok_or_else(|| format!("{}'s {} has no bitmap", hud.name, w.name))?;
    map.sprite_size(id, w.sequence(m))
}

/// The grenade row of `hud` ([`GrenadeRow`]).
pub fn grenade_row(map: &mut dyn MapView, hud: &RawHud) -> Result<GrenadeRow, String> {
    let (frag, plasma) = (pill(hud, FRAG_SELECTED)?, pill(hud, PLASMA_SELECTED)?);
    let fc = count(hud, FRAG_COUNT_INPUT, FRAG_SELECTED)?;
    let pc = count(hud, PLASMA_COUNT_INPUT, PLASMA_SELECTED)?;
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
        let s = sprite_size(map, hud, frag, m)?;
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
        step: steps,
    })
}

/// The indices, among `hud`'s bitmap widgets in block order, of the ones
/// a user in monitor mode still draws ([`crate::model::MONITOR_KEEPS`]).
pub fn monitor_keeps(hud: &RawHud) -> Vec<u32> {
    hud.widgets
        .iter()
        .filter(|w| w.kind == WidgetKind::Bitmap)
        .enumerate()
        .filter(|(_, w)| crate::model::MONITOR_KEEPS.contains(&w.name.as_str()))
        .map(|(i, _)| i as u32)
        .collect()
}

fn text_template(map: &mut dyn MapView, hud: &RawHud) -> Result<TextTemplate, String> {
    let w = hud
        .widgets
        .iter()
        .find(|w| w.kind == WidgetKind::Text && w.inputs()[0] == TEXT_TEMPLATE_INPUT)
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
        shader: (map.group_of(shader)?, shader),
        string: w.string(),
    })
}

fn arrow_template(
    map: &mut dyn MapView,
    arrow: Result<ArrowRef, String>,
) -> Result<ArtTemplate, String> {
    let arrow = arrow?;
    let seq =
        i8::try_from(arrow.sequence).map_err(|_| format!("arrow sequence {}", arrow.sequence))?;
    Ok(ArtTemplate {
        bitmap: arrow.image,
        shader: (map.group_of(arrow.material)?, arrow.material),
        sequences: [seq; 3],
        crop: None,
        ramp: None,
    })
}

fn strip(map: &mut dyn MapView) -> Result<ArtTemplate, String> {
    let bitmap = map.named(STRIP_BITMAP, Some(BITM))?;
    let shader = map.named(STRIP_SHADER, None)?;
    Ok(ArtTemplate {
        bitmap,
        shader: (map.group_of(shader)?, shader),
        sequences: [STRIP_SEQUENCE; 3],
        crop: None,
        ramp: None,
    })
}

fn widget_template(
    map: &mut dyn MapView,
    hud: &RawHud,
    w: &RawWidget,
) -> Result<ArtTemplate, String> {
    let bitmap = w
        .bitmap()
        .ok_or_else(|| format!("{}'s {} has no bitmap", hud.name, w.name))?;
    let shader = w
        .shader()
        .ok_or_else(|| format!("{}'s {} has no shader", hud.name, w.name))?;
    Ok(ArtTemplate {
        bitmap,
        shader: (map.group_of(shader)?, shader),
        sequences: Mode::ALL.map(|m| w.sequence(m)),
        crop: None,
        ramp: None,
    })
}

fn art_template(
    map: &mut dyn MapView,
    player: &Result<RawHud, String>,
    row: Result<&GrenadeRow, &String>,
    arrows: &[Result<ArrowRef, String>; 2],
    a: &Art,
) -> Result<ArtTemplate, String> {
    match a {
        Art::StripMeter => strip(map),
        Art::StripMeterEnd(n) => {
            let mut t = strip(map)?;
            let [w, _] = map.sprite_size(t.bitmap, STRIP_SEQUENCE)?;
            let crop = end_crop(w, *n)?;
            let blues = map.sprite_blues(t.bitmap, STRIP_SEQUENCE)?;
            let from = blues.len().saturating_sub(usize::from(*n));
            t.ramp = Some(ramp(&blues[from..]).map_err(|why| format!("{STRIP_BITMAP}: {why}"))?);
            t.crop = Some(crop);
            Ok(t)
        }
        Art::WaypointArrow => arrow_template(map, arrows[0].clone()),
        Art::NavpointArrow => arrow_template(map, arrows[1].clone()),
        Art::GrenadePill(part) => {
            let row = row.map_err(|why| format!("the grenade row: {why}"))?;
            let h = player.as_ref().map_err(Clone::clone)?;
            let mut t = widget_template(map, h, pill(h, FRAG_SELECTED)?)?;
            t.crop = Some(row.crop(*part));
            Ok(t)
        }
        Art::Widget { hud, name } => {
            let named;
            let h = match hud {
                HudRef::Player => player.as_ref().map_err(Clone::clone)?,
                HudRef::Named(n) => {
                    let id = map.named(n, Some(NHDT))?;
                    named = map.nhdt(id)?;
                    &named
                }
            };
            let w = h
                .widgets
                .iter()
                .find(|w| w.kind == WidgetKind::Bitmap && w.name == *name)
                .ok_or_else(|| format!("{} has no bitmap widget {name}", h.name))?;
            widget_template(map, h, w)
        }
    }
}

/// Resolves every template `hud` needs on `map`, for the player `nhdt`
/// `nhdt`.
pub fn resolve(map: &mut dyn MapView, nhdt: TagId, hud: &Hud) -> Templates {
    let player = map.nhdt(nhdt);
    let (text, row) = match &player {
        Ok(h) => (text_template(map, h), grenade_row(map, h)),
        Err(e) => (Err(e.clone()), Err(e.clone())),
    };
    let arrows = map.arrows();
    let art = hud
        .arts()
        .into_iter()
        .map(|a| {
            let t = art_template(map, &player, row.as_ref(), &arrows, &a);
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
