//! The UI Screens tool's edits (docs/game/ui-builder.md "Editing in the
//! editor"): the selected widget, and edits of the screen shown as a
//! rect or a text by widget id, sent to the screen's sink.
//!
//! - [`ours`]: our screens, whose presentation is `ScreenDoc`s
//!   (`assets/ui/screens/<name>.yaml`, `boot::ui_kit::data`), saved into
//!   the checkout the editor was started from.
//! - [`retail`]: the disc's screens, `wgit` tags of `mainmenu.map`, edited
//!   through the map's tag document and saved with the map. [`retail_tag`]
//!   has its edits past a rect: a block hidden, a block copied, a string
//!   ID set.
//!
//! Every edit goes through [`EditAction`] ([`crate::app::Action::UiEdit`]),
//! so the window and the agent API make the same edits and the same undo
//! steps. The view the checks run on is the edited one.

pub mod ours;
pub mod retail;
pub mod retail_tag;

use std::collections::BTreeMap;

use anyhow::{Context as _, Result};
use boot::ui_kit::animation::{AnimatedView, Phase, Transition};
use boot::ui_kit::preview::{self, Canvas, RetailBlocks, RetailEdits, RetailPlaces};
use boot::ui_kit::view::{View, Widget};

use super::{Pick, UiScreens};
use crate::App;

/// What an edit sets on a widget, in UI units (640 by 480, origin at the
/// centre, Y up).
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// `top, left, bottom, right`.
    Rect([i16; 4]),
    Text(String),
}

/// An edit of the screen shown ([`crate::app::Action::UiEdit`]).
#[derive(Clone, Debug, PartialEq)]
pub enum EditAction {
    /// Select a widget by id, or nothing.
    Select(Option<String>),
    /// Move a widget by `dx`, `dy` units (Y up). `merge` joins the last
    /// step of the same widget (a drag, a run of nudges).
    Move {
        id: String,
        dx: i16,
        dy: i16,
        merge: bool,
    },
    /// Give a widget a rect.
    SetRect {
        id: String,
        rect: [i16; 4],
        merge: bool,
    },
    /// Give a text widget a text.
    SetText { id: String, text: String },
    /// Park a retail text or bitmap off screen, as the disc's spare
    /// blocks are ([`retail_tag`]).
    Hide { id: String },
    /// Append a copy of a retail text, button or bitmap block to its
    /// block; the copy is selected ([`retail_tag`]).
    Duplicate { id: String },
    /// Set the string ID of a retail title, text or button to the string
    /// named `string` (added to the map when it has none). `shows` is the
    /// stand-in text the picture draws for that string
    /// ([`retail_tag`]).
    SetString {
        id: String,
        string: String,
        shows: Option<String>,
    },
    /// Put a widget (or, with none, the whole screen) back as last loaded
    /// or saved, as one undo step.
    Revert(Option<String>),
    /// Write the edits: ours into the checkout, retail with the map.
    Save,
    /// End the current merge run (a drag let go).
    Seal,
    /// Read our screens' files again from the checkout.
    Reload,
}

/// Which sink a pick's edits go to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sink {
    Ours,
    Retail,
}

impl Sink {
    pub fn of(pick: &Pick) -> Sink {
        match pick {
            Pick::Native { .. } => Sink::Ours,
            Pick::Retail { .. } => Sink::Retail,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Sink::Ours => "ours",
            Sink::Retail => "retail",
        }
    }
}

/// The tool's edit state.
#[derive(Default)]
pub struct UiEdit {
    /// The selected widget of the screen shown.
    pub selected: Option<String>,
    /// Our screens' documents, edits and history.
    pub ours: ours::Ours,
    /// Each retail screen's widgets as the disc has them, by tag: what
    /// the map's moved fields are read against.
    pub(super) retail_base: BTreeMap<String, View>,
    /// Retail widgets moved in the map's tag document, by tag.
    pub(super) places: BTreeMap<String, RetailPlaces>,
    /// Each edited retail screen's pane blocks as the map's tag document
    /// has them, by tag ([`retail_tag`]); none while they are the disc's.
    pub(super) blocks: BTreeMap<String, RetailBlocks>,
    /// What differs from the loaded map in each retail screen, by tag.
    pub(super) changes: BTreeMap<String, retail_tag::Changes>,
    /// Stand-in texts by string ID name: what the picture draws for a
    /// string the game's code answers at run time. Not saved.
    pub stand_ins: BTreeMap<String, String>,
    /// [`UiEdit::stand_ins`] by the string IDs the open map gives them.
    pub(super) texts: BTreeMap<u32, String>,
    /// Why the last edit did nothing, if it didn't.
    pub last_error: Option<String>,
}

