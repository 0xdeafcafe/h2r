//! The editor shell: the tool sidebar and map list, the top bar (Open, Save,
//! Save As, Undo, Redo), the status line, and the prompts (a path to open
//! or save, and unsaved changes before they would be lost).
//!
//! The decisions (what a request does given the tools' state, what each
//! prompt answer does next) are plain methods, tested without a window;
//! [`Shell::ui`] draws them.

use std::path::PathBuf;

use crate::source::MapSource;
use crate::tool::{Status, Tool, ToolContext, ToolError, suggested_path};

/// What waits on an unsaved-changes prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pending {
    /// Open this file in the active tool.
    Open(PathBuf),
    /// Close the editor.
    Quit,
}

/// What a path prompt is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathPurpose {
    /// A file for the active tool.
    Open,
    /// Save the active tool's file here.
    SaveAs,
    /// A disc image or loose `.map` as the map source.
    Source,
}

/// A modal question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prompt {
    /// Asks for a path; `then` runs after a successful Save As.
    Path {
        purpose: PathPurpose,
        text: String,
        then: Option<Pending>,
    },
    /// Unsaved changes would be lost.
    Unsaved { then: Pending },
}

/// An answer to [`Prompt::Unsaved`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Save,
    Discard,
    Cancel,
}

pub struct Shell {
    tools: Vec<Box<dyn Tool>>,
    active: usize,
    source: Option<MapSource>,
    selected_map: Option<String>,
    map_names: Vec<String>,
    render_state: Option<egui_wgpu::RenderState>,
    status: Option<Status>,
    prompt: Option<Prompt>,
    /// The user agreed to close; the next frame closes the window.
    quit: bool,
}

impl Default for Shell {
    fn default() -> Self {
        Self::new()
    }
}

impl Shell {
    pub fn new() -> Self {
        Shell {
            tools: Vec::new(),
            active: 0,
            source: None,
            selected_map: None,
            map_names: Vec::new(),
            render_state: None,
            status: None,
            prompt: None,
            quit: false,
        }
    }

    /// Adds a tool to the sidebar (in order).
    pub fn add_tool(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }

    pub fn tools(&self) -> &[Box<dyn Tool>] {
        &self.tools
    }

    pub fn active(&self) -> usize {
        self.active
    }

    /// Switches the central area to tool `i`. Tools keep their state while
    /// hidden, so nothing is lost and nothing is asked.
    pub fn select_tool(&mut self, i: usize) {
        if i < self.tools.len() {
            self.active = i;
        }
    }

    pub fn set_render_state(&mut self, rs: Option<egui_wgpu::RenderState>) {
        self.render_state = rs;
    }

    pub fn status(&self) -> Option<&Status> {
        self.status.as_ref()
    }

    pub fn prompt(&self) -> Option<&Prompt> {
        self.prompt.as_ref()
    }

    pub fn wants_quit(&self) -> bool {
        self.quit
    }

    pub fn source(&self) -> Option<&MapSource> {
        self.source.as_ref()
    }

    pub fn selected_map(&self) -> Option<&str> {
        self.selected_map.as_deref()
    }

    pub fn select_map(&mut self, name: Option<String>) {
        self.selected_map = name;
    }

    fn info(&mut self, msg: impl Into<String>) {
        self.status = Some(Status::Info(msg.into()));
    }

    fn error(&mut self, msg: impl Into<String>) {
        self.status = Some(Status::Error(msg.into()));
    }

