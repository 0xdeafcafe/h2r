//! Save, Save as and Load on the map variant store
//! (docs/game/forge-map-variants-ui.md, "Saving, loading and renaming").
//!
//! GAME MENU's **Save**, **Save as** and **Load** act on
//! `forge::files::variants`; this module is their decisions, as plain
//! functions the session's editor (`forge_glue::editing`) and GAME MENU
//! ([`super::menu_press`]) call, so a table of flows can test them.
//!
//! - **Save** writes the open variant under its name; a session with no
//!   name (opened from Original) opens the keyboard, as Save as does.
//! - **Save as** takes the typed name. The open variant's own name saves
//!   over it; any other name the store holds is asked about
//!   ([`replace_question`]) once the store has said it is taken, so the
//!   question never rests on a listing that went stale.
//! - **Load** picks a variant of this map that can be used
//!   ([`Row::loadable`]); with edits not saved the pick asks
//!   [`LOAD_QUESTION`] before it loads.
//! - Every flow ends in a notice: [`saved`], [`loaded`], [`NOT_SAVED`],
//!   [`NOT_LOADED`], or the store's own sentence for a refusal.

use forge::files::variants::{Listed, Source};

/// The body of the question a Load over unsaved edits asks.
pub const LOAD_QUESTION: &str = "Load without saving?";
/// The notice when No answers the Replace question.
pub const NOT_SAVED: &str = "Not saved.";
/// The notice when No answers the Load question.
pub const NOT_LOADED: &str = "Not loaded.";
/// Load with nothing to pick.
pub const NONE_SAVED: &str = "No map variants are saved.";
/// Save with no name to save under, when the keyboard cannot open.
pub const UNNAMED: &str = "This map has no name: use Save as.";

/// The body of the Replace question.
pub fn replace_question(name: &str) -> String {
    format!("Replace {name}?")
}

/// The notice after a save.
pub fn saved(name: &str) -> String {
    format!("Saved {name}.")
}

/// The notice after a load.
pub fn loaded(name: &str) -> String {
    format!("Loaded {name}.")
}

/// One variant of the open map as the screens list it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// The file's stem, the handle the store takes.
    pub file: String,
    /// The variant's name.
    pub name: String,
    /// Saved on this machine rather than fetched from Xbox Live.
    pub saved: bool,
    /// Why it cannot be used, if it cannot.
    pub sentence: Option<String>,
}

impl Row {
    pub fn of(listed: &Listed) -> Row {
        Row {
            file: listed.file.clone(),
            name: listed.name.clone(),
            saved: listed.source == Source::Saved,
            sentence: listed.sentence.clone(),
        }
    }

    /// Whether Load can open it: the store loads what this machine saved,
    /// and a variant that fails a check is listed, not loaded.
    pub fn loadable(&self) -> bool {
        self.saved && self.sentence.is_none()
    }
}

/// The rows of a store listing, in its order.
pub fn rows(listed: &[Listed]) -> Vec<Row> {
    listed.iter().map(Row::of).collect()
}

/// The rows Load offers.
pub fn loadable(rows: &[Row]) -> Vec<&Row> {
    rows.iter().filter(|r| r.loadable()).collect()
}

/// Names compare without case, as the store compares them.
pub fn same_name(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// What a save does next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveStep {
    /// Ask for the name on the game's keyboard.
    Keyboard,
    /// Write `name`; `replace` says the file that has it may be
    /// overwritten without asking.
    Write { name: String, replace: bool },
}

/// **Save** on a session whose open variant is `open`.
pub fn save(open: Option<&str>) -> SaveStep {
    match open {
        None => SaveStep::Keyboard,
        Some(name) => SaveStep::Write {
            name: name.to_owned(),
            replace: true,
        },
    }
}

/// **Save as** with `typed` from the keyboard. The name is trimmed and
/// checked as the store checks it; the open variant's own name replaces
/// without asking.
pub fn save_as(open: Option<&str>, typed: &str) -> Result<SaveStep, String> {
    let name = typed.trim();
    forge_doc::variant_name(name).map_err(|e| e.to_string())?;
    Ok(SaveStep::Write {
        name: name.to_owned(),
        replace: open.is_some_and(|open| same_name(open, name)),
    })
}

#[cfg(test)]
#[path = "variants_tests.rs"]
mod tests;