impl UiEdit {
    /// Retail screen `tag`'s edits as the picture takes them: the moved
    /// widgets, the edited pane's blocks and the stand-in texts.
    pub(super) fn retail_edits(&self, tag: &str) -> RetailEdits {
        RetailEdits {
            places: self.places.get(tag).cloned().unwrap_or_default(),
            blocks: self.blocks.get(tag).cloned(),
            texts: self.texts.clone(),
        }
    }
}

/// A retail screen as the tool draws it: pane [`retail::PANE`], the tag's
/// own texts, no cells, no footer, in the sample variant `sample` names
/// (none: the plain sample).
pub(super) fn retail_pick<'a>(tag: &'a str, sample: Option<&'a str>) -> preview::Retail<'a> {
    preview::Retail {
        tag,
        pane: retail::PANE,
        text: &no_text,
        cells: &[],
        footer: None,
        rows: None,
        sample,
    }
}

/// No string id answered by the tool: every text is the tag's own.
fn no_text(_: u32) -> Option<String> {
    None
}

/// One frame of a transition filmstrip: its tick, its time and where it
/// sits in the strip (left edge, width in pixels).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FilmFrame {
    pub tick: u32,
    pub seconds: f32,
    pub x: u32,
    pub w: u32,
}

impl UiScreens {
    /// The resting view of the screen shown: ours, or a retail screen's
    /// widgets (the disc's, with the map's moves).
    pub fn edit_view(&mut self) -> Option<&View> {
        let shot = self.shot()?;
        shot.view.as_ref().or(shot.retail_view.as_ref())
    }

    /// Widget `id` of [`UiScreens::edit_view`].
    pub fn edit_widget(&mut self, id: &str) -> Result<Widget, String> {
        let view = self
            .edit_view()
            .ok_or("nothing is shown with widgets: pick a screen (a retail one needs the disc)")?;
        view.widget(id).cloned().ok_or_else(|| {
            format!(
                "{} has no widget {id:?} (ui.widgets lists them)",
                view.screen
            )
        })
    }

    /// Widget `id`'s JSON, as `ui.widgets` lists it (rect in UI units and
    /// pixels), from the resting edited view.
    pub fn edit_widget_json(&mut self, id: &str) -> Option<serde_json::Value> {
        let json = self.edit_view()?.json();
        json["widgets"]
            .as_array()?
            .iter()
            .find(|w| w["id"] == id)
            .cloned()
    }

    /// Draws again after the edits changed outside an edit (discarded).
    pub fn redraw_edits(&mut self) {
        self.redraw();
    }

    /// Goes back to rest, for an edit: edits are made on the resting pose.
    pub(super) fn rest_for_edit(&mut self) {
        if self.animation != Phase::Rest {
            self.animation = Phase::Rest;
        }
        self.redraw();
    }

    /// Retail screen `tag`'s widgets as the disc has them (pane
    /// [`retail::PANE`]).
    pub(super) fn retail_base_view(&mut self, tag: &str) -> Result<View, String> {
        let d = self.disc().ok_or("retail screens need the disc")?;
        d.retail_view(&retail_pick(tag, None), &RetailPlaces::new())
            .map_err(|e| format!("{tag}: {e:#}"))
    }

    /// The edited docs our screens are drawn with.
    pub(super) fn shown_docs(&self) -> ours::Docs {
        self.edit.ours.shown()
    }

    /// The screen shown at each of `phases` (ours only), for the onion
    /// skin: the edited view posed, not drawn.
    pub fn poses(&mut self, phases: &[Phase]) -> Vec<View> {
        let Some(Pick::Native { state, .. }) = self.pick.clone() else {
            return Vec::new();
        };
        let docs = self.shown_docs();
        let Some(spec) = ours::with_docs(&docs, || self.spec()) else {
            return Vec::new();
        };
        let disc = self.disc();
        let from = match disc {
            Some(d) => preview::From::Disc(d),
            None => preview::From::Synthetic,
        };
        let Ok(view) = ours::with_docs(&docs, || from.view(&spec, &state)) else {
            return Vec::new();
        };
        let mut animated = AnimatedView::new(view, spec.animation, spec.dialog.is_some());
        phases
            .iter()
            .map(|p| animated.seek(*p).clone())
            .collect()
    }

