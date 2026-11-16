//! The contract between the shell and a tool. Kept small on purpose: tool
//! crates expose a plain struct with a `ui` method, and a few lines of
//! adapter (docs/editor.md) make it a [`Tool`].

use std::path::{Path, PathBuf};

use crate::source::MapSource;

/// What a tool's file operations can fail with. The shell shows the message
/// in the status line.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    /// The tool has no file to save to yet: the shell asks for a path and
    /// calls [`Tool::save_as`].
    #[error("no file yet; choose where to save it")]
    NoPath,
    /// The tool does not open or save files this way.
    #[error("this tool does not support {0}")]
    Unsupported(&'static str),
    /// The edits do not validate; nothing was written.
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// A status-line message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Info(String),
    Error(String),
}

/// What a tool sees of the shell while it draws or saves.
pub struct ToolContext<'a> {
    /// The opened disc image or loose `.map`, if any.
    pub source: Option<&'a MapSource>,
    /// The map picked in the sidebar's map list (a name from
    /// [`MapSource::map_names`]), if any.
    pub selected_map: Option<&'a str>,
    /// eframe's wgpu device, queue and renderer, for 3D viewports (register
    /// paint callbacks or textures with it). `None` in headless tests.
    pub render_state: Option<&'a egui_wgpu::RenderState>,
    /// Set through [`ToolContext::info`] / [`ToolContext::error`].
    pub status: &'a mut Option<Status>,
}

impl ToolContext<'_> {
    /// Shows `msg` in the status line.
    pub fn info(&mut self, msg: impl Into<String>) {
        *self.status = Some(Status::Info(msg.into()));
    }

    /// Shows `msg` in the status line as an error.
    pub fn error(&mut self, msg: impl Into<String>) {
        *self.status = Some(Status::Error(msg.into()));
    }
}

/// Work one tool hands to another, through the shell: after drawing the
/// active tool, the shell takes its [`Tool::take_handoff`] and offers it
/// to each tool's [`Tool::receive`] in turn; the first to take it becomes
/// the active tool.
#[derive(Debug, Clone, PartialEq)]
pub enum Handoff {
    /// Open this game variant as a new, unsaved variant to edit.
    EditVariant(Box<variants::GameVariant>),
}

/// One tool of the suite. Only [`Tool::title`] and [`Tool::ui`] are
/// required; the rest default to "not supported".
pub trait Tool {
    /// The sidebar and window label.
    fn title(&self) -> &str;

    /// Draws the tool into the central area. Called every frame while the
    /// tool is open.
    fn ui(&mut self, ui: &mut egui::Ui, cx: &mut ToolContext<'_>);

    /// Unsaved changes? The shell prompts before discarding them.
    fn is_dirty(&self) -> bool {
        false
    }

    /// The file the tool is editing, if it has one (shown in the top bar;
    /// the default for Save As).
    fn path(&self) -> Option<&Path> {
        None
    }

    /// File extensions the tool opens and saves (without the dot), for the
    /// path prompt's hint. Empty: the tool has no files.
    fn extensions(&self) -> &[&str] {
        &[]
    }

    /// Opens `path`, replacing what the tool holds. The shell has already
    /// asked about unsaved changes.
    fn open(&mut self, path: &Path, cx: &mut ToolContext<'_>) -> Result<(), ToolError> {
        let _ = (path, cx);
        Err(ToolError::Unsupported("opening files"))
    }

    /// Saves to [`Tool::path`]; [`ToolError::NoPath`] makes the shell ask
    /// for one.
    fn save(&mut self, cx: &mut ToolContext<'_>) -> Result<(), ToolError> {
        let _ = cx;
        Err(ToolError::Unsupported("saving"))
    }

    /// Saves to `path`, which becomes the tool's file.
    fn save_as(&mut self, path: &Path, cx: &mut ToolContext<'_>) -> Result<(), ToolError> {
        let _ = (path, cx);
        Err(ToolError::Unsupported("saving"))
    }

    fn can_undo(&self) -> bool {
        false
    }

    fn can_redo(&self) -> bool {
        false
    }

    /// Steps back one edit; false when there is none.
    fn undo(&mut self) -> bool {
        false
    }

    /// Steps forward one undone edit; false when there is none.
    fn redo(&mut self) -> bool {
        false
    }

    /// Work for another tool, asked for while drawing ([`Handoff`]).
    fn take_handoff(&mut self) -> Option<Handoff> {
        None
    }

    /// Takes work from another tool. `Err` gives it back: not this tool's
    /// kind of work, or the tool cannot take it now (the reason goes to
    /// the status line).
    fn receive(&mut self, work: Handoff, cx: &mut ToolContext<'_>) -> Result<(), Handoff> {
        let _ = cx;
        Err(work)
    }
}

/// The default path for a Save As prompt: the tool's file, else
/// `untitled.<first extension>` in the working directory.
pub fn suggested_path(tool: &dyn Tool) -> PathBuf {
    if let Some(p) = tool.path() {
        return p.to_owned();
    }
    let ext = tool.extensions().first().copied().unwrap_or("bin");
    PathBuf::from(format!("untitled.{ext}"))
}
