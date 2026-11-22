//! Our screens' presentation as data (docs/game/ui-builder.md "Screens in
//! YAML"): the design system's tokens in `assets/ui/design.yaml`
//! ([`DesignDoc`]) and each screen's widgets, texts, places and components
//! in `assets/ui/screens/<name>.yaml` ([`ScreenDoc`]). Rust keeps
//! behaviour; it reads these, baked in at compile time ([`baked`]), and
//! the editor swaps a doc it has edited in ([`reload`]).
//!
//! A doc that names a component, token, glyph, layout or slot that doesn't
//! exist is an error ([`DataError`]), never a quiet fallback: the tests
//! load and validate every baked file.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, RwLock};

use indexmap::IndexMap;
use serde::Deserialize;

use crate::ui_kit::input::{Glyph, Prompt};

pub mod baked;
mod emit;
pub mod export;
mod present;

pub use emit::to_yaml;
pub use present::present;

/// What is wrong with a doc, by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataError {
    /// The file (`design.yaml`, `screens/coop.yaml`).
    pub file: String,
    /// Where in it (`widgets.title.component`), empty for the whole file.
    pub at: String,
    pub what: String,
}

impl DataError {
    pub(crate) fn new(file: &str, at: &str, what: impl Into<String>) -> Self {
        DataError {
            file: file.into(),
            at: at.into(),
            what: what.into(),
        }
    }
}

impl fmt::Display for DataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.at.is_empty() {
            write!(f, "assets/ui/{}: {}", self.file, self.what)
        } else {
            write!(f, "assets/ui/{} {}: {}", self.file, self.at, self.what)
        }
    }
}

impl std::error::Error for DataError {}

// ---- the design system ----

/// `assets/ui/design.yaml`: the palette, the fonts by role, spacing, the
/// legend's format and every component's preset.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignDoc {
    /// Named colours, red green blue in 0..1.
    pub palette: IndexMap<String, [f32; 3]>,
    /// Font indices by role (`title`, `body`, `row_controls` ...).
    pub fonts: IndexMap<String, i16>,
    /// Named distances in UI units (row heights, gaps).
    pub spacing: IndexMap<String, i32>,
    /// Named alphas (an unfocused row's text, its frame).
    pub alpha: IndexMap<String, u8>,
    pub legend: LegendFormat,
    /// Each component's preset, by name (`title`, `row.action` ...).
    pub components: IndexMap<String, ComponentDoc>,
}

/// How a legend is written: each prompt's glyphs `glyph_gap` spaces
/// apart, one space, its words (in capitals when `words` is `upper`),
/// prompts `prompt_gap` spaces apart.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegendFormat {
    pub glyph_gap: usize,
    pub prompt_gap: usize,
    pub words: String,
}

/// A component's preset.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentDoc {
    /// What it is for.
    pub role: String,
    /// A `fonts` key.
    pub font: Option<String>,
    /// A `palette` key.
    pub colour: Option<String>,
    /// `left`, `right` or `centre`.
    pub justify: Option<String>,
    /// Its place when a screen gives none: top-left, UI units.
    pub at: Option<[i16; 2]>,
    pub size: Option<[u16; 2]>,
    /// Its variants: its specimen page's states.
    #[serde(default)]
    pub variants: Vec<String>,
    /// Its specimen page (`design.<name>`), when it has one.
    pub specimen: Option<String>,
}

impl ComponentDoc {
    /// Its justification as a view has it.
    pub fn justify(&self) -> crate::ui_kit::view::Justify {
        use crate::ui_kit::view::Justify;
        match self.justify.as_deref() {
            Some("right") => Justify::Right,
            Some("centre") => Justify::Centre,
            _ => Justify::Left,
        }
    }
}

impl DesignDoc {
    /// Parses and validates `design.yaml`.
    pub fn parse(text: &str) -> Result<DesignDoc, DataError> {
        let d: DesignDoc = serde_yaml_ng::from_str(text)
            .map_err(|e| DataError::new("design.yaml", "", e.to_string()))?;
        d.validate()?;
        Ok(d)
    }

