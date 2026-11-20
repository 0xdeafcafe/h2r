//! The Scenario inspector's history. Placement selection still belongs
//! to the scenario session, so a reference jump leaves the viewport alone.
use map::TagId;
use scenario_edit::{BlockKind, Session};

use super::{App, Selection, tag_api};

const HISTORY_LIMIT: usize = 128;

#[derive(Clone, Debug, PartialEq)]
pub enum NavAction {
    SelectTag(TagId),
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
    pub trail: Vec<TagId>,
    pub path: Option<String>,
}

impl NavLocation {
    fn placements(session: &Session) -> Self {
        Self {
            placements: session.selection(),
            trail: Vec::new(),
            path: None,
        }
    }

    pub fn selection(&self) -> Option<Selection> {
        if let Some(&(kind, index)) = self.placements.first() {
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
        }
    }

    pub(super) fn navigate(&mut self, action: NavAction) -> Result<(), String> {
        if matches!(action, NavAction::Back | NavAction::Forward) {
            if let Some(next) = self.nav.target(action == NavAction::Forward, &self.session) {
                self.nav.cursor = next;
                let placements = &self.nav.entries[next].placements;
                if *placements != self.session.selection() {
                    self.session.select_many(placements.clone());
                }
                self.tags.seal();
            }
            return Ok(());
        }

        self.sync_tags();
        let before = self.nav_location();
        let mut after = before.clone();
        match action {
            NavAction::SelectTag(tag) => {
                after.placements.clear();
                after.trail = vec![tag];
                after.path = None;
            }
            NavAction::Go { tag, path } => {
                after.trail = self.trail_to(&before, tag);
                after.path = path;
            }
            NavAction::Open(trail) => {
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
                self.nav.push(after);
                self.tags.seal();
                return Ok(());
            }
            NavAction::Back | NavAction::Forward => unreachable!(),
        }
        self.check_location(&after)?;
        if after != before {
            if before.placements != after.placements {
                self.session.select_many(after.placements.clone());
            }
            self.nav.sync(self.session.world_rev(), before);
            self.nav.push(after);
            self.tags.seal();
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
