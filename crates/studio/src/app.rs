//! The studio's state and commands, with no UI toolkit in sight: the
//! opened source and its maps, the active tool, the scenario session, the
//! log, and the unsaved-changes guard. The Dear ImGui layer (`ui`) turns
//! clicks and shortcuts into [`Action`]s and draws this state; tests drive
//! it directly.
use std::path::{Path, PathBuf};

use editor_core::{MapSource, Status};
use scenario_edit::{BlockKind, MapSource as EditSource, PackageForm, Session};

/// The log keeps at most this many lines.
pub const LOG_LIMIT: usize = 500;

/// The tools of the suite. Phase 1 ships the scenario tool; the others are
/// listed (disabled) so the shell's layout is final.
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
    /// Whether the tool is in this build (the others arrive in phase 2).
    pub fn available(self) -> bool {
        matches!(self, ToolKind::Scenario)
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
    /// Quit the app.
    Quit,
}

impl Action {
    /// Whether the action would drop the open map's unsaved edits.
    fn discards_edits(&self, app: &App) -> bool {
        match self {
            Action::OpenSource(_) | Action::Quit => true,
            Action::SelectMap(name) => app.loaded.as_deref() != Some(name.as_str()),
            _ => false,
        }
    }
}

/// The answer to the unsaved-changes prompt.
#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    /// Save the edited map here first, then go on.
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
    /// The action held behind the unsaved-changes prompt.
    pending: Option<Action>,
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
            log: Vec::new(),
            started: std::time::Instant::now(),
            quit: false,
            filter: String::new(),
            package_form: None,
            package_image: String::new(),
        }
    }

    /// Packages are saved to and listed from `<root>/maps`.
    pub fn set_data_root(&mut self, root: PathBuf) {
        self.session.set_data_root(root);
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
    pub fn is_dirty(&self) -> bool {
        self.session.is_dirty()
    }
    /// The action waiting on the unsaved-changes prompt, if any.
    pub fn pending(&self) -> Option<&Action> {
        self.pending.as_ref()
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

    /// The window title: the map, a dirty mark, the app.
    pub fn title(&self) -> String {
        match &self.loaded {
            Some(name) => format!(
                "{name}{} - h2-studio",
                if self.is_dirty() { " *" } else { "" }
            ),
            None => "h2-studio".to_owned(),
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
        if self.is_dirty() && action.discards_edits(self) {
            self.pending = Some(action);
            return;
        }
        self.run(action);
    }

    /// Answer the unsaved-changes prompt.
    pub fn resolve(&mut self, answer: Answer) {
        let Some(action) = self.pending.take() else {
            return;
        };
        match answer {
            Answer::Cancel => {}
            Answer::Discard => {
                self.session.close();
                self.loaded = None;
                self.run(action);
            }
            Answer::SaveAs(path) => {
                self.run(Action::SaveMapAs(path));
                if self.is_dirty() {
                    // The save failed (the log says why): ask again.
                    self.pending = Some(action);
                } else {
                    self.run(action);
                }
            }
        }
    }

    fn run(&mut self, action: Action) {
        match action {
            Action::OpenSource(path) => self.open_source(&path),
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
            Action::Undo => {
                if self.session.undo() {
                    let msg = self.session.status().to_owned();
                    self.info(msg);
                } else {
                    self.info("nothing to undo");
                }
            }
            Action::Redo => {
                if self.session.redo() {
                    let msg = self.session.status().to_owned();
                    self.info(msg);
                } else {
                    self.info("nothing to redo");
                }
            }
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
            Action::Quit => self.quit = true,
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
            .and_then(|s| {
                self.session
                    .open_named(s, name)
                    .map_err(|e| e.to_string())
            });
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
