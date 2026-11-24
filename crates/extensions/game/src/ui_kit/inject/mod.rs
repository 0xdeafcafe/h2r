//! Our screens as real `wgit` tags in the maps (docs/game/ui-in-maps.md).
//!
//! Every screen in [`NAMES`] has a screen id ([`id_of`]) and a tag
//! (`ui\screens\h2recomp\<name>`), generated at boot from the user's own
//! map and our docs, never kept in the repository:
//!
//! - [`plan`] reads each screen's doc (`content/ui/screens/<name>.yaml`,
//!   [`crate::ui_kit::data`]) and its sample ([`crate::ui_kit::screens::sample`]):
//!   what kind of screen it is, which retail screen its tag is shaped
//!   like, where its list goes and which animation tracks it plays;
//! - [`tag`] turns an [`Entry`] and the template's own bytes into a
//!   `wgit` and adds the tags to a map through `map::build::TagSet`, with
//!   the screens listed in the map's `wgtz` so the game finds them by id
//!   like a retail screen;
//! - The package build serves the rebuilt maps through the VFS.
//!
//! The tag is what `list_class::copy_tag` wrote into guest memory on each
//! open, made once, in the map: the template's root, one pane with the
//! template's other blocks, one list placed by the screen's layout, the
//! frame art kept. Texts and pictures a class adds after the pane is built
//! stay the class's (docs/game/ui-in-maps.md, "What is not in the tag").

use std::collections::BTreeMap;

use serde::Serialize;

use crate::ui_kit::animation::TransitionStyle;
use crate::ui_kit::data;
use crate::ui_kit::screens::{self, NAMES};
use crate::ui_kit::spec::ListPlace;

pub mod tag;

#[cfg(any(test, feature = "ui-fixture"))]
pub mod fixture;

#[cfg(test)]
mod tests;

pub use tag::{Added, add_screens};

/// The maps whose interfaces hold the screens our UI opens by id.
pub const MAPS: [&str; 3] = ["mainmenu", "shared", "single_player_shared"];

/// The first of our screen ids. Retail's are below 0x400 (the highest read
/// off the disc is 0xba and the in-game ones 200 to 240), and a screen id
/// is an `i16`, so ours sit well clear: `ID_BASE + the screen's index in
/// NAMES`.
pub const ID_BASE: i16 = 0x4000;

/// Where our screens' tags are named in a map.
pub const TAG_DIR: &str = "ui\\screens\\h2recomp\\";

/// The screen id of our screen `name`, `None` for a name not in [`NAMES`].
pub fn id_of(name: &str) -> Option<i16> {
    NAMES
        .iter()
        .position(|n| *n == name)
        .map(|i| ID_BASE + i as i16)
}

/// Whether `id` is one of ours.
pub fn is_injected_id(id: i16) -> bool {
    (ID_BASE..ID_BASE + NAMES.len() as i16).contains(&id)
}

/// The tag name of screen `name`: dots are not folders, so `coop.friends`
/// is `coop_friends`.
pub fn tag_name(name: &str) -> String {
    format!("{TAG_DIR}{}", name.replace('.', "_"))
}

/// The id a class of ours opens with: set only for a `list` screen, the
/// kind the loader builds on the injected tag ([`crate::list_class::load_with`]).
pub fn list_id(name: &str) -> Option<i16> {
    let doc = data::screen(name)?;
    (doc.kind == "list").then(|| id_of(name)).flatten()
}

/// The retail screen ids our tags are shaped like.
pub mod template {
    /// Settings, Controls, Controller Settings (`mainmenu.map` only): the
    /// screen the list classes are built on in the main menu.
    pub const CONTROLS: i16 = crate::list_class::CONTROLS_SCREEN_ID as i16;
    /// The in-game Controller Settings (`shared`, `single_player_shared`
    /// and the levels).
    pub const INGAME: i16 = crate::list_class::INGAME_SCREEN_ID as i16;
}

