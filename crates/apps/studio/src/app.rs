//! The studio's state and commands, with no UI toolkit in sight: the
//! opened source and its maps, the active tool, each tool's session
//! (scenario, game variants, playlists), the log, and the unsaved-changes
//! guard. The Dear ImGui layer (`ui`) turns
//! clicks and shortcuts into [`Action`]s and draws this state; tests drive
//! it directly.
use std::path::{Path, PathBuf};

pub mod commands;
pub(crate) mod convert;
pub mod hud;
mod editing;
mod library;
pub mod map_cards;
pub mod map_pack;
pub mod shared_refs;
pub mod tag_api;
pub mod tag_browse;
pub mod tag_bundle;
pub mod tag_doc;
pub mod tag_tree;
pub mod ui_screens;
pub use commands::{PlaylistEdit, Refused};
pub use convert::{
    Built, Converter, Input as ConvertInput, Running, Written, explain, from_scratch,
    retail_sources,
};
pub use h1convert::convert::Sandbox as ConvertSandbox;
pub use library::{Compare, block_of, kind_of, markers as library_markers, tris as library_tris};
pub use hud::HudTool;
pub use tag_doc::{BlockOp, TagAction, TagDoc, TagWrite};
pub use ui_screens::{Pick, UiScreens};

use editor_core::{MapPack, MapSource, Status, variant_sources};
use hopper_edit::HopperSession;
use scenario_edit::{BlockKind, MapSource as EditSource, PackageForm, Session};
use variants::panel::Origin;
use variants::{GameType, GameVariant, SettingId, VariantEditor};

/// The log keeps at most this many lines.
pub const LOG_LIMIT: usize = 500;

/// The tools of the suite.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolKind {
    Scenario,
    Variants,
    Playlists,
    /// Our native game screens, drawn and stepped offline
    /// (docs/game/ui-builder.md).
    UiScreens,
    /// A map's HUDs, drawn as the game places them and edited
    /// (docs/editor/hud-editor.md).
    Hud,
    /// Every field of every tag of the open map (docs/editor/editor.md).
    Tags,
}

impl ToolKind {
    pub const ALL: [ToolKind; 6] = [
        ToolKind::Scenario,
        ToolKind::Tags,
        ToolKind::Variants,
        ToolKind::Playlists,
        ToolKind::UiScreens,
        ToolKind::Hud,
    ];
    pub fn title(self) -> &'static str {
        match self {
            ToolKind::Scenario => "Scenario",
            ToolKind::Variants => "Game variants",
            ToolKind::Playlists => "Playlists",
            ToolKind::UiScreens => "UI Screens",
            ToolKind::Hud => "HUD",
            ToolKind::Tags => "Tags",
        }
    }
}

/// The primary subject of the scenario inspector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    Placement(BlockKind, usize),
    Tag(map::TagId),
}

