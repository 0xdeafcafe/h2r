//! The document tabs (docs/editor/studio-redesign.md, "The shell"): every
//! open document as a tab, the one in front, and each document's own undo
//! stack. A tab's editor is picked from what it holds: a tag's from its
//! group ([`kinds::kind`]), a map, a variant, a playlist file, a screen or
//! a HUD from the tool that holds it.
//!
//! The tools' documents (the map, the variant, the playlists, the screen,
//! the HUD) are opened by their own commands; [`App::sync_tabs`] gives each
//! open one a tab and drops the tab of a closed one. A tag tab is opened by
//! [`Action::OpenDoc`]; it belongs to the map it was opened on and closes
//! with it. Its undo stack is its tag's steps of the map's tag document
//! ([`super::TagDoc::undo_tag`]). Closing a tag tab keeps its edits: they
//! are the map's, saved or dropped with it.
//!
//! The window's tab strip and `shell.*` both run [`Action::OpenDoc`],
//! [`Action::FocusDoc`], [`Action::CloseDoc`], [`Action::UndoDoc`] and
//! [`Action::RedoDoc`], so they leave the same state.
//!
//! [`Action::OpenDoc`]: super::Action::OpenDoc
//! [`Action::FocusDoc`]: super::Action::FocusDoc
//! [`Action::CloseDoc`]: super::Action::CloseDoc
//! [`Action::UndoDoc`]: super::Action::UndoDoc
//! [`Action::RedoDoc`]: super::Action::RedoDoc

use map::TagId;
use serde_json::{Value, json};

use super::kinds::{self, Editor};
use super::textures::BitmapAction;
use super::{Action, App, Pick, TagAction, ToolKind};

/// What a tab holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Doc {
    /// The open map: its placements, in the Scenario tool.
    Map,
    /// The game variant open in the Variants tool.
    Variant,
    /// The playlist file open in the Playlists tool.
    Playlists,
    /// The screen shown in the UI Screens tool.
    Ui,
    /// The map open in the HUD tool.
    Hud,
    /// A tag of the open map, in the editor its group gets.
    Tag(TagId),
}

/// The tools' documents, in the order their tabs are first given.
pub const TOOL_DOCS: [Doc; 5] = [Doc::Map, Doc::Variant, Doc::Playlists, Doc::Ui, Doc::Hud];

impl Doc {
    /// The tool that holds a tool's document (`None` for a tag).
    pub fn holder(self) -> Option<ToolKind> {
        Some(match self {
            Doc::Map => ToolKind::Scenario,
            Doc::Variant => ToolKind::Variants,
            Doc::Playlists => ToolKind::Playlists,
            Doc::Ui => ToolKind::UiScreens,
            Doc::Hud => ToolKind::Hud,
            Doc::Tag(_) => return None,
        })
    }

    /// A tool document's id, the name `doc.*` takes for it.
    pub fn tool_id(self) -> Option<&'static str> {
        Some(match self {
            Doc::Map => "map",
            Doc::Variant => "variant",
            Doc::Playlists => "playlists",
            Doc::Ui => "ui",
            Doc::Hud => "hud",
            Doc::Tag(_) => return None,
        })
    }
}

/// The tool an editor of the kind registry draws in. The field panel, the
/// Modeling section, audition, the animation window, the HUD and screen
/// editors and the scripts view of a tag all show in the Tags tool until
/// their own documents dock (the slices of docs/editor/studio-redesign.md).
pub fn tool_for(editor: Editor) -> ToolKind {
    match editor {
        Editor::Scenario => ToolKind::Scenario,
        Editor::Textures => ToolKind::Textures,
        Editor::FieldPanel
        | Editor::Modeling
        | Editor::Audition
        | Editor::Hud
        | Editor::UiScreens
        | Editor::Scripts
        | Editor::Animation => ToolKind::Tags,
    }
}

/// One tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tab {
    pub doc: Doc,
    /// For a tag: the map it was opened on.
    pub map: Option<String>,
}

/// The open tabs, in order, and the one in front.
#[derive(Clone, Debug, Default)]
pub struct Tabs {
    open: Vec<Tab>,
    front: Option<Doc>,
}

