//! The studio's state and commands, with no UI toolkit in sight: the
//! opened source and its maps, the active tool, each tool's session
//! (scenario, game variants, playlists), the log, and the unsaved-changes
//! guard. The Dear ImGui layer (`ui`) turns
//! clicks and shortcuts into [`Action`]s and draws this state; tests drive
//! it directly.
use std::path::{Path, PathBuf};

use editor_core::{MapSource, Status, variant_sources};
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
}

impl ToolKind {
    pub const ALL: [ToolKind; 3] = [ToolKind::Scenario, ToolKind::Variants, ToolKind::Playlists];
    pub fn title(self) -> &'static str {
        match self {
            ToolKind::Scenario => "Scenario",
            ToolKind::Variants => "Game variants",
            ToolKind::Playlists => "Playlists",
        }
    }
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
    Undo,
    Redo,
    Select(Option<(BlockKind, usize)>),
    /// Delete the selected placement.
    Delete,
    /// Duplicate the selected placement.
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
            Action::SelectMap(name) if app.loaded.as_deref() != Some(name.as_str()) => {
                dirty(ToolKind::Scenario)
            }
            Action::OpenVariant(o) if app.variant_origin() != Some(o) => dirty(ToolKind::Variants),
            Action::NewVariant(_)
            | Action::ImportVariantFile(_)
            | Action::ImportVariantText(_)
            | Action::EditEntryVariant(..) => dirty(ToolKind::Variants),
            Action::OpenHopper(_) | Action::NewHopper | Action::NewDefaultHopper => {
                dirty(ToolKind::Playlists)
            }
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
    started: std::time::Instant,
    quit: bool,
    /// The outliner's filter text.
    pub filter: String,
    /// The "Save as package" form while it is open.
    pub package_form: Option<PackageForm>,
    /// The image path typed into that form.
    pub package_image: String,
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
            started: std::time::Instant::now(),
            quit: false,
            filter: String::new(),
            package_form: None,
            package_image: String::new(),
        }
    }

    /// Packages are saved to and listed from `<root>/maps`, and variants
    /// to and from the save storage under it.
    pub fn set_data_root(&mut self, root: PathBuf) {
        self.session.set_data_root(root.clone());
        self.hoppers.set_data_root(root.clone());
        self.data_root = Some(root);
        self.variants_for = None;
    }

    pub fn source(&self) -> Option<&MapSource> {
        self.source.as_ref()
    }
    /// The maps of the opened source, in its order.
    pub fn maps(&self) -> &[String] {
        &self.maps
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
        }
    }
    pub fn can_redo(&self) -> bool {
        match self.tool {
            ToolKind::Scenario => self.session.can_redo(),
            ToolKind::Variants => self.variants.as_ref().is_some_and(|e| e.can_redo()),
            ToolKind::Playlists => self.hoppers.can_redo(),
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
        };
        match what {
            Some(name) => format!(
                "{name}{} - h2-editor",
                if self.tool_dirty(self.tool) { " *" } else { "" }
            ),
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
            },
            Answer::SaveAs(path) => match tool {
                ToolKind::Scenario => self.run(Action::SaveMapAs(path)),
                ToolKind::Playlists => self.run(Action::SaveHopperAs(path)),
                ToolKind::Variants => self.run(Action::SaveVariant),
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
    fn close_tool(&mut self, tool: ToolKind) {
        match tool {
            ToolKind::Scenario => {
                self.session.close();
                self.loaded = None;
            }
            ToolKind::Variants => {
                if let Some(e) = &mut self.variants {
                    e.close();
                }
            }
            ToolKind::Playlists => self.hoppers.close(),
        }
    }

    fn run(&mut self, action: Action) {
        match action {
            Action::OpenSource(path) => self.open_source(&path),
            // Re-picking the open map keeps it (and its edits).
            Action::SelectMap(name) if self.loaded.as_deref() == Some(name.as_str()) => {}
            Action::SelectMap(name) => self.select_map(&name),
            Action::SaveMapAs(path) => {
                let r = self.session.save_as(&path);
                if self.session_result(r).is_some() {
                    self.saved_path = Some(path);
                }
            }
            Action::SavePackage(form) => {
                let r = self.session.save_package(&form);
                if self.session_result(r).is_some() {
                    self.package_form = None;
                }
            }
            Action::OpenPackage(dir) => {
                let r = self.session.open_package(&dir);
                self.session_result(r);
            }
            Action::Undo => self.undo_redo(true),
            Action::Redo => self.undo_redo(false),
            Action::Select(sel) => self.session.select(sel),
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
            Action::FocusSelected => self.session.focus_selected(),
            Action::SetAllowGrowth(allow) => self.session.set_allow_growth(allow),
            Action::SetTool(tool) => self.set_tool(tool),
            Action::Quit => self.quit = true,
            a @ (Action::OpenVariant(_)
            | Action::NewVariant(_)
            | Action::CopyBuiltin(_)
            | Action::SetVariantName(_)
            | Action::SetSetting(..)
            | Action::SaveVariant
            | Action::RevertVariant
            | Action::DeleteVariant
            | Action::ExportVariant(_)
            | Action::ImportVariantFile(_)
            | Action::ImportVariantText(_)) => self.run_variant(a),
            a => self.run_playlist(a),
        }
    }

    fn undo_redo(&mut self, undo: bool) {
        let done = match self.tool {
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
        };
        match (done, undo) {
            (false, true) => self.info("nothing to undo"),
            (false, false) => self.info("nothing to redo"),
            (true, _) if self.tool != ToolKind::Scenario => {
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
        }
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
        match MapSource::open(path) {
            Ok(source) => {
                self.maps = source.map_names();
                self.info(format!(
                    "opened {} ({} maps)",
                    path.display(),
                    self.maps.len()
                ));
                self.source = Some(source);
                self.session.close();
                self.loaded = None;
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
                    ToolKind::Scenario => {}
                }
            }
            Err(e) => self.error(format!("{}: {e:#}", path.display())),
        }
    }

    fn select_map(&mut self, name: &str) {
        let Some(source) = &self.source else {
            self.error("open a disc, map or folder first");
            return;
        };
        let opened = source
            .location(name)
            .map(|(path, region)| EditSource::region(path, region))
            .map_err(|e| e.to_string())
            .and_then(|s| self.session.open_named(s, name).map_err(|e| e.to_string()));
        match opened {
            Ok(()) => {
                self.loaded = Some(name.to_owned());
                self.saved_path = None;
                self.tool = ToolKind::Scenario;
                let msg = format!("opened {name}: {}", self.session.status());
                self.info(msg);
            }
            Err(e) => self.error(format!("{name}: {e}")),
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
mod tests;