    fn validate(&self) -> Result<(), DataError> {
        let err = |at: String, what: String| Err(DataError::new("design.yaml", &at, what));
        for (name, c) in &self.palette {
            if c.iter().any(|v| !(0.0..=1.0).contains(v)) {
                return err(format!("palette.{name}"), "each channel is in 0..1".into());
            }
        }
        if self.legend.words != "upper" && self.legend.words != "as_written" {
            return err("legend.words".into(), "upper or as_written".into());
        }
        for (name, c) in &self.components {
            let at = |field: &str| format!("components.{name}.{field}");
            if let Some(f) = &c.font
                && !self.fonts.contains_key(f)
            {
                return err(at("font"), format!("no font role {f:?} in fonts"));
            }
            if let Some(col) = &c.colour
                && !self.palette.contains_key(col)
            {
                return err(at("colour"), format!("no colour {col:?} in palette"));
            }
            if let Some(j) = &c.justify
                && !matches!(j.as_str(), "left" | "right" | "centre")
            {
                return err(at("justify"), format!("{j:?} is not left, right or centre"));
            }
        }
        Ok(())
    }

    /// Colour `name` of the palette.
    pub fn colour(&self, name: &str) -> Result<[f32; 3], DataError> {
        self.palette.get(name).copied().ok_or_else(|| {
            DataError::new("design.yaml", "palette", format!("no colour {name:?}"))
        })
    }
    /// The font of role `role`.
    pub fn font(&self, role: &str) -> Result<i16, DataError> {
        self.fonts
            .get(role)
            .copied()
            .ok_or_else(|| DataError::new("design.yaml", "fonts", format!("no font role {role:?}")))
    }
    /// Spacing `name`.
    pub fn spacing(&self, name: &str) -> Result<i32, DataError> {
        self.spacing.get(name).copied().ok_or_else(|| {
            DataError::new("design.yaml", "spacing", format!("no spacing {name:?}"))
        })
    }
    /// Component `name`'s preset.
    pub fn component(&self, name: &str) -> Result<&ComponentDoc, DataError> {
        self.components.get(name).ok_or_else(|| {
            DataError::new("design.yaml", "components", format!("no component {name:?}"))
        })
    }
    /// Component `name`'s font and colour as a view carries them.
    pub fn text_style(&self, name: &str) -> Result<(i16, [f32; 3]), DataError> {
        let c = self.component(name)?;
        let at = format!("components.{name}");
        let font = c
            .font
            .as_deref()
            .ok_or_else(|| DataError::new("design.yaml", &at, "a text component names a font"))?;
        let colour = c
            .colour
            .as_deref()
            .ok_or_else(|| DataError::new("design.yaml", &at, "a text component names a colour"))?;
        Ok((self.font(font)?, self.colour(colour)?))
    }
    /// Every palette colour as a view carries it (`[u8; 3]`), by name.
    pub fn palette_rgb(&self) -> Vec<(&str, [u8; 3])> {
        self.palette
            .iter()
            .map(|(n, c)| (n.as_str(), crate::ui_kit::components::rgb(*c)))
            .collect()
    }
}

// ---- a screen ----

/// `assets/ui/screens/<name>.yaml`: one screen's presentation. Keys are in
/// the order [`to_yaml`] writes them.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenDoc {
    /// Its name in `crate::ui_kit::screens::NAMES`; the file's stem.
    pub screen: String,
    /// `list`, `dialog`, `code_entry`, `picker` or `hud`.
    pub kind: String,
    /// The list's place or the dialog's base, by name ([`LAYOUTS`]).
    pub layout: Option<String>,
    /// `shell` (the game shell), `over` (the opener, as it was),
    /// `screen` (a kit screen of ours, [`ScreenDoc::opener`]) or `none`.
    pub backing: Option<String>,
    /// What the dialog opened over: an opener's name (`main-menu`), or
    /// for `backing: screen` a screen of ours.
    pub opener: Option<String>,
    /// More rows than the list shows is meant.
    #[serde(default)]
    pub scrolls: bool,
    /// Named states: where the focus (and a code's cursor) is.
    #[serde(default)]
    pub states: IndexMap<String, StateDoc>,
    /// Keyed by the view's widget id, in draw order.
    #[serde(default)]
    pub widgets: IndexMap<String, WidgetDoc>,
    /// Fixed rows, by row id (`row.0`).
    #[serde(default)]
    pub rows: IndexMap<String, RowDoc>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateDoc {
    #[serde(default)]
    pub focus: usize,
    pub cursor: Option<usize>,
}