/// One user command. Everything the menus, toolbar, outliner and keyboard
/// shortcuts do goes through [`App::dispatch`].
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Open a disc image, a `.map` file or a folder of maps as the source.
    OpenSource(PathBuf),
    /// Open a map from the source's list.
    SelectMap(String),
    /// Write the edited map as a new `.map` file.
    SaveMapAs(PathBuf),
    /// Save the edits as a map package under the data root.
    SavePackage(PackageForm),
    /// Open one of the open map's packages (by directory name).
    OpenPackage(String),
    /// Open a package from the Maps list (by [`PackageEntry::key`]): a
    /// disc map's edit, a new map, or a converted Halo 1 map's variant.
    ///
    /// [`PackageEntry::key`]: editor_core::packages::PackageEntry::key
    OpenPackageEntry(String),
    /// Open a Map Pack map (by its map name, `warlock`, or an installed
    /// package's offer directory): [`map_pack::PackMap`].
    OpenMapPack(String),
    /// Show a map of the open Halo 1 source, read-only.
    SelectHalo1Map(String),
    /// Draw a converted map's Halo 1 source over it.
    SetOverlay(bool),
    /// Convert the Halo 1 map on show to Halo 2, in the background.
    ConvertHalo1(ConvertSandbox),
    /// The sandbox "Convert to Halo 2" uses.
    SetConvertSandbox(ConvertSandbox),
    Undo,
    Redo,
    /// Undo or redo in a given tool, whichever is shown (the agent API's
    /// `doc.undo`).
    UndoIn(ToolKind),
    RedoIn(ToolKind),
    /// Set one field of a placement, checked by the scenario first. Edits
    /// with the same merge key in a row are one undo step (a drag).
    SetField {
        block: BlockKind,
        index: usize,
        field: String,
        value: scenario_edit::Value,
        merge: Option<u64>,
    },
    /// Edit a field of every selected placement (the inspector), one undo
    /// step; edits with the same merge key in a row stay one.
    EditSelection {
        field: String,
        edit: scenario_edit::session::FieldEdit,
        merge: u64,
    },
    /// End a run of merged edits: the next edit is a new undo step.
    SealEdit,
    /// The viewport gizmo's drag: put the selected placement at
    /// `position`, turned to `ypr` (the rest of a multi-selection rides
    /// along). Every one until [`Action::EndGesture`] is one undo step.
    GestureSet {
        position: [f32; 3],
        ypr: [f32; 3],
    },
    /// The scale gizmo's drag: the selection's scale, as
    /// [`Action::GestureSet`].
    GestureScale(f32),
    /// The gizmo was let go: the drag's undo step ends.
    EndGesture,
    /// Move a placement (and turn it, given yaw, pitch and roll) as one
    /// undo step.
    MovePlacement {
        block: BlockKind,
        index: usize,
        position: [f32; 3],
        ypr: Option<[f32; 3]>,
    },
    Select(Option<(BlockKind, usize)>),
    /// Add a placement to the selection, or take it out (Ctrl/Cmd+click).
    ToggleSelect((BlockKind, usize)),
    /// Select these placements, the first as the primary (Shift+click in
    /// the outliner).
    SelectMany(Vec<(BlockKind, usize)>),
    /// Add a new placement where the cursor met the level: a copy of an
    /// entry, or the block's first palette item.
    PlaceAt(BlockKind, Option<usize>, [f32; 3]),
    /// Delete the selected placements.
    Delete,
    /// Duplicate the selected placements.
    Duplicate,
    /// Add a new placement to a block at the camera focus.
    Add(BlockKind),
    /// Delete or duplicate a given entry (the outliner's context menu).
    DeleteEntry(BlockKind, usize),
    DuplicateEntry(BlockKind, usize),
    FrameAll,
    FocusSelected,
    SetAllowGrowth(bool),
    /// Show another tool.
    SetTool(ToolKind),

    // Game variants.
    /// Open a list entry (a built-in or a save).
    OpenVariant(Origin),
    /// Start a new, unsaved variant of a game type with its defaults.
    NewVariant(GameType),
    /// Copy the open built-in under a name (built-ins are read-only).
    CopyBuiltin(String),
    SetVariantName(String),
    SetSetting(SettingId, i32),
    /// Set the open variant's h2recomp options (its extension block).
    SetVariantExt(variants::ext::Ext),
    /// Write the open variant to the save storage.
    SaveVariant,
    /// Back to the variant as last loaded or saved.
    RevertVariant,
    /// Delete the open saved variant.
    DeleteVariant,
    /// Write the open variant as a portable `.h2variant` file.
    ExportVariant(PathBuf),
    /// Open a portable `.h2variant` file as a new, unsaved variant.
    ImportVariantFile(PathBuf),
    /// The same, from pasted text.
    ImportVariantText(String),

    // UI Screens.
    /// Show a screen of ours in a state, or a retail screen.
    UiPick(Pick),
    /// A pad press on the screen shown.
    UiPress(boot::ui_kit::input::Input),
    /// Write the screen's PNG and JSON and the sheet into a folder (never
    /// a Git checkout).
    UiExport(PathBuf),

    // HUD.
    /// Open a map of the source in the HUD tool.
    HudOpenMap(String),
    /// Open a package from the Maps list (by key) in the HUD tool.
    HudOpenPackage(String),
    /// Show and edit one of the open map's HUDs.
    HudPick(map::TagId),
    /// Draw another HUD with it (a weapon held, a vehicle seat), or stop.
    HudWith(map::TagId, bool),
    HudSelect(Option<hud_edit::WidgetKey>),
    /// Set a widget field (by its key in the field table); with `merge`
    /// the step joins the last one of the same field.
    HudSet(hud_edit::WidgetKey, String, hud_edit::Value, bool),
    /// Move a widget in the screen mode shown (a drag merges its moves).
    HudMove(hud_edit::WidgetKey, (i16, i16), bool),
    /// The state the HUD is drawn at.
    HudState(Box<hud_edit::HudState>),
    /// Write the map with the HUD edits as a new `.map`.
    HudSaveAs(PathBuf),
    /// A write from the generic field panel into the selected widget's
    /// element (an HUD edit, with the HUD tool's undo).
    HudWrite(TagWrite),
    /// The field panel asked for something the HUD tool leaves to the Tags
    /// tool: why.
    HudRefused(String),
    // Tags.
    /// A tag editor command: select a tag, write a field, change a block.
    Tag(TagAction),
    /// An inspector edit always targets the open map's tag document.
    InspectorTag(TagAction),
    /// Open a tag from a shared cache by group and name.
    BrowseSharedTag(map::TagId),
    /// Show another map of the source in the Tags tool, read-only (a
    /// shared cache), or (`None`) the open map again.
    BrowseTags(Option<String>),
    /// Decode a bitmap tag of the map the Tags tool shows for its preview.
    TagPreview(map::TagId),
    /// Write tags of the map the Tags tool shows as a tag bundle: to
    /// `out`, else under the data root's `tag-bundles/`.
    TagExport {
        tags: Vec<map::TagId>,
        out: Option<PathBuf>,
        options: tag_bundle::ExportOptions,
    },
    /// Replace tags of the open map with a tag bundle's (one undo step).
    TagImport {
        path: PathBuf,
        options: tag_bundle::ImportOptions,
    },

    // Playlists.
    /// Open a `multiplayer_hopper.dat` (either form).
    OpenHopper(PathBuf),
    /// Start from a blank playlist file.
    NewHopper,
    /// Start from the Live server's built-in lineup.
    NewDefaultHopper,
    /// Save the playlist file in place.
    SaveHopper,
    /// Save the playlist file to a path (and edit that from now on).
    SaveHopperAs(PathBuf),
    /// Give a playlist entry a game variant (one undo step).
    SetEntryVariant(usize, usize, Box<GameVariant>),
    /// Open a playlist entry's variant in the variant tool, as a new copy.
    EditEntryVariant(usize, usize),
    /// Add a `.h2variant` file to the entries' variant picker.
    LoadVariantFile(PathBuf),
    /// Re-read the saved variants for the picker.
    RefreshPickerVariants,
    /// Edit the open playlist file, as one undo step.
    EditPlaylist(PlaylistEdit),

    /// Quit the app.
    Quit,
}

impl Action {
    /// The tool whose unsaved edits the action would drop, if any.
    fn discards(&self, app: &App) -> Option<ToolKind> {
        let dirty = |t: ToolKind| app.tool_dirty(t).then_some(t);
        match self {
            Action::Quit => ToolKind::ALL.into_iter().find(|t| app.tool_dirty(*t)),
            Action::OpenSource(_) => dirty(ToolKind::Scenario),
            // A shared cache opens in the Tags tool, beside the open map.
            Action::SelectMap(name) if tag_browse::opens_as_tags(name) => None,
            Action::SelectMap(name)
                if app.loaded.as_deref() != Some(name.as_str()) || app.opened_package.is_some() =>
            {
                dirty(ToolKind::Scenario)
            }
            Action::OpenPackageEntry(key) if app.opened_package.as_deref() != Some(key) => {
                dirty(ToolKind::Scenario)
            }
            Action::OpenMapPack(key)
                if app.pack_entry(key).is_some_and(|m| app.opened_pack_map() != Some(m.map.as_str())) =>
            {
                dirty(ToolKind::Scenario)
            }
            Action::SelectHalo1Map(_) => dirty(ToolKind::Scenario),
            Action::OpenVariant(o) if app.variant_origin() != Some(o) => dirty(ToolKind::Variants),
            Action::NewVariant(_)
            | Action::ImportVariantFile(_)
            | Action::ImportVariantText(_)
            | Action::EditEntryVariant(..) => dirty(ToolKind::Variants),
            Action::OpenHopper(_) | Action::NewHopper | Action::NewDefaultHopper => {
                dirty(ToolKind::Playlists)
            }
            Action::HudOpenMap(_) | Action::HudOpenPackage(_) => dirty(ToolKind::Hud),
            _ => None,
        }
    }
}

/// The answer to the unsaved-changes prompt.
#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    /// Save in place first (a variant to its save, a playlist file to its
    /// path), then go on.
    Save,
    /// Save the edited map or playlist file here first, then go on.
    SaveAs(PathBuf),
    /// Drop the edits and go on.
    Discard,
    /// Do nothing.
    Cancel,
}

