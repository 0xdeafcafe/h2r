//! The document tabs (docs/editor/studio-redesign.md, "The shell"): every
//! open document as a tab, the one in front, and each document's own undo
//! stack. A tab's editor is picked from what it holds: a tag's from its
//! group ([`kinds::kind`]), a map, a variant, a playlist file, a screen or
//! a HUD from the tool that holds it, the game type script from the script
//! editor.
//!
//! The tools' documents (the map, the variant, the playlists, the screen,
//! the HUD) are opened by their own commands; [`App::sync_tabs`] gives each
//! open one a tab and drops the tab of a closed one. A tag tab is opened by
//! [`Action::OpenDoc`]; it belongs to the map it was opened on and closes
//! with it. Its undo stack is its tag's steps of the map's tag document
//! ([`super::TagDoc::undo_tag`]) and the steps of the editors that work on
//! the tag outside its bytes ([`Stack`]), taken in the order they were
//! made. Closing a tag tab keeps its edits: they are the map's, saved or
//! dropped with it.
//!
//! The window's tab strip and `shell.*` both run [`Action::OpenDoc`],
//! [`Action::FocusDoc`], [`Action::CloseDoc`], [`Action::UndoDoc`] and
//! [`Action::RedoDoc`], so they leave the same state. The editors' own
//! Undo and Redo buttons, and the Cmd+Z of the window, run the same two
//! (docs/editor/studio-flow.md, "Shared patterns").
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

/// The stacks a tag document's steps are kept in: the map's tag document
/// (writes of the tag's bytes), and the editor of the tag's kind whose
/// edits are not writes of them, if its kind has one. Every stack numbers
/// its steps on the tag document's counter ([`super::TagDoc::next_step`]),
/// so a document takes them newest first wherever they are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stack {
    /// The tag's own fields: [`super::TagDoc`].
    Tags,
    /// A sound's mouth curves ([`super::sound_mouth`]).
    Mouth,
    /// A bitmap's pixels and shape ([`super::textures`]).
    Textures,
    /// An animation graph's documents ([`super::anim`]).
    Anim,
}

impl Stack {
    /// What a step of this stack changed, for the log.
    fn what(self) -> &'static str {
        match self {
            Stack::Tags => "the tag",
            Stack::Mouth => "the mouth curve",
            Stack::Textures => "the bitmap",
            Stack::Anim => "the animation",
        }
    }

    /// Take its step of tag `id`'s document in `app`: the newest not undone
    /// (`undo`), or the oldest undone that is newer than `floor`. Whether
    /// one moved. The tag document's steps are taken by `App::doc_step`,
    /// which names the field the step wrote.
    fn take_in(self, app: &mut App, id: TagId, undo: bool, floor: u64) -> bool {
        match self {
            Stack::Mouth => app.sound_mouth_step_in(id, undo),
            Stack::Textures => app.textures_undo_in(id, undo),
            Stack::Anim if undo => app.anim_undo_in(id),
            Stack::Anim => app.anim_redo_in(id, floor),
            Stack::Tags => unreachable!("the tag document's steps are taken by App::doc_step"),
        }
    }
}

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
    /// The game type script in the script editor (`app::rules_edit`).
    Rules,
    /// The voice open in the voice document (`app::voices`): the id is
    /// `Voices::doc`, the pane `Voices::pane`.
    Voice,
    /// The Forge map variant open as a document (`app::forge_variants`).
    Forge,
    /// The focused package manifest and its items.
    Package,
    /// A tag of the open map, in the editor its group gets.
    Tag(TagId),
    /// A package item shown from its authored source before it is in a map.
    SourceAsset(usize),
}