/// One widget: its component and what the screen gives it.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WidgetDoc {
    /// A component of `design.yaml`.
    pub component: Option<String>,
    /// Literal, or a template whose `{slots}` Rust fills ([`fill`]).
    pub text: Option<String>,
    /// Top-left, UI units, origin at the centre, Y up.
    pub at: Option<[i16; 2]>,
    /// Width and height.
    pub size: Option<[u16; 2]>,
    /// A legend's prompts.
    pub prompts: Option<Vec<PromptDoc>>,
    /// Code cells' digits.
    pub digits: Option<String>,
    /// Code cells: the wider gap after this cell.
    pub gap_after: Option<usize>,
    /// An art widget's retail bitmap.
    pub art: Option<ArtDoc>,
}

/// A prompt: its glyph (or glyphs) by name and its words.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct PromptDoc(pub Glyphs, pub String);

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Glyphs {
    One(String),
    Many(Vec<String>),
}

impl Glyphs {
    pub fn names(&self) -> Vec<&str> {
        match self {
            Glyphs::One(g) => vec![g.as_str()],
            Glyphs::Many(gs) => gs.iter().map(String::as_str).collect(),
        }
    }
}

/// A retail screen's bitmap block, as `crate::ui_kit::view::Art::Screen`
/// names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtDoc {
    pub screen: i16,
    pub pane: u16,
    pub index: u16,
    pub image: i16,
}

/// A fixed row.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RowDoc {
    /// `action`, `open`, `value`, `pick`, `info`, `text` or `disabled`
    /// ([`ROW_KINDS`]); empty on a screen whose rows Rust makes and whose
    /// labels alone are here.
    #[serde(default)]
    pub kind: String,
    /// Its label.
    pub text: String,
    /// An info or text row's value.
    pub value: Option<String>,
    /// A value row's choices, and the one shown.
    pub values: Option<Vec<String>>,
    pub current: Option<usize>,
    /// Its description.
    pub help: Option<String>,
    /// The screen an `open` row opens.
    pub open: Option<String>,
    /// A text row's limit.
    pub max_chars: Option<u16>,
}

/// The kinds a screen doc may be.
pub const KINDS: [&str; 5] = ["list", "dialog", "code_entry", "picker", "hud"];
/// Each kind's layouts.
pub const LAYOUTS: [(&str, &[&str]); 5] = [
    ("list", &["controls", "level_select", "moved"]),
    ("dialog", &["settings", "large"]),
    ("code_entry", &["large"]),
    ("picker", &["picker", "large"]),
    ("hud", &["none"]),
];
pub const BACKINGS: [&str; 4] = ["shell", "over", "screen", "none"];
pub const ROW_KINDS: [&str; 8] = ["", "action", "open", "value", "pick", "info", "text", "disabled"];

/// The openers a doc may name.
pub fn opener_named(name: &str) -> Option<crate::ui_kit::view::Opener> {
    use crate::ui_kit::view::Opener;
    Some(match name {
        "main-menu" => Opener::MAIN_MENU,
        "game-setup" => Opener::GAME_SETUP,
        "game-setup.clankers" => Opener::GAME_SETUP_CLANKERS,
        "game-setup.firefight" => Opener::GAME_SETUP_FIREFIGHT,
        "game-setup.forge" => Opener::GAME_SETUP_FORGE,
        "xbox-live-menu" => Opener::XBOX_LIVE_MENU,
        _ => return None,
    })
}

