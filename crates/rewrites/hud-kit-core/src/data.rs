//! A HUD as data (docs/game/hud-kit.md, "HUDs as data"): the YAML of a
//! package's `hud` item read into a [`Hud`].
//!
//! ```yaml
//! name: superbounce
//! elements:
//!   - id: sb.arm
//!     kind: meter
//!     art: {type: widget, hud: 'ui\hud\scoreboard', name: bomb_defusal}
//!     at: {anchor: top-left, full: [160, 325], half: [272, 67], quarter: [129, 67], registration: [0.5, 0.5]}
//!     hide_when_d: 1024
//!   - id: sb.prompt
//!     kind: prompt
//!     longest: Find a crouch spot to charge again
//! ```
//!
//! An element's `kind` is `text`, `meter`, `icon`, `pointer`, `gauge`,
//! `message`, `prompt`, `training`, `banner`, `navpoint` or `world-mark`.
//! What each takes:
//!
//! - `text`: `font` (`hud`, the default, or `lcd`), `justify` (`start`, the
//!   default, `centre` or `end`), `longest`, `at`;
//! - `meter`, `icon`, `pointer`: `art`, `at`;
//! - `gauge`: `glyphs` (`first`, `steps`, `size`), `at`;
//! - the channels `message`, `prompt`, `training`, `banner`, `navpoint`:
//!   `longest`, and `at` when it is not the game's place (fullscreen only);
//! - `world-mark`: `shape` (`pipper`, `bracket`, `arrow`, `ticks`, `bar`,
//!   `line`, `segment`), `longest` (default empty), `at` as a channel's.
//!
//! Every element may also say `over` (the element it is drawn on),
//! `hide_when_d` (the bits of the game's word D that hide it) and `outside`
//! (HUD pixels it may sit outside the frame). `at` is `anchor` (`top-right`,
//! `top-left`, `bottom-left`, `bottom-right`, `centre`), `full`, and `half`
//! and `quarter` when the element has those modes, `registration`, `origin`
//! and `split_origin` (`anchor`, `next-grenade-pill`, `next-grenade-count`).
//! `art` is `type` (`strip-meter`, `strip-meter-end` with `columns`,
//! `waypoint-arrow`, `navpoint-arrow`, `grenade-pill` with `part` `body` or
//! `end`, `widget` with `name` and `hud`, the retail `nhdt`
//! it is in by tag name, left out for the player's). Unknown keys are refused.
//!
//! The strings of a [`Hud`] are `'static`: each text read is kept once for
//! the life of the process ([`intern`]).
use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use serde::Deserialize;

use crate::model::{
    Anchor, Art, Element, Font, Glyphs, Hud, HudRef, Justify, Kind, Origin, PillPart, Place,
};
use crate::world::Shape;

/// Why a HUD's YAML is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    /// The YAML's line, when the text could not be read at all.
    pub line: Option<usize>,
    pub why: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(l) => write!(f, "line {l}: {}", self.why),
            None => f.write_str(&self.why),
        }
    }
}

impl std::error::Error for Error {}

fn fail<T>(why: impl Into<String>) -> Result<T, Error> {
    Err(Error {
        line: None,
        why: why.into(),
    })
}

