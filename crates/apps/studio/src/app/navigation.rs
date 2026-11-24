//! The Scenario inspector's history. Placement selection still belongs
//! to the scenario session, so a reference jump leaves the viewport alone.
use map::TagId;
use scenario_edit::{BlockKind, Session};

use super::{App, Doc, Selection, SelectionRevision, tag_api};

const HISTORY_LIMIT: usize = 128;

#[derive(Clone, Debug, PartialEq)]
pub enum NavAction {
    SelectTag(TagId),
    SelectPalette {
        tag: TagId,
        placements: Vec<(BlockKind, usize)>,
    },
    /// Show a tag, a field of it (`path`), and in the Tags tool one list
    /// element as the whole panel (`element`, `parts[1]`; none for the
    /// root).
    Go {
        tag: TagId,
        path: Option<String>,
        element: Option<String>,
    },
    Follow {
        from: Vec<TagId>,
        path: String,
        tag: TagId,
    },
    Open(Vec<TagId>),
    Back,
    Forward,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavLocation {
    placements: Vec<(BlockKind, usize)>,
    palette_subject: bool,
    pub trail: Vec<TagId>,
    pub path: Option<String>,
    /// The list element the Tags tool shows alone (`parts[1]`), by field
    /// names; none for the tag's root.
    pub element: Option<String>,
}

impl NavLocation {
    fn placements(session: &Session) -> Self {
        Self {
            placements: session.selection(),
            palette_subject: false,
            trail: Vec::new(),
            path: None,
            element: None,
        }
    }

    pub fn selection(&self) -> Option<Selection> {
        if !self.palette_subject
            && let Some(&(kind, index)) = self.placements.first()
        {
            Some(Selection::Placement(kind, index))
        } else {
            self.trail.first().copied().map(Selection::Tag)
        }
    }

    pub fn tag(&self) -> Option<TagId> {
        self.trail.last().copied()
    }
}

/// The documents brought to the front, in order. The tag history above
/// keeps the selection inside tags; this keeps the way between documents
/// of every kind, so Back from a tag returns to the variant that led to it.
#[derive(Default)]
pub struct DocTrail {
    entries: Vec<Doc>,
    cursor: usize,
    replay: bool,
}

impl DocTrail {
    /// A document came to the front. A visit made by Back or Forward only
    /// moves the cursor to the document's nearest entry.
    pub(super) fn visit(&mut self, doc: Doc) {
        if self.entries.get(self.cursor) == Some(&doc) {
            return;
        }
        if self.replay {
            let at = |i: &usize| self.entries[*i] == doc;
            let near = (0..self.cursor).rev().find(at).or_else(|| (self.cursor..self.entries.len()).find(at));
            if let Some(i) = near {
                self.cursor = i;
            }
            return;
        }
        if !self.entries.is_empty() {
            self.entries.truncate(self.cursor + 1);
        }
        self.entries.push(doc);
        if self.entries.len() > HISTORY_LIMIT {
            self.entries.remove(0);
        }
        self.cursor = self.entries.len() - 1;
    }

    /// The nearest entry in a direction that is open and not in front.
    fn target(&self, forward: bool, front: Option<Doc>, open: impl Fn(Doc) -> bool) -> Option<usize> {
        let ok = |i: &usize| Some(self.entries[*i]) != front && open(self.entries[*i]);
        if forward {
            (self.cursor + 1..self.entries.len()).find(ok)
        } else {
            (0..self.cursor.min(self.entries.len())).rev().find(ok)
        }
    }

    /// The documents visited, oldest first, and the one the cursor is on.
    pub fn visited(&self) -> (&[Doc], usize) {
        (&self.entries, self.cursor)
    }
}

#[derive(Default)]
pub struct NavHistory {
    world_rev: u64,
    entries: Vec<NavLocation>,
    cursor: usize,
    revision: u64,
}

impl NavHistory {
    fn at(&self, session: &Session) -> Option<&NavLocation> {
        let entry = self.entries.get(self.cursor)?;
        (self.world_rev == session.world_rev() && entry.placements == session.selection())
            .then_some(entry)
    }

    fn sync(&mut self, world_rev: u64, location: NavLocation) {
        if self.world_rev != world_rev {
            self.world_rev = world_rev;
            self.entries.clear();
            self.cursor = 0;
        }
        if self.entries.get(self.cursor) != Some(&location) {
            self.push(location);
        }
    }

    fn push(&mut self, location: NavLocation) {
        self.revision += 1;
        self.entries.truncate(self.cursor + 1);
        self.entries.push(location);
        if self.entries.len() > HISTORY_LIMIT {
            self.entries.remove(0);
        }
        self.cursor = self.entries.len() - 1;
    }