/// The glyph named `name` ([`Glyph::name`]: `A`, `Left`, `LT` ...).
pub fn glyph_named(name: &str) -> Option<Glyph> {
    Glyph::ALL.into_iter().find(|g| g.name() == name)
}

/// A name YAML reads as itself unquoted.
pub(crate) fn plain(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'))
        && s.starts_with(|c: char| c.is_ascii_lowercase())
}

impl ScreenDoc {
    /// Parses and validates `screens/<file>.yaml` against `design`.
    pub fn parse(file: &str, text: &str, design: &DesignDoc) -> Result<ScreenDoc, DataError> {
        let path = format!("screens/{file}.yaml");
        let d: ScreenDoc =
            serde_yaml_ng::from_str(text).map_err(|e| DataError::new(&path, "", e.to_string()))?;
        if d.screen != file {
            return Err(DataError::new(
                &path,
                "screen",
                format!("{:?} is not the file's name", d.screen),
            ));
        }
        d.validate(design)?;
        Ok(d)
    }

    fn file(&self) -> String {
        format!("screens/{}.yaml", self.screen)
    }

    /// Every name the doc uses exists: its kind, layout, backing and
    /// opener, each widget's component, glyphs and words, each row's kind.
    pub fn validate(&self, design: &DesignDoc) -> Result<(), DataError> {
        let file = self.file();
        let err = |at: String, what: String| Err(DataError::new(&file, &at, what));
        for (field, value) in [("screen", Some(&self.screen)), ("layout", self.layout.as_ref()), ("backing", self.backing.as_ref()), ("opener", self.opener.as_ref())] {
            if let Some(v) = value
                && !plain(v)
            {
                return err(field.into(), format!("{v:?}: lower case, digits, '.', '-', '_'"));
            }
        }
        if !KINDS.contains(&self.kind.as_str()) {
            return err("kind".into(), format!("{:?} is not one of {KINDS:?}", self.kind));
        }
        let layouts = LAYOUTS
            .iter()
            .find(|(k, _)| *k == self.kind)
            .map(|(_, l)| *l)
            .unwrap_or(&[]);
        match &self.layout {
            Some(l) if !layouts.contains(&l.as_str()) => {
                return err("layout".into(), format!("{l:?}: a {} is one of {layouts:?}", self.kind));
            }
            None => return err("layout".into(), format!("a {} names its layout ({layouts:?})", self.kind)),
            _ => {}
        }
        if let Some(b) = &self.backing
            && !BACKINGS.contains(&b.as_str())
        {
            return err("backing".into(), format!("{b:?} is not one of {BACKINGS:?}"));
        }
        match (self.backing.as_deref(), &self.opener) {
            (Some("over"), Some(o)) if opener_named(o).is_none() => {
                return err("opener".into(), format!("no opener {o:?}"));
            }
            (Some("over" | "screen"), None) => {
                return err("opener".into(), "a dialog over something names it".into());
            }
            (Some("shell" | "none") | None, Some(_)) => {
                return err("opener".into(), "only a backing of over or screen has an opener".into());
            }
            _ => {}
        }
        for (name, w) in &self.widgets {
            let at = |field: &str| format!("widgets.{name}.{field}");
            if name.is_empty() || name.chars().any(char::is_control) {
                return err(format!("widgets.{name}"), "a widget id is a name".into());
            }
            if let Some(c) = &w.component
                && !design.components.contains_key(c)
            {
                return err(at("component"), format!("no component {c:?} in design.yaml"));
            }
            for (i, p) in w.prompts.iter().flatten().enumerate() {
                let names = p.0.names();
                if names.is_empty() {
                    return err(at(&format!("prompts.{i}")), "a prompt has a glyph".into());
                }
                for g in names {
                    if glyph_named(g).is_none() {
                        return err(at(&format!("prompts.{i}")), format!("no glyph {g:?}"));
                    }
                }
                if p.1.trim().is_empty() {
                    return err(at(&format!("prompts.{i}")), "a prompt has words".into());
                }
                if design.legend.words == "upper" && p.1.chars().any(char::is_lowercase) {
                    return err(at(&format!("prompts.{i}")), format!("{:?}: a legend's words are in capitals", p.1));
                }
            }
            if let Some(t) = &w.text {
                slots_of(t).map_err(|what| DataError::new(&file, &at("text"), what))?;
            }
        }
        for (id, r) in &self.rows {
            let at = |field: &str| format!("rows.{id}.{field}");
            let n = id.strip_prefix("row.").and_then(|n| n.parse::<usize>().ok());
            if n.is_none() {
                return err(format!("rows.{id}"), "a row id is row.N".into());
            }
            if !ROW_KINDS.contains(&r.kind.as_str()) {
                return err(at("kind"), format!("{:?} is not one of {ROW_KINDS:?}", r.kind));
            }
            if matches!(r.kind.as_str(), "value" | "pick") {
                let values = r.values.as_deref().unwrap_or(&[]);
                if values.is_empty() || r.current.is_none_or(|c| c >= values.len()) {
                    return err(at("values"), "a value row has values and a current one among them".into());
                }
            }
            if r.kind == "open" && r.open.is_none() {
                return err(at("open"), "an open row names the screen it opens".into());
            }
            slots_of(&r.text).map_err(|what| DataError::new(&file, &at("text"), what))?;
        }
        let rows = self.rows.len();
        for (name, s) in &self.states {
            if name.is_empty() || name.chars().any(char::is_control) {
                return err(format!("states.{name}"), "a state has a name".into());
            }
            if rows > 0 && s.focus >= rows {
                return err(format!("states.{name}.focus"), format!("focus {} past {rows} rows", s.focus));
            }
        }
        Ok(())
    }