/// `text` kept for the life of the process, once.
pub fn intern(text: &str) -> &'static str {
    static KEPT: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut kept = KEPT
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(s) = kept.get(text) {
        return s;
    }
    let s: &'static str = Box::leak(text.to_owned().into_boxed_str());
    kept.insert(s);
    s
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HudFile {
    name: String,
    elements: Vec<ElementFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ElementFile {
    id: String,
    kind: KindName,
    #[serde(default)]
    art: Option<ArtFile>,
    #[serde(default)]
    font: Option<FontName>,
    #[serde(default)]
    justify: Option<JustifyName>,
    #[serde(default)]
    longest: Option<String>,
    #[serde(default)]
    glyphs: Option<GlyphsFile>,
    #[serde(default)]
    shape: Option<ShapeName>,
    #[serde(default)]
    at: Option<PlaceFile>,
    #[serde(default)]
    over: Option<String>,
    #[serde(default)]
    hide_when_d: u16,
    #[serde(default)]
    outside: i16,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum KindName {
    Text,
    Meter,
    Icon,
    Pointer,
    Gauge,
    Message,
    Prompt,
    Training,
    Banner,
    Navpoint,
    WorldMark,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
enum FontName {
    Hud,
    Lcd,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
enum JustifyName {
    Start,
    Centre,
    End,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
enum ShapeName {
    Pipper,
    Bracket,
    Arrow,
    Ticks,
    Bar,
    Line,
    Segment,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
enum AnchorName {
    TopRight,
    TopLeft,
    BottomLeft,
    BottomRight,
    Centre,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
enum OriginName {
    Anchor,
    NextGrenadePill,
    NextGrenadeCount,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
enum PartName {
    Body,
    End,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
enum ArtName {
    StripMeter,
    StripMeterEnd,
    WaypointArrow,
    NavpointArrow,
    GrenadePill,
    Widget,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtFile {
    #[serde(rename = "type")]
    art: ArtName,
    #[serde(default)]
    columns: Option<u8>,
    #[serde(default)]
    part: Option<PartName>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    hud: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GlyphsFile {
    first: u16,
    steps: u8,
    size: [i16; 2],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PlaceFile {
    #[serde(default = "top_left")]
    anchor: AnchorName,
    full: [i16; 2],
    #[serde(default)]
    half: Option<[i16; 2]>,
    #[serde(default)]
    quarter: Option<[i16; 2]>,
    #[serde(default)]
    registration: Option<[f32; 2]>,
    #[serde(default)]
    origin: Option<OriginName>,
    #[serde(default)]
    split_origin: Option<OriginName>,
}

fn top_left() -> AnchorName {
    AnchorName::TopLeft
}

/// Read a HUD's YAML.
pub fn parse(yaml: &str) -> Result<Hud, Error> {
    let file: HudFile = serde_yaml_ng::from_str(yaml).map_err(|e| Error {
        line: e.location().map(|l| l.line()),
        why: e.to_string(),
    })?;
    if file.name.is_empty() {
        return fail("the HUD has no name");
    }
    let mut elements = Vec::with_capacity(file.elements.len());
    for e in file.elements {
        let id = e.id.clone();
        elements.push(element(e).map_err(|mut err| {
            err.why = format!("element {id:?}: {}", err.why);
            err
        })?);
    }
    let hud = Hud {
        name: intern(&file.name),
        elements,
    };
    if let Some(p) = hud.problems().into_iter().next() {
        return fail(p);
    }
    Ok(hud)
}

fn element(e: ElementFile) -> Result<Element, Error> {
    if e.id.is_empty() {
        return fail("has no id");
    }
    let name = |k: KindName| match k {
        KindName::Text => "a text",
        KindName::Meter => "a meter",
        KindName::Icon => "an icon",
        KindName::Pointer => "a pointer",
        KindName::Gauge => "a gauge",
        KindName::Message => "a message",
        KindName::Prompt => "a prompt",
        KindName::Training => "a training box",
        KindName::Banner => "a banner",
        KindName::Navpoint => "a navpoint",
        KindName::WorldMark => "a world mark",
    };
    let has_art = matches!(
        e.kind,
        KindName::Meter | KindName::Icon | KindName::Pointer
    );
    let has_longest = !matches!(
        e.kind,
        KindName::Meter | KindName::Icon | KindName::Pointer | KindName::Gauge
    );
    let takes = |field: &str, given: bool, ok: bool| -> Result<(), Error> {
        if given && !ok {
            return fail(format!("{} takes no `{field}`", name(e.kind)));
        }
        Ok(())
    };
    takes("art", e.art.is_some(), has_art)?;
    takes("longest", e.longest.is_some(), has_longest)?;
    takes("font", e.font.is_some(), e.kind == KindName::Text)?;
    takes("justify", e.justify.is_some(), e.kind == KindName::Text)?;
    takes("glyphs", e.glyphs.is_some(), e.kind == KindName::Gauge)?;
    takes("shape", e.shape.is_some(), e.kind == KindName::WorldMark)?;

    let longest = || -> Result<&'static str, Error> {
        match (&e.longest, e.kind) {
            (Some(l), _) => Ok(intern(l)),
            (None, KindName::WorldMark) => Ok(""),
            (None, _) => fail("needs `longest`, the widest text any state gives it"),
        }
    };
    let kind = match e.kind {
        KindName::Text => Kind::Text {
            font: match e.font.unwrap_or(FontName::Hud) {
                FontName::Hud => Font::Hud,
                FontName::Lcd => Font::Lcd,
            },
            justify: match e.justify.unwrap_or(JustifyName::Start) {
                JustifyName::Start => Justify::Start,
                JustifyName::Centre => Justify::Centre,
                JustifyName::End => Justify::End,
            },
            longest: longest()?,
        },
        KindName::Meter | KindName::Icon | KindName::Pointer => {
            let Some(art) = &e.art else {
                return fail(format!("{} needs `art`", name(e.kind)));
            };
            let art = art_of(art)?;
            match e.kind {
                KindName::Meter => Kind::Meter { art },
                KindName::Icon => Kind::Icon { art },
                _ => Kind::Pointer { art },
            }
        }
        KindName::Gauge => {
            let Some(g) = &e.glyphs else {
                return fail("a gauge needs `glyphs`");
            };
            Kind::Gauge {
                glyphs: Glyphs {
                    first: g.first,
                    steps: g.steps,
                    size: (g.size[0], g.size[1]),
                },
            }
        }
        KindName::Message => Kind::Message {
            longest: longest()?,
        },
        KindName::Prompt => Kind::Prompt {
            longest: longest()?,
        },
        KindName::Training => Kind::Training {
            longest: longest()?,
        },
        KindName::Banner => Kind::Banner {
            longest: longest()?,
        },
        KindName::Navpoint => Kind::Navpoint {
            longest: longest()?,
        },
        KindName::WorldMark => {
            let Some(shape) = e.shape else {
                return fail("a world mark needs `shape`");
            };
            Kind::WorldMark {
                shape: match shape {
                    ShapeName::Pipper => Shape::Pipper,
                    ShapeName::Bracket => Shape::Bracket,
                    ShapeName::Arrow => Shape::Arrow,
                    ShapeName::Ticks => Shape::Ticks,
                    ShapeName::Bar => Shape::Bar,
                    ShapeName::Line => Shape::Line,
                    ShapeName::Segment => Shape::Segment,
                },
                longest: longest()?,
            }
        }
    };
    let at = match &e.at {
        Some(p) => place_of(p),
        None if kind.is_channel() => Place::GAME,
        None => return fail(format!("{} needs `at`", name(e.kind))),
    };
    Ok(Element {
        id: intern(&e.id),
        kind,
        at,
        over: e.over.as_deref().map(intern),
        hide_when_d: e.hide_when_d,
        outside: e.outside,
    })
}

fn art_of(a: &ArtFile) -> Result<Art, Error> {
    let only = |field: &str, given: bool, ok: bool| -> Result<(), Error> {
        if given && !ok {
            return fail(format!("this art takes no `{field}`"));
        }
        Ok(())
    };
    only(
        "columns",
        a.columns.is_some(),
        matches!(a.art, ArtName::StripMeterEnd),
    )?;
    only(
        "part",
        a.part.is_some(),
        matches!(a.art, ArtName::GrenadePill),
    )?;
    only(
        "name",
        a.name.is_some(),
        matches!(a.art, ArtName::Widget),
    )?;
    only("hud", a.hud.is_some(), matches!(a.art, ArtName::Widget))?;
    Ok(match a.art {
        ArtName::StripMeter => Art::StripMeter,
        ArtName::StripMeterEnd => match a.columns {
            Some(n) => Art::StripMeterEnd(n),
            None => return fail("strip-meter-end needs `columns`"),
        },
        ArtName::WaypointArrow => Art::WaypointArrow,
        ArtName::NavpointArrow => Art::NavpointArrow,
        ArtName::GrenadePill => match a.part {
            Some(PartName::Body) => Art::GrenadePill(PillPart::Body),
            Some(PartName::End) => Art::GrenadePill(PillPart::End),
            None => return fail("grenade-pill needs `part`, `body` or `end`"),
        },
        ArtName::Widget => {
            let Some(name) = &a.name else {
                return fail("widget art needs `name`, the retail widget's");
            };
            Art::Widget {
                hud: match &a.hud {
                    Some(h) => HudRef::Named(intern(h)),
                    None => HudRef::Player,
                },
                name: intern(name),
            }
        }
    })
}

fn place_of(p: &PlaceFile) -> Place {
    let mut at = Place::new(
        match p.anchor {
            AnchorName::TopRight => Anchor::TopRight,
            AnchorName::TopLeft => Anchor::TopLeft,
            AnchorName::BottomLeft => Anchor::BottomLeft,
            AnchorName::BottomRight => Anchor::BottomRight,
            AnchorName::Centre => Anchor::Centre,
        },
        (p.full[0], p.full[1]),
    );
    at.half = p.half.map(|h| (h[0], h[1]));
    at.quarter = p.quarter.map(|h| (h[0], h[1]));
    if let Some(r) = p.registration {
        at.registration = r;
    }
    let origin = |o: OriginName| match o {
        OriginName::Anchor => Origin::Anchor,
        OriginName::NextGrenadePill => Origin::NextGrenadePill,
        OriginName::NextGrenadeCount => Origin::NextGrenadeCount,
    };
    if let Some(o) = p.origin {
        at.origin = origin(o);
    }
    at.split_origin = p.split_origin.map(origin);
    at
}

/// The HUDs of ours, each one's YAML: the files of the first-party
/// packages' `hud` items (`content/<package>/huds/`), which `hud_kit_core`
/// reads for the HUDs it knows (`views`).
pub const BUILTIN: [(&str, &str); 14] = [
    (
        "superbounce",
        include_str!("../../../../content/superbounce/huds/superbounce.hud.yaml"),
    ),
    (
        "superbounce-readout",
        include_str!("../../../../content/superbounce/huds/superbounce-readout.hud.yaml"),
    ),
    (
        "pill-block",
        include_str!("../../../../content/superbounce/huds/pill-block.hud.yaml"),
    ),
    (
        "forge",
        include_str!("../../../../content/forge/huds/forge.hud.yaml"),
    ),
    (
        "forge-readout",
        include_str!("../../../../content/forge/huds/forge-readout.hud.yaml"),
    ),
    (
        "firefight",
        include_str!("../../../../content/modes/huds/firefight.hud.yaml"),
    ),
    (
        "script-objective",
        include_str!("../../../../content/modes/huds/script-objective.hud.yaml"),
    ),
    (
        "base",
        include_str!("../../../../content/modes/huds/base.hud.yaml"),
    ),
    (
        "ctf",
        include_str!("../../../../content/modes/huds/ctf.hud.yaml"),
    ),
    (
        "assault",
        include_str!("../../../../content/modes/huds/assault.hud.yaml"),
    ),
    (
        "oddball",
        include_str!("../../../../content/modes/huds/oddball.hud.yaml"),
    ),
    (
        "koth",
        include_str!("../../../../content/modes/huds/koth.hud.yaml"),
    ),
    (
        "territories",
        include_str!("../../../../content/modes/huds/territories.hud.yaml"),
    ),
    (
        "theater",
        include_str!("../../../../content/core/huds/theater.hud.yaml"),
    ),
];

/// The built-in HUD named `name`. The files are ours and a test reads each;
/// one that does not read is a bug in the build, not input.
pub fn builtin(name: &str) -> Hud {
    static READ: OnceLock<HashMap<&'static str, Hud>> = OnceLock::new();
    READ.get_or_init(|| {
        BUILTIN
            .iter()
            .map(|(n, yaml)| {
                let hud = parse(yaml).unwrap_or_else(|e| panic!("the {n} HUD's YAML: {e}"));
                assert_eq!(hud.name, *n, "the {n} HUD's YAML names another HUD");
                (*n, hud)
            })
            .collect()
    })
    .get(name)
    .unwrap_or_else(|| panic!("no built-in HUD {name:?}"))
    .clone()
}