    fn target(&self, forward: bool, session: &Session) -> Option<usize> {
        self.at(session)?;
        let valid = |i: usize| {
            self.entries[i].placements.iter().all(|&(kind, index)| {
                session
                    .scenario()
                    .is_some_and(|s| index < s.block(kind).len())
            })
        };
        if forward {
            (self.cursor + 1..self.entries.len()).find(|&i| valid(i))
        } else {
            (0..self.cursor).rev().find(|&i| valid(i))
        }
    }
}

impl App {
    /// Changes to any source of the inspector's selection, without cloning its history.
    pub(crate) fn selection_revision(&self) -> SelectionRevision {
        SelectionRevision {
            world: self.session.world_rev(),
            primary: self.session.selected(),
            volume: self.session.selected_volume(),
            navigation: self.nav.revision,
        }
    }

    pub fn nav_location(&self) -> NavLocation {
        self.nav
            .at(&self.session)
            .cloned()
            .unwrap_or_else(|| NavLocation::placements(&self.session))
    }

    pub fn can_navigate(&self, forward: bool) -> bool {
        self.nav.target(forward, &self.session).is_some() || self.trail_target(forward).is_some()
    }

    fn trail_target(&self, forward: bool) -> Option<usize> {
        self.doc_trail
            .target(forward, self.front_doc(), |d| self.tabs.has(d))
    }

    /// Back or Forward to the nearest earlier or later document.
    fn step_doc_trail(&mut self, forward: bool) {
        if let Some(i) = self.trail_target(forward) {
            let doc = self.doc_trail.entries[i];
            self.show_replayed(doc);
            self.doc_trail.cursor = i;
        }
    }

    /// Bring a document to the front without recording a visit.
    fn show_replayed(&mut self, doc: Doc) {
        self.doc_trail.replay = true;
        self.open_doc(doc);
        self.doc_trail.replay = false;
    }

    /// The documents visited and the cursor, for tests and `shell` readers.
    pub fn doc_trail(&self) -> (&[Doc], usize) {
        self.doc_trail.visited()
    }

    pub(super) fn record_placement_selection(&mut self, before: NavLocation) {
        let after = NavLocation::placements(&self.session);
        if before != after {
            self.tags.seal();
            self.nav.sync(self.session.world_rev(), before);
            self.nav.push(after);
        } else {
            // Re-selecting or clearing the current subject releases a loose object.
            self.nav.revision += 1;
        }
    }

    pub(super) fn navigate(&mut self, action: NavAction) -> Result<(), String> {
        if matches!(action, NavAction::Back | NavAction::Forward) {
            if let Some(next) = self.nav.target(action == NavAction::Forward, &self.session) {
                self.nav.cursor = next;
                self.nav.revision += 1;
                let destination = &self.nav.entries[next];
                if destination.placements != self.session.selection() {
                    self.session.select_many(destination.placements.clone());
                }
                self.tags.selected = destination.tag();
                self.tags.seal();
                if let Some(tag) = self.tags.selected {
                    self.show_replayed(Doc::Tag(tag));
                }
            } else {
                self.step_doc_trail(action == NavAction::Forward);
            }
            return Ok(());
        }

        self.sync_tags();
        let choosing_subject = matches!(action, NavAction::SelectTag(_) | NavAction::SelectPalette { .. });
        let before = self.nav_location();
        let mut after = before.clone();
        match action {
            NavAction::SelectTag(tag) => {
                after.placements.clear();
                after.palette_subject = false;
                after.trail = vec![tag];
                after.path = None;
                after.element = None;
            }
            NavAction::SelectPalette {
                tag,
                mut placements,
            } => {
                placements.retain(|&(kind, index)| {
                    self.session
                        .scenario()
                        .is_some_and(|s| index < s.block(kind).len())
                });
                after.placements = placements;
                after.palette_subject = true;
                after.trail = vec![tag];
                after.path = None;
                after.element = None;
            }
            NavAction::Go { tag, path, element } => {
                after.trail = self.trail_to(&before, tag);
                after.path = path;
                after.element = match element {
                    Some(e) if !e.is_empty() => Some(
                        tag_api::element_steps(&self.tags, tag, &e)?
                            .last()
                            .map_or(e, |s| s.path.clone()),
                    ),
                    _ => None,
                };
            }
            NavAction::Open(trail) => {
                after.palette_subject &= !trail.is_empty();
                after.trail = trail;
                after.path = None;
                after.element = None;
            }
            NavAction::Follow { from, path, tag } => {
                let source = NavLocation {
                    trail: from,
                    path: Some(path),
                    element: None,
                    ..before.clone()
                };
                self.check_location(&source)?;
                after.trail = source.trail.clone();
                if let Some(i) = after.trail.iter().position(|&t| t == tag) {
                    after.trail.truncate(i + 1);
                } else {
                    after.trail.push(tag);
                }
                after.path = None;
                after.element = None;
                self.check_location(&after)?;
                self.nav.sync(self.session.world_rev(), before);
                // The field clicked is the exact place Back returns to.
                self.nav.entries[self.nav.cursor] = source;
                self.tags.selected = after.tag();
                self.nav.push(after);
                self.tags.seal();
                return Ok(());
            }
            NavAction::Back | NavAction::Forward => unreachable!(),
        }
        self.check_location(&after)?;
        self.tags.selected = after.tag();
        if after != before {
            if before.placements != after.placements {
                self.session.select_many(after.placements.clone());
            }
            self.nav.sync(self.session.world_rev(), before);
            self.nav.push(after);
            self.tags.seal();
        } else if choosing_subject {
            // An explicit click on the inspected definition releases a loose live object.
            self.nav.revision += 1;
        }
        Ok(())
    }