    /// Widget `id`'s doc.
    pub fn widget(&self, id: &str) -> Option<&WidgetDoc> {
        self.widgets.get(id)
    }
    /// Widget `id`'s text with `slots` filled; an error when the doc has
    /// none or a slot isn't given.
    pub fn text(&self, id: &str, slots: &[(&str, &str)]) -> Result<String, DataError> {
        let t = self
            .widget(id)
            .and_then(|w| w.text.as_deref())
            .ok_or_else(|| DataError::new(&self.file(), &format!("widgets.{id}.text"), "missing"))?;
        fill(t, slots).map_err(|what| DataError::new(&self.file(), &format!("widgets.{id}.text"), what))
    }
    /// Widget `id`'s prompts, as the kit's legend takes them.
    pub fn prompts(&self, id: &str) -> Result<Vec<Prompt>, DataError> {
        let w = self.widget(id).ok_or_else(|| {
            DataError::new(&self.file(), &format!("widgets.{id}"), "missing")
        })?;
        Ok(w.prompts
            .iter()
            .flatten()
            .map(|p| {
                let glyphs: Vec<Glyph> = p.0.names().into_iter().filter_map(glyph_named).collect();
                Prompt::new(&glyphs, &p.1)
            })
            .collect())
    }
    /// Row `row`'s label, when the doc has the row.
    pub fn row_text(&self, row: usize) -> Option<&str> {
        self.rows.get(&format!("row.{row}")).map(|r| r.text.as_str())
    }
}

/// The `{slot}` names of template `t`. `{{` and `}}` are a brace.
pub fn slots_of(t: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut chars = t.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
            }
            '{' => {
                let mut name = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some(c) if c.is_ascii_alphanumeric() || c == '_' => name.push(c),
                        Some(c) => return Err(format!("{c:?} in a slot's name")),
                        None => return Err("a slot is not closed".into()),
                    }
                }
                if name.is_empty() {
                    return Err("an empty slot".into());
                }
                out.push(name);
            }
            '}' => return Err("a '}' that closes no slot".into()),
            _ => {}
        }
    }
    Ok(out)
}