/// What the window draws under the bar: the editor of the front document.
/// The window has no tool tabs; this is read from the front tab and the
/// panels it brings ([`App::surface`]), and Go to anything's editor
/// commands ask for one ([`Action::Show`]).
///
/// [`Action::Show`]: super::Action::Show
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Surface {
    /// The map's scene: the browser, the outliner, the viewport.
    Scene,
    /// The tag field panel and the editors of the tag kinds.
    Tags,
    /// The map's scripts, read only.
    Scripts,
    Variants,
    Playlists,
    UiScreens,
    Hud,
    Textures,
    Package,
}

impl Surface {
    /// The surfaces the View menu lists for opening an editor.
    pub const EDITORS: [(Surface, &'static str); 6] = [
        (Surface::Scene, "Map"),
        (Surface::Variants, "Game variants"),
        (Surface::Playlists, "Playlists"),
        (Surface::UiScreens, "UI screens"),
        (Surface::Hud, "HUD"),
        (Surface::Textures, "Textures"),
    ];

    /// The surface shares the map's panels (browser, scene, tags,
    /// scripts, viewport and inspector).
    pub fn is_map(self) -> bool {
        matches!(self, Surface::Scene | Surface::Tags | Surface::Scripts)
    }

    /// The holder of the document this surface shows.
    pub fn tool(self) -> ToolKind {
        match self {
            Surface::Scene => ToolKind::Scenario,
            Surface::Tags => ToolKind::Tags,
            Surface::Scripts => ToolKind::Scripts,
            Surface::Variants => ToolKind::Variants,
            Surface::Playlists => ToolKind::Playlists,
            Surface::UiScreens => ToolKind::UiScreens,
            Surface::Hud => ToolKind::Hud,
            Surface::Textures => ToolKind::Textures,
            Surface::Package => ToolKind::Package,
        }
    }
}

impl From<ToolKind> for Surface {
    fn from(tool: ToolKind) -> Self {
        match tool {
            ToolKind::Scenario => Surface::Scene,
            ToolKind::Tags => Surface::Tags,
            ToolKind::Scripts => Surface::Scripts,
            ToolKind::Variants => Surface::Variants,
            ToolKind::Playlists => Surface::Playlists,
            ToolKind::UiScreens => Surface::UiScreens,
            ToolKind::Hud => Surface::Hud,
            ToolKind::Textures => Surface::Textures,
            ToolKind::Package => Surface::Package,
        }
    }
}

/// The tools' documents, in the order their tabs are first given.
pub const TOOL_DOCS: [Doc; 9] = [
    Doc::Map,
    Doc::Variant,
    Doc::Playlists,
    Doc::Ui,
    Doc::Hud,
    Doc::Rules,
    Doc::Voice,
    Doc::Forge,
    Doc::Package,
];

impl Doc {
    /// The tool that holds a tool's document (`None` for a tag).
    pub fn holder(self) -> Option<ToolKind> {
        Some(match self {
            Doc::Map => ToolKind::Scenario,
            Doc::Variant => ToolKind::Variants,
            Doc::Playlists => ToolKind::Playlists,
            Doc::Ui => ToolKind::UiScreens,
            Doc::Hud => ToolKind::Hud,
            // The script editor docks beside the variant's settings.
            Doc::Rules => ToolKind::Variants,
            Doc::SourceAsset(_) => ToolKind::Tags,
            // The voice document draws over the field panel's surface,
            // as the sound editor does (`ui::voices::window`).
            Doc::Voice => ToolKind::Tags,
            // The variant's window draws over the same surface.
            Doc::Forge => ToolKind::Tags,
            Doc::Package => ToolKind::Package,
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
            Doc::Rules => "rules",
            Doc::Voice => "voice",
            Doc::Forge => "forge",
            Doc::SourceAsset(_) => "item",
            Doc::Package => "package",
            Doc::Tag(_) => return None,
        })
    }
}

impl App {
    /// What the window draws: the surface of the tool the front document
    /// brings forward.
    pub fn surface(&self) -> Surface {
        Surface::from(self.tool)
    }

    /// The surface the guard's pending action asks about.
    pub fn pending_surface(&self) -> Option<Surface> {
        self.pending_tool().map(Surface::from)
    }

