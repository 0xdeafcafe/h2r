//! Our screens' presentation as data (docs/game/ui-builder.md "Screens in
//! YAML"): the design system's tokens in `content/ui/design.yaml`
//! ([`DesignDoc`]) and each screen's widgets, texts, places and components
//! in `content/ui/screens/<name>.yaml` ([`ScreenDoc`]). Rust keeps
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
pub use present::{lone_slot, present};

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

/// `content/ui/design.yaml`: the palette, the fonts by role, spacing, the
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
    /// The Halo 2 pattern its widgets keep, which the consistency drive
    /// holds them to (`button`: drawn on a list skin's art or as a row,
    /// never bare text; docs/game/ui-builder.md "Consistency").
    pub pattern: Option<String>,
    /// The widget ids it draws (`button.*`: a prefix and a star), for the
    /// pattern rules.
    #[serde(default)]
    pub ids: Vec<String>,
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
            if let Some(p) = &c.pattern
                && p != "button"
            {
                return err(at("pattern"), format!("{p:?} is not button"));
            }
            if c.pattern.is_some() && c.ids.is_empty() {
                return err(at("ids"), "a pattern names the widget ids it holds".into());
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

/// `content/ui/screens/<name>.yaml`: one screen's presentation. Keys are in
/// the order [`to_yaml`] writes them.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenDoc {
    /// Its name in `crate::ui_kit::screens::NAMES`; the file's stem.
    pub screen: String,
    /// `list`, `dialog`, `code_entry` or `picker`.
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
    /// A settings dialog's host, header and caller (`layout: settings`
    /// only, docs/game/ui-builder.md "The settings dialog"): the shared
    /// component lays out its title, header, rows, help and legend, so the
    /// doc gives none of their places.
    pub settings: Option<SettingsDoc>,
    /// Keyed by the view's widget id, in draw order.
    #[serde(default)]
    pub widgets: IndexMap<String, WidgetDoc>,
    /// Fixed rows, by row id (`row.0`).
    #[serde(default)]
    pub rows: IndexMap<String, RowDoc>,
    /// A Forge page's rows bound to a property of the thing it shows, by
    /// id (`prop.0`), in order (docs/game/forge-map-variants-ui.md, "The
    /// Properties page"). Apart from `rows`, which are fixed by position.
    #[serde(default)]
    pub props: IndexMap<String, PropDoc>,
}

/// A row of a Forge page that is bound to a property of the thing shown:
/// the kinds it is for, the property, what A does and a picker's values.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropDoc {
    /// Its label.
    pub text: String,
    /// Its description, shown while it has the focus.
    pub help: Option<String>,
    /// The kinds of thing it is for ([`PROP_KINDS`]); none is any.
    #[serde(default)]
    pub on: Vec<String>,
    /// The property: a name in `forge::props`, or one of
    /// [`PROP_VIRTUAL`], which are not fields.
    pub prop: String,
    /// What A does: `picker`, `keyboard`, `act` (do it and close), or the
    /// name of a Forge screen.
    pub open: String,
    /// A picker's choices, in order; the choice sent is the index.
    pub values: Option<Vec<String>>,
}

/// The kinds a [`PropDoc`] may be `on`.
pub const PROP_KINDS: [&str; 6] = ["any", "object", "spawner", "starting-point", "goal", "teleporter"];
/// The properties that are not fields of a thing.
pub const PROP_VIRTUAL: [&str; 9] = [
    "coordinates",
    "heading",
    "label",
    "label number",
    "clear label",
    "prefab",
    "games",
    "reset orientation",
    "duplicate",
];

/// A settings dialog's own: which host it sits on, its static header
/// (icon, name, what the window is for), the row elsewhere that opens it
/// and its extra buttons.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsDoc {
    /// `menu` (Quick Options' tag, the main menu's) or `game` (the large
    /// dialog, in the game maps too).
    pub host: String,
    /// The header's icon of `mp_games2` by name
    /// ([`crate::ui_kit::dialog::icon::NAMES`]), or `image-N` for an
    /// image added to it in Studio; the menu host only.
    pub icon: Option<String>,
    /// The header's white line: what the window changes. The menu host
    /// only.
    pub name: Option<String>,
    /// The header's line about the window: what it is for. A dialog
    /// opened from a row elsewhere has none of its own: its about line is
    /// that row's help ([`CallerDoc::help`]), so the two always agree.
    pub about: Option<String>,
    /// The row on another screen that opens this one.
    pub caller: Option<CallerDoc>,
    /// Buttons besides the rows' (`X CLEAR FILTERS`).
    #[serde(default)]
    pub actions: Vec<ActionDoc>,
}