    /// Transition `t` of the screen shown (ours only) as a filmstrip of
    /// `n` frames from its start to its end, each labelled with its time.
    pub fn filmstrip(&mut self, t: Transition, n: usize) -> Result<(Canvas, Vec<FilmFrame>)> {
        let Some(Pick::Native { state, .. }) = self.pick.clone() else {
            anyhow::bail!("a filmstrip is of our screens: a retail screen has no transition here");
        };
        anyhow::ensure!((1..=32).contains(&n), "frames: 1 to 32, not {n}");
        let docs = self.shown_docs();
        let spec = ours::with_docs(&docs, || self.spec()).context("no page")?;
        let disc = self.disc();
        let from = match disc {
            Some(d) => preview::From::Disc(d),
            None => preview::From::Synthetic,
        };
        let src = from.source();
        let view = ours::with_docs(&docs, || from.view(&spec, &state))?;
        let mut animated = AnimatedView::new(view, spec.animation, spec.dialog.is_some());
        let ticks = t.frames(n);
        let frames: Vec<(String, Canvas)> = ticks
            .iter()
            .map(|&tick| {
                let v = animated.seek(t.phase(tick));
                (
                    format!("{} t{:.2}s", t.name(), Transition::seconds(tick)),
                    preview::render_view(v, src),
                )
            })
            .collect();
        let (strip, at) = preview::filmstrip(&frames);
        let info = ticks
            .iter()
            .zip(at)
            .map(|(&tick, [x, w])| FilmFrame {
                tick,
                seconds: Transition::seconds(tick),
                x,
                w,
            })
            .collect();
        Ok((strip, info))
    }
}

impl App {
    /// [`crate::app::Action::UiEdit`]: runs the edit; a refusal or failure
    /// goes to the log and to `ui_screens.edit.last_error`.
    pub(crate) fn run_ui_edit(&mut self, a: EditAction) {
        match self.ui_edit(a) {
            Ok(Some(line)) => {
                self.ui_screens.edit.last_error = None;
                self.info(line);
            }
            Ok(None) => self.ui_screens.edit.last_error = None,
            Err(e) => {
                self.ui_screens.edit.last_error = Some(e.clone());
                self.error(e);
            }
        }
    }

