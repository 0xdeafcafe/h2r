//! The Scenario inspector's history. Placement selection still belongs
//! to the scenario session, so a reference jump leaves the viewport alone.
use map::TagId;
use scenario_edit::{BlockKind, Session};

use super::{App, Selection, SelectionRevision, tag_api};

const HISTORY_LIMIT: usize = 128;

#[derive(Clone, Debug, PartialEq)]
pub enum NavAction {
    SelectTag(TagId),
    SelectPalette {
        tag: TagId,
        placements: Vec<(BlockKind, usize)>,
    },
    Go {
        tag: TagId,
        path: Option<String>,
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
}

impl NavLocation {
    fn placements(session: &Session) -> Self {
        Self {
            placements: session.selection(),
            palette_subject: false,
            trail: Vec::new(),
            path: None,
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
        self.nav.target(forward, &self.session).is_some()
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
            }
            NavAction::Go { tag, path } => {
                after.trail = self.trail_to(&before, tag);
                after.path = path;
            }
            NavAction::Open(trail) => {
                after.palette_subject &= !trail.is_empty();
                after.trail = trail;
                after.path = None;
            }
            NavAction::Follow { from, path, tag } => {
                let source = NavLocation {
                    trail: from,
                    path: Some(path),
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