impl SettingsDoc {
    /// The header's line about the window: its own, else the caller's
    /// help.
    pub fn about(&self) -> &str {
        match (&self.about, &self.caller) {
            (Some(a), _) => a,
            (None, Some(c)) => &c.help,
            (None, None) => "",
        }
    }
}

/// The row on another screen that opens a settings dialog: its label and
/// its help there (Game Setup's description).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallerDoc {
    pub label: String,
    pub help: String,
}

/// An extra button of a settings dialog: its glyph, its legend words and
/// the binding it runs.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionDoc {
    pub button: String,
    pub words: String,
    pub bind: String,
}

/// The hosts a settings dialog may sit on.
pub const SETTINGS_HOSTS: [&str; 2] = ["menu", "game"];
/// The buttons a settings dialog's extra action may take, by glyph name.
pub const SETTINGS_BUTTONS: [&str; 4] = ["X", "Y", "Start", "Back"];

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
    /// Its description: on a settings dialog, its help under the rows
    /// while it has the focus.
    pub help: Option<String>,
    /// A settings dialog's `value` row over a setting of the store, by key.
    pub setting: Option<String>,
    /// A settings dialog's row whose behaviour a feature registers by
    /// name (`crate::ui_kit::settings::Bindings`).
    pub bind: Option<String>,
    /// What the value picker A opens says about the row; its help when
    /// unset.
    pub explain: Option<String>,
    /// The screen an `open` row opens.
    pub open: Option<String>,
    /// A text row's limit.
    pub max_chars: Option<u16>,
}

