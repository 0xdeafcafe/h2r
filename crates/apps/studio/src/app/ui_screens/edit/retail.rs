//! The retail sink (docs/game/ui-builder.md "Editing in the editor"): a
//! retail screen is a `wgit` tag of the user's `mainmenu.map`, so moving
//! one of its widgets writes a field of that tag through the map's tag
//! document ([`App::tags`], [`tag_api::set_field`]): the Tags tool's
//! path, with its undo and its save, which patches the user's map. Nothing
//! of the disc goes anywhere else.
//!
//! The picture is still drawn from the disc's map; the moved fields are
//! read back from the document as [`RetailPlaces`], and the pane's edited
//! blocks as `RetailBlocks` ([`super::retail_tag`]), and applied before it
//! is drawn ([`boot::ui_kit::preview::Disc::render_retail_edited`]).

use boot::ui_kit::preview::{RetailField, RetailPlaces, retail_block, retail_field};
use boot::ui_kit::view::{View, Widget};
use map::TagId;

use super::Change;
use crate::App;
use crate::app::TagAction;
use crate::app::tag_api;

/// The map whose screen tags the retail screens are.
pub const MENU_MAP: &str = "mainmenu";

/// The pane the tool shows of a retail screen.
pub const PANE: usize = 0;

/// A merge key per widget, so one drag is one undo step.
pub(super) fn merge_key(tag: &str, id: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    ("ui-retail", tag, id).hash(&mut h);
    h.finish()
}

/// The rect a field's JSON value gives, `base` sizing a point.
fn rect_of(field: &RetailField, v: &serde_json::Value, base: [i16; 4]) -> Option<[i16; 4]> {
    let n: Vec<i16> = v
        .as_array()?
        .iter()
        .map(|x| x.as_i64().map(|n| n as i16))
        .collect::<Option<_>>()?;
    match (field, n.as_slice()) {
        (RetailField::Rect(_), [t, l, b, r]) => Some([*t, *l, *b, *r]),
        (RetailField::Point(_), [x, y]) => {
            let [t, l, b, r] = base;
            Some([*y, *x, *y - (t - b), *x + (r - l)])
        }
        _ => None,
    }
}

impl App {
    /// Screen tag `tag` in the map's tag document, or why it can't be
    /// edited there.
    pub(super) fn ui_retail_tag(&mut self, tag: &str) -> Result<TagId, String> {
        self.sync_tags();
        let open = self.loaded_map().map(str::to_ascii_lowercase);
        if open.as_deref() != Some(MENU_MAP) {
            return Err(format!(
                "refused: {tag} is a tag of {MENU_MAP}.map, and the map open is {}: open \
                 {MENU_MAP} (Maps) to edit the retail screens; their edits save with that map",
                open.as_deref().unwrap_or("none")
            ));
        }
        if !self.tags.is_open() {
            return Err(format!(
                "{MENU_MAP}'s tags did not load: {}",
                self.tags.error.clone().unwrap_or_default()
            ));
        }
        self.tags
            .tags()
            .iter()
            .find(|t| t.group == tags::ui::SCREEN && t.name.eq_ignore_ascii_case(tag))
            .map(|t| t.id)
            .ok_or_else(|| format!("{MENU_MAP} has no screen tag {tag}"))
    }

    /// Moves or resizes retail widget `w` of `tag` (a text is refused) as
    /// one step of the map's tag document.
    pub(super) fn ui_retail_change(
        &mut self,
        tag: &str,
        w: &Widget,
        change: Change,
        merge: bool,
    ) -> Result<String, String> {
        let Change::Rect(rect) = change else {
            return Err(format!(
                "refused: {}'s text is in the map's localized string table (unic), which the \
                 editor does not write",
                w.id
            ));
        };
        let field = retail_field(PANE, &w.id)?;
        if matches!(field, RetailField::Point(_)) {
            let size = |r: [i16; 4]| (r[3] - r[1], r[0] - r[2]);
            if size(rect) != size(w.rect) {
                return Err(format!(
                    "refused: {}'s size is its image's or skin's; it moves only",
                    w.id
                ));
            }
        }
        let id = self.ui_retail_tag(tag)?;
        let action = tag_api::set_field(&self.tags, id, field.path(), &field.value(rect))?;
        let label = format!("{tag}: {} to {rect:?}", w.id);
        match action {
            TagAction::Write(mut write) => {
                write.label = label.clone();
                write.merge = merge.then(|| merge_key(tag, &w.id));
                self.tags.write(write)?;
            }
            other => return Err(format!("{}: not a field write ({other:?})", field.path())),
        }
        self.push_tag_patches();
        self.ui_retail_refresh(tag);
        Ok(label)
    }