impl Tabs {
    pub fn list(&self) -> &[Tab] {
        &self.open
    }
    pub fn front(&self) -> Option<Doc> {
        self.front
    }
    pub fn has(&self, doc: Doc) -> bool {
        self.open.iter().any(|t| t.doc == doc)
    }
    fn add(&mut self, tab: Tab) {
        if !self.has(tab.doc) {
            self.open.push(tab);
        }
    }
    /// Drop `doc`'s tab. The front moves to its left neighbour, else its
    /// right one; returns the new front when it moved.
    fn remove(&mut self, doc: Doc) -> Option<Doc> {
        let i = self.open.iter().position(|t| t.doc == doc)?;
        self.open.remove(i);
        if self.front != Some(doc) {
            return None;
        }
        self.front = self
            .open
            .get(i.saturating_sub(1))
            .or_else(|| self.open.get(i))
            .map(|t| t.doc);
        self.front
    }
}

impl App {
    /// The open tabs, in order.
    pub fn tabs(&self) -> &[Tab] {
        self.tabs.list()
    }
    /// The tab in front.
    pub fn front_doc(&self) -> Option<Doc> {
        self.tabs.front()
    }

    /// The editor a tab's document opens in: a tool's for a tool document,
    /// the kind registry's for a tag.
    pub fn doc_tool(&self, doc: Doc) -> ToolKind {
        match doc {
            Doc::Tag(id) => tool_for(self.tag_editor(id)),
            other => other.holder().expect("a tool document"),
        }
    }

    fn tag_editor(&self, id: TagId) -> Editor {
        self.tags
            .tag(id)
            .map_or(Editor::FieldPanel, |t| kinds::kind(t.group).editor)
    }

    /// A tab's id: a tool document's name (`map`, `variant`, `playlists`,
    /// `ui`, `hud`), or a tag's `group:path` as `tag.*` takes it.
    pub fn doc_id(&self, doc: Doc) -> String {
        match doc {
            Doc::Tag(id) => match self.tags.tag(id) {
                Some(t) => format!("{}:{}", tags::defs::fourcc_text(t.group), t.name),
                None => format!("{:#010x}", id.0),
            },
            other => other.tool_id().expect("a tool document").to_owned(),
        }
    }

    /// The open tab `id` names: [`Self::doc_id`], or a tag's `0x...` id.
    pub fn find_doc(&self, id: &str) -> Option<Doc> {
        self.tabs.list().iter().map(|t| t.doc).find(|&d| {
            self.doc_id(d) == id
                || matches!(d, Doc::Tag(t) if format!("{:#010x}", t.0).eq_ignore_ascii_case(id))
        })
    }

    /// What a tab shows as its name.
    pub fn doc_title(&self, doc: Doc) -> String {
        match doc {
            Doc::Map => self.loaded_map().unwrap_or("map").to_owned(),
            Doc::Variant => self
                .variants
                .as_ref()
                .and_then(|e| e.current())
                .map_or_else(
                    || "variant".to_owned(),
                    |v| variants::panel::strip_icon(&v.name()),
                ),
            Doc::Playlists => self.hoppers.path().and_then(|p| p.file_name()).map_or_else(
                || "untitled playlists".to_owned(),
                |n| n.to_string_lossy().into_owned(),
            ),
            Doc::Ui => match &self.ui_screens.pick {
                Some(Pick::Native { screen, state }) => format!("{screen} / {state}"),
                Some(Pick::Retail { tag, .. }) => tag.clone(),
                None => "screen".to_owned(),
            },
            Doc::Hud => self
                .hud
                .session
                .as_ref()
                .map_or_else(|| "HUD".to_owned(), |s| s.name.clone()),
            Doc::Tag(id) => match self.tags.tag(id) {
                Some(t) => format!(
                    "{}.{}",
                    super::tag_tree::leaf(&t.name),
                    tags::defs::fourcc_text(t.group)
                ),
                None => format!("{:#010x}", id.0),
            },
        }
    }

    /// The tab's document has unsaved edits. A tag's are the map's: a tag
    /// is dirty while it has changed bytes in an unsaved map.
    pub fn doc_dirty(&self, doc: Doc) -> bool {
        match doc {
            Doc::Tag(id) => self.tool_dirty(ToolKind::Scenario) && self.tags.is_changed(id),
            Doc::Map => self.tool_dirty(ToolKind::Scenario) || self.tool_dirty(ToolKind::Textures),
            other => self.tool_dirty(other.holder().expect("a tool document")),
        }
    }

