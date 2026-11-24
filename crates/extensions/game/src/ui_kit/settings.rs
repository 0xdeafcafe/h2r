//! The shared settings dialog (docs/game/ui-builder.md "The settings
//! dialog"): one component for every settings page of ours, built from
//! its doc, `content/ui/screens/<name>.yaml`, with a `settings:` block.
//!
//! The rule it keeps, by construction:
//!
//! - **Top:** the icon, the name and a line about the window are the
//!   doc's `settings:` block, the same in every state. Nothing
//!   row-indexed reaches them ([`crate::ui_kit::screen::Header`]).
//! - **Bottom:** the focused row's `help`, under the rows.
//! - **Picker:** the value picker a row opens says that row's `explain`,
//!   else its `help`.
//!
//! A page's Rust has no label, help, place or legend in it. A row over a
//! setting of the store (`setting: <key>`) needs no Rust at all: its
//! choices come from the setting's [`settings::Def`]
//! ([`crate::ui_kit::choice`]). A row whose behaviour is a feature's
//! (`bind: <name>`) takes one closure the feature registers by name
//! ([`Bindings`]). The legend is derived from the rows and the doc's
//! extra buttons ([`legend`]).
//!
//! A page whose rows are not known until it opens (Quick Options for a
//! script variant, `crate::variant_quick::page`) writes a row's `text`
//! and `help` as lone slots (`"{label}"`, `"{help}"`) and binds it as a
//! slot ([`Bindings::slot`]): its label, value and help are the
//! binding's, and a slot the binding leaves empty is not drawn. A
//! `settings.name` that is a lone slot (`"{variant}"`) is the binding
//! [`Bindings::name`]'s.
//!
//! ```ignore
//! pub fn page(store: Arc<Store>) -> KitScreen {
//!     settings::screen(FEATURE, SCREEN, IDS, Some(store), Bindings::new())
//!         .unwrap_or_else(|e| panic!("{e}"))
//! }
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use settings::Store;

use crate::list_class::TextBlock;
use crate::ui_kit::choice;
use crate::ui_kit::data::{self, DataError, RowDoc, ScreenDoc};
use crate::ui_kit::ids::Family;
use crate::ui_kit::input::{Glyph, Input, Prompt};
use crate::ui_kit::picker::Pick;
use crate::ui_kit::screen::{Filled, KitScreen, Row, Then};

/// The longest picker text a settings row's `explain` may be: about
/// seven lines of the picker's description block (235 units wide in
/// font 1).
pub const EXPLAIN_UNITS: usize = 200;

/// Screens with settings rows that are not the shared dialog, and why.
/// `settings::tests::every_settings_list_is_the_shared_dialog_or_says_why`
/// holds every other such screen to a `settings:` doc, but for the
/// `design.*` and `layout.*` pages: each is another component's specimen.
pub const NOT_SHARED: &[(&str, &str)] = &[
    (
        "variant-more-options",
        "a page of retail's variant editor, laid out as its category pages",
    ),
    ("skulls", "a level select page with the focused skull's picture"),
    ("skulls.h1", "a level select page with the focused skull's picture"),
    ("skulls.mp", "a level select page with the focused skull's picture"),
    ("forge.properties", "an object's properties on the in-game Controller Settings host"),
    ("forge.games", "a tick list: several game types are on at once, so no row is a one-of choice"),
    ("forge.variants", "a dialog listing the map's variants, each opening its own dialog"),
    ("forge.variant", "a dialog of actions on one map variant, no row is a setting"),
    ("forge.settings", "a Forge tool page on the in-game Controller Settings host"),
    ("kit.choices", "the kit's list example on the level select's layout"),
    ("kit.example", "the kit's first example, every row kind on the Controls list"),
    ("kit.dialog.large", "the kit's rows on the large dialog, which has no help block"),
];

type Get<T> = Arc<dyn Fn() -> T + Send + Sync>;

/// A row's behaviour, registered by name ([`Bindings`]).
pub enum Bound {
    /// A value row not in the store: the values it offers now, the held
    /// index, and the setter (what to log). The values are made as the
    /// row is drawn or pressed ([`Bindings::menu`]).
    Value {
        values: Get<Vec<String>>,
        get: Get<usize>,
        set: Arc<dyn Fn(usize) -> String + Send + Sync>,
    },
    /// A pick row: what the row shows, and the pick made as A is pressed
    /// (or why there is none).
    Pick {
        shown: Get<String>,
        offer: Get<Result<Pick, String>>,
    },
    /// An action row, an open row's work first, or an extra button.
    Action(Get<Then>),
    /// A pick row filled as the page shows it: its label, value and help,
    /// or none (not drawn); and the pick made as A is pressed.
    Slot {
        fill: Get<Option<Filled>>,
        offer: Get<Result<Pick, String>>,
    },
}

