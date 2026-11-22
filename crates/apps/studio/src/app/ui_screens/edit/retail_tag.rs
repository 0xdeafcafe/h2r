//! Retail tag edits past a rect (docs/game/ui-builder.md "Retail tag
//! edits"): a text or bitmap block hidden, a text, button or bitmap block
//! copied to the end of its block, and the string ID of a title, a text
//! or a button set. Each is a write of the screen's `wgit` tag through
//! the map's tag document, as a move is ([`super::retail`]): one undo
//! step, saved with the map as a patch.
//!
//! - **Hide** parks the block off screen, where the disc keeps its own
//!   spare blocks (`preview::parked`). The game builds the widget and
//!   draws nothing of it. A button is refused: it keeps its place in the
//!   focus order wherever its rect is.
//! - **Copy** appends an element to the block and writes the copied
//!   element's bytes into it, so every block before it keeps its index
//!   (the game's code finds a pane's widgets by block index). A full
//!   block grows only with Edit > Allow growing full blocks on.
//! - **Set string** writes the string ID field with the ID of a string
//!   by name, adding the name to the map when it has none
//!   (`TagDoc::set_string`). The localized text of a retail string lives
//!   in the map's `unic` table, which the editor does not write; a string
//!   of ours has no text there, and the game's code answers it at run
//!   time. The picture draws the stand-in text given with the edit
//!   (`UiEdit::stand_ins`), which is the tool's and is not saved.
//!
//! The picture is drawn from the disc's map, so after every edit the
//! pane's text, button and bitmap blocks are read back whole from the
//! document ([`App::ui_retail_read_blocks`]) and drawn in the disc's
//! place (`preview::RetailEdits`).

use std::sync::Arc;

use boot::ui_kit::preview::{
    RetailBlocks, RetailField, parked, retail_block, retail_copy_id, retail_field,
    retail_string_field,
};
use boot::ui_kit::view::Widget;
use map::TagId;
use tags::registry::{Kind, Layout};

use super::retail::{MENU_MAP, PANE};
use crate::App;
use crate::app::tag_api;
use crate::app::{BlockOp, Pick, TagAction, TagWrite};

/// The pane's blocks read back whole: a widget id's kind and the block's
/// field name, in [`RetailBlocks::decode`]'s order.
const BLOCKS: [(&str, &str); 3] = [
    ("text", "text_blocks"),
    ("button", "buttons"),
    ("bitmap", "bitmaps"),
];

/// The retail screen of `pick`, or the refusal for one of ours.
pub fn retail_only<'a>(pick: &'a Pick, what: &str) -> Result<&'a str, String> {
    match pick {
        Pick::Retail { tag, .. } => Ok(tag),
        Pick::Native { screen, .. } => Err(format!(
            "refused: {what} is an edit of a retail screen's tag; {screen} is ours, and its \
             widgets come from its YAML and its Rust"
        )),
    }
}

/// Whether `name` can name a string ID of ours: lowercase letters, digits
/// and `_`, as the map's own are.
pub fn string_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// What differs from the loaded map in a retail screen's pane.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// Blocks the pane gained, by widget id.
    pub added: Vec<String>,
    /// Blocks whose element differs (a rect, a string ID), by widget id.
    pub edited: Vec<String>,
    /// The screen's header string ID differs.
    pub header: bool,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.edited.is_empty() && !self.header
    }
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "added": self.added,
            "edited": self.edited,
            "header": self.header,
        })
    }
}

fn le32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// A block of the screen tag in the map's tag document.
struct BlockAt {
    /// The address of its `(count, address)` pair.
    pair: u32,
    layout: Arc<Layout>,
    count: u32,
    start: u32,
    /// Its count and elements in the loaded map.
    source_count: u32,
    source_start: u32,
}

impl App {
    fn ui_retail_block_at(&self, tag: TagId, path: &str) -> Result<BlockAt, String> {
        let (at, _, f) = tag_api::resolve(&self.tags, tag, path)?;
        let Kind::Block(layout) = &f.kind else {
            return Err(format!("{path} is not a block"));
        };
        let pair = at + f.offset;
        let (count, start) = self
            .tags
            .block(tag, pair, layout)
            .ok_or_else(|| format!("{path}: the block is unreadable"))?;
        let (source_count, source_start) = match self.tags.read_source(tag, pair, 8) {
            Some(b) => (le32(b), le32(&b[4..])),
            None => (count, start),
        };
        Ok(BlockAt {
            pair,
            layout: layout.clone(),
            count,
            start,
            source_count,
            source_start,
        })
    }