/// Template `t` with each `{slot}` replaced by its value in `slots`; an
/// error names a slot `slots` doesn't give.
pub fn fill(t: &str, slots: &[(&str, &str)]) -> Result<String, String> {
    for s in slots_of(t)? {
        if !slots.iter().any(|(n, _)| *n == s) {
            return Err(format!("slot {{{s}}} is not supplied by the screen"));
        }
    }
    let mut out = t.replace("{{", "\u{0}").replace("}}", "\u{1}");
    for (n, v) in slots {
        out = out.replace(&format!("{{{n}}}"), v);
    }
    Ok(out.replace('\u{0}', "{").replace('\u{1}', "}"))
}

// ---- which screens draw from their doc ----

/// The screens whose presentation is their doc's alone: the game and the
/// editor build them from it ([`crate::ui_kit::screen::KitScreen::new`]).
/// Every other screen's doc is written from its Rust by the converter
/// ([`export`]) and held to it by the equivalence test, until its Rust
/// reads the doc too.
pub const FROM_DOC: [&str; 7] = [
    "kit.example",
    "kit.choices",
    "kit.dialog",
    "kit.dialog.large",
    crate::clankers::SCREEN,
    crate::firefight::options::SCREEN,
    crate::clanker_card::SCREEN,
];

// ---- where they are, and the loaded set ----

/// A screen doc's path in a checkout: `assets/ui/screens/<name>.yaml`.
pub fn path_of(screen: &str) -> PathBuf {
    PathBuf::from("assets/ui/screens").join(format!("{screen}.yaml"))
}
/// `design.yaml`'s path in a checkout.
pub fn design_path() -> PathBuf {
    PathBuf::from("assets/ui/design.yaml")
}

static DESIGN: LazyLock<DesignDoc> = LazyLock::new(|| {
    DesignDoc::parse(baked::DESIGN).unwrap_or_else(|e| panic!("{e}"))
});

/// The baked design system. Panics with the named error when the baked
/// file is wrong (the tests load it first).
pub fn design() -> &'static DesignDoc {
    &DESIGN
}

/// How many times the baked files have been parsed: once a process.
static BAKED_PARSES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// How many times the baked screens have been parsed in this process
/// (one, once any doc was asked for).
pub fn baked_parses() -> usize {
    BAKED_PARSES.load(std::sync::atomic::Ordering::SeqCst)
}

static BAKED: LazyLock<HashMap<&'static str, Arc<ScreenDoc>>> = LazyLock::new(|| {
    BAKED_PARSES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    baked::SCREENS
        .iter()
        .map(|(name, text)| {
            let doc = ScreenDoc::parse(name, text, design()).unwrap_or_else(|e| panic!("{e}"));
            (*name, Arc::new(doc))
        })
        .collect()
});

static RELOADED: LazyLock<RwLock<HashMap<String, Arc<ScreenDoc>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Screen `name`'s doc: one the editor reloaded ([`reload`]), else the
/// baked one; `None` for a screen with no doc.
pub fn screen(name: &str) -> Option<Arc<ScreenDoc>> {
    if let Some(d) = RELOADED
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(name)
    {
        return Some(Arc::clone(d));
    }
    BAKED.get(name).cloned()
}

/// Screen `name`'s doc, or the named error a screen without one is.
pub fn require(name: &str) -> Result<Arc<ScreenDoc>, DataError> {
    screen(name).ok_or_else(|| {
        DataError::new(
            &format!("screens/{name}.yaml"),
            "",
            "the screen has no doc (add it and list it in ui_kit::data::baked)",
        )
    })
}

/// Shows `doc` in place of its screen's baked doc (the editor's edit, or
/// its reload from the checkout), after validating it.
pub fn reload(doc: ScreenDoc) -> Result<(), DataError> {
    doc.validate(design())?;
    RELOADED
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .insert(doc.screen.clone(), Arc::new(doc));
    Ok(())
}

/// Drops every reloaded doc: the baked ones show again.
pub fn forget_reloads() {
    RELOADED.write().unwrap_or_else(|e| e.into_inner()).clear();
}

/// Every baked screen doc's name.
pub fn names() -> Vec<&'static str> {
    baked::SCREENS.iter().map(|(n, _)| *n).collect()
}

#[cfg(test)]
mod tests;