    fn ui_edit(&mut self, a: EditAction) -> Result<Option<String>, String> {
        let pick = self
            .ui_screens
            .pick
            .clone()
            .ok_or("no screen is shown: pick one first")?;
        let sink = Sink::of(&pick);
        match a {
            EditAction::Select(None) => {
                self.ui_screens.edit.selected = None;
                Ok(None)
            }
            EditAction::Select(Some(id)) => {
                self.ui_screens.edit_widget(&id)?;
                self.ui_screens.edit.selected = Some(id);
                Ok(None)
            }
            EditAction::Move { id, dx, dy, merge } => {
                self.ui_screens.rest_for_edit();
                let [t, l, b, r] = self.ui_screens.edit_widget(&id)?.rect;
                let rect = [
                    t.saturating_add(dy),
                    l.saturating_add(dx),
                    b.saturating_add(dy),
                    r.saturating_add(dx),
                ];
                self.ui_change(&pick, &id, Change::Rect(rect), merge)
            }
            EditAction::SetRect { id, rect, merge } => {
                let [t, l, b, r] = rect;
                if t < b || r < l {
                    return Err(format!(
                        "{id}: a rect is top, left, bottom, right with top >= bottom and right >= left, not {rect:?}"
                    ));
                }
                self.ui_screens.rest_for_edit();
                self.ui_change(&pick, &id, Change::Rect(rect), merge)
            }
            EditAction::SetText { id, text } => {
                self.ui_screens.rest_for_edit();
                self.ui_change(&pick, &id, Change::Text(text), false)
            }
            EditAction::Hide { id } => {
                let tag = retail_tag::retail_only(&pick, "hiding a widget")?;
                self.ui_screens.rest_for_edit();
                let widget = self.ui_screens.edit_widget(&id)?;
                let line = self.ui_retail_hide(tag, &widget)?;
                if self.ui_screens.edit.selected.as_deref() == Some(id.as_str()) {
                    self.ui_screens.edit.selected = None;
                }
                self.ui_screens.redraw();
                Ok(Some(line))
            }
            EditAction::Duplicate { id } => {
                let tag = retail_tag::retail_only(&pick, "copying a widget")?;
                self.ui_screens.rest_for_edit();
                let widget = self.ui_screens.edit_widget(&id)?;
                let (copy, line) = self.ui_retail_copy(tag, &widget)?;
                self.ui_screens.edit.selected = Some(copy);
                self.ui_screens.redraw();
                Ok(Some(line))
            }
            EditAction::SetString { id, string, shows } => {
                let tag = retail_tag::retail_only(&pick, "setting a string ID")?;
                self.ui_screens.rest_for_edit();
                let line = self.ui_retail_set_string(tag, &id, &string, shows)?;
                self.ui_screens.redraw();
                Ok(Some(line))
            }
            EditAction::Revert(id) => {
                let line = match (sink, &pick) {
                    (Sink::Ours, Pick::Native { screen, .. }) => {
                        self.ui_screens.edit.ours.revert(screen, id.as_deref())?
                    }
                    (_, Pick::Retail { tag, .. }) => self.ui_retail_revert(tag, id.as_deref())?,
                    _ => unreachable!("the sink follows the pick"),
                };
                self.ui_screens.redraw();
                Ok(Some(line))
            }
            EditAction::Save => match sink {
                Sink::Ours => {
                    let written = self.ui_screens.edit.ours.save()?;
                    self.ui_screens.redraw();
                    Ok(Some(match written.len() {
                        0 => "no edits of our screens to save".to_owned(),
                        n => format!(
                            "saved {n} screen file{}: {}",
                            if n == 1 { "" } else { "s" },
                            written
                                .iter()
                                .map(|p| p.display().to_string())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    }))
                }
                Sink::Retail => {
                    self.run(crate::app::Action::Save);
                    Ok(None)
                }
            },
            EditAction::Seal => {
                self.ui_screens.edit.ours.seal();
                if sink == Sink::Retail && self.tags.is_open() {
                    self.run_map_tag(crate::app::TagAction::Seal);
                }
                Ok(None)
            }
            EditAction::Reload => {
                let n = self.ui_screens.edit.ours.reload()?;
                self.ui_screens.redraw();
                Ok(Some(format!("read {n} screen files again")))
            }
        }
    }

    /// Sends `change` of widget `id` to the pick's sink, then redraws.
    fn ui_change(
        &mut self,
        pick: &Pick,
        id: &str,
        change: Change,
        merge: bool,
    ) -> Result<Option<String>, String> {
        let widget = self.ui_screens.edit_widget(id)?;
        let line = match pick {
            Pick::Native { screen, .. } => {
                self.ui_screens
                    .edit
                    .ours
                    .change(screen, &widget, change, merge)?
            }
            Pick::Retail { tag, .. } => self.ui_retail_change(tag, &widget, change, merge)?,
        };
        self.ui_screens.edit.selected = Some(id.to_owned());
        self.ui_screens.redraw();
        // A drag sends a step every frame: only its first is logged.
        Ok((!merge).then_some(line))
    }

    /// Undo or redo in the UI Screens tool: our screens' history, or the
    /// map's tag document for a retail screen. Whether a step was there.
    pub(crate) fn ui_undo_redo(&mut self, undo: bool) -> bool {
        let label = match self.ui_screens.pick.clone() {
            Some(Pick::Native { .. }) => {
                let s = &mut self.ui_screens.edit.ours;
                if undo { s.undo() } else { s.redo() }
            }
            Some(Pick::Retail { tag, .. }) => {
                self.sync_tags();
                let r = if undo {
                    self.tags.undo()
                } else {
                    self.tags.redo()
                };
                self.push_tag_patches();
                self.ui_retail_refresh(&tag);
                r
            }
            None => None,
        };
        match label {
            Some(l) => {
                self.ui_screens.redraw();
                self.info(format!("{} {l}", if undo { "undid" } else { "redid" }));
                true
            }
            None => false,
        }
    }

    /// The UI Screens tool can undo (`redo` false) or redo.
    pub(crate) fn ui_can_undo_redo(&self, redo: bool) -> bool {
        match &self.ui_screens.pick {
            Some(Pick::Native { .. }) => {
                let s = &self.ui_screens.edit.ours;
                if redo { s.can_redo() } else { s.can_undo() }
            }
            Some(Pick::Retail { .. }) => {
                if redo {
                    self.tags.can_redo()
                } else {
                    self.tags.can_undo()
                }
            }
            None => false,
        }
    }

    /// The edits as `ui.edits` reports them: the sink of the screen shown,
    /// its edits, and whether anything is unsaved.
    pub fn ui_edits_json(&self) -> serde_json::Value {
        let sink = self.ui_screens.pick.as_ref().map(|p| Sink::of(p).name());
        let retail: serde_json::Map<String, serde_json::Value> = self
            .ui_screens
            .edit
            .places
            .iter()
            .filter(|(_, p)| !p.is_empty())
            .map(|(tag, p)| (tag.clone(), serde_json::json!(p)))
            .collect();
        let blocks: serde_json::Map<String, serde_json::Value> = self
            .ui_screens
            .edit
            .changes
            .iter()
            .filter(|(_, c)| !c.is_empty())
            .map(|(tag, c)| (tag.clone(), c.json()))
            .collect();
        let edited = !retail.is_empty() || !blocks.is_empty();
        serde_json::json!({
            "sink": sink,
            "selected": self.ui_screens.edit.selected,
            "ours": self.ui_screens.edit.ours.edits_json(),
            "ours_dirty": self.ui_screens.edit.ours.dirty(),
            "retail": retail,
            "retail_blocks": blocks,
            "stand_ins": self.ui_screens.edit.stand_ins,
            "retail_dirty": edited && self.session.is_dirty(),
            "checkout": self.ui_screens.edit.ours.checkout_path().map(|p| p.display().to_string()),
            "changed_on_disk": self.ui_screens.edit.ours.changed_on_disk(),
        })
    }
}

#[cfg(test)]
#[path = "edit_tests.rs"]
mod tests;