    /// The pane's text, button and bitmap blocks and the header string ID
    /// as the document has them, and what differs from the loaded map.
    fn ui_retail_blocks(&self, tag: TagId) -> Result<(RetailBlocks, Changes), String> {
        let mut changes = Changes::default();
        let mut bytes: Vec<Vec<u8>> = Vec::new();
        for (kind, name) in BLOCKS {
            let b = self.ui_retail_block_at(tag, &format!("panes[{PANE}].{name}"))?;
            let size = b.layout.size;
            let now = if b.count == 0 {
                Vec::new()
            } else {
                self.tags
                    .read(tag, b.start, b.count * size)
                    .ok_or_else(|| format!("{name}: the block is unreadable"))?
            };
            for i in 0..b.count {
                let at = (i * size) as usize;
                let element = &now[at..at + size as usize];
                if i >= b.source_count {
                    changes.added.push(format!("{kind}.{i}"));
                } else if self.tags.read_source(tag, b.source_start + i * size, size)
                    != Some(element)
                {
                    changes.edited.push(format!("{kind}.{i}"));
                }
            }
            bytes.push(now);
        }
        let (at, _, f) = tag_api::resolve(&self.tags, tag, "header")?;
        let a = at + f.offset;
        let header = self
            .tags
            .read(tag, a, 4)
            .ok_or("the header string ID is unreadable")?;
        changes.header = self.tags.read_source(tag, a, 4) != Some(header.as_slice());
        Ok((
            RetailBlocks::decode(le32(&header), &bytes[0], &bytes[1], &bytes[2]),
            changes,
        ))
    }

    /// Reads `tag`'s blocks back from the map's tag document for the
    /// picture, and gives the stand-in texts the string IDs the map has
    /// for their names. With the menu map not open, the picture is the
    /// disc's.
    pub(super) fn ui_retail_read_blocks(&mut self, tag: &str) {
        let read = self
            .ui_retail_tag(tag)
            .and_then(|id| self.ui_retail_blocks(id));
        let edit = &mut self.ui_screens.edit;
        match read {
            Ok((blocks, changes)) => {
                edit.blocks.insert(tag.to_owned(), blocks);
                edit.changes.insert(tag.to_owned(), changes);
            }
            Err(_) => {
                edit.blocks.remove(tag);
                edit.changes.remove(tag);
            }
        }
        let texts = self
            .ui_screens
            .edit
            .stand_ins
            .iter()
            .filter_map(|(name, text)| Some((self.tags.string_id(name)?, text.clone())))
            .collect();
        self.ui_screens.edit.texts = texts;
    }

    /// A retail screen was picked: with the menu map open, its picture
    /// takes that map's edits of the tag (saved ones too).
    pub(crate) fn ui_retail_picked(&mut self, tag: &str) {
        if self
            .loaded_map()
            .is_some_and(|m| m.eq_ignore_ascii_case(MENU_MAP))
        {
            self.ui_retail_refresh(tag);
        }
    }

    /// Parks retail text or bitmap `w` of `tag` off screen, as one step
    /// of the map's tag document.
    pub(super) fn ui_retail_hide(&mut self, tag: &str, w: &Widget) -> Result<String, String> {
        match w.id.split('.').next() {
            Some("text" | "bitmap") => {}
            Some("button") => {
                return Err(format!(
                    "refused: {} is a button, which keeps its place in the screen's focus \
                     order wherever its rect is; the game's code shows and hides buttons",
                    w.id
                ));
            }
            Some("title") => {
                return Err(
                    "refused: the title is hidden by the tag's no_header flag (tag.set, \
                     flags), not by a rect"
                        .to_owned(),
                );
            }
            _ => {
                return Err(format!(
                    "refused: {} is not a text or bitmap block; a list or a table is the \
                     game's code's to fill and to hide",
                    w.id
                ));
            }
        }
        let field = retail_field(PANE, &w.id)?;
        let rect = parked(w.rect, matches!(field, RetailField::Point(_)));
        let id = self.ui_retail_tag(tag)?;
        let label = format!("{tag}: hid {}", w.id);
        match tag_api::set_field(&self.tags, id, field.path(), &field.value(rect))? {
            TagAction::Write(mut write) => {
                write.label = label.clone();
                self.tags.write(write)?;
            }
            other => return Err(format!("{}: not a field write ({other:?})", field.path())),
        }
        self.tags.seal();
        self.push_tag_patches();
        self.ui_retail_refresh(tag);
        Ok(label)
    }