    /// The tab's own undo stack can undo, and redo.
    pub fn doc_can(&self, doc: Doc) -> (bool, bool) {
        match doc {
            Doc::Tag(id) => (self.tags.can_undo_tag(id), self.tags.can_redo_tag(id)),
            Doc::Map if self.modeling.session.active.is_some() => {
                let g = &self.modeling.session;
                (g.can_undo(), g.can_redo())
            }
            Doc::Map => (self.session.can_undo(), self.session.can_redo()),
            Doc::Variant => self
                .variants
                .as_ref()
                .map_or((false, false), |e| (e.can_undo(), e.can_redo())),
            Doc::Playlists => (self.hoppers.can_undo(), self.hoppers.can_redo()),
            Doc::Ui => (self.ui_can_undo_redo(false), self.ui_can_undo_redo(true)),
            Doc::Hud => self
                .hud
                .session
                .as_ref()
                .map_or((false, false), |s| (s.can_undo(), s.can_redo())),
        }
    }

    /// A tool's document is open.
    fn tool_doc_open(&self, doc: Doc) -> bool {
        match doc {
            Doc::Map => self.session.is_open(),
            Doc::Variant => self
                .variants
                .as_ref()
                .is_some_and(|e| e.current().is_some()),
            Doc::Playlists => self.hoppers.is_open(),
            Doc::Ui => self.ui_screens.pick.is_some(),
            Doc::Hud => self.hud.session.is_some(),
            Doc::Tag(_) => false,
        }
    }

    /// Give every open tool document a tab (a new one comes to the front)
    /// and drop the tabs of closed ones, and of tags whose map is no
    /// longer the one open.
    pub fn sync_tabs(&mut self) {
        for doc in TOOL_DOCS {
            let open = self.tool_doc_open(doc);
            if open && !self.tabs.has(doc) {
                self.tabs.add(Tab { doc, map: None });
                self.tabs.front = Some(doc);
            } else if !open && self.tabs.has(doc) {
                self.tabs.remove(doc);
            }
        }
        let loaded = self
            .session
            .is_open()
            .then(|| self.loaded.clone())
            .flatten();
        let stale: Vec<Doc> = self
            .tabs
            .list()
            .iter()
            .filter(|t| matches!(t.doc, Doc::Tag(_)) && t.map != loaded)
            .map(|t| t.doc)
            .collect();
        for doc in stale {
            self.tabs.remove(doc);
        }
        if self.tabs.front.is_some_and(|f| !self.tabs.has(f)) {
            self.tabs.front = self.tabs.list().last().map(|t| t.doc);
        }
    }

    /// The tool shown changed through its old tab: bring forward the tab
    /// it shows, if any (the tab of the tag the Tags tool shows, the tool's
    /// own document, else the map's for a view of the map).
    pub(super) fn follow_tool(&mut self, tool: ToolKind) {
        if self.tabs.front.is_some_and(|f| self.shows(f, tool)) {
            return;
        }
        let own = match tool {
            ToolKind::Tags => self.tags.selected.map(Doc::Tag),
            _ => TOOL_DOCS.into_iter().find(|d| d.holder() == Some(tool)),
        };
        let own = own.filter(|&d| self.tabs.has(d) && self.shows(d, tool));
        let map_view = matches!(
            tool,
            ToolKind::Scenario | ToolKind::Tags | ToolKind::Scripts | ToolKind::Textures
        );
        let map = (map_view && self.tabs.has(Doc::Map)).then_some(Doc::Map);
        self.tabs.front = own.or(map);
    }

    /// `tool` shows `doc`: its editor's tool, and for a tag in the Tags
    /// tool, the tag selected there.
    fn shows(&self, doc: Doc, tool: ToolKind) -> bool {
        self.doc_tool(doc) == tool
            && match doc {
                Doc::Tag(id) => tool != ToolKind::Tags || self.tags.selected == Some(id),
                _ => true,
            }
    }

    /// [`super::Action::OpenDoc`]: a tool document that is open comes to
    /// the front; a tag of the open map gets a tab (the scenario tag is the
    /// map's own).
    pub(super) fn open_doc(&mut self, doc: Doc) {
        self.sync_tabs();
        let doc = match doc {
            Doc::Tag(id) => {
                if self.browsing().is_some() {
                    return self.error(
                        "a browsed map's tags are read-only: open the map to open its tags",
                    );
                }
                self.sync_tags();
                let Some(t) = self.tags.tag(id) else {
                    return self.error(format!("no tag {:#010x} in the open map", id.0));
                };
                if kinds::kind(t.group).editor == Editor::Scenario {
                    Doc::Map
                } else {
                    let map = self.loaded.clone();
                    self.tabs.add(Tab { doc, map });
                    doc
                }
            }
            other => other,
        };
        if !self.tabs.has(doc) {
            return self.error(format!(
                "no {} is open: open one first (doc.open)",
                self.doc_id(doc)
            ));
        }
        self.focus_doc(doc);
    }