    /// [`Tabs::set_item`]: the package item a tool document's tab was
    /// opened from. No tab, nothing kept.
    pub(super) fn set_tab_item(&mut self, doc: Doc, item: Option<String>, path: Option<String>) {
        self.tabs.set_item(doc, item, path);
    }

    /// The item whose id a tab answers: the tab holding `doc` was opened
    /// from a package item (`app::assets::open_item`).
    pub fn tab_item(&self, doc: Doc) -> Option<&str> {
        self.tabs.item(doc)
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
    /// For a tool document opened from a package item (`item:<package>:<n>`):
    /// the item's asset id, which the tab's id answers, and the file it was
    /// opened from, kept so `sync_tabs` can see the editor still holds it.
    /// A script's path must match; a variant's is not checked (the
    /// variant's origin says nothing about a file).
    pub item: Option<String>,
    pub item_path: Option<String>,
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
    /// Set or clear the package item a tool document's tab was opened
    /// from, with the file it came from.
    fn set_item(&mut self, doc: Doc, item: Option<String>, path: Option<String>) {
        if let Some(t) = self.open.iter_mut().find(|t| t.doc == doc) {
            t.item = item;
            t.item_path = path;
        }
    }
    /// The item whose id the tab answers, if it was opened from one.
    fn item(&self, doc: Doc) -> Option<&str> {
        self.open
            .iter()
            .find(|t| t.doc == doc)
            .and_then(|t| t.item.as_deref())
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
    /// `ui`, `hud`), a package item's `item:<package>:<n>`, a voice's
    /// `voice:<id>`, or a tag's `group:path` as `tag.*` takes it.
    pub fn doc_id(&self, doc: Doc) -> String {
        match doc {
            Doc::Tag(id) => match self.tags.tag(id) {
                Some(t) => format!("{}:{}", tags::defs::fourcc_text(t.group), t.name),
                None => format!("{:#010x}", id.0),
            },
            Doc::SourceAsset(_) => self.source_asset(doc).map(|s| s.id.clone()).unwrap_or_default(),
            Doc::Voice => format!("voice:{}", self.voices.doc.as_deref().unwrap_or_default()),
            Doc::Forge => self
                .forge_variants
                .open
                .as_ref()
                .map_or_else(|| "forge".to_owned(), |open| open.asset()),
            other => match self.tab_item(other) {
                Some(item) => item.to_owned(),
                None => other.tool_id().expect("a tool document").to_owned(),
            },
        }
    }

    /// The open tab `id` names: [`Self::doc_id`], the tool document's own
    /// name when its tab was opened from an item, or a tag's `0x...` id.
    pub fn find_doc(&self, id: &str) -> Option<Doc> {
        self.tabs.list().iter().map(|t| t.doc).find(|&d| {
            self.doc_id(d) == id
                || matches!(d, Doc::Tag(t) if format!("{:#010x}", t.0).eq_ignore_ascii_case(id))
                || d.tool_id().is_some_and(|t| t == id)
        })
    }

    /// What a tab shows as its name.
    pub fn doc_title(&self, doc: Doc) -> String {
        match doc {
            Doc::SourceAsset(_) => self.source_asset(doc).map(|s| s.key.path.clone()).unwrap_or_default(),
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
            Doc::Rules => self.rules.title.clone(),
            Doc::Package => self.package_document.name().unwrap_or("Package").to_owned(),
            Doc::Voice => self
                .voices
                .doc
                .as_deref()
                .and_then(|id| self.voice_status(id))
                .map_or_else(|| "voice".to_owned(), |v| v.name.to_owned()),
            Doc::Forge => self
                .forge_variants
                .open
                .as_ref()
                .map_or_else(|| "Forge variant".to_owned(), |open| open.doc.title.clone()),
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
            Doc::Rules => self.rules.dirty(),
            Doc::SourceAsset(_) => false,
            // Lines that are said and not kept are the batch's, not a
            // document's edits: nothing to ask about on close.
            Doc::Voice => false,
            Doc::Forge => self.forge_variants.open.as_ref().is_some_and(|open| open.dirty()),
            Doc::Map => self.tool_dirty(ToolKind::Scenario) || self.tool_dirty(ToolKind::Textures),
            other => self.tool_dirty(other.holder().expect("a tool document")),
        }
    }

    /// The tab's own undo stack can undo, and redo. A tag's is every step
    /// of its document (`Stack`): it can undo while one is not undone,
    /// and redo while an undone step is newer than every step that is (a
    /// step made after an undo leaves the undone ones wrong). The map's
    /// scripts are the map document's while the Scripts surface shows them,
    /// and a target in edit mode takes the map's undo
    /// (docs/editor/modeling.md).
    pub fn doc_can(&self, doc: Doc) -> (bool, bool) {
        match doc {
            Doc::Tag(id) => {
                let top = self.doc_top(id);
                let floor = top.map_or(0, |(_, serial)| serial);
                (top.is_some(), self.doc_redo(id, floor).is_some())
            }
            Doc::Map if self.tool == ToolKind::Scripts => {
                self.scripts.edit.as_ref().map_or((false, false), |d| {
                    let (undo, redo) = d.depth();
                    (undo > 0, redo > 0)
                })
            }
            Doc::Map if self.modeling.session.active.is_some() => {
                let g = &self.modeling.session;
                (g.can_undo(), g.can_redo())
            }
            Doc::Map => (self.session.can_undo(), self.session.can_redo()),
            // The text widget keeps the script's own undo.
            Doc::Rules => (false, false),
            Doc::SourceAsset(_) => (false, false),
            // A voice's lines are rows: Remove takes one away.
            Doc::Voice | Doc::Package => (false, false),
            // Each field edit is a step of the variant's own stack.
            Doc::Forge => self
                .forge_variants
                .open
                .as_ref()
                .map_or((false, false), |open| open.can()),
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
            Doc::Rules => self.rules.opened,
            Doc::Voice => self.voices.doc.is_some(),
            Doc::Forge => self.forge_variants.open.is_some(),
            Doc::Package => self.package_document.id().is_some(),
            Doc::Tag(_) => false,
            Doc::SourceAsset(_) => false,
        }
    }

    /// Give every open tool document a tab (a new one comes to the front)
    /// and drop the tabs of closed ones, and of tags whose map is no
    /// longer the one open. A tab opened from a package item keeps its
    /// item id only while the editor still holds that item's file (a
    /// script); the others are cleared when the document is replaced.
    pub fn sync_tabs(&mut self) {
        for doc in TOOL_DOCS {
            let open = self.tool_doc_open(doc);
            if open && !self.tabs.has(doc) {
                self.tabs.add(Tab {
                    doc,
                    map: None,
                    item: None,
                    item_path: None,
                });
                self.tabs.front = Some(doc);
                if doc == Doc::Rules {
                    self.set_tool(ToolKind::Variants);
                }
            } else if !open && self.tabs.has(doc) {
                self.tabs.remove(doc);
            }
        }
        // A script tab's item id answers only its own file.
        if self.tabs.item(Doc::Rules).is_some() {
            let path = self
                .tabs
                .list()
                .iter()
                .find(|t| t.doc == Doc::Rules)
                .and_then(|t| t.item_path.clone());
            let held = path
                .as_deref()
                .zip(self.rules.path.as_deref())
                .is_some_and(|(kept, open)| open.display().to_string() == kept);
            if !held {
                self.tabs.set_item(Doc::Rules, None, None);
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
                    self.tabs.add(Tab {
                        doc,
                        map,
                        item: None,
                        item_path: None,
                    });
                    doc
                }
            }
            Doc::SourceAsset(_) => {
                self.tabs.add(Tab {
                    doc,
                    map: None,
                    item: None,
                    item_path: None,
                });
                doc
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
        if doc == Doc::Rules {
            // Picks the script's tab among the variant's panels.
            self.rules.show = true;
        }
        if let Doc::Tag(id) = doc {
            if tool == ToolKind::Textures {
                self.run(Action::Bitmap(BitmapAction::Select { tag: id, image: 0 }));
            } else {
                self.run(Action::Tag(TagAction::Select(id)));
            }
        }
        self.tabs.front = Some(doc);
        self.doc_trail.visit(doc);
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
        if doc == Doc::Forge && self.doc_dirty(doc) {
            return self.error(
                "the map variant has unsaved edits: save it, or undo them, before closing",
            );
        }
        let was_front = self.tabs.front() == Some(doc);
        match doc {
            Doc::Tag(_) => {}
            Doc::SourceAsset(index) => self.source_assets[index] = None,
            Doc::Map => {
                self.close_tool(ToolKind::Scenario);
                self.close_tool(ToolKind::Textures);
            }
            Doc::Ui => {
                self.close_tool(ToolKind::UiScreens);
                self.ui_screens.pick = None;
            }
            // The tab goes; the text stays in the editor for the next open.
            Doc::Rules => self.rules.opened = false,
            // The batch keeps its rows for the next open.
            Doc::Voice => self.voices.doc = None,
            Doc::Forge => self.forge_variants.open = None,
            Doc::Package => {
                self.package_document.read = None;
                self.package_document.result = None;
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
            None => {
                self.tabs.front = None;
                if doc == Doc::Package {
                    self.set_tool(ToolKind::Scenario);
                }
            }
        }
    }

    /// The stacks that hold the steps of tag `id`'s document
    /// ([`Stack`]): the tag document's, then its kind's editor's.
    fn doc_stacks(&self, id: TagId) -> impl Iterator<Item = Stack> {
        let editor = match self.shown_tags().tag(id).map(|t| &t.group.0) {
            Some(b"snd!") => Some(Stack::Mouth),
            Some(b"bitm") => Some(Stack::Textures),
            Some(b"jmad") => Some(Stack::Anim),
            _ => None,
        };
        [Some(Stack::Tags), editor].into_iter().flatten()
    }

    /// The stack holding the newest step of tag `id`'s document that is not
    /// undone, with the step's number.
    fn doc_top(&self, id: TagId) -> Option<(Stack, u64)> {
        self.doc_stacks(id)
            .filter_map(|stack| self.stack_top(stack, id).map(|serial| (stack, serial)))
            .max_by_key(|(_, serial)| *serial)
    }

    /// The stack holding the step a redo of tag `id`'s document takes: the
    /// oldest undone one that is newer than `floor`, the newest step not
    /// undone. An undone step at or below it was made wrong by that step.
    fn doc_redo(&self, id: TagId, floor: u64) -> Option<Stack> {
        self.doc_stacks(id)
            .filter_map(|stack| {
                self.stack_redo(stack, id, floor)
                    .filter(|serial| *serial > floor)
                    .map(|serial| (serial, stack))
            })
            .min_by_key(|(serial, _)| *serial)
            .map(|(_, stack)| stack)
    }

    /// The number of the newest step of tag `id` not undone in `stack`.
    fn stack_top(&self, stack: Stack, id: TagId) -> Option<u64> {
        match stack {
            Stack::Tags => self.tags.step_serial(id),
            Stack::Mouth => self.sound_mouth_step(id),
            Stack::Textures => self.textures.step_serial(id),
            Stack::Anim => self.anim_step_serial(id),
        }
    }

    /// The number of the step of tag `id` a redo of `stack` takes next.
    fn stack_redo(&self, stack: Stack, id: TagId, floor: u64) -> Option<u64> {
        match stack {
            Stack::Tags => self.tags.redo_serial(id),
            Stack::Mouth => self.sound_mouth_redo_step(id),
            Stack::Textures => self.textures.redo_serial(id),
            Stack::Anim => self.anim_redo_serial(id, floor),
        }
    }

    /// A step of tag `id`'s document: the newest not undone (`undo`), or
    /// the oldest undone that is still right. The stack it came from and
    /// what it changed; `Ok(None)` with no step to take, `Err` when the tag
    /// document refuses.
    fn doc_step(&mut self, id: TagId, undo: bool) -> Result<Option<(Stack, String)>, String> {
        let top = self.doc_top(id);
        let floor = top.map_or(0, |(_, serial)| serial);
        let stack = if undo {
            top.map(|(stack, _)| stack)
        } else {
            self.doc_redo(id, floor)
        };
        let from = match stack {
            // The tag document names the field its step wrote.
            Some(Stack::Tags) => {
                let step = if undo {
                    self.tags.undo_tag(id)
                } else {
                    self.tags.redo_tag(id)
                };
                self.push_tag_patches();
                return Ok(step?.map(|label| (Stack::Tags, label)));
            }
            Some(from) if from.take_in(self, id, undo, floor) => from,
            // Chosen from its stack, the step was gone or could not be taken.
            _ => return Ok(None),
        };
        Ok(Some((from, from.what().to_owned())))
    }

    /// [`super::Action::UndoDoc`] and [`super::Action::RedoDoc`]: a step of
    /// the tab's own undo stack.
    pub(super) fn undo_redo_doc(&mut self, doc: Doc, undo: bool) {
        if doc == Doc::Rules {
            return self.info("the script editor keeps its own undo: use it in the text");
        }
        if matches!(doc, Doc::SourceAsset(_)) {
            return;
        }
        if doc == Doc::Voice {
            return self.info("a voice's lines are rows: Remove takes one away");
        }
        if doc == Doc::Forge {
            return self.forge_undo_redo(undo);
        }
        let Doc::Tag(id) = doc else {
            // The map's scripts are the map document's while the Scripts
            // surface shows them: a compile is its step.
            let tool = if doc == Doc::Map && self.tool == ToolKind::Scripts {
                ToolKind::Scripts
            } else {
                self.doc_tool(doc)
            };
            return self.undo_redo(tool, undo);
        };
        self.sync_tags();
        match self.doc_step(id, undo) {
            Ok(Some((_, what))) => {
                let verb = if undo { "undid" } else { "redid" };
                self.info(format!("{verb} {what} in {}", self.doc_title(doc)));
            }
            Ok(None) => self.info(if undo {
                "nothing to undo"
            } else {
                "nothing to redo"
            }),
            Err(why) => self.error(why),
        }
    }

    /// The document whose undo stack Undo and Redo take: the tag tab in
    /// front while the Tags tool shows it, or the bitmap on the Textures
    /// canvas (its tab or not); else none (the tool's own).
    pub(super) fn undo_target(&self) -> Option<Doc> {
        match self.tool {
            ToolKind::Tags => self.tabs.front().filter(|&d| self.shows(d, ToolKind::Tags)),
            ToolKind::Textures => self.textures.selected.map(Doc::Tag),
            _ => None,
        }
    }

    /// The tag whose tab is in front while the Tags tool shows it: the
    /// document a tag-kind editor (the sound editor) draws.
    pub fn front_tag_doc(&self) -> Option<TagId> {
        match self.undo_target()? {
            Doc::Tag(id) if self.tool == ToolKind::Tags => Some(id),
            _ => None,
        }
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
        if let Some(source) = self.source_asset(doc) {
            v["badge"] = json!(format!("new in {}", source.package));
            v["asset"] = json!(super::assets::tag_id(&source.key));
            v["editor"] = json!(match &source.view {
                super::assets::SourceView::Tag(_) => "tag_source",
                super::assets::SourceView::Sound { .. } => "audition",
                super::assets::SourceView::Model(_) => "modeling",
            });
        }
        if let Some(item) = &tab.item {
            v["item"] = json!(item);
        }
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
