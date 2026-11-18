//! The HUD editor tool's state, no UI toolkit: the map open in it (a map
//! of the source, or a converted Halo 1 package), its HUDs, the HUD shown
//! with the state it is drawn at, the selected widget, the picture and the
//! checks' findings. Edits, undo and the save live in
//! [`hud_edit::HudSession`]; docs/editor/hud-editor.md.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use editor_core::packages;
use hud_edit::checks::{self, Finding};
use hud_edit::render::Picture;
use hud_edit::{HudSession, HudState, Mode, Value, WidgetKey};
use map::TagId;
use scenario_edit::MapSource as EditSource;

use super::App;

/// The picture of the HUD shown and its findings.
pub struct Shot {
    pub picture: Picture,
    pub findings: Vec<Finding>,
    /// Bumped on every redraw, for the texture.
    pub rev: u64,
}

/// The tool's state.
pub struct HudTool {
    pub session: Option<HudSession>,
    /// What is open: a map of the source by name, or a package's label.
    pub opened: Option<String>,
    pub state: HudState,
    pub selected: Option<WidgetKey>,
    pub zoom: f32,
    pub show_rects: bool,
    pub show_frame: bool,
    shot: Option<Shot>,
    rev: u64,
}

impl Default for HudTool {
    fn default() -> Self {
        HudTool {
            session: None,
            opened: None,
            state: HudState::default(),
            selected: None,
            zoom: 1.0,
            show_rects: true,
            show_frame: true,
            shot: None,
            rev: 0,
        }
    }
}

impl HudTool {
    pub fn is_dirty(&self) -> bool {
        self.session.as_ref().is_some_and(HudSession::is_dirty)
    }
    pub fn redraw(&mut self) {
        self.shot = None;
    }
    /// The HUD shown's picture, drawn when anything changed.
    pub fn shot(&mut self) -> Option<&Shot> {
        if self.shot.is_none() {
            let picture = self.session.as_mut()?.draw(&self.state)?;
            self.rev += 1;
            let findings = checks::check(&picture);
            self.shot = Some(Shot {
                picture,
                findings,
                rev: self.rev,
            });
        }
        self.shot.as_ref()
    }
    /// The topmost widget of the edited HUD at HUD pixel `px` (hidden
    /// widgets included when their rectangle is known).
    pub fn widget_at(&mut self, px: [f32; 2]) -> Option<WidgetKey> {
        let shot = self.shot()?;
        shot.picture
            .placed
            .iter()
            .rev()
            .filter(|w| w.hud == 0 && w.shown)
            .find(|w| w.rect.is_some_and(|r| r.contains_point(px)))
            .map(|w| w.key)
    }
    /// The selected widget's offset in the mode shown.
    pub fn offset(&self, key: WidgetKey) -> Option<(i16, i16)> {
        let s = self.session.as_ref()?;
        Some(s.hud()?.widget(key)?.offset(self.state.mode))
    }
    fn open(&mut self, source: EditSource, name: &str) -> Result<usize> {
        let s = HudSession::open(source, name)?;
        let n = s.huds.len();
        let first = s.huds.first().map(|e| e.id);
        self.session = Some(s);
        self.opened = Some(name.to_owned());
        self.selected = None;
        self.redraw();
        if let Some(id) = first {
            self.pick(id)?;
        }
        Ok(n)
    }
    pub fn pick(&mut self, id: TagId) -> Result<()> {
        let s = self.session.as_mut().context("open a map first")?;
        s.select(id)?;
        self.selected = None;
        self.redraw();
        Ok(())
    }
}

impl App {
    /// Opens a map of the source in the HUD tool.
    pub(super) fn hud_open_map(&mut self, name: &str) {
        let Some(source) = &self.source else {
            self.error("open a disc, map or folder first");
            return;
        };
        let r = source
            .location(name)
            .map_err(|e| anyhow::anyhow!("{e}"))
            .map(|(path, region)| EditSource::region(path, region))
            .and_then(|s| self.hud.open(s, name));
        match r {
            Ok(n) => self.info(format!("{name}: {n} HUDs")),
            Err(e) => self.error(format!("{name}: {e:#}")),
        }
    }

    /// Opens a package from the Maps list (a converted Halo 1 map) in the
    /// HUD tool: its bytes, with the shared caches from the open disc.
    pub(super) fn hud_open_package(&mut self, key: &str) {
        let Some(entry) = self.packages.iter().find(|e| e.key() == key).cloned() else {
            self.error(format!("no package {key}"));
            return;
        };
        let maps = self.session.maps_dir();
        let r = packages::read_map(&maps, &entry, None).and_then(|bytes| {
            let anchor = self
                .source
                .as_ref()
                .and_then(|s| s.location("shared").ok())
                .map(|(path, region)| EditSource::region(path, region))
                .unwrap_or_else(|| EditSource::file(maps.join(&entry.dir).join("package.map")));
            let source = EditSource {
                bytes: Some(Arc::from(bytes)),
                ..anchor
            };
            self.hud.open(source, &entry.label())
        });
        match r {
            Ok(n) => self.info(format!("{}: {n} HUDs", entry.label())),
            Err(e) => self.error(format!("{}: {e:#}", entry.label())),
        }
    }

    pub(super) fn hud_result(&mut self, r: Result<()>) {
        match r {
            Ok(()) => self.hud.redraw(),
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    pub(super) fn hud_set(&mut self, key: WidgetKey, field: &str, v: Value, merge: bool) {
        let r = match self.hud.session.as_mut() {
            Some(s) => s.set(key, field, v, merge),
            None => Err(anyhow::anyhow!("no HUD open")),
        };
        self.hud_result(r);
    }

    pub(super) fn hud_move(&mut self, key: WidgetKey, mode: Mode, to: (i16, i16), merge: bool) {
        let r = match self.hud.session.as_mut() {
            Some(s) => s.move_to(key, mode, to, merge),
            None => Err(anyhow::anyhow!("no HUD open")),
        };
        self.hud_result(r);
    }

    pub(super) fn hud_save_as(&mut self, path: &Path) {
        let Some(s) = self.hud.session.as_mut() else {
            self.error("no HUD open");
            return;
        };
        match s.save_as(path) {
            Ok(n) => self.info(format!("saved {} ({n} bytes)", path.display())),
            Err(e) => self.error(format!("{}: {e:#}", path.display())),
        }
    }

    pub(super) fn hud_undo_redo(&mut self, undo: bool) -> bool {
        let Some(s) = self.hud.session.as_mut() else {
            return false;
        };
        let done = if undo { s.undo() } else { s.redo() };
        match done {
            Some(label) => {
                self.hud.redraw();
                self.info(format!("{} {label}", if undo { "undid" } else { "redid" }));
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
#[path = "hud_tests.rs"]
mod tests;