    /// Opens a disc image or loose map as the source; on failure the old
    /// source stays.
    pub fn open_source(&mut self, path: PathBuf) {
        match MapSource::open(&path) {
            Ok(s) => self.set_source(s),
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    pub fn set_source(&mut self, source: MapSource) {
        self.map_names = source.map_names();
        self.info(format!(
            "opened {} ({} maps)",
            source.path().display(),
            self.map_names.len()
        ));
        self.selected_map = None;
        self.source = Some(source);
    }

    pub fn map_names(&self) -> &[String] {
        &self.map_names
    }

    /// Runs `f` on the active tool with a context.
    fn with_tool<R>(
        &mut self,
        f: impl FnOnce(&mut dyn Tool, &mut ToolContext<'_>) -> R,
    ) -> Option<R> {
        let tool = self.tools.get_mut(self.active)?;
        let mut cx = ToolContext {
            source: self.source.as_ref(),
            selected_map: self.selected_map.as_deref(),
            render_state: self.render_state.as_ref(),
            status: &mut self.status,
        };
        Some(f(tool.as_mut(), &mut cx))
    }

    fn active_dirty(&self) -> bool {
        self.tools.get(self.active).is_some_and(|t| t.is_dirty())
    }

    pub fn any_dirty(&self) -> bool {
        self.tools.iter().any(|t| t.is_dirty())
    }

    /// Open `path` in the active tool, asking first if that would drop
    /// unsaved changes.
    pub fn request_open(&mut self, path: PathBuf) {
        if self.active_dirty() {
            self.prompt = Some(Prompt::Unsaved {
                then: Pending::Open(path),
            });
        } else {
            self.open_now(path);
        }
    }

    fn open_now(&mut self, path: PathBuf) {
        let r = self.with_tool(|t, cx| t.open(&path, cx));
        if let Some(Err(e)) = r {
            self.error(format!("open {}: {e}", path.display()));
        }
    }

    /// The window wants to close: true when it may close now, else the
    /// unsaved-changes prompt is up.
    pub fn request_quit(&mut self) -> bool {
        if self.quit || !self.any_dirty() {
            self.quit = true;
            return true;
        }
        self.prompt = Some(Prompt::Unsaved {
            then: Pending::Quit,
        });
        false
    }

    /// Save the active tool; with no path yet, asks for one.
    pub fn save(&mut self) {
        match self.with_tool(|t, cx| t.save(cx)) {
            Some(Err(ToolError::NoPath)) => self.ask_save_as(None),
            Some(Err(e)) => self.error(format!("save: {e}")),
            _ => {}
        }
    }

    /// Asks where to save the active tool, then runs `then`.
    pub fn ask_save_as(&mut self, then: Option<Pending>) {
        let Some(tool) = self.tools.get(self.active) else {
            return;
        };
        self.prompt = Some(Prompt::Path {
            purpose: PathPurpose::SaveAs,
            text: suggested_path(tool.as_ref()).display().to_string(),
            then,
        });
    }

    /// Asks for a path for `purpose`.
    pub fn ask_path(&mut self, purpose: PathPurpose) {
        let text = match purpose {
            PathPurpose::Source => self
                .source
                .as_ref()
                .map(|s| s.path().display().to_string())
                .unwrap_or_default(),
            _ => self
                .tools
                .get(self.active)
                .and_then(|t| t.path())
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        };
        self.prompt = Some(Prompt::Path {
            purpose,
            text,
            then: None,
        });
    }

    /// Saves the active tool to `path`; true on success.
    pub fn save_as(&mut self, path: PathBuf) -> bool {
        match self.with_tool(|t, cx| t.save_as(&path, cx)) {
            Some(Ok(())) => true,
            Some(Err(e)) => {
                self.error(format!("save {}: {e}", path.display()));
                false
            }
            None => false,
        }
    }

    /// The path prompt's OK.
    pub fn submit_path(&mut self) {
        let Some(Prompt::Path {
            purpose,
            text,
            then,
        }) = self.prompt.take()
        else {
            return;
        };
        let path = PathBuf::from(text.trim());
        if path.as_os_str().is_empty() {
            return;
        }
        match purpose {
            PathPurpose::Open => self.request_open(path),
            PathPurpose::Source => self.open_source(path),
            PathPurpose::SaveAs => {
                if self.save_as(path)
                    && let Some(next) = then
                {
                    self.run(next);
                }
            }
        }
    }

    fn run(&mut self, p: Pending) {
        match p {
            Pending::Open(path) => self.open_now(path),
            Pending::Quit => self.quit = true,
        }
    }

    /// Answers the unsaved-changes prompt.
    pub fn answer(&mut self, a: Answer) {
        let Some(Prompt::Unsaved { then }) = self.prompt.take() else {
            return;
        };
        match a {
            Answer::Cancel => {}
            Answer::Discard => self.run(then),
            Answer::Save => match then {
                Pending::Open(_) => match self.with_tool(|t, cx| t.save(cx)) {
                    Some(Ok(())) => self.run(then),
                    Some(Err(ToolError::NoPath)) => self.ask_save_as(Some(then)),
                    Some(Err(e)) => self.error(format!("save: {e}")),
                    None => {}
                },
                Pending::Quit => {
                    // Save every dirty tool; stay open if any cannot.
                    let mut failed = Vec::new();
                    for i in 0..self.tools.len() {
                        if !self.tools[i].is_dirty() {
                            continue;
                        }
                        self.active = i;
                        match self.with_tool(|t, cx| t.save(cx)) {
                            Some(Ok(())) => {}
                            Some(Err(e)) => failed.push(format!("{}: {e}", self.tools[i].title())),
                            None => {}
                        }
                    }
                    if failed.is_empty() {
                        self.quit = true;
                    } else {
                        self.error(format!("not closed: {}", failed.join("; ")));
                    }
                }
            },
        }
    }

    pub fn cancel_prompt(&mut self) {
        self.prompt = None;
    }

    pub fn undo(&mut self) {
        if let Some(t) = self.tools.get_mut(self.active)
            && !t.undo()
        {
            self.info("nothing to undo");
        }
    }

    pub fn redo(&mut self) {
        if let Some(t) = self.tools.get_mut(self.active)
            && !t.redo()
        {
            self.info("nothing to redo");
        }
    }

    // ---------- drawing ----------

    /// Draws the whole window. Handles the close button (asks about
    /// unsaved changes) and the keyboard shortcuts.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if ctx.input(|i| i.viewport().close_requested()) && !self.request_quit() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        if self.prompt.is_none() {
            self.shortcuts(&ctx);
        }

        egui::Panel::top("shell_top").show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("shell_status").show(ui, |ui| self.status_line(ui));
        egui::Panel::left("shell_tools")
            .resizable(true)
            .default_size(220.0)
            .show(ui, |ui| self.sidebar(ui));
        egui::CentralPanel::default_margins().show(ui, |ui| {
            if self.tools.is_empty() {
                ui.label("No tools.");
                return;
            }
            self.with_tool(|t, cx| t.ui(ui, cx));
        });
        self.prompts(&ctx);

        if self.quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let cmd = Modifiers::COMMAND;
        let cmd_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        let hit = |m, k| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)));
        // Longer chords first: Cmd+Shift+Z must not read as Cmd+Z.
        if hit(cmd_shift, Key::Z) || hit(cmd, Key::Y) {
            self.redo();
        } else if hit(cmd, Key::Z) {
            self.undo();
        }
        if hit(cmd_shift, Key::S) {
            self.ask_save_as(None);
        } else if hit(cmd, Key::S) {
            self.save();
        }
        if hit(cmd, Key::O) {
            self.ask_path(PathPurpose::Open);
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let (can_undo, can_redo, path, dirty, exts) = match self.tools.get(self.active) {
            Some(t) => (
                t.can_undo(),
                t.can_redo(),
                t.path().map(|p| p.display().to_string()),
                t.is_dirty(),
                !t.extensions().is_empty(),
            ),
            None => (false, false, None, false, false),
        };
        ui.horizontal(|ui| {
            if ui.add_enabled(exts, egui::Button::new("Open...")).clicked() {
                self.ask_path(PathPurpose::Open);
            }
            if ui.add_enabled(exts, egui::Button::new("Save")).clicked() {
                self.save();
            }
            if ui
                .add_enabled(exts, egui::Button::new("Save As..."))
                .clicked()
            {
                self.ask_save_as(None);
            }
            ui.separator();
            if ui
                .add_enabled(can_undo, egui::Button::new("Undo"))
                .clicked()
            {
                self.undo();
            }
            if ui
                .add_enabled(can_redo, egui::Button::new("Redo"))
                .clicked()
            {
                self.redo();
            }
            ui.separator();
            if ui.button("Open disc or map...").clicked() {
                self.ask_path(PathPurpose::Source);
            }
            ui.separator();
            let file = path.unwrap_or_else(|| "(no file)".into());
            ui.label(if dirty { format!("{file} *") } else { file });
        });
    }

    fn status_line(&self, ui: &mut egui::Ui) {
        match &self.status {
            Some(Status::Info(m)) => ui.label(m),
            Some(Status::Error(m)) => ui.colored_label(ui.visuals().error_fg_color, m),
            None => ui.weak("ready"),
        };
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.heading("Tools");
        let mut pick = None;
        for (i, t) in self.tools.iter().enumerate() {
            let label = if t.is_dirty() {
                format!("{} *", t.title())
            } else {
                t.title().to_string()
            };
            if ui.selectable_label(i == self.active, label).clicked() {
                pick = Some(i);
            }
        }
        if let Some(i) = pick {
            self.select_tool(i);
        }
        ui.separator();
        ui.heading("Maps");
        match &self.source {
            None => {
                ui.weak("No disc open.");
                if ui.button("Open disc or map...").clicked() {
                    self.ask_path(PathPurpose::Source);
                }
            }
            Some(s) => {
                ui.small(s.path().display().to_string());
                let mut pick = None;
                egui::ScrollArea::vertical()
                    .id_salt("shell_maps")
                    .show(ui, |ui| {
                        for name in &self.map_names {
                            let on = self.selected_map.as_deref() == Some(name.as_str());
                            if ui.selectable_label(on, name).clicked() {
                                pick = Some(name.clone());
                            }
                        }
                    });
                if pick.is_some() {
                    self.selected_map = pick;
                }
            }
        }
    }

    fn prompts(&mut self, ctx: &egui::Context) {
        match &mut self.prompt {
            None => {}
            Some(Prompt::Path { purpose, text, .. }) => {
                let title = match purpose {
                    PathPurpose::Open => "Open file",
                    PathPurpose::SaveAs => "Save as",
                    PathPurpose::Source => "Open a disc image (.iso) or a loose .map",
                };
                let mut ok = false;
                let mut cancel = false;
                egui::Modal::new(egui::Id::unique("shell_path")).show(ctx, |ui| {
                    ui.heading(title);
                    let r = ui.add(egui::TextEdit::singleline(text).desired_width(480.0));
                    r.request_focus();
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        ok = true;
                    }
                    ui.horizontal(|ui| {
                        ok |= ui.button("OK").clicked();
                        cancel = ui.button("Cancel").clicked()
                            || ui.input(|i| i.key_pressed(egui::Key::Escape));
                    });
                });
                if ok {
                    self.submit_path();
                } else if cancel {
                    self.cancel_prompt();
                }
            }
            Some(Prompt::Unsaved { then }) => {
                let what = match then {
                    Pending::Open(_) => "Opening another file discards the unsaved changes.",
                    Pending::Quit => "Closing the editor discards the unsaved changes.",
                };
                let mut answer = None;
                egui::Modal::new(egui::Id::unique("shell_unsaved")).show(ctx, |ui| {
                    ui.heading("Unsaved changes");
                    ui.label(what);
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            answer = Some(Answer::Save);
                        }
                        if ui.button("Discard").clicked() {
                            answer = Some(Answer::Discard);
                        }
                        if ui.button("Cancel").clicked() {
                            answer = Some(Answer::Cancel);
                        }
                    });
                });
                if let Some(a) = answer {
                    self.answer(a);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