/// The behaviours a settings dialog's `bind:` rows and extra buttons
/// name. Every one must be named by the doc, and every name the doc uses
/// must be here: either way is a named error.
#[derive(Default)]
pub struct Bindings {
    binds: Vec<(&'static str, Bound)>,
    /// The header's name line, for a doc whose `settings.name` is a lone
    /// slot.
    name: Option<Get<String>>,
}

impl Bindings {
    pub fn new() -> Self {
        Self::default()
    }
    /// A value row over something other than the store.
    pub fn value(
        mut self,
        name: &'static str,
        values: Vec<String>,
        get: impl Fn() -> usize + Send + Sync + 'static,
        set: impl Fn(usize) -> String + Send + Sync + 'static,
    ) -> Self {
        self.menu(name, move || values.clone(), get, set)
    }
    /// A [`Self::value`] row whose offered values are made as the row is
    /// drawn or pressed (the wave sets a lobby's map offers): `values` is
    /// the list now.
    pub fn menu(
        mut self,
        name: &'static str,
        values: impl Fn() -> Vec<String> + Send + Sync + 'static,
        get: impl Fn() -> usize + Send + Sync + 'static,
        set: impl Fn(usize) -> String + Send + Sync + 'static,
    ) -> Self {
        self.binds.push((
            name,
            Bound::Value {
                values: Arc::new(values),
                get: Arc::new(get),
                set: Arc::new(set),
            },
        ));
        self
    }
    /// A pick row: its values made when A is pressed.
    pub fn pick(
        mut self,
        name: &'static str,
        shown: impl Fn() -> String + Send + Sync + 'static,
        offer: impl Fn() -> Result<Pick, String> + Send + Sync + 'static,
    ) -> Self {
        self.binds.push((
            name,
            Bound::Pick {
                shown: Arc::new(shown),
                offer: Arc::new(offer),
            },
        ));
        self
    }
    /// An action row, an open row's work, or an extra button.
    pub fn action(
        mut self,
        name: &'static str,
        press: impl Fn() -> Then + Send + Sync + 'static,
    ) -> Self {
        self.binds.push((name, Bound::Action(Arc::new(press))));
        self
    }
    /// A slot row (its doc text and help lone slots): `fill` gives its
    /// label, value and help as the page shows it, `None` for an empty
    /// slot, which is not drawn; `offer` the pick A opens.
    pub fn slot(
        mut self,
        name: &'static str,
        fill: impl Fn() -> Option<Filled> + Send + Sync + 'static,
        offer: impl Fn() -> Result<Pick, String> + Send + Sync + 'static,
    ) -> Self {
        self.binds.push((
            name,
            Bound::Slot {
                fill: Arc::new(fill),
                offer: Arc::new(offer),
            },
        ));
        self
    }
    /// The header's name line, for a doc whose `settings.name` is a lone
    /// slot: read each time it is drawn, the same at every focus.
    pub fn name(mut self, name: impl Fn() -> String + Send + Sync + 'static) -> Self {
        self.name = Some(Arc::new(name));
        self
    }
    fn take(&mut self, name: &str) -> Option<Bound> {
        let i = self.binds.iter().position(|(n, _)| *n == name)?;
        Some(self.binds.remove(i).1)
    }
}

/// Component `component` of `design.yaml` as a text block: its place,
/// font and colour, with `animation`.
pub fn block(component: &str, animation: u16) -> Result<TextBlock, DataError> {
    let d = data::active_design();
    let c = d.component(component)?;
    let (font, colour) = d.text_style(component)?;
    let missing = || {
        DataError::new(
            "design.yaml",
            &format!("components.{component}"),
            "a settings dialog's part has its place (at and size)",
        )
    };
    let [x, y] = c.at.ok_or_else(missing)?;
    let [w, h] = c.size.ok_or_else(missing)?;
    Ok(TextBlock {
        rect: [y, x, y - h as i16, x + w as i16],
        font,
        colour,
        animation,
    })
}

/// A settings dialog's legend, from its rows: left and right CHANGE when
/// a row is a `value` row, then A SELECT and B BACK. Its extra buttons'
/// prompts follow ([`KitScreen::extra`]). No settings doc has a legend
/// of its own.
pub fn legend(doc: &ScreenDoc) -> Vec<Prompt> {
    let mut v = Vec::new();
    if doc.rows.values().any(|r| r.kind == "value") {
        v.push(Prompt::new(&[Glyph::DpadLeft, Glyph::DpadRight], "CHANGE"));
    }
    v.push(Prompt::new(&[Glyph::A], "SELECT"));
    v.push(Prompt::new(&[Glyph::B], "BACK"));
    v
}

/// The input an extra button's glyph name raises
/// ([`data::SETTINGS_BUTTONS`]).
fn button(name: &str) -> Option<Input> {
    match name {
        "X" => Some(Input::X),
        "Y" => Some(Input::Y),
        "Start" => Some(Input::Start),
        "Back" => Some(Input::Back),
        _ => None,
    }
}

/// The settings dialog `name`'s doc as a kit screen: its header, rows,
/// help and legend from the doc, its `setting:` rows over `store`, its
/// `bind:` rows and extra buttons from `binds`. `feature` switches it,
/// `ids` is its family in [`crate::ui_kit::ids::FAMILIES`]. An unknown
/// setting or binding, a binding no row names, or a `values` override of
/// the wrong length is a named error.
pub fn screen(
    feature: &'static str,
    name: &'static str,
    ids: Family,
    store: Option<Arc<Store>>,
    binds: Bindings,
) -> Result<KitScreen, DataError> {
    let doc = data::require(name)?;
    from_doc(feature, name, ids, &doc, store, binds)
}

/// [`screen`] over doc `doc`.
pub fn from_doc(
    feature: &'static str,
    name: &'static str,
    ids: Family,
    doc: &ScreenDoc,
    store: Option<Arc<Store>>,
    mut binds: Bindings,
) -> Result<KitScreen, DataError> {
    let file = format!("screens/{}.yaml", doc.screen);
    let bad = |at: String, what: String| DataError::new(&file, &at, what);
    let Some(st) = &doc.settings else {
        return Err(bad("settings".into(), "a settings dialog's doc has a settings: block".into()));
    };
    let mut s = KitScreen::from_doc(feature, name, ids, doc)?;
    match (st.name.as_deref().is_some_and(data::lone_slot), binds.name.take()) {
        (true, Some(get)) => s = s.header_name(move || get()),
        (true, None) => {
            return Err(bad("settings.name".into(), "a lone slot names the header's name binding (Bindings::name)".into()));
        }
        (false, Some(_)) => {
            return Err(bad("settings.name".into(), "a name binding fills a settings.name that is a lone slot".into()));
        }
        (false, None) => {}
    }
    let mut n = 0;
    while let Some(r) = doc.rows.get(&format!("row.{n}")) {
        let id = format!("row.{n}");
        let row = row(&id, r, store.as_ref(), &mut binds).map_err(|(at, what)| bad(at, what))?;
        s = s.row(row);
        n += 1;
    }
    if n != doc.rows.len() {
        return Err(bad("rows".into(), "a settings dialog's rows are row.0, row.1, ... in order".into()));
    }
    for (i, a) in st.actions.iter().enumerate() {
        let at = |f: &str| format!("settings.actions.{i}.{f}");
        let input = button(&a.button).ok_or_else(|| bad(at("button"), format!("no button {:?}", a.button)))?;
        let Some(Bound::Action(press)) = binds.take(&a.bind) else {
            return Err(bad(at("bind"), format!("no action binding {:?}", a.bind)));
        };
        s = s.extra(input, &a.words, move || match press() {
            Then::Stay(line) => line,
            Then::Open(screen) => format!("an extra button opens nothing ({screen})"),
            Then::Close | Then::CloseThen(_) => "an extra button closes nothing".into(),
        });
    }
    if let Some((left, _)) = binds.binds.first() {
        return Err(bad("rows".into(), format!("no row or action names the binding {left:?}")));
    }
    Ok(s)
}

/// Doc row `r` (`id`) as a kit row.
fn row(
    id: &str,
    r: &RowDoc,
    store: Option<&Arc<Store>>,
    binds: &mut Bindings,
) -> Result<Row, (String, String)> {
    let at = |f: &str| format!("rows.{id}.{f}");
    let label = r.text.as_str();
    let help = r.help.clone().unwrap_or_default();
    // A slot: its text and help are its binding's, as the page shows it.
    let slot = data::lone_slot(label);
    if slot != data::lone_slot(&help) {
        return Err((at("help"), "a slot row's text and help are both lone slots".into()));
    }
    if slot {
        return match (r.kind.as_str(), &r.bind, r.explain.is_some()) {
            (_, _, true) => Err((at("explain"), "a slot row's picker says its binding's help".into())),
            ("pick", Some(b), false) => match binds.take(b) {
                Some(Bound::Slot { fill, offer }) => Ok(Row::slot(move || fill(), move || offer())),
                _ => Err((at("bind"), format!("no slot binding {b:?}"))),
            },
            _ => Err((at("kind"), "a row whose text is a lone slot is a pick row bound to a slot".into())),
        };
    }
    let row = match (r.kind.as_str(), &r.setting, &r.bind) {
        ("value", Some(key), _) => {
            let store = store.ok_or_else(|| (at("setting"), format!("{key}: the screen has no store")))?;
            let def = store.def(key).ok_or_else(|| (at("setting"), format!("no setting {key:?}")))?;
            let choices = choice::choices(def);
            let values: Vec<String> = match &r.values {
                Some(v) if v.len() != choices.len() => {
                    return Err((
                        at("values"),
                        format!("{} values for {key}'s {} choices", v.len(), choices.len()),
                    ));
                }
                Some(v) => v.clone(),
                None => choices.into_iter().map(|(_, n)| n).collect(),
            };
            let values: Vec<&str> = values.iter().map(String::as_str).collect();
            let (get, set) = (Arc::clone(store), Arc::clone(store));
            Row::value(
                label,
                &values,
                move || choice::index(def, &get),
                move |i| choice::set(def, &set, i),
            )
        }
        ("value", None, Some(b)) => match binds.take(b) {
            Some(Bound::Value { values, get, set }) => {
                let now = (values)();
                if let Some(v) = &r.values
                    && v.len() != now.len()
                {
                    return Err((at("values"), format!("{} values for {b}'s {}", v.len(), now.len())));
                }
                match &r.values {
                    // The doc pins the offered values: they are its own.
                    Some(pinned) => {
                        let values: Vec<&str> = pinned.iter().map(String::as_str).collect();
                        Row::value(label, &values, move || get(), move |i| set(i))
                    }
                    // The binding's list, made as the row is drawn.
                    None => Row::menu(label, values, move || get(), move |i| set(i)),
                }
            }
            _ => return Err((at("bind"), format!("no value binding {b:?}"))),
        },
        ("value", None, None) => {
            // A specimen's row: its values, the one shown held here.
            let values: Vec<&str> = r.values.iter().flatten().map(String::as_str).collect();
            let held = Arc::new(AtomicUsize::new(r.current.unwrap_or(0)));
            let (get, set) = (Arc::clone(&held), held);
            let shown: Vec<String> = values.iter().map(|v| v.to_string()).collect();
            Row::value(
                label,
                &values,
                move || get.load(Ordering::SeqCst),
                move |i| {
                    set.store(i, Ordering::SeqCst);
                    format!("-> {}", shown.get(i).map_or("?", String::as_str))
                },
            )
        }
        ("pick", _, Some(b)) => match binds.take(b) {
            Some(Bound::Pick { shown, offer }) => Row::pick(label, move || shown(), move || offer()),
            _ => return Err((at("bind"), format!("no pick binding {b:?}"))),
        },
        ("action", _, Some(b)) => match binds.take(b) {
            Some(Bound::Action(press)) => Row::action(label, move || press()),
            _ => return Err((at("bind"), format!("no action binding {b:?}"))),
        },
        ("open", _, bind) => {
            let open = r.open.as_deref().unwrap_or_default();
            let screen = crate::ui_kit::screens::NAMES
                .iter()
                .copied()
                .find(|n| *n == open)
                .ok_or_else(|| (at("open"), format!("no screen {open:?}")))?;
            match bind {
                None => Row::action(label, move || Then::Open(screen)),
                // Its work first; the binding says where it goes.
                Some(b) => match binds.take(b) {
                    Some(Bound::Action(press)) => Row::action(label, move || press()),
                    _ => return Err((at("bind"), format!("no action binding {b:?}"))),
                },
            }
        }
        (k, _, _) => return Err((at("kind"), format!("{k:?}: a settings row is value, pick, open or action"))),
    };
    let row = row.describe(&help);
    Ok(match &r.explain {
        Some(e) => row.explain(e),
        None => row.explain(&help),
    })
}

#[cfg(test)]
mod tests;