    /// [`super::Action::FocusDoc`]: show the tab's document in its editor.
    pub(super) fn focus_doc(&mut self, doc: Doc) {
        self.sync_tabs();
        if !self.tabs.has(doc) {
            return self.error(format!("no tab {} is open", self.doc_id(doc)));
        }
        let tool = self.doc_tool(doc);
        self.set_tool(tool);
        if let Doc::Tag(id) = doc {
            if tool == ToolKind::Textures {
                self.run(Action::Bitmap(BitmapAction::Select { tag: id, image: 0 }));
            } else {
                self.run(Action::Tag(TagAction::Select(id)));
            }
        }
        self.tabs.front = Some(doc);
    }

    /// [`super::Action::CloseDoc`]: close a tool's document (the guard has
    /// asked about its edits), or a tag's tab (its edits stay the map's).
    /// Closing the front tab shows the nearest one left open, to its left
    /// first. A tab already gone does nothing.
    pub(super) fn close_doc(&mut self, doc: Doc) {
        // Not synced first: the unsaved-changes guard may have closed the
        // tool's document already, and its tab still says where it was.
        let before: Vec<Doc> = self.tabs.list().iter().map(|t| t.doc).collect();
        let Some(i) = before.iter().position(|&d| d == doc) else {
            return;
        };
        let was_front = self.tabs.front() == Some(doc);
        match doc {
            Doc::Tag(_) => {}
            Doc::Map => {
                self.close_tool(ToolKind::Scenario);
                self.close_tool(ToolKind::Textures);
            }
            Doc::Ui => {
                self.close_tool(ToolKind::UiScreens);
                self.ui_screens.pick = None;
            }
            other => self.close_tool(other.holder().expect("a tool document")),
        }
        self.tabs.remove(doc);
        self.sync_tabs();
        if !was_front {
            return;
        }
        let next = before[..i]
            .iter()
            .rev()
            .chain(&before[i + 1..])
            .copied()
            .find(|&d| self.tabs.has(d));
        match next {
            Some(next) => self.focus_doc(next),
            None => self.tabs.front = None,
        }
    }

    /// [`super::Action::UndoDoc`] and [`super::Action::RedoDoc`]: a step of
    /// the tab's own undo stack.
    pub(super) fn undo_redo_doc(&mut self, doc: Doc, undo: bool) {
        let Doc::Tag(id) = doc else {
            return self.undo_redo(self.doc_tool(doc), undo);
        };
        self.sync_tags();
        let r = if undo {
            self.tags.undo_tag(id)
        } else {
            self.tags.redo_tag(id)
        };
        self.push_tag_patches();
        match r {
            Ok(Some(label)) => {
                let verb = if undo { "undid" } else { "redid" };
                self.info(format!("{verb} {label} in {}", self.doc_title(doc)));
            }
            Ok(None) => self.info(if undo {
                "nothing to undo"
            } else {
                "nothing to redo"
            }),
            Err(why) => self.error(why),
        }
    }

    /// The tab whose undo stack Undo and Redo take: the front tag tab
    /// while the Tags tool shows its tag, else none (the tool's own).
    pub(super) fn undo_target(&self) -> Option<Doc> {
        self.tabs
            .front()
            .filter(|&d| matches!(d, Doc::Tag(_)) && self.tool == ToolKind::Tags)
            .filter(|&d| self.shows(d, ToolKind::Tags))
    }

    /// One tab as `shell.tabs` lists it.
    pub fn tab_json(&self, tab: &Tab) -> Value {
        let doc = tab.doc;
        let (can_undo, can_redo) = self.doc_can(doc);
        let mut v = json!({
            "doc": self.doc_id(doc),
            "kind": match doc {
                Doc::Tag(_) => "tag",
                other => other.tool_id().expect("a tool document"),
            },
            "title": self.doc_title(doc),
            "tool": crate::api::tool_name(self.doc_tool(doc)),
            "dirty": self.doc_dirty(doc),
            "can_undo": can_undo,
            "can_redo": can_redo,
            "front": self.tabs.front() == Some(doc),
        });
        if let Doc::Tag(id) = doc {
            v["editor"] = json!(self.tag_editor(id).id());
            v["id"] = json!(format!("{:#010x}", id.0));
            v["map"] = json!(tab.map);
            if let Some(t) = self.tags.tag(id) {
                v["group"] = json!(kinds::group_id(t.group));
            }
        }
        v
    }
}