    /// Puts widget `id` of `tag` (or the whole screen) back as the loaded
    /// map has it, as one step of the map's tag document: a moved widget
    /// where it was, an edited text, button or bitmap block's element
    /// whole (its rect, its string ID), the title's string ID, and a
    /// block the screen gained removed ([`super::retail_tag`]).
    pub(super) fn ui_retail_revert(&mut self, tag: &str, id: Option<&str>) -> Result<String, String> {
        let edits = &self.ui_screens.edit;
        if id.is_none()
            && edits.places.get(tag).is_none_or(|p| p.is_empty())
            && edits.changes.get(tag).is_none_or(|c| c.is_empty())
        {
            return Ok(format!("{tag}: nothing was edited"));
        }
        let tag_id = self.ui_retail_tag(tag)?;
        let mark = self.tags.undo_len();
        self.tags.seal();
        if let Err(e) = self.ui_retail_revert_steps(tag, tag_id, id) {
            self.tags.rollback(mark);
            return Err(e);
        }
        let label = match id {
            Some(id) => format!("{tag}: reverted {id}"),
            None => format!("{tag}: reverted the screen"),
        };
        if self.tags.undo_len() > mark {
            self.tags.squash(mark, label.clone());
        }
        self.tags.seal();
        self.push_tag_patches();
        self.ui_retail_refresh(tag);
        if id.is_some_and(|id| self.ui_screens.edit.selected.as_deref() == Some(id))
            && id.is_some_and(|id| self.ui_screens.edit_widget(id).is_err())
        {
            // A removed copy is no longer there to be selected.
            self.ui_screens.edit.selected = None;
        }
        Ok(label)
    }

    /// The writes of [`App::ui_retail_revert`], each a step of its own.
    fn ui_retail_revert_steps(
        &mut self,
        tag: &str,
        tag_id: TagId,
        id: Option<&str>,
    ) -> Result<(), String> {
        match id {
            Some(id) if id == "title" || retail_block(PANE, id).is_ok() => {
                self.ui_retail_restore(tag_id, id)
            }
            Some(id) => self.ui_retail_replace(tag, tag_id, id),
            None => {
                self.ui_retail_restore_all(tag_id)?;
                // Lists and tables: the widgets moved that are not blocks
                // restored whole.
                let moved: Vec<String> = self
                    .ui_screens
                    .edit
                    .places
                    .get(tag)
                    .map(|p| p.keys().cloned().collect())
                    .unwrap_or_default();
                for wid in moved.iter().filter(|w| retail_block(PANE, w).is_err()) {
                    self.ui_retail_replace(tag, tag_id, wid)?;
                }
                Ok(())
            }
        }
    }

    /// Writes widget `wid`'s place field back to the disc's.
    fn ui_retail_replace(&mut self, tag: &str, tag_id: TagId, wid: &str) -> Result<(), String> {
        let base = self.ui_retail_base(tag)?;
        let w = base
            .widget(wid)
            .ok_or_else(|| format!("{tag} has no widget {wid:?}"))?;
        let field = retail_field(PANE, wid)?;
        match tag_api::set_field(&self.tags, tag_id, field.path(), &field.value(w.rect))? {
            TagAction::Write(w) => self.tags.write(w),
            other => Err(format!("{}: not a field write ({other:?})", field.path())),
        }
    }

    /// Retail screen `tag`'s widgets as the disc has them.
    pub(super) fn ui_retail_base(&mut self, tag: &str) -> Result<View, String> {
        if let Some(v) = self.ui_screens.edit.retail_base.get(tag) {
            return Ok(v.clone());
        }
        let v = self.ui_screens.retail_base_view(tag)?;
        self.ui_screens
            .edit
            .retail_base
            .insert(tag.to_owned(), v.clone());
        Ok(v)
    }

    /// Reads `tag`'s widget places back from the map's tag document: each
    /// widget whose field differs from the disc's.
    pub(crate) fn ui_retail_refresh(&mut self, tag: &str) {
        let places = self.ui_retail_places(tag).unwrap_or_default();
        self.ui_screens.edit.places.insert(tag.to_owned(), places);
        self.ui_retail_read_blocks(tag);
        self.ui_screens.redraw();
    }

    fn ui_retail_places(&mut self, tag: &str) -> Result<RetailPlaces, String> {
        let base = self.ui_retail_base(tag)?;
        let id = match self.ui_retail_tag(tag) {
            Ok(id) => id,
            // Not the open map: the disc's places.
            Err(_) => return Ok(RetailPlaces::new()),
        };
        let mut places = RetailPlaces::new();
        for w in &base.widgets {
            let Ok(field) = retail_field(PANE, &w.id) else {
                continue;
            };
            let Ok((at, _, f)) = tag_api::resolve(&self.tags, id, field.path()) else {
                continue;
            };
            let v = tag_api::value(&self.tags, id, at, &f);
            if let Some(r) = rect_of(&field, &v, w.rect)
                && r != w.rect
            {
                places.insert(w.id.clone(), r);
            }
        }
        Ok(places)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_point_field_keeps_the_widgets_size() {
        let base = [100, -50, 80, 20];
        let point = RetailField::Point(String::new());
        assert_eq!(
            rect_of(&point, &serde_json::json!([-40, 110]), base),
            Some([110, -40, 90, 30])
        );
        let rect = RetailField::Rect(String::new());
        assert_eq!(
            rect_of(&rect, &serde_json::json!([1, 2, 0, 9]), base),
            Some([1, 2, 0, 9])
        );
        assert_eq!(rect_of(&rect, &serde_json::json!([1, 2]), base), None);
        assert_eq!(rect_of(&point, &serde_json::json!("x"), base), None);
        assert_ne!(merge_key("a", "text.0"), merge_key("a", "text.1"));
    }
}