    /// Appends a copy of retail text, button or bitmap `w` to its block,
    /// as one step of the map's tag document. The copy's widget id and
    /// the log line.
    pub(super) fn ui_retail_copy(
        &mut self,
        tag: &str,
        w: &Widget,
    ) -> Result<(String, String), String> {
        let (path, index) = retail_block(PANE, &w.id)?;
        let id = self.ui_retail_tag(tag)?;
        let b = self.ui_retail_block_at(id, &path)?;
        let index = u32::try_from(index)
            .ok()
            .filter(|i| *i < b.count)
            .ok_or_else(|| format!("{tag} has no block element for {}", w.id))?;
        let size = b.layout.size;
        let element = self
            .tags
            .read(id, b.start + index * size, size)
            .ok_or("the block's element is unreadable")?;
        let copy = retail_copy_id(&w.id, b.count as usize)
            .ok_or_else(|| format!("{} is not a block of the screen tag", w.id))?;
        let mark = self.tags.undo_len();
        self.tags.seal();
        // A blank element at the end, then the copied bytes: no block
        // before it changes its index.
        let mut made = self
            .tags
            .block_op(id, b.pair, &b.layout, &BlockOp::Add)
            .map_err(|e| format!("refused: {path}: {e}"));
        if made.is_ok() {
            // Growing moves the block: its elements are where the pair
            // says now.
            made = match self.tags.block(id, b.pair, &b.layout) {
                Some((_, start)) => self.tags.write(TagWrite {
                    tag: id,
                    address: start + b.count * size,
                    bytes: element,
                    label: copy.clone(),
                    merge: None,
                }),
                None => Err(format!("{path}: unreadable after the add")),
            };
        }
        if let Err(e) = made {
            self.tags.rollback(mark);
            return Err(e);
        }
        let label = format!("{tag}: copied {} to {copy}", w.id);
        self.tags.squash(mark, label.clone());
        self.tags.seal();
        self.push_tag_patches();
        self.ui_retail_refresh(tag);
        Ok((copy, label))
    }

    /// Sets the string ID of retail widget `wid` (the title, a text, a
    /// button) of `tag` to the string named `string`, as one step of the
    /// map's tag document; `shows` becomes that string's stand-in text.
    pub(super) fn ui_retail_set_string(
        &mut self,
        tag: &str,
        wid: &str,
        string: &str,
        shows: Option<String>,
    ) -> Result<String, String> {
        let path = retail_string_field(PANE, wid)?;
        if !string_name_ok(string) {
            return Err(format!(
                "string: {string:?} is not a string ID's name (lowercase letters, digits and _)"
            ));
        }
        let id = self.ui_retail_tag(tag)?;
        let label = format!("{tag}: {wid} reads {string}");
        match tag_api::set_field(&self.tags, id, &path, &serde_json::json!(string))? {
            TagAction::Write(mut write) => {
                write.label = label.clone();
                self.tags.write(write)?;
            }
            TagAction::SetString {
                tag, address, text, ..
            } => self.tags.set_string(tag, address, &text, &label)?,
            other => return Err(format!("{path}: not a string ID write ({other:?})")),
        }
        self.tags.seal();
        if let Some(text) = shows {
            self.ui_screens
                .edit
                .stand_ins
                .insert(string.to_owned(), text);
        }
        self.push_tag_patches();
        self.ui_retail_refresh(tag);
        Ok(label)
    }

    /// Writes the title's string ID or the block element of widget `wid`
    /// back as the loaded map has it; a block the pane gained is removed
    /// (the blocks after it move down one).
    pub(super) fn ui_retail_restore(&mut self, tag: TagId, wid: &str) -> Result<(), String> {
        if wid == "title" {
            return match tag_api::reset(&self.tags, tag, "header")? {
                Some(TagAction::Write(w)) => self.tags.write(w),
                _ => Ok(()),
            };
        }
        let (path, index) = retail_block(PANE, wid)?;
        let b = self.ui_retail_block_at(tag, &path)?;
        let index = u32::try_from(index)
            .ok()
            .filter(|i| *i < b.count)
            .ok_or_else(|| format!("the screen has no widget {wid:?}"))?;
        self.ui_retail_restore_element(tag, &b, index, wid)
    }