/// The kinds a screen doc may be.
pub const KINDS: [&str; 4] = ["list", "dialog", "code_entry", "picker"];
/// Each kind's layouts.
pub const LAYOUTS: [(&str, &[&str]); 4] = [
    ("list", &["controls", "level_select", "moved"]),
    ("dialog", &["settings", "large"]),
    ("code_entry", &["large"]),
    ("picker", &["picker", "large"]),
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
        "lobby" => Opener::LOBBY,
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
            if matches!(r.kind.as_str(), "value" | "pick") && self.settings.is_none() {
                let values = r.values.as_deref().unwrap_or(&[]);
                if values.is_empty() || r.current.is_none_or(|c| c >= values.len()) {
                    return err(at("values"), "a value row has values and a current one among them".into());
                }
            }
            if self.settings.is_none() && (r.setting.is_some() || r.bind.is_some() || r.explain.is_some()) {
                return err(format!("rows.{id}"), "setting, bind and explain are a settings dialog's".into());
            }
            if r.kind == "open" && r.open.is_none() {
                return err(at("open"), "an open row names the screen it opens".into());
            }
            slots_of(&r.text).map_err(|what| DataError::new(&file, &at("text"), what))?;
        }
        for (id, p) in &self.props {
            let at = |field: &str| format!("props.{id}.{field}");
            if id.strip_prefix("prop.").and_then(|n| n.parse::<usize>().ok()).is_none() {
                return err(format!("props.{id}"), "a prop id is prop.N".into());
            }
            slots_of(&p.text).map_err(|what| DataError::new(&file, &at("text"), what))?;
            if !PROP_VIRTUAL.contains(&p.prop.as_str()) && forge::props::prop(&p.prop).is_none() {
                return err(at("prop"), format!("{:?} is no property of forge::props", p.prop));
            }
            if let Some(k) = p.on.iter().find(|k| !PROP_KINDS.contains(&k.as_str())) {
                return err(at("on"), format!("{k:?} is not one of {PROP_KINDS:?}"));
            }
            let screen = crate::forge_screens::sheet::Kind::of_screen(&p.open).is_some();
            if !screen && !matches!(p.open.as_str(), "picker" | "keyboard" | "act") {
                return err(at("open"), format!("{:?} is neither picker, keyboard, act nor a Forge screen", p.open));
            }
            let values = p.values.as_deref().unwrap_or(&[]);
            if p.open == "picker" && values.is_empty() && p.prop != "heading" {
                return err(at("values"), "a picker row has values".into());
            }
            if p.open != "picker" && !values.is_empty() {
                return err(at("values"), "only a picker row has values".into());
            }
        }
        self.validate_settings(design)?;
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

    /// A settings dialog's own rules (docs/game/ui-builder.md "The
    /// settings dialog"): the block on `layout: settings` and nowhere
    /// else, a host and a header for it, no place for what the component
    /// places, and each row attached and with its help.
    fn validate_settings(&self, design: &DesignDoc) -> Result<(), DataError> {
        let file = self.file();
        let err = |at: String, what: String| Err(DataError::new(&file, &at, what));
        let on_settings =
            (self.kind.as_str(), self.layout.as_deref()) == ("dialog", Some("settings"));
        let Some(st) = &self.settings else {
            if on_settings {
                return err(
                    "settings".into(),
                    "a settings dialog has a settings: block (its host, header and caller)".into(),
                );
            }
            return Ok(());
        };
        if !on_settings {
            return err("settings".into(), "only a dialog on layout: settings has a settings: block".into());
        }
        for id in self.widgets.keys() {
            if id == "description" || id == "legend" || id.starts_with("header.") || id.starts_with("art.") {
                return err(
                    format!("widgets.{id}"),
                    "the settings component places it (design.yaml components.settings.*); a settings dialog's doc gives no such widget".into(),
                );
            }
        }
        if !SETTINGS_HOSTS.contains(&st.host.as_str()) {
            return err("settings.host".into(), format!("{:?} is not one of {SETTINGS_HOSTS:?}", st.host));
        }
        let menu = st.host == "menu";
        match (&st.icon, menu) {
            (Some(i), true) if crate::ui_kit::dialog::icon::named(i).is_none() => {
                return err(
                    "settings.icon".into(),
                    format!(
                        "no icon {i:?}: one of {:?}, or image-N for an image added to mp_games2",
                        crate::ui_kit::dialog::icon::NAMES
                    ),
                );
            }
            (None, true) => {
                return err("settings.icon".into(), "the menu host's header has an icon".into());
            }
            (Some(_), false) => {
                return err(
                    "settings.icon".into(),
                    "the game host has no icon (mp_games2 is in mainmenu.map only)".into(),
                );
            }
            _ => {}
        }
        match (&st.name, menu) {
            (Some(n), true) if n.trim().is_empty() => {
                return err("settings.name".into(), "the header's name says what the window changes".into());
            }
            (None, true) => {
                return err("settings.name".into(), "the menu host's header has a name".into());
            }
            (Some(_), false) => {
                return err("settings.name".into(), "the game host has no name line; its title names it".into());
            }
            _ => {}
        }
        match (&st.about, &st.caller) {
            (Some(_), Some(_)) => {
                return err(
                    "settings.about".into(),
                    "a dialog opened from a row elsewhere says what it is for in that row's help (settings.caller.help); it has no about of its own".into(),
                );
            }
            (None, None) => {
                return err("settings.about".into(), "the header says what the window is for".into());
            }
            (Some(a), None) if a.trim().is_empty() => {
                return err("settings.about".into(), "the header says what the window is for".into());
            }
            _ => {}
        }
        if let Some(c) = &st.caller {
            if c.label.trim().is_empty() {
                return err("settings.caller.label".into(), "the opening row has a label".into());
            }
            if c.help.trim().is_empty() {
                return err("settings.caller.help".into(), "the opening row has a help".into());
            }
        }
        for (i, a) in st.actions.iter().enumerate() {
            let at = |f: &str| format!("settings.actions.{i}.{f}");
            if !SETTINGS_BUTTONS.contains(&a.button.as_str()) {
                return err(
                    at("button"),
                    format!("{:?}: an extra button is one of {SETTINGS_BUTTONS:?} (A and B are the rows' and back)", a.button),
                );
            }
            if a.words.trim().is_empty()
                || (design.legend.words == "upper" && a.words.chars().any(char::is_lowercase))
            {
                return err(at("words"), format!("{:?}: a legend's words, in capitals", a.words));
            }
            if a.bind.trim().is_empty() {
                return err(at("bind"), "an action runs a binding".into());
            }
        }
        let cap_name = if menu { "settings_rows" } else { "settings_game_rows" };
        let cap = usize::try_from(design.spacing(cap_name)?).unwrap_or(0);
        if self.rows.len() > cap {
            return err(
                "rows".into(),
                format!("{} rows; the {} host lays out {cap} (spacing.{cap_name})", self.rows.len(), st.host),
            );
        }
        for (id, r) in &self.rows {
            let at = |field: &str| format!("rows.{id}.{field}");
            if r.help.as_deref().is_none_or(|h| h.trim().is_empty()) {
                return err(at("help"), "a settings row has its help, shown under the rows while it has the focus".into());
            }
            match r.kind.as_str() {
                "value" => match (&r.setting, &r.bind) {
                    (Some(_), Some(_)) => {
                        return err(at("setting"), "a value row is over a setting or a binding, not both".into());
                    }
                    (None, None) => {
                        // A specimen's row: its values and the one shown.
                        let values = r.values.as_deref().unwrap_or(&[]);
                        if values.is_empty() || r.current.is_none_or(|c| c >= values.len()) {
                            return err(
                                at("setting"),
                                "a value row names its setting or binding (a specimen's gives values and current)".into(),
                            );
                        }
                    }
                    _ => {
                        if r.current.is_some() {
                            return err(at("current"), "a bound row's value is its setting's; current is a specimen's".into());
                        }
                    }
                },
                "pick" | "action" => {
                    if r.bind.is_none() {
                        return err(at("bind"), format!("a {} row names its binding", r.kind));
                    }
                }
                "open" => {
                    if r.open.is_none() {
                        return err(at("open"), "an open row names the screen it opens".into());
                    }
                }
                k => {
                    return err(at("kind"), format!("{k:?}: a settings row is value, pick, open or action"));
                }
            }
            if r.kind != "value" && r.setting.is_some() {
                return err(at("setting"), "only a value row is over a setting".into());
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
pub const FROM_DOC: [&str; 11] = [
    "kit.example",
    "kit.choices",
    "kit.dialog",
    "kit.dialog.large",
    crate::clankers::SCREEN,
    crate::firefight::options::SCREEN,
    crate::clanker_card::SCREEN,
    crate::h1_options::SCREEN,
    crate::custom_games::browser_filters::SCREEN,
    "layout.dialog.settings",
    "design.row.setting",
];

// ---- where they are, and the loaded set ----

/// A screen doc's path in a checkout: `content/ui/screens/<name>.yaml`.
pub fn path_of(screen: &str) -> PathBuf {
    PathBuf::from("content/ui/screens").join(format!("{screen}.yaml"))
}
/// `design.yaml`'s path in a checkout.
pub fn design_path() -> PathBuf {
    PathBuf::from("content/ui/design.yaml")
}

static DESIGN: LazyLock<Arc<DesignDoc>> =
    LazyLock::new(|| Arc::new(DesignDoc::parse(baked::DESIGN).unwrap_or_else(|e| panic!("{e}"))));

/// The baked design system. Panics with the named error when the baked
/// file is wrong (the tests load it first).
pub fn design() -> &'static DesignDoc {
    &DESIGN
}

thread_local! {
    static SCOPED_DESIGN: std::cell::RefCell<Option<Arc<DesignDoc>>> =
        const { std::cell::RefCell::new(None) };
}

/// The design used by a screen sample during a package build, or the baked design.
pub(crate) fn active_design() -> impl std::ops::Deref<Target = DesignDoc> {
    SCOPED_DESIGN.with(|d| d.borrow().clone()).unwrap_or_else(|| Arc::clone(&DESIGN))
}

/// Runs a screen sample with the design supplied by the package.
pub(crate) fn with_design<R>(design: DesignDoc, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Arc<DesignDoc>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            SCOPED_DESIGN.with(|d| *d.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(SCOPED_DESIGN.with(|d| d.borrow_mut().replace(Arc::new(design))));
    f()
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
    if let Some(d) = SCOPED.with(|s| s.borrow().as_ref().and_then(|m| m.get(name).cloned())) {
        return Some(d);
    }
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

thread_local! {
    /// Docs standing in for their screens on this thread while
    /// [`with_screens`] runs (the editor's unsaved edits).
    static SCOPED: std::cell::RefCell<Option<HashMap<String, Arc<ScreenDoc>>>> =
        const { std::cell::RefCell::new(None) };
}

/// Runs `f` with `docs` in place of their screens' docs on this thread
/// only, so the views `f` makes show them and nothing else sees them.
/// The docs as they were come back when `f` returns or panics.
pub fn with_screens<R>(
    docs: &std::collections::BTreeMap<String, ScreenDoc>,
    f: impl FnOnce() -> R,
) -> R {
    struct Restore(Option<HashMap<String, Arc<ScreenDoc>>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let old = self.0.take();
            SCOPED.with(|s| *s.borrow_mut() = old);
        }
    }
    let mut map = SCOPED.with(|s| s.borrow().clone()).unwrap_or_default();
    for (name, doc) in docs {
        map.insert(name.clone(), Arc::new(doc.clone()));
    }
    let _restore = Restore(SCOPED.with(|s| s.borrow_mut().replace(map)));
    f()
}

/// Screen `name`'s doc as the build baked it, ignoring reloads and
/// [`with_screens`].
pub fn baked(name: &str) -> Option<Arc<ScreenDoc>> {
    BAKED.get(name).cloned()
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