/// A log line.
#[derive(Clone, Debug, PartialEq)]
pub struct LogLine {
    pub status: Status,
    /// Seconds since the app started.
    pub at: f64,
}

/// The whole studio state.
pub struct App {
    source: Option<MapSource>,
    maps: Vec<String>,
    /// The map loaded into the scenario session, by name.
    loaded: Option<String>,
    /// The file the edited map was last saved to.
    saved_path: Option<PathBuf>,
    pub tool: ToolKind,
    pub session: Session,
    /// The action held behind the unsaved-changes prompt, and the tool
    /// whose edits it would drop.
    pending: Option<(Action, ToolKind)>,
    /// The game variant editor, built from the source when first shown.
    pub variants: Option<VariantEditor>,
    /// The source path the variant editor was built for.
    variants_for: Option<Option<PathBuf>>,
    /// The runtime data root (`--data`); `None` is the default one.
    data_root: Option<PathBuf>,
    /// The playlist editor.
    pub hoppers: HopperSession,
    /// The playlist shown (0..16).
    pub hopper_selected: usize,
    /// The preset picked for the shown playlist, before it is applied.
    pub hopper_preset: Option<usize>,
    log: Vec<LogLine>,
    /// Lines ever logged (the log drops its oldest past [`LOG_LIMIT`]).
    logged: u64,
    started: std::time::Instant,
    quit: bool,
    /// The outliner's filter text.
    pub filter: String,
    /// The "Save as package" form while it is open.
    pub package_form: Option<PackageForm>,
    /// The image path typed into that form.
    pub package_image: String,
    /// The packages under the data root.
    packages: Vec<editor_core::packages::PackageEntry>,
    /// The package entry open in the scenario tool.
    opened_package: Option<String>,
    /// The open Map Pack, beside the Halo 2 source (its maps resolve their
    /// shared caches from that source).
    mappack: Option<MapPack>,
    /// The Map Pack group (the open disc's maps, else the installed
    /// ones), and the problems with the install already logged.
    pack: Vec<map_pack::PackMap>,
    pack_notes: Vec<String>,
    /// The Maps list's cards (names, descriptions, pictures): the Halo 2
    /// source's, the Halo 1 source's, the packages' pictures; each read
    /// on its own thread (`map_cards`).
    h2_cards: map_cards::CardSet,
    h1_cards: map_cards::CardSet,
    pkg_cards: map_cards::CardSet,
    /// The open Halo 1 source and its maps.
    h1: Option<editor_core::halo1::H1Source>,
    h1_maps: Vec<String>,
    /// The Halo 1 map shown read-only, and its name.
    h1_view: Option<editor_core::halo1::H1View>,
    h1_loaded: Option<String>,
    /// The open converted map's Halo 1 source.
    compare: Option<Compare>,
    show_overlay: bool,
    overlay_rev: u64,
    /// The conversion running, why the last one wrote nothing, and what
    /// runs it.
    convert: Option<Running>,
    convert_error: Option<String>,
    converter: Converter,
    /// The sandbox "Convert to Halo 2" uses.
    pub convert_sandbox: ConvertSandbox,
    /// The UI Screens tool.
    pub ui_screens: UiScreens,
    /// The HUD tool.
    pub hud: HudTool,
    /// Pairing with a running game: what the agent API asks of it and what
    /// it publishes back (`crate::play::pair`).
    pub pairing: crate::play::pair::Bridge,
    /// The tag editor's document over the open map.
    pub tags: TagDoc,
    /// The selected tag's shared-map line, for the tag and revision.
    tag_homes: std::cell::RefCell<Option<(map::TagId, u64, Option<String>, Option<String>)>>,
    /// Another map of the source shown read-only in the Tags tool (a
    /// shared cache), beside the open map's document ([`Self::tags`]).
    pub(crate) tag_browse: Option<tag_browse::Browse>,
    /// The bitmap preview of the open map's document (the browsed map's
    /// is its own): the tag and the picture, or why there is none.
    tag_preview: Option<(map::TagId, Result<std::sync::Arc<map_pack::Picture>, String>)>,
    /// What the last tag export and import did, for the agent API.
    pub(crate) last_tag_export: Option<(PathBuf, tag_bundle::ExportReport)>,
    pub(crate) last_tag_import: Option<tag_bundle::ImportReport>,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        Self {
            source: None,
            maps: Vec::new(),
            loaded: None,
            saved_path: None,
            tool: ToolKind::Scenario,
            session: Session::new(),
            pending: None,
            variants: None,
            variants_for: None,
            data_root: None,
            hoppers: HopperSession::new(),
            hopper_selected: 0,
            hopper_preset: None,
            log: Vec::new(),
            logged: 0,
            started: std::time::Instant::now(),
            quit: false,
            filter: String::new(),
            package_form: None,
            package_image: String::new(),
            packages: Vec::new(),
            opened_package: None,
            mappack: None,
            pack: Vec::new(),
            pack_notes: Vec::new(),
            h2_cards: Default::default(),
            h1_cards: Default::default(),
            pkg_cards: Default::default(),
            h1: None,
            h1_maps: Vec::new(),
            h1_view: None,
            h1_loaded: None,
            compare: None,
            show_overlay: true,
            overlay_rev: 0,
            convert: None,
            convert_error: None,
            converter: from_scratch,
            convert_sandbox: ConvertSandbox::Halo2,
            ui_screens: UiScreens::default(),
            hud: HudTool::default(),
            pairing: Default::default(),
            tags: TagDoc::default(),
            tag_homes: Default::default(),
            tag_browse: None,
            tag_preview: None,
            last_tag_export: None,
            last_tag_import: None,
        }
    }

    /// Packages are saved to and listed from `<root>/maps`, and variants
    /// to and from the save storage under it.
    pub fn set_data_root(&mut self, root: PathBuf) {
        self.session.set_data_root(root.clone());
        self.hoppers.set_data_root(root.clone());
        self.data_root = Some(root);
        self.variants_for = None;
        self.refresh_packages();
    }

    pub fn source(&self) -> Option<&MapSource> {
        self.source.as_ref()
    }
    /// The maps of the opened source, in its order.
    pub fn maps(&self) -> &[String] {
        &self.maps
    }
    /// The open Map Pack, if any: its maps are their own group in the Maps
    /// list and open and save like the disc's.
    pub fn mappack(&self) -> Option<&MapPack> {
        self.mappack.as_ref()
    }
    /// The Map Pack's maps, sorted; empty with none open.
    pub fn mappack_maps(&self) -> Vec<String> {
        self.mappack.as_ref().map(MapPack::map_names).unwrap_or_default()
    }
    /// Whether `name` is a Map Pack map, from the open map pack disc or
    /// the installed copy (and not one of the source's own).
    pub fn is_mappack_map(&self, name: &str) -> bool {
        (self.mappack.as_ref().is_some_and(|p| p.has(name))
            || self.pack.iter().any(|m| m.map.eq_ignore_ascii_case(name)))
            && !self.maps.iter().any(|m| m.eq_ignore_ascii_case(name))
    }
    /// The map open in the scenario tool.
    pub fn loaded_map(&self) -> Option<&str> {
        self.loaded.as_deref()
    }
    pub fn saved_path(&self) -> Option<&Path> {
        self.saved_path.as_deref()
    }
    /// Any tool has unsaved edits.
    pub fn is_dirty(&self) -> bool {
        ToolKind::ALL.into_iter().any(|t| self.tool_dirty(t))
    }
    /// `tool` has unsaved edits.
    pub fn tool_dirty(&self, tool: ToolKind) -> bool {
        match tool {
            ToolKind::Scenario => self.session.is_dirty(),
            ToolKind::Variants => self.variants.as_ref().is_some_and(|e| e.is_dirty()),
            ToolKind::Playlists => self.hoppers.is_dirty(),
            ToolKind::Hud => self.hud.is_dirty(),
            // The tag editor's edits are the map's: the scenario session
            // saves them and its dirty mark counts them.
            ToolKind::UiScreens | ToolKind::Tags => false,
        }
    }
    /// The action waiting on the unsaved-changes prompt, if any.
    pub fn pending(&self) -> Option<&Action> {
        self.pending.as_ref().map(|(a, _)| a)
    }
    /// The tool whose edits the waiting action would drop.
    pub fn pending_tool(&self) -> Option<ToolKind> {
        self.pending.as_ref().map(|(_, t)| *t)
    }
    /// Where the open variant came from.
    pub fn variant_origin(&self) -> Option<&Origin> {
        self.variants.as_ref().and_then(|e| e.origin())
    }
    /// The active tool can undo or redo.
    pub fn can_undo(&self) -> bool {
        match self.tool {
            ToolKind::Scenario => self.session.can_undo(),
            ToolKind::Variants => self.variants.as_ref().is_some_and(|e| e.can_undo()),
            ToolKind::Playlists => self.hoppers.can_undo(),
            ToolKind::UiScreens => false,
            ToolKind::Hud => self.hud.session.as_ref().is_some_and(|s| s.can_undo()),
            ToolKind::Tags => self.tags.can_undo(),
        }
    }
    pub fn can_redo(&self) -> bool {
        match self.tool {
            ToolKind::Scenario => self.session.can_redo(),
            ToolKind::Variants => self.variants.as_ref().is_some_and(|e| e.can_redo()),
            ToolKind::Playlists => self.hoppers.can_redo(),
            ToolKind::UiScreens => false,
            ToolKind::Hud => self.hud.session.as_ref().is_some_and(|s| s.can_redo()),
            ToolKind::Tags => self.tags.can_redo(),
        }
    }
    pub fn should_quit(&self) -> bool {
        self.quit
    }
    pub fn log(&self) -> &[LogLine] {
        &self.log
    }
    /// The latest log line, for the status bar.
    pub fn status(&self) -> Option<&Status> {
        self.log.last().map(|l| &l.status)
    }

    /// The window title: what the active tool edits, a dirty mark, the
    /// app.
    pub fn title(&self) -> String {
        let what = match self.tool {
            ToolKind::Scenario => self.loaded.clone(),
            ToolKind::Variants => self
                .variants
                .as_ref()
                .and_then(|e| e.current())
                .map(|v| variants::panel::strip_icon(&v.name())),
            ToolKind::Playlists => self.hoppers.is_open().then(|| {
                self.hoppers
                    .path()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "untitled playlists".to_owned())
            }),
            ToolKind::UiScreens => self.ui_screens.pick.as_ref().map(|p| match p {
                Pick::Native { screen, state } => format!("{screen} / {state}"),
                Pick::Retail(tag) => tag.clone(),
            }),
            ToolKind::Hud => self.hud.session.as_ref().map(|s| {
                let hud = s.hud().map(|h| h.name.clone()).unwrap_or_default();
                format!("{} / {hud}", s.name)
            }),
            ToolKind::Tags => self.shown_map_name().map(str::to_owned),
        };
        // A browsed map is never dirty: the open map's mark is its own.
        let dirty = self.tool_dirty(self.tool)
            || (self.tool == ToolKind::Tags && self.browsing().is_none() && self.session.is_dirty());
        match what {
            Some(name) => format!("{name}{} - h2-editor", if dirty { " *" } else { "" }),
            None => "h2-editor".to_owned(),
        }
    }

    pub fn info(&mut self, msg: impl Into<String>) {
        self.push(Status::Info(msg.into()));
    }
    pub fn error(&mut self, msg: impl Into<String>) {
        self.push(Status::Error(msg.into()));
    }
    fn push(&mut self, status: Status) {
        self.logged += 1;
        self.log.push(LogLine {
            status,
            at: self.started.elapsed().as_secs_f64(),
        });
        if self.log.len() > LOG_LIMIT {
            let extra = self.log.len() - LOG_LIMIT;
            self.log.drain(..extra);
        }
    }
    /// Logs an error; the caller logs success its own way.
    fn ok_or_log<T>(&mut self, r: scenario_edit::Result<T>) -> Option<T> {
        r.map_err(|e| self.error(e.to_string())).ok()
    }
    /// Logs the session's status line on success, the error otherwise.
    fn session_result<T>(&mut self, r: scenario_edit::Result<T>) -> Option<T> {
        match r {
            Ok(v) => {
                let msg = self.session.status().to_owned();
                self.info(msg);
                Some(v)
            }
            Err(e) => {
                self.error(e.to_string());
                None
            }
        }
    }

    /// Run an action. One that would drop unsaved edits is held instead,
    /// and [`Self::pending`] asks the user ([`Self::resolve`]).
    pub fn dispatch(&mut self, action: Action) {
        if let Some(tool) = action.discards(self) {
            self.pending = Some((action, tool));
            return;
        }
        self.run(action);
    }

    /// Answer the unsaved-changes prompt. With the edits saved or
    /// discarded the action runs, or asks again about the next tool with
    /// unsaved edits (quitting).
    pub fn resolve(&mut self, answer: Answer) {
        let Some((action, tool)) = self.pending.take() else {
            return;
        };
        match answer {
            Answer::Cancel => return,
            Answer::Discard => self.close_tool(tool),
            Answer::Save => match tool {
                ToolKind::Scenario => self.error("save the map with Save as..."),
                ToolKind::Variants => self.run(Action::SaveVariant),
                ToolKind::Playlists => self.run(Action::SaveHopper),
                ToolKind::Hud => self.error("save the HUD edits with Save as..."),
                ToolKind::UiScreens | ToolKind::Tags => {}
            },
            Answer::SaveAs(path) => match tool {
                ToolKind::Scenario => self.run(Action::SaveMapAs(path)),
                ToolKind::Playlists => self.run(Action::SaveHopperAs(path)),
                ToolKind::Variants => self.run(Action::SaveVariant),
                ToolKind::Hud => self.run(Action::HudSaveAs(path)),
                ToolKind::UiScreens | ToolKind::Tags => {}
            },
        }
        if self.tool_dirty(tool) {
            // The save failed (the log says why): ask again.
            self.pending = Some((action, tool));
        } else {
            self.dispatch(action);
        }
    }

    /// Drop a tool's open document and its edits.
    pub(crate) fn close_tool(&mut self, tool: ToolKind) {
        match tool {
            ToolKind::Scenario => {
                self.session.close();
                self.loaded = None;
                self.forget_library_state();
            }
            ToolKind::Variants => {
                if let Some(e) = &mut self.variants {
                    e.close();
                }
            }
            ToolKind::Playlists => self.hoppers.close(),
            ToolKind::Hud => self.hud = HudTool::default(),
            ToolKind::UiScreens | ToolKind::Tags => {}
        }
    }

    fn run(&mut self, action: Action) {
        match action {
            Action::OpenSource(path) => self.open_source(&path),
            // Re-picking the open map keeps it (and its edits).
            Action::SelectMap(name)
                if self.loaded.as_deref() == Some(name.as_str())
                    && self.opened_package.is_none() => {}
            Action::SelectMap(name) => self.select_map(&name),
            Action::SaveMapAs(path) => {
                if let Some(why) = self.save_refusal(&path) {
                    self.error(why);
                    return;
                }
                let r = self.session.save_as(&path);
                if self.session_result(r).is_some() {
                    self.saved_path = Some(path);
                }
            }
            Action::SavePackage(form) => {
                let r = self.session.save_package(&form);
                if self.session_result(r).is_some() {
                    self.package_form = None;
                    self.refresh_packages();
                    if self.packages.iter().any(|e| e.key() == form.dir) {
                        self.opened_package = Some(form.dir.clone());
                    }
                }
            }
            Action::OpenPackageEntry(key) => self.open_package_entry(&key),
            Action::OpenMapPack(key) => self.open_pack_map(&key),
            Action::SelectHalo1Map(name) => self.select_halo1_map(&name),
            Action::SetOverlay(on) => self.set_overlay(on),
            Action::ConvertHalo1(sandbox) => self.start_convert(sandbox),
            Action::SetConvertSandbox(sandbox) => {
                self.convert_sandbox = sandbox;
                self.convert_error = None;
            }
            Action::OpenPackage(dir) => {
                let r = self.session.open_package(&dir);
                self.session_result(r);
            }
            Action::Undo => self.undo_redo(self.tool, true),
            Action::Redo => self.undo_redo(self.tool, false),
            Action::UndoIn(tool) => self.undo_redo(tool, true),
            Action::RedoIn(tool) => self.undo_redo(tool, false),
            Action::SetField {
                block,
                index,
                field,
                value,
                merge,
            } => self.set_field(block, index, &field, value, merge),
            Action::EditSelection { field, edit, merge } => {
                if let Err(e) = self.session.edit_selection(&field, edit, merge) {
                    self.error(e.to_string());
                }
            }
            Action::SealEdit => self.session.seal(),
            Action::GestureSet { position, ypr } => {
                if let Err(e) = self.session.gesture_set(position, ypr) {
                    self.error(e.to_string());
                }
            }
            Action::GestureScale(scale) => {
                if let Err(e) = self.session.gesture_scale(scale) {
                    self.error(e.to_string());
                }
            }
            Action::EndGesture => self.session.end_gesture(),
            Action::MovePlacement {
                block,
                index,
                position,
                ypr,
            } => self.move_placement(block, index, position, ypr),
            Action::EditPlaylist(edit) => self.edit_playlist(edit),
            Action::Select(sel) => self.session.select(sel),
            Action::ToggleSelect(e) => self.session.toggle_select(e),
            Action::SelectMany(all) => self.session.select_many(all),
            Action::PlaceAt(kind, like, at) => self.place_at(kind, like, at),
            Action::Delete if self.session.selection().len() > 1 => self.delete_selection(),
            Action::Duplicate if self.session.selection().len() > 1 => self.duplicate_selection(),
            Action::Delete => match self.session.selected() {
                Some((kind, i)) => self.run(Action::DeleteEntry(kind, i)),
                None => self.error("select a placement to delete"),
            },
            Action::Duplicate => match self.session.selected() {
                Some((kind, i)) => self.run(Action::DuplicateEntry(kind, i)),
                None => self.error("select a placement to duplicate"),
            },
            Action::DeleteEntry(kind, i) => {
                let r = self.session.remove_entry(kind, i);
                if self.ok_or_log(r).is_some() {
                    self.info(format!("deleted {kind:?} {i}"));
                }
            }
            Action::DuplicateEntry(kind, i) => {
                let r = self.session.add_entry(kind, Some(i));
                if let Some(n) = self.ok_or_log(r) {
                    self.info(format!("duplicated {kind:?} {i} as {n}"));
                }
            }
            Action::Add(kind) => {
                let r = self.session.add_entry(kind, None);
                if let Some(n) = self.ok_or_log(r) {
                    self.info(format!("added {kind:?} {n}"));
                }
            }
            Action::FrameAll => self.session.frame_all(),
            Action::FocusSelected => self.frame_selection(),
            Action::SetAllowGrowth(allow) => {
                self.session.set_allow_growth(allow);
                self.tags.allow_growth = allow;
            }
            Action::SetTool(tool) => self.set_tool(tool),
            Action::Quit => self.quit = true,
            Action::UiPick(pick) => {
                self.sync_ui_disc();
                if let Err(e) = self.ui_screens.select(pick) {
                    self.error(format!("{e:#}"));
                }
            }
            Action::UiPress(input) => {
                if let Some(line) = self.ui_screens.press(input) {
                    self.info(line);
                }
            }
            Action::UiExport(dir) => match self.ui_screens.export(&dir) {
                Ok(files) => self.info(format!("wrote {} files to {}", files.len(), dir.display())),
                Err(e) => self.error(format!("{e:#}")),
            },
            Action::HudOpenMap(name) => self.hud_open_map(&name),
            Action::HudOpenPackage(key) => self.hud_open_package(&key),
            Action::HudPick(id) => {
                let r = self.hud.pick(id);
                self.hud_result(r);
            }
            Action::HudWith(id, on) => {
                let r = match self.hud.session.as_mut() {
                    Some(s) => s.set_with(id, on),
                    None => Err(anyhow::anyhow!("no HUD open")),
                };
                self.hud_result(r);
            }
            Action::HudSelect(key) => self.hud.selected = key,
            Action::HudSet(key, field, v, merge) => self.hud_set(key, &field, v, merge),
            Action::HudMove(key, to, merge) => {
                let mode = self.hud.state.mode;
                self.hud_move(key, mode, to, merge);
            }
            Action::HudState(st) => {
                self.hud.state = *st;
                self.hud.redraw();
            }
            Action::HudSaveAs(path) => self.hud_save_as(&path),
            Action::HudWrite(w) => {
                let r = match self.hud.session.as_mut() {
                    Some(s) => s.write(w.address, &w.bytes, &w.label, w.merge).map(drop),
                    None => Err(anyhow::anyhow!("no HUD open")),
                };
                self.hud_result(r);
            }
            Action::HudRefused(why) => self.error(why),
            Action::Tag(a) => self.run_tag(a),
            Action::InspectorTag(TagAction::Select(tag)) => {
                self.browse_tags(None);
                self.set_tool(ToolKind::Tags);
                self.run_map_tag(TagAction::Select(tag));
            }
            Action::InspectorTag(action) => {
                self.sync_tags();
                self.run_map_tag(action);
            }
            Action::BrowseSharedTag(tag) => self.browse_shared_tag(tag),
            Action::BrowseTags(name) => self.browse_tags(name.as_deref()),
            Action::TagPreview(id) => self.preview_tag(id),
            Action::TagExport { tags, out, options } => {
                self.last_tag_export = None;
                match self.export_tags(&tags, out.as_deref(), &options) {
                    Ok((path, report)) => {
                        let mut msg = format!(
                            "exported {} to {}",
                            match report.tags.as_slice() {
                                [one] => one.clone(),
                                many => format!("{} tags", many.len()),
                            },
                            path.display()
                        );
                        if report.blobs > 0 {
                            msg += &format!(
                                " with {} resources ({} bytes)",
                                report.blobs, report.blob_bytes
                            );
                        }
                        if !report.left_out.is_empty() {
                            msg += &format!("; left out: {}", report.left_out.join("; "));
                        }
                        self.info(msg);
                        self.last_tag_export = Some((path, report));
                    }
                    Err(e) => self.error(format!("export: {e}")),
                }
            }
            Action::TagImport { path, options } => {
                self.last_tag_import = None;
                match self.import_tags(&path, &options) {
                    Ok(report) => {
                        self.push_tag_patches();
                        let mut msg = format!("imported {}", report.replaced.join(", "));
                        if !report.skipped.is_empty() {
                            msg += &format!("; skipped (not in this map): {}", report.skipped.join(", "));
                        }
                        if report.raw_kept > 0 {
                            msg += &format!(
                                "; {} resource pointers kept this map's (the bundle's are another map's)",
                                report.raw_kept
                            );
                        }
                        if !report.nulled.is_empty() {
                            msg += &format!(
                                "; references to tags the bundle's map lacked set to none: {}",
                                report.nulled.join(", ")
                            );
                        }
                        self.info(msg);
                        self.last_tag_import = Some(report);
                    }
                    Err(e) => self.error(format!("import: {e}")),
                }
            }
            a @ (Action::OpenVariant(_)
            | Action::NewVariant(_)
            | Action::CopyBuiltin(_)
            | Action::SetVariantName(_)
            | Action::SetSetting(..)
            | Action::SetVariantExt(_)
            | Action::SaveVariant
            | Action::RevertVariant
            | Action::DeleteVariant
            | Action::ExportVariant(_)
            | Action::ImportVariantFile(_)
            | Action::ImportVariantText(_)) => self.run_variant(a),
            a => self.run_playlist(a),
        }
    }

    fn undo_redo(&mut self, tool: ToolKind, undo: bool) {
        let done = match tool {
            ToolKind::Scenario => {
                let done = if undo {
                    self.session.undo()
                } else {
                    self.session.redo()
                };
                if done {
                    let msg = self.session.status().to_owned();
                    self.info(msg);
                }
                done
            }
            ToolKind::Variants => self
                .variants
                .as_mut()
                .is_some_and(|e| if undo { e.undo() } else { e.redo() }),
            ToolKind::Playlists => {
                if undo {
                    self.hoppers.undo()
                } else {
                    self.hoppers.redo()
                }
            }
            ToolKind::UiScreens => false,
            ToolKind::Hud => self.hud_undo_redo(undo),
            ToolKind::Tags => {
                self.sync_tags();
                let r = if undo {
                    self.tags.undo()
                } else {
                    self.tags.redo()
                };
                self.push_tag_patches();
                if let Some(label) = &r {
                    let msg = format!("{} {label}", if undo { "undid" } else { "redid" });
                    self.info(msg);
                }
                r.is_some()
            }
        };
        match (done, undo) {
            (false, true) => self.info("nothing to undo"),
            (false, false) => self.info("nothing to redo"),
            (true, _) if !matches!(tool, ToolKind::Scenario | ToolKind::Tags | ToolKind::Hud) => {
                self.info(if undo { "undone" } else { "redone" })
            }
            _ => {}
        }
    }

    fn set_tool(&mut self, tool: ToolKind) {
        self.tool = tool;
        match tool {
            ToolKind::Scenario => {}
            ToolKind::Variants => self.ensure_variants(),
            ToolKind::Playlists => self.refresh_playlist_sources(),
            ToolKind::UiScreens => self.sync_ui_disc(),
            ToolKind::Hud => {}
            ToolKind::Tags => self.sync_tags(),
        }
    }

    /// The scenario session owns placement selection, including multi-select.
    pub fn selection(&self) -> Option<Selection> {
        self.session.selected().map(|(kind, index)| Selection::Placement(kind, index))
    }

    /// Load the tag editor's document from the open map when the session
    /// opened another one since (its edits went with that map).
    pub fn sync_tags(&mut self) {
        self.tags.allow_growth = self.session.allow_growth();
        let rev = Some(self.session.world_rev());
        if self.tags.for_rev == rev && self.tags.is_open() == self.session.is_open() {
            return;
        }
        self.tags.close();
        self.tags.for_rev = rev;
        let Some(source) = self.session.source().cloned() else {
            return;
        };
        if !self.session.is_open() {
            return;
        }
        let r = source
            .open()
            .map_err(|e| e.to_string())
            .and_then(|mut m| self.tags.load(&mut m).map_err(|e| e.to_string()));
        match r {
            Ok(()) => self.mark_elsewhere(),
            Err(e) => {
                self.tags.error = Some(e.clone());
                self.error(format!("tags: {e}"));
            }
        }
        self.tag_preview = None;
    }

    /// Hand the tag editor's changed runs to the session, which saves them.
    fn push_tag_patches(&mut self) {
        if !self.tags.is_open() {
            return;
        }
        let r = self.session.set_tag_patches(self.tags.patches());
        self.ok_or_log(r);
        let r = self.session.set_tag_grown(self.tags.grown());
        self.ok_or_log(r);
        let r = self
            .session
            .set_tag_strings(self.tags.new_strings().to_vec());
        self.ok_or_log(r);
    }

    fn run_tag(&mut self, action: TagAction) {
        self.sync_tags();
        if let Some(b) = self.tag_browse.as_mut() {
            // A browsed map's tags are shown, not edited.
            let r = match action {
                TagAction::Select(id) if b.doc.tag(id).is_some() => {
                    b.doc.selected = Some(id);
                    Ok(())
                }
                TagAction::Select(id) => Err(format!("no tag {:#010x} in {}", id.0, b.map)),
                TagAction::Seal => Ok(()),
                _ => Err(tag_browse::BROWSING.to_owned()),
            };
            if let Err(e) = r {
                self.error(e);
            }
            return;
        }
        self.run_map_tag(action);
    }

    fn run_map_tag(&mut self, action: TagAction) {
        let r = match action {
            TagAction::Select(id) => {
                if self.tags.tag(id).is_some() {
                    self.tags.selected = Some(id);
                    Ok(())
                } else {
                    Err(format!("no tag {:#010x} in this map", id.0))
                }
            }
            TagAction::Write(w) => self.tags.write(w),
            TagAction::Block {
                tag,
                pair,
                layout,
                op,
            } => self.tags.block_op(tag, pair, &layout, &op),
            TagAction::SetString {
                tag,
                address,
                text,
                label,
            } => self.tags.set_string(tag, address, &text, &label),
            TagAction::Seal => {
                self.tags.seal();
                Ok(())
            }
        };
        match r {
            Ok(()) => self.push_tag_patches(),
            Err(e) => self.error(e),
        }
    }

    /// The UI Screens tool draws from the open source when it is a disc
    /// image, else from the synthetic source.
    pub fn sync_ui_disc(&mut self) {
        let disc = match &self.source {
            Some(MapSource::Disc(_)) => self.source.as_ref().map(|s| s.path().to_path_buf()),
            _ => None,
        };
        self.ui_screens.set_disc(disc.as_deref());
    }

    /// Build the variant editor for the source when there is none, or the
    /// source changed and the open variant has no unsaved edits.
    pub fn ensure_variants(&mut self) {
        let now = self.source.as_ref().map(|s| s.path().to_owned());
        let stale = self.variants_for.as_ref() != Some(&now);
        let dirty = self.tool_dirty(ToolKind::Variants);
        if self.variants.is_some() && !(stale && !dirty) {
            return;
        }
        let (editor, note) =
            variant_sources::editor(self.source.as_ref(), self.data_root.as_deref());
        self.variants = Some(editor);
        self.variants_for = Some(now);
        if let Some(note) = note {
            self.info(note);
        }
        self.variant_status();
    }

    /// Log the variant editor's last message.
    fn variant_status(&mut self) {
        if let Some((err, msg)) = self.variants.as_mut().and_then(|e| e.take_status()) {
            if err { self.error(msg) } else { self.info(msg) }
        }
    }

    fn run_variant(&mut self, action: Action) {
        self.ensure_variants();
        let e = self.variants.as_mut().expect("built");
        let r: Result<Option<String>, String> = match action {
            Action::OpenVariant(o) => {
                e.open_origin(o);
                Ok(e.current()
                    .map(|v| format!("opened {}", variants::panel::strip_icon(&v.name()))))
            }
            Action::NewVariant(ty) => {
                e.new_variant(ty);
                Ok(Some(format!("new {} variant", ty.label())))
            }
            Action::CopyBuiltin(name) => e
                .copy_builtin(&name)
                .map(|()| Some(format!("copied as {name:?}: save it to keep it")))
                .map_err(|e| e.to_string()),
            Action::SetVariantName(name) => {
                e.set_name(&name).map(|()| None).map_err(|e| e.to_string())
            }
            Action::SetSetting(id, v) => e
                .set_setting(id, v)
                .map(|()| None)
                .map_err(|e| e.to_string()),
            Action::SetVariantExt(x) => e.set_ext(&x).map(|()| None).map_err(|e| e.to_string()),
            Action::SaveVariant => e.save().map(|()| None).map_err(|e| e.to_string()),
            Action::RevertVariant => {
                e.revert();
                Ok(Some("reverted".into()))
            }
            Action::DeleteVariant => e
                .delete()
                .map(|()| Some("deleted the saved variant".into()))
                .map_err(|e| e.to_string()),
            Action::ExportVariant(path) => match e.export_current() {
                None => Err("no variant is open".into()),
                Some(text) => std::fs::write(&path, text)
                    .map(|()| Some(format!("exported {}", path.display())))
                    .map_err(|err| format!("{}: {err}", path.display())),
            },
            Action::ImportVariantFile(path) => std::fs::read_to_string(&path)
                .map_err(|err| format!("{}: {err}", path.display()))
                .and_then(|text| e.import(&text).map_err(|err| err.to_string()))
                .map(|()| Some(format!("imported {}", path.display()))),
            Action::ImportVariantText(text) => e
                .import(&text)
                .map(|()| Some("imported the pasted variant".into()))
                .map_err(|e| e.to_string()),
            _ => unreachable!("not a variant action"),
        };
        match r {
            Ok(Some(msg)) => self.info(msg),
            Ok(None) => {}
            Err(msg) => self.error(msg),
        }
        self.variant_status();
    }

    /// Read the playlist tool's map list and variant choices for the
    /// source (each only when the source changed).
    pub fn refresh_playlist_sources(&mut self) {
        self.hoppers.refresh_catalog(self.source.as_ref());
        self.hoppers.refresh_variants(self.source.as_ref());
    }

    fn run_playlist(&mut self, action: Action) {
        match action {
            Action::OpenHopper(path) => match self.hoppers.open_path(&path) {
                Ok(format) => {
                    self.hopper_selected = 0;
                    self.hopper_preset = None;
                    self.tool = ToolKind::Playlists;
                    self.refresh_playlist_sources();
                    self.info(format!("opened {} ({})", path.display(), format.describe()));
                }
                Err(e) => self.error(format!("{}: {e}", path.display())),
            },
            Action::NewHopper => {
                self.hoppers.new_blank();
                self.hopper_selected = 0;
                self.tool = ToolKind::Playlists;
                self.hopper_preset = None;
                self.refresh_playlist_sources();
                self.info("new playlist file: every playlist empty");
            }
            Action::NewDefaultHopper => {
                self.hoppers.new_default_set();
                self.hopper_selected = 0;
                self.tool = ToolKind::Playlists;
                self.hopper_preset = None;
                self.refresh_playlist_sources();
                self.info("new playlist file from the Live server's built-in lineup");
            }
            Action::SaveHopper => match self.hoppers.path().map(Path::to_owned) {
                Some(path) => self.run_playlist(Action::SaveHopperAs(path)),
                None => self.error("the playlist file has no path yet: use Save as..."),
            },
            Action::SaveHopperAs(path) => match self.hoppers.save_to(&path) {
                Ok(n) => self.info(format!("saved {} ({n} bytes)", path.display())),
                Err(e) => self.error(e.to_string()),
            },
            Action::SetEntryVariant(h, e, v) => match self.hoppers.set_entry_variant(h, e, &v) {
                Ok(()) => self.info(format!(
                    "playlist {} entry {}: {}",
                    h + 1,
                    e + 1,
                    hopper_edit::choices::variant_label(&v)
                )),
                Err(err) => self.error(format!("entry {}: {err}", e + 1)),
            },
            Action::EditEntryVariant(h, e) => {
                let Some(v) = self.hoppers.entry_variant(h, e) else {
                    self.error("this entry's game variant cannot be read");
                    return;
                };
                self.ensure_variants();
                let editor = self.variants.as_mut().expect("built");
                match editor.import(&variants::portable::export(&v)) {
                    Ok(()) => {
                        self.tool = ToolKind::Variants;
                        self.info(format!("editing a copy of {:?}", v.name()));
                    }
                    Err(err) => self.error(format!("cannot open the variant: {err}")),
                }
                self.variant_status();
            }
            Action::LoadVariantFile(path) => {
                self.refresh_playlist_sources();
                match self.hoppers.load_variant_file(&path) {
                    Ok(label) => {
                        self.info(format!("loaded {label}: pick it from an entry's variant"))
                    }
                    Err(e) => self.error(e),
                }
            }
            Action::RefreshPickerVariants => {
                self.hoppers.refresh_saved_variants();
                self.info("re-read the saved variants");
            }
            other => unreachable!("not a playlist action: {other:?}"),
        }
    }

    fn open_source(&mut self, path: &Path) {
        // The Map Pack is told by its contents. It joins the open source
        // rather than replacing it: its maps need the Halo 2 disc's caches.
        if self.try_open_mappack(path) {
            return;
        }
        if self.try_open_halo1(path) {
            return;
        }
        match MapSource::open(path) {
            Ok(source) => {
                self.maps = source.map_names();
                self.info(format!(
                    "opened {} ({} maps)",
                    path.display(),
                    self.maps.len()
                ));
                self.source = Some(source);
                self.tag_browse = None;
                self.read_halo2_cards(path.to_owned());
                self.session.close();
                self.loaded = None;
                self.forget_library_state();
                self.saved_path = None;
                // A loose map has nothing to pick between.
                if let (Some(MapSource::Loose(_)), [only]) = (&self.source, self.maps.as_slice()) {
                    let only = only.clone();
                    self.select_map(&only);
                }
                // The other tools read the new source when shown.
                match self.tool {
                    ToolKind::Variants => self.ensure_variants(),
                    ToolKind::Playlists => self.refresh_playlist_sources(),
                    ToolKind::UiScreens => self.sync_ui_disc(),
                    ToolKind::Scenario | ToolKind::Hud => {}
                    ToolKind::Tags => self.sync_tags(),
                }
            }
            Err(e) => self.error(format!("{}: {e:#}", path.display())),
        }
    }

    fn select_map(&mut self, name: &str) {
        if self.select_shared_cache(name) {
            return;
        }
        let located = if self.is_mappack_map(name) {
            // A Map Pack map: its own bytes, the Halo 2 source's caches.
            let shared = self.shared_caches();
            if shared.is_none() {
                self.info(format!(
                    "{name} needs the Halo 2 disc's shared maps for its textures and geometry: open the disc too"
                ));
            }
            self.pack_location(name).map(|located| {
                located.map(|(path, region)| EditSource::region(path, region).with_shared(shared))
            })
        } else {
            self.source.as_ref().map(|source| {
                source
                    .location(name)
                    .map(|(path, region)| EditSource::region(path, region))
            })
        };
        let Some(located) = located else {
            self.error("open a disc, map or folder first");
            return;
        };
        let opened = located
            .map_err(|e| e.to_string())
            .and_then(|s| self.session.open_named(s, name).map_err(|e| e.to_string()));
        match opened {
            Ok(()) => {
                self.forget_library_state();
                self.loaded = Some(name.to_owned());
                self.saved_path = None;
                self.tool = ToolKind::Scenario;
                self.tag_browse = None;
                let msg = format!("opened {name}: {}", self.session.status());
                self.info(msg);
            }
            Err(e) => {
                self.error(format!("{name}: {e}"));
                // Its tags may still read: show them in the Tags tool.
                if !self.is_mappack_map(name) && self.source.is_some() {
                    self.browse_tags(Some(name));
                    if self.browsing().is_some() {
                        self.tool = ToolKind::Tags;
                    }
                }
            }
        }
    }

    /// The default file name for "Save map as": the last save, else
    /// `<map>.map`.
    pub fn suggested_save_name(&self) -> String {
        if let Some(name) = self
            .saved_path
            .as_deref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
        {
            return name.to_owned();
        }
        format!("{}.map", self.loaded.as_deref().unwrap_or("untitled"))
    }

    /// Open the "Save as package" form with its defaults.
    pub fn open_package_form(&mut self) {
        if self.session.is_open() {
            self.package_form = Some(self.session.form_defaults());
            self.package_image.clear();
        } else {
            self.error("open a map first");
        }
    }
}

#[cfg(test)]
pub(crate) mod tag_tests;
#[cfg(test)]
pub(crate) mod map_cards_tests;
#[cfg(test)]
mod map_pack_tests;
#[cfg(test)]
mod tests;