/// How an entry's tag is shaped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    /// A list screen: [`crate::list_class::copy_tag`]'s tag. The root and
    /// the pane (with its other blocks) of the Controls screen, its list
    /// placed by the layout, the frame art left of the layout's edge.
    List,
    /// A dialog: [`crate::ui_kit::dialog_tag::write_base_tag`]'s tag. The
    /// base dialog's root, a pane holding its flags and animation, the
    /// list placed by the layout and the panel art; no text block.
    Dialog { base: i16 },
}

/// One screen's tag, as far as the map needs to know.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Entry {
    pub name: String,
    pub id: i16,
    /// The tag's name in the map ([`tag_name`]).
    pub tag: String,
    pub shape: Shape,
    pub list: ListPlace,
    pub animation: TransitionStyle,
}

impl Entry {
    /// The screens (retail ids) the tag can be shaped like, in the order
    /// tried: the first the map's interface lists is the template.
    pub fn templates(&self) -> Vec<i16> {
        match self.shape {
            Shape::List => vec![template::CONTROLS, template::INGAME],
            Shape::Dialog { base } => vec![base],
        }
    }
}

/// The base dialog a doc of `kind` and `layout` is built on, by name
/// (`ui_kit::data::LAYOUTS`).
fn dialog_base(name: &str, kind: &str, layout: Option<&str>) -> Result<i16, String> {
    use crate::ui_kit::dialog::DialogBase;
    let base = match (kind, layout) {
        ("dialog", Some("settings")) => DialogBase::QuickOptions,
        ("dialog" | "code_entry", Some("large")) | ("picker", Some("large")) => DialogBase::Large,
        ("picker", Some("picker")) => DialogBase::ValuePicker,
        _ => {
            return Err(format!(
                "{name}: a {kind} doc with layout {layout:?} has no base dialog"
            ));
        }
    };
    Ok(base.screen_id())
}

/// The entry for screen `name`, or what is wrong with it by name.
pub fn entry(name: &str) -> Result<Entry, String> {
    let id = id_of(name).ok_or_else(|| format!("{name} is not in ui_kit::screens::NAMES"))?;
    let doc = data::require(name).map_err(|e| e.to_string())?;
    let spec = screens::sample(name).ok_or_else(|| format!("{name} has no sample"))?;
    let shape = match doc.kind.as_str() {
        "list" => Shape::List,
        kind => Shape::Dialog {
            base: dialog_base(name, kind, doc.layout.as_deref())?,
        },
    };
    Ok(Entry {
        name: name.into(),
        id,
        tag: tag_name(name),
        shape,
        list: spec.list,
        animation: spec.animation,
    })
}

/// Every screen's entry, in [`NAMES`] order.
pub fn plan() -> Result<Vec<Entry>, String> {
    NAMES.iter().map(|n| entry(n)).collect()
}

/// The package's screens in the same order as the game's screen IDs.
/// Design specimens without a screen ID are checked as docs and have no tag.
pub fn package_plan(
    design: data::DesignDoc,
    docs: &BTreeMap<String, data::ScreenDoc>,
) -> Result<Vec<Entry>, String> {
    data::with_design(design, || {
        data::with_screens(docs, || {
            NAMES
                .iter()
                .filter(|name| docs.contains_key(**name))
                .map(|name| {
                    // The live sample assumes its presentation can make a
                    // page. An invalid package should give a named build error.
                    std::panic::catch_unwind(|| entry(name)).unwrap_or_else(|panic| {
                        let why = panic
                            .downcast_ref::<String>()
                            .map(String::as_str)
                            .or_else(|| panic.downcast_ref::<&str>().copied())
                            .unwrap_or("the sample could not be made");
                        Err(format!("{name}: {why}"))
                    })
                })
                .collect()
        })
    })
}

/// The animation tracks a style asks for, as the class would ask at run
/// time (`dialog_tag::PaneTracks::for_style`): `None` keeps the template's
/// own (the default style, or a slot past the interface's collections).
pub fn tracks(style: TransitionStyle, animations: usize) -> Option<(u16, u16)> {
    crate::ui_kit::dialog_tag::PaneTracks::override_style(style, Some(animations as u32))
        .map(|t| (t.pane, t.rows))
}
