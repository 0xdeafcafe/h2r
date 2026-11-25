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
//! - **Map variants** ([`list_sheet`]) lists every variant of the map, and
//!   A on one opens its dialog ([`variant_sheet`]): **Load**, **Rename**,
//!   **Duplicate** and **Delete**, each present
//!   only when it can act ([`Row::loadable`], [`Row::usable`], [`Row::saved`]
//!   and the open variant's own name). Rename and Duplicate type the name
//!   on the game's keyboard; Delete asks [`delete_question`]. Each ends in
//!   a notice ([`renamed`], [`duplicated`], [`deleted`]) or the store's
//!   sentence, and writes nothing on a refusal.
//! - Every flow ends in a notice: [`saved`], [`loaded`], [`NOT_SAVED`],
//!   [`NOT_LOADED`], or the store's own sentence for a refusal.

use forge::files::variants::{Listed, Source};

use super::sheet::{Act, Choice, Kind, NumberField, Sheet, SheetRow, TypeSpec};

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

/// The Map variants page's row value for a variant on this machine.
pub const SAVED: &str = "Saved";
/// The row value for a variant fetched from Xbox Live.
pub const ON_LIVE: &str = "Xbox Live";
/// The row value for a variant that fails a check; its sentence is the
/// row's description.
pub const UNUSABLE: &str = "Unusable";
/// The page's one row when the map has no variant.
pub const NO_VARIANTS: &str = "No map variants";

/// The body of the Delete question.
pub fn delete_question(name: &str) -> String {
    format!("Delete {name}?")
}

/// The notice after a rename.
pub fn renamed(name: &str) -> String {
    format!("Renamed to {name}.")
}

/// The notice after a duplicate.
pub fn duplicated(name: &str) -> String {
    format!("Copied as {name}.")
}

/// The notice after a delete.
pub fn deleted(name: &str) -> String {
    format!("Deleted {name}.")
}

/// The notice when No answers the Delete question.
pub const NOT_DELETED: &str = "Not deleted.";

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

    /// Whether it passes the store's checks.
    pub fn usable(&self) -> bool {
        self.sentence.is_none()
    }

    /// The page's value column: where it is, or that it cannot be used.
    pub fn value(&self) -> &'static str {
        match (&self.sentence, self.saved) {
            (Some(_), _) => UNUSABLE,
            (None, true) => SAVED,
            (None, false) => ON_LIVE,
        }
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

/// What a variant's line in the Map variant picker says: its author and
/// what it adds, by kind of thing (the object menu's groups), in the
/// menu's order.
pub fn detail(doc: &forge_doc::ForgeDoc) -> String {
    use super::sheet::{GROUPS, Group};
    use forge_doc::{Edit, Placement};
    let mut counts = vec![0usize; GROUPS.len()];
    for op in &doc.ops {
        if let Edit::Placement(Placement::Add { block, .. }) = &op.edit {
            let group = Group::of_block(*block);
            if let Some(i) = GROUPS.iter().position(|d| d.group == group) {
                counts[i] += 1;
            }
        }
    }
    let adds: Vec<String> = GROUPS
        .iter()
        .zip(counts)
        .filter(|(_, n)| *n > 0)
        .map(|(d, n)| format!("{n} {}", if n == 1 { d.one } else { d.name }))
        .collect();
    let by = if doc.author.trim().is_empty() {
        String::new()
    } else {
        format!("By {}. ", doc.author.trim())
    };
    if adds.is_empty() {
        format!("{by}Adds no objects.")
    } else {
        format!("{by}Adds {}.", adds.join(", "))
    }
}

/// The Map variants page: a row a variant in the store's order, the open
/// variant's own row first described as open. A variant that fails a check
/// says why in the description. The page's focus is the first row.
pub fn list_sheet(rows: &[Row], open: Option<&str>) -> Sheet {
    let list: Vec<SheetRow> = if rows.is_empty() {
        vec![SheetRow::new(NO_VARIANTS, Act::Info).describe(NONE_SAVED)]
    } else {
        rows.iter()
            .map(|r| {
                let is_open = open.is_some_and(|o| same_name(&r.name, o));
                let help = match (&r.sentence, is_open) {
                    (Some(why), _) => why.clone(),
                    (None, true) => "The map variant open in this session.".to_owned(),
                    (None, false) if r.saved => "Saved on this machine.".to_owned(),
                    (None, false) => "Fetched from Xbox Live.".to_owned(),
                };
                SheetRow::new(
                    &r.name,
                    Act::Variant(r.file.clone(), Box::new(variant_sheet(r, open))),
                )
                .value(r.value())
                .describe(&help)
            })
            .collect()
    };
    Sheet {
        kind: Kind::Variants,
        title: "MAP VARIANTS".into(),
        body: String::new(),
        rows: list,
        focus: 0,
    }
}

/// A variant's dialog: **Load**, **Rename**, **Duplicate** and **Delete**,
/// each only when it can act (**Upload to Xbox Live** joins when the game
/// has a Live client for it). The open variant cannot be deleted: an info
/// row says so in place of Delete. A variant that fails a check can be
/// renamed and deleted, not loaded.
pub fn variant_sheet(row: &Row, open: Option<&str>) -> Sheet {
    let is_open = open.is_some_and(|o| same_name(&row.name, o));
    let mut rows = Vec::new();
    if row.loadable() {
        rows.push(
            SheetRow::new("Load", Act::Choose(Choice::Load(row.file.clone())))
                .describe(&format!("Open {} in this session.", row.name)),
        );
    }
    if row.saved {
        rows.push(
            SheetRow::new(
                "Rename",
                Act::Type(TypeSpec {
                    field: NumberField::RenameVariant,
                    initial: row.name.clone(),
                }),
            )
            .describe(&format!("Give {} another name. A name another variant has is refused.", row.name)),
        );
    }
    if row.saved && row.usable() {
        rows.push(
            SheetRow::new(
                "Duplicate",
                Act::Type(TypeSpec {
                    field: NumberField::DuplicateVariant,
                    initial: row.name.clone(),
                }),
            )
            .describe(&format!("Copy {} under a new name.", row.name)),
        );
    }
    if is_open {
        rows.push(SheetRow::new("Open now", Act::Info).describe(OPEN_CANNOT_DELETE));
    } else {
        rows.push(
            SheetRow::new("Delete", Act::Choose(Choice::DeleteVariant(row.file.clone())))
                .describe(&format!("Remove {} from this machine.", row.name)),
        );
    }
    Sheet {
        kind: Kind::Variant,
        title: "MAP VARIANT".into(),
        body: String::new(),
        rows,
        focus: 0,
    }
}

/// Why the open variant has no Delete row.
pub const OPEN_CANNOT_DELETE: &str = "The open map variant cannot be deleted.";

#[cfg(test)]
#[path = "variants_tests.rs"]
mod tests;
