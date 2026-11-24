//! Edits go to the edit target (docs/editor/studio-redesign.md, "Editing a
//! disc asset"). The edit target is a local package
//! (`<data>/packages/target.txt`, `package_doc::target`). Every change to
//! the open map's tags is written there as edit lines
//! (`edits/<map>.yaml`, an `edits` item for that map), by [`edit_lines`]
//! after each write, undo and redo. The file is the journal: an undo
//! that takes a field back to the file's bytes takes its line out, also
//! after the package was saved or the Studio restarted.
//!
//! The lines of tags this session has not changed stay as the file has
//! them; the lines of a tag it has changed are the document's, so undoing
//! to the file's bytes removes them.
//!
//! With no edit target the first edit asks for one: the window's change is
//! held, the picker offers the local packages and **New package...**, and
//! the choice sets the target and runs the held change. The agent API
//! answers `NO_TARGET` instead of asking.
use std::collections::BTreeSet;

use ::workspace::{TagEdit, edits_text, parse_edits};

use super::edit_lines;
use super::package_doc;
use super::tag_doc::TagAction;
use crate::App;

/// What an edit without a target says; the API's `NO_TARGET` message.
pub const NO_TARGET: &str = "there is no edit target: package.target {id} sets one (or the window's first edit asks)";

/// The answer to the first-edit picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditChoice {
    /// An existing local package.
    Existing(String),
    /// A new package of this display name.
    New(String),
    /// Drop the change that asked.
    Cancel,
}

/// The first-edit picker while it is open.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Picker {
    /// The name typed for **New package...**.
    pub new_name: String,
}

/// Where edits go, and what is waiting for it.
#[derive(Default)]
pub struct EditState {
    /// Whether edits go to the target: the Studio sets it
    /// ([`App::edits_to_target`]); a document held in a test without a data
    /// root of its own does not touch the user's packages.
    armed: bool,
    /// The picker, open while a change waits for a target.
    pub picker: Option<Picker>,
    /// The change the picker was opened for.
    held: Option<TagAction>,
    pub(super) held_asset: Option<super::assets::CreateAsset>,
    /// The tags this session has changed, by `group:path`.
    touched: BTreeSet<String>,
    /// The map's tag document the touched set is for.
    rev: Option<u64>,
    /// Notices already shown, so each is said once.
    said: BTreeSet<String>,
}

impl EditState {
    /// Open the first-edit picker with nothing held.
    pub fn open_picker(&mut self) {
        self.picker.get_or_insert_with(Picker::default);
    }
}

/// A package id from a display name: lowercase letters, digits and `-`.
pub fn slug_of(name: &str) -> String {
    let mut slug = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "my-edits".to_owned()
    } else {
        slug.chars().take(40).collect::<String>().trim_end_matches('-').to_owned()
    }
}

fn changes_tag(action: &TagAction) -> bool {
    matches!(
        action,
        TagAction::Write(_)
            | TagAction::Block { .. }
            | TagAction::SetString { .. }
            | TagAction::ResizeData { .. }
    )
}

impl App {
    /// Make edits go to the edit target: the Studio's setting.
    pub fn edits_to_target(&mut self) {
        self.edits.armed = true;
    }

    /// The edit target: a local package.
    pub fn edit_target(&self) -> Option<String> {
        package_doc::target(&self.data_root())
    }

    /// Why an edit cannot be made: with the target on and none set.
    pub fn edit_refusal(&self) -> Option<&'static str> {
        (self.edits.armed && self.edit_target().is_none()).then_some(NO_TARGET)
    }

    /// The local packages the picker lists: `(id, name)`.
    pub fn local_packages(&self) -> Vec<(String, String)> {
        super::assets::read_set(&self.data_root())
            .listed
            .into_iter()
            .filter(|l| l.dir.is_some())
            .map(|l| (l.id, l.name))
            .collect()
    }

    /// A change of the tag editor on its way in: held and the picker
    /// opened when there is no target, else handed back to run.
    pub(super) fn gate_tag_action(&mut self, action: TagAction) -> Option<TagAction> {
        if !changes_tag(&action) || self.edit_refusal().is_none() {
            return Some(action);
        }
        self.edits.held = Some(action);
        self.edits.picker.get_or_insert_with(Picker::default);
        self.error(NO_TARGET);
        None
    }

    /// [`Action::ChooseEditTarget`].
    pub(super) fn choose_edit_target(&mut self, choice: EditChoice) {
        let data = self.data_root();
        let picked = match choice {
            EditChoice::Cancel => {
                self.edits.picker = None;
                self.edits.held = None;
                self.edits.held_asset = None;
                return;
            }
            EditChoice::Existing(id) => Ok(id),
            EditChoice::New(name) => {
                let id = slug_of(&name);
                package_doc::new(&data, &id, name.trim()).map(|_| id)
            }
        };
        let set = picked.and_then(|id| package_doc::edit_target(&data, Some(id.as_str()), false).map(|_| id));
        match set {
            Ok(id) => {
                self.edits.picker = None;
                self.info(format!("edits go to {id}"));
                match self.edits.held.take() {
                    Some(action) => self.run_map_tag(action),
                    None => self.sync_edit_lines(),
                }
                if let Some(request) = self.edits.held_asset.take() {
                    self.create_in_target(request);
                }
            }
            Err(e) => self.error(e.to_string()),
        }
    }

    /// Write the open map's changes to the edit target as edit lines. Does
    /// nothing with no target, or with edits not going to it.
    pub(super) fn sync_edit_lines(&mut self) {
        if !self.edits.armed || !self.tags.is_open() {
            return;
        }
        let data = self.data_root();
        let (Some(id), Some(map)) = (
            package_doc::target(&data),
            self.session.map_name().map(str::to_owned),
        ) else {
            return;
        };
        if self.edits.rev != self.tags.for_rev {
            self.edits.rev = self.tags.for_rev;
            self.edits.touched.clear();
        }
        let now = edit_lines::of_doc(&self.tags);
        self.edits.touched.extend(now.changed.iter().cloned());
        for why in now.skipped {
            if self.edits.said.insert(why.clone()) {
                self.error(format!("not recorded as an edit line: {why}"));
            }
        }
        // The lines the file has for tags this session has not touched stay.
        let path = data
            .join(::workspace::PACKAGES_DIR)
            .join(&id)
            .join(package_doc::edits_file(&map));
        let mut edits: Vec<TagEdit> = match std::fs::read_to_string(&path) {
            Ok(text) => match parse_edits(&text) {
                Ok(list) => list
                    .into_iter()
                    .map(|(e, _)| e)
                    .filter(|e| !self.edits.touched.contains(&e.tag))
                    .collect(),
                Err(e) => {
                    return self.error(format!(
                        "{} does not read, so no edit line was written: {}",
                        path.display(),
                        e.why
                    ));
                }
            },
            Err(_) => Vec::new(),
        };
        edits.extend(now.edits);
        edits.sort_by(|a, b| a.tag.cmp(&b.tag));
        let text = match edits_text(&edits) {
            Ok(t) => t,
            Err(e) => return self.error(format!("edit lines: {e}")),
        };
        if let Err(e) = package_doc::put_edits(&data, &id, &map, &text) {
            self.error(format!("writing the edit lines to {id}: {e}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::slug_of;

    #[test]
    fn a_name_becomes_a_package_id() {
        assert_eq!(slug_of("My Edits!"), "my-edits");
        assert_eq!(slug_of("  "), "my-edits");
        assert_eq!(slug_of("Pogo 2"), "pogo-2");
        assert!(content::package::valid_slug(&slug_of("A very long name ".repeat(10).as_str())));
    }
}