    fn ui_retail_restore_element(
        &mut self,
        tag: TagId,
        b: &BlockAt,
        index: u32,
        label: &str,
    ) -> Result<(), String> {
        let size = b.layout.size;
        if index >= b.source_count {
            return self
                .tags
                .block_op(tag, b.pair, &b.layout, &BlockOp::Remove(index));
        }
        let source = self
            .tags
            .read_source(tag, b.source_start + index * size, size)
            .ok_or("the loaded map has no such element")?
            .to_vec();
        self.tags.write(TagWrite {
            tag,
            address: b.start + index * size,
            bytes: source,
            label: label.to_owned(),
            merge: None,
        })
    }

    /// [`App::ui_retail_restore`] for the title and every text, button
    /// and bitmap block of the pane, the gained ones removed last first.
    pub(super) fn ui_retail_restore_all(&mut self, tag: TagId) -> Result<(), String> {
        self.ui_retail_restore(tag, "title")?;
        for (kind, name) in BLOCKS {
            let b = self.ui_retail_block_at(tag, &format!("panes[{PANE}].{name}"))?;
            for index in (0..b.count).rev() {
                self.ui_retail_restore_element(tag, &b, index, &format!("{kind}.{index}"))?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tag_edits_are_of_retail_screens_only() {
        let retail = Pick::retail(r"ui\screens\x");
        assert_eq!(retail_only(&retail, "hiding a widget"), Ok(r"ui\screens\x"));
        let ours = Pick::Native {
            screen: "kit.example".into(),
            state: "sample".into(),
        };
        let e = retail_only(&ours, "hiding a widget").unwrap_err();
        assert!(e.starts_with("refused"), "{e}");
        assert!(e.contains("kit.example") && e.contains("hiding a widget"), "{e}");
    }

    #[test]
    fn a_string_name_is_lowercase_snake() {
        for ok in ["lobby_mode", "h2r_lobby_mode_2", "a"] {
            assert!(string_name_ok(ok), "{ok}");
        }
        for bad in ["", "Lobby Mode", "lobby-mode", "lobby mode", "MODE", "mode\0"] {
            assert!(!string_name_ok(bad), "{bad:?}");
        }
    }

    #[test]
    fn changes_say_what_differs() {
        let mut c = Changes::default();
        assert!(c.is_empty());
        c.header = true;
        assert!(!c.is_empty());
        c.added.push("text.48".into());
        c.edited.push("button.1".into());
        assert_eq!(
            c.json(),
            serde_json::json!({"added": ["text.48"], "edited": ["button.1"], "header": true})
        );
    }

    #[test]
    fn retail_tag_edits_need_the_disc() {
        use super::super::EditAction;
        use crate::app::{Action, ToolKind};
        let mut app = App::new();
        app.dispatch(Action::SetTool(ToolKind::UiScreens));
        app.dispatch(Action::UiPick(Pick::retail(r"ui\screens\x")));
        // No disc: no picture, so no widget to hide or copy.
        for a in [
            EditAction::Hide { id: "text.0".into() },
            EditAction::Duplicate { id: "text.0".into() },
        ] {
            app.dispatch(Action::UiEdit(a));
            let e = app.ui_screens.edit.last_error.clone().expect("refused");
            assert!(e.contains("nothing is shown"), "{e}");
        }
        // A string ID is set by index, so it reaches the map check: the
        // open map is not the menu's.
        app.dispatch(Action::UiEdit(EditAction::SetString {
            id: "text.0".into(),
            string: "lobby_mode".into(),
            shows: Some("CUSTOM GAMES".into()),
        }));
        let e = app.ui_screens.edit.last_error.clone().expect("refused");
        assert!(e.starts_with("refused") && e.contains("mainmenu"), "{e}");
        assert!(app.ui_screens.edit.stand_ins.is_empty());
        // A bad name is caught before the map is.
        app.dispatch(Action::UiEdit(EditAction::SetString {
            id: "title".into(),
            string: "Lobby Mode".into(),
            shows: None,
        }));
        let e = app.ui_screens.edit.last_error.clone().expect("refused");
        assert!(e.contains("not a string ID's name"), "{e}");
        assert!(!app.ui_can_undo_redo(false));
    }
}