    fn check_location(&self, location: &NavLocation) -> Result<(), String> {
        for tag in &location.trail {
            if self.tags.tag(*tag).is_none() {
                return Err(format!("no tag {:#010x} in the open map", tag.0));
            }
        }
        if let Some(path) = &location.path {
            let tag = location.tag().ok_or("a field needs a tag")?;
            tag_api::field_focus(&self.tags, tag, path)?;
        }
        if let Some(element) = &location.element {
            let tag = location.tag().ok_or("an element needs a tag")?;
            tag_api::element_steps(&self.tags, tag, element)?;
            if let Some(path) = &location.path {
                let named = tag_api::named_path(&self.tags, tag, path)?;
                if !named.starts_with(&format!("{element}.")) {
                    return Err(format!("{path} is not in {element}"));
                }
            }
        }
        Ok(())
    }

    fn trail_to(&self, from: &NavLocation, tag: TagId) -> Vec<TagId> {
        if let Some(i) = from.trail.iter().rposition(|&t| t == tag) {
            return from.trail[..=i].to_vec();
        }
        let placement_root = match from.selection() {
            Some(Selection::Placement(kind, index)) => self
                .session
                .scenario()
                .and_then(|s| tag_api::placement_root(s, kind, index))
                .map(|(id, _)| id),
            _ => None,
        };
        let from_tag = from.tag().or(placement_root);
        if let Some(root) = from_tag {
            let chain = tag_api::chain(&self.tags, Some((root, "")), tag_api::MAX_DEPTH);
            let mut trail = from.trail.clone();
            trail.pop();
            if chain.root.is_some_and(|node| node.path_to(tag, &mut trail)) {
                return trail;
            }
        }
        // Jumping to an arbitrary tag starts a new breadcrumb at that tag.
        vec![tag]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Action;
    use crate::play::testing::Temp;

    #[test]
    fn explicit_and_implicit_placement_selection_change_the_revision() {
        let t = Temp::new("placement-selection-revision");
        let mut app = App::new();
        app.set_data_root(t.0.join("data"));
        app.dispatch(Action::OpenSource(t.map("example")));
        let a = (BlockKind::Weapons, 0);
        let b = (BlockKind::Weapons, 1);
        for action in [
            Action::Select(None), Action::Select(None),
            Action::Select(Some(a)), Action::Select(Some(a)),
            Action::SelectMany(vec![a, b]), Action::SelectMany(vec![a]),
        ] {
            let before = app.selection_revision();
            app.dispatch(action);
            assert_ne!(app.selection_revision(), before);
        }

        let before = app.selection_revision();
        app.session.select(Some(b));
        assert_ne!(app.selection_revision(), before);
    }
}

#[cfg(test)]
mod trail_tests {
    use super::*;
    use map::TagId;

    fn tag(n: u32) -> Doc {
        Doc::Tag(TagId(n))
    }

    #[test]
    fn the_trail_keeps_documents_of_every_kind_in_order_and_cuts_ahead_on_a_new_visit() {
        let mut t = DocTrail::default();
        for d in [Doc::Variant, tag(1), Doc::Playlists] {
            t.visit(d);
        }
        t.visit(Doc::Playlists);
        assert_eq!(t.visited(), (&[Doc::Variant, tag(1), Doc::Playlists][..], 2));
        let open = |_: Doc| true;
        assert_eq!(t.target(false, Some(Doc::Playlists), open), Some(1));
        assert_eq!(t.target(true, Some(Doc::Playlists), open), None);
        // A document that is closed is skipped, as is the one in front.
        let no_tag = |d: Doc| d != tag(1);
        assert_eq!(t.target(false, Some(Doc::Playlists), no_tag), Some(0));
        t.cursor = 0;
        t.visit(Doc::Rules);
        assert_eq!(t.visited(), (&[Doc::Variant, Doc::Rules][..], 1));
    }

    #[test]
    fn a_replayed_visit_moves_the_cursor_and_never_the_entries() {
        let mut t = DocTrail::default();
        for d in [Doc::Variant, tag(1), Doc::Hud] {
            t.visit(d);
        }
        t.replay = true;
        t.visit(tag(1));
        assert_eq!(t.visited(), (&[Doc::Variant, tag(1), Doc::Hud][..], 1));
        t.visit(tag(9));
        assert_eq!(t.visited().0.len(), 3);
        assert_eq!(t.target(true, Some(tag(1)), |_| true), Some(2));
    }
}
