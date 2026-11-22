//! The game type script editor (docs/editor/editor-guide.md, "Game type
//! scripts"): one `.rules` file open, its language intelligence from
//! `rules::ide` and its check report from `rules::workbench`, and its
//! reload into the paired game (`rules.reload`, docs/game/rules-engine.md,
//! "Reloading a running script").
//!
//! No toolkit here: `ui::game_script` draws it, the agent API's `rules.*`
//! (`api::game_script`) drives it, and tests read the pure helpers
//! ([`spans`], [`problems`], [`byte_at`], [`position_at`],
//! [`reload_params`], [`rank`], [`expand_snippet`], [`completion_choices`],
//! [`apply_text_edits`], [`rename_edit`], [`definition_at`],
//! [`occurrences`], [`hover_blocks`], and for "Open in editor"
//! [`script_file_name`], [`external_path`], [`editor_launches`],
//! [`launch_step`] and [`disk_change`]).
//!
//! Every change the editor makes to the text (a completion, a quick fix,
//! a rename, the formatter, an open) is an [`Edit`]: the whole new text
//! and where the cursor goes. The text widget owns its buffer while it
//! has the keyboard, so the window hands the edit to the widget's
//! callback rather than writing `text` under it.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use agent_proto::rules::{ReloadMode, ReloadResult};
use rules::ide::{self, Diagnostic, Position, Range, TextEdit, Token, TokenKind};
use rules::workbench::{self, Check};
use serde_json::{Value, json};

/// A new script's text.
pub const BLANK: &str = "rules \"New game type\" 1\ncarrier slayer\n\noption kill_points: int 1..5 = 1 \"Kill points\" \"Points for each kill\"\n\non kill(victim, killer) {\n    if killer != none and killer != victim {\n        score(killer, kill_points)\n    }\n}\n";

/// The file extension of a script.
pub const EXTENSION: &str = "rules";

/// The most completions the list holds.
pub const COMPLETIONS: usize = 50;

/// A tab in a snippet becomes this.
pub const INDENT: &str = "    ";

/// A change of the whole text, for the widget to take: the new text, the
/// cursor's byte, and a selection (a snippet's first placeholder) when
/// there is one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub text: String,
    pub cursor: usize,
    pub select: Option<(usize, usize)>,
}

impl Edit {
    pub fn to(text: String, cursor: usize) -> Edit {
        Edit {
            text,
            cursor,
            select: None,
        }
    }
}

/// What the list at the caret offers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MenuKind {
    #[default]
    Completions,
    Fixes,
}

/// One row of the list at the caret, with the edit taking it makes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub label: String,
    pub detail: String,
    /// The item's documentation, markdown.
    pub doc: String,
    pub edit: Edit,
}

/// The list at the caret: completions as you type (or on Ctrl+Space), or
/// the quick fixes at the cursor (Cmd+.).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Menu {
    pub kind: MenuKind,
    pub choices: Vec<Choice>,
    pub selected: usize,
    /// Opened by Ctrl+Space: stays open with nothing typed.
    pub manual: bool,
    /// The text and cursor the choices are for.
    text: String,
    cursor: usize,
}

/// A rename in progress (F2): the name's byte, the name as it is, and the
/// new name typed so far.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rename {
    pub at: usize,
    pub old: String,
    pub name: String,
    /// The field takes the keyboard when it next draws.
    pub focus: bool,
}

/// What a typed character does to the completions list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    Open,
    Close,
}

/// What the source gave: tokens, problems and the check report.
#[derive(Clone, Debug, Default)]
pub struct Analysis {
    pub tokens: Vec<Token>,
    pub diagnostics: Vec<Diagnostic>,
    pub check: Option<Check>,
}

/// Where the last reload into the game stands.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ReloadState {
    #[default]
    Idle,
    /// Sent; the game has not answered. The blob's hash.
    Sending(String),
    Done(ReloadResult),
    /// The editor or the game refused it, with the reason.
    Refused(String),
}

/// How a reload row reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Busy,
    Good,
    /// Done, but by a restart.
    Notice,
    Bad,
}

/// The editor's one script.
#[derive(Clone, Debug)]
pub struct RulesEditor {
    pub path: Option<PathBuf>,
    pub text: String,
    /// The text as last opened or saved.
    saved: String,
    /// What it was started from: a file name, a built-in, or "Untitled".
    pub title: String,
    /// The cursor's byte offset, as the text widget last reported it.
    pub cursor: usize,
    /// Where the widget should put the cursor next (a problem clicked, a
    /// definition).
    pub want_cursor: Option<usize>,
    /// A change for the widget to take ([`Edit`]).
    pub edit: Option<Edit>,
    /// The text the analysis is for.
    analysed: Option<String>,
    pub analysis: Analysis,
    /// Saving reloads the script into the paired game.
    pub reload_on_save: bool,
    pub mode: ReloadMode,
    pub reload: ReloadState,
    /// The reload waiting for the game link (`play` takes it).
    pub outbox: Option<Value>,
    /// The list at the caret, when open.
    pub menu: Option<Menu>,
    /// The rename field, when open.
    pub renaming: Option<Rename>,
    /// The last hover asked for: its byte and its text.
    pub hover: Option<(usize, Option<String>)>,
    /// The uses of the name at the cursor, for their highlight: the text
    /// and cursor they are for, and the spans.
    marks: Option<(String, usize, Vec<Mark>)>,
    /// The text widget was active last frame (`ui::game_script`).
    pub typing: bool,
    /// Bring the window to the front next frame (set by every open).
    pub show: bool,
    /// The text area the window last drew: x, y, width and height in
    /// points (`ui::game_script`). `None` before it is drawn.
    pub drawn: Option<[f32; 4]>,
    /// The file "Open in editor" handed to an outside editor, polled for
    /// its changes ([`RulesEditor::poll_disk`]).
    pub watch: Option<Watch>,
    /// The file's text when it changed on disk while the buffer had
    /// unsaved edits: the window asks which to keep.
    pub conflict: Option<String>,
    /// The outside editor's start, filled by its thread ([`launch`]).
    pub launched: Option<Launched>,
}

impl Default for RulesEditor {
    fn default() -> Self {
        Self {
            path: None,
            text: BLANK.to_string(),
            saved: BLANK.to_string(),
            title: "Untitled".into(),
            cursor: 0,
            want_cursor: None,
            edit: None,
            analysed: None,
            analysis: Analysis::default(),
            reload_on_save: true,
            mode: ReloadMode::Auto,
            reload: ReloadState::Idle,
            outbox: None,
            menu: None,
            renaming: None,
            hover: None,
            marks: None,
            typing: false,
            show: false,
            drawn: None,
            watch: None,
            conflict: None,
            launched: None,
        }
    }
}

/// The scripts to start from: every built-in, the listed variants, then
/// the examples, as `(label, source)`.
pub fn starters() -> Vec<(String, &'static str)> {
    let mut out: Vec<(String, &'static str)> = rules::builtin::ALL
        .iter()
        .chain(rules::builtin::LISTED)
        .map(|b| (format!("{} (built in)", b.name), b.source))
        .collect();
    out.extend(
        rules::builtin::examples()
            .iter()
            .map(|&(name, src)| (format!("{name} (example)"), src)),
    );
    out
}

/// A starter by its label or its name, any case.
pub fn find_starter(name: &str) -> Option<(String, &'static str)> {
    let want = name.trim().to_ascii_lowercase();
    starters().into_iter().find(|(label, _)| {
        let label = label.to_ascii_lowercase();
        label == want || label.split(" (").next() == Some(want.as_str())
    })
}

/// A script the editor can name by its hash: a starter, compiled once.
struct Known {
    ir: rules::Ir,
}

fn known() -> &'static [Known] {
    static KNOWN: std::sync::OnceLock<Vec<Known>> = std::sync::OnceLock::new();
    KNOWN.get_or_init(|| {
        starters()
            .into_iter()
            .filter_map(|(_, src)| rules::compile(src).ok().map(|ir| Known { ir }))
            .collect()
    })
}

/// The first 8 bytes of a script's hash as lowercase hex.
pub fn hex_prefix(prefix: [u8; 8]) -> String {
    prefix.iter().map(|b| format!("{b:02x}")).collect()
}

/// The script a record's script reference names, when this editor has it:
/// a built-in, a listed variant or an example, else the script open in the
/// editor (as it was last analysed).
pub fn find_script(prefix: [u8; 8], editor: &RulesEditor) -> Option<rules::Ir> {
    if let Some(k) = known().iter().find(|k| k.ir.id().0[..8] == prefix) {
        return Some(k.ir.clone());
    }
    let hash = editor.analysis.check.as_ref()?.hash.as_deref()?;
    if !hash.starts_with(&hex_prefix(prefix)) {
        return None;
    }
    rules::compile(&editor.text).ok()
}

/// What the script section of the editor lists, from the check report's
/// `settings` and `quick`: one line each.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScriptSection {
    /// "Label: value", in table order.
    pub settings: Vec<String>,
    /// "1. Label: value (setting)" or "(option)", in the script's order.
    pub quick: Vec<String>,
}

/// The section for a check, or `None` when the script names no setting
/// and marks nothing.
pub fn script_section(check: &Check) -> Option<ScriptSection> {
    let rep = check.report.as_ref()?;
    if rep.settings.is_empty() && rep.quick.is_empty() {
        return None;
    }
    Some(ScriptSection {
        settings: rep
            .settings
            .iter()
            .map(|s| format!("{}: {}", s.label, s.text))
            .collect(),
        quick: rep
            .quick
            .iter()
            .enumerate()
            .map(|(i, q)| {
                format!(
                    "{}. {}: {} ({})",
                    i + 1,
                    q.label,
                    q.value.as_deref().unwrap_or("the carrier's stock value"),
                    q.kind
                )
            })
            .collect(),
    })
}

/// The `.h2variant` of a script's own variant, as `h2 variants
/// from-script` writes it: the script shipped (compiled, verified, held
/// to the budget), then [`variants::portable::from_script`] with its
/// source attached and every option at its default. Returns the file
/// name the variant's name gives, and the file's text.
pub fn game_variant_file(source: &str) -> Result<(String, String), String> {
    let (ir, _) = rules::ship(source).map_err(|e| e.to_string())?;
    let bundle = variants::portable::from_script(&ir, Some(source), &[]).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(&bundle).map_err(|e| e.to_string())?;
    Ok((variants::portable::file_name(&bundle.name), text + "\n"))
}

impl RulesEditor {
    pub fn dirty(&self) -> bool {
        self.text != self.saved
    }

    fn replace(&mut self, text: String, path: Option<PathBuf>, title: String) {
        self.saved = text.clone();
        self.path = path;
        self.title = title;
        self.want_cursor = None;
        self.analysed = None;
        self.renaming = None;
        self.show = true;
        self.conflict = None;
        if self.watch.as_ref().is_some_and(|w| self.path.as_ref() != Some(&w.path)) {
            self.watch = None;
        }
        self.apply(Edit::to(text, 0));
    }

    /// Takes an edit: the text and cursor change now, and the widget
    /// takes the same edit when it next draws (it owns its buffer while
    /// it has the keyboard). Closes the list at the caret.
    pub fn apply(&mut self, edit: Edit) {
        self.text = edit.text.clone();
        self.cursor = edit.cursor.min(self.text.len());
        self.edit = Some(edit);
        self.menu = None;
    }

    /// Writes the script's own variant ([`game_variant_file`]) to `to`, a
    /// file path.
    pub fn save_game_variant(&self, to: &Path) -> Result<PathBuf, String> {
        let (_, text) = game_variant_file(&self.text)?;
        std::fs::write(to, text).map_err(|e| format!("{}: {e}", to.display()))?;
        Ok(to.to_path_buf())
    }

    /// Replaces the whole text (`rules.set`), keeping the cursor where it
    /// can. The widget takes it only while it has the keyboard: otherwise
    /// it reads `text` when it next draws, and the keyboard stays where it
    /// is.
    pub fn set_text(&mut self, text: String) {
        let mut cursor = self.cursor.min(text.len());
        while !text.is_char_boundary(cursor) {
            cursor -= 1;
        }
        self.apply(Edit::to(text, cursor));
        if !self.typing {
            self.edit = None;
        }
    }

    /// A blank script.
    pub fn new_blank(&mut self) {
        self.replace(BLANK.to_string(), None, "Untitled".into());
    }

    /// A built-in or example by its [`starters`] label or its name, any
    /// case. Unsaved: it has no path.
    pub fn open_starter(&mut self, name: &str) -> Result<(), String> {
        let Some((label, src)) = find_starter(name) else {
            let names: Vec<String> = starters().into_iter().map(|(l, _)| l).collect();
            return Err(format!("no built-in or example script named {name} (there are: {})", names.join(", ")));
        };
        self.replace(src.to_string(), None, label);
        // Not saved anywhere yet.
        self.saved.clear();
        Ok(())
    }

    /// Opens a `.rules` file.
    pub fn open_path(&mut self, path: &Path) -> Result<(), String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let title = path
            .file_name()
            .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        self.replace(text, Some(path.to_path_buf()), title);
        Ok(())
    }

    /// Writes the text to its path, or to `to` (which becomes its path).
    pub fn save(&mut self, to: Option<&Path>) -> Result<PathBuf, String> {
        let path = match to {
            Some(p) => p.to_path_buf(),
            None => self
                .path
                .clone()
                .ok_or("the script has no file yet: save it as one")?,
        };
        std::fs::write(&path, &self.text).map_err(|e| format!("{}: {e}", path.display()))?;
        self.saved = self.text.clone();
        // Our own write is no outside change; a save elsewhere leaves the
        // outside editor's file behind.
        if self.watch.as_ref().is_some_and(|w| w.path == path) {
            if let Some(w) = &mut self.watch {
                w.seen = modified(&path);
            }
        } else {
            self.watch = None;
        }
        if self.path.as_deref() != Some(path.as_path()) {
            self.title = path
                .file_name()
                .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        }
        self.path = Some(path.clone());
        Ok(path)
    }

    /// Formats the text (`rules::ide::format`); refused while it does not
    /// parse.
    pub fn format(&mut self) -> Result<(), String> {
        let out = ide::format(&self.text).map_err(|e| format!("not formatted: {e}"))?;
        if out != self.text {
            // The cursor keeps its line and column where the line has them.
            let cursor = byte_at(&out, position_at(&self.text, self.cursor));
            self.apply(Edit::to(out, cursor));
        }
        Ok(())
    }

    /// Runs the analysis again when the text changed since the last one.
    pub fn refresh(&mut self) -> &Analysis {
        if self.analysed.as_deref() != Some(self.text.as_str()) {
            self.analysis = Analysis {
                tokens: ide::semantic_tokens(&self.text),
                diagnostics: ide::diagnostics(&self.text),
                check: Some(workbench::check(&self.text)),
            };
            self.analysed = Some(self.text.clone());
            self.hover = None;
        }
        &self.analysis
    }

    /// The cursor as an LSP position.
    pub fn cursor_position(&self) -> Position {
        position_at(&self.text, self.cursor)
    }

    /// Completions at the cursor.
    pub fn completions(&self) -> Vec<ide::CompletionItem> {
        ide::complete(&self.text, self.cursor_position())
    }

    /// The signature of the call around the cursor, as one line.
    pub fn signature(&self) -> Option<String> {
        let help = ide::signature_help(&self.text, self.cursor_position())?;
        let sig = help.signatures.get(help.active_signature as usize)?;
        let param = sig.parameters.get(help.active_parameter as usize);
        Some(match param {
            Some(p) => format!("{}  ·  {}", sig.label, p.label),
            None => sig.label.clone(),
        })
    }

    /// Hover text at a byte offset.
    pub fn hover_at(&self, byte: usize) -> Option<String> {
        ide::hover(&self.text, position_at(&self.text, byte)).map(|h| h.markdown)
    }

    /// [`Self::hover_at`], asked once per byte until the text changes.
    pub fn hover_cached(&mut self, byte: usize) -> Option<&str> {
        if self.hover.as_ref().is_none_or(|(at, _)| *at != byte) {
            self.hover = Some((byte, self.hover_at(byte)));
        }
        self.hover.as_ref().and_then(|(_, h)| h.as_deref())
    }

    /// Takes a completion: it replaces the word before the cursor.
    pub fn complete_with(&mut self, item: &ide::CompletionItem) {
        let edit = completion_edit(&self.text, self.cursor, item);
        self.apply(edit);
    }

    /// Opens the completions at the cursor; `manual` (Ctrl+Space) keeps
    /// the list open with nothing typed. Nothing to offer closes it.
    pub fn open_completions(&mut self, manual: bool) {
        self.menu = Some(Menu {
            kind: MenuKind::Completions,
            manual,
            ..Menu::default()
        });
        self.refresh_menu();
    }

    /// Opens the quick fixes at the cursor (`rules::ide::actions`), or says
    /// there are none.
    pub fn open_fixes(&mut self) -> Result<(), String> {
        let at = self.cursor_position();
        let actions = ide::actions(&self.text, Range { start: at, end: at });
        let choices = fix_choices(&self.text, self.cursor, &actions);
        if choices.is_empty() {
            self.menu = None;
            return Err("No quick fixes here.".into());
        }
        self.menu = Some(Menu {
            kind: MenuKind::Fixes,
            choices,
            selected: 0,
            manual: true,
            text: self.text.clone(),
            cursor: self.cursor,
        });
        Ok(())
    }

    /// Keeps the list at the caret in step with the text and cursor:
    /// completions are asked again and ranked by the word typed, and close
    /// when none is left, when the one left is the word itself, or (opened
    /// by typing) when the word is gone. Quick fixes close when anything
    /// moves.
    pub fn refresh_menu(&mut self) {
        let Some(menu) = &self.menu else {
            return;
        };
        if menu.text == self.text && menu.cursor == self.cursor && !menu.choices.is_empty() {
            return;
        }
        if menu.kind == MenuKind::Fixes {
            self.menu = None;
            return;
        }
        let manual = menu.manual;
        let selected = menu.selected;
        let cursor = self.cursor.min(self.text.len());
        let from = word_start(&self.text, cursor);
        let word = &self.text[from..cursor];
        let after_dot = self.text[..from].ends_with('.');
        if !manual && word.is_empty() && !after_dot {
            self.menu = None;
            return;
        }
        let items = ide::complete(&self.text, position_at(&self.text, cursor));
        let choices = completion_choices(&self.text, cursor, &items);
        let only_itself = choices.len() == 1 && choices[0].label == word;
        if choices.is_empty() || (!manual && only_itself) {
            self.menu = None;
            return;
        }
        self.menu = Some(Menu {
            kind: MenuKind::Completions,
            selected: selected.min(choices.len() - 1),
            choices,
            manual,
            text: self.text.clone(),
            cursor,
        });
    }

    /// What one frame of typing does to the list at the caret: a typed
    /// character opens or closes the completions ([`trigger`]); a change
    /// without one (a deletion, a cursor move) keeps the list in step.
    pub fn after_typing(&mut self, typed: Option<char>) {
        match typed.map(trigger) {
            Some(Trigger::Open) => {
                let completing = self
                    .menu
                    .as_ref()
                    .is_some_and(|m| m.kind == MenuKind::Completions);
                if completing {
                    self.refresh_menu();
                } else {
                    self.open_completions(false);
                }
            }
            Some(Trigger::Close) => self.menu = None,
            None => self.refresh_menu(),
        }
    }

    /// Moves the list's selection by `by`, wrapping.
    pub fn menu_move(&mut self, by: i32) {
        if let Some(m) = &mut self.menu
            && !m.choices.is_empty()
        {
            let n = m.choices.len() as i32;
            m.selected = (m.selected as i32 + by).rem_euclid(n) as usize;
        }
    }

    /// The edit taking the selected row would make.
    pub fn menu_edit(&self) -> Option<Edit> {
        let m = self.menu.as_ref()?;
        m.choices.get(m.selected).map(|c| c.edit.clone())
    }

    /// Takes row `i` of the list at the caret.
    pub fn choose(&mut self, i: usize) {
        let edit = self
            .menu
            .as_ref()
            .and_then(|m| m.choices.get(i))
            .map(|c| c.edit.clone());
        if let Some(edit) = edit {
            self.apply(edit);
        }
    }

    /// Takes a diagnostic's fix (a problem row's button).
    pub fn apply_fix(&mut self, fix: &ide::Fix) {
        let edit = apply_text_edits(&self.text, self.cursor, &fix.edits);
        self.apply(edit);
    }

    /// Opens the rename field on the name at the cursor.
    pub fn start_rename(&mut self) -> Result<(), String> {
        let (s, e) = word_at(&self.text, self.cursor).ok_or("Put the cursor on a name to rename it.")?;
        let old = self.text[s..e].to_string();
        self.renaming = Some(Rename {
            at: s,
            name: old.clone(),
            old,
            focus: true,
        });
        self.menu = None;
        Ok(())
    }

    /// Renames the name under the rename field to what it holds
    /// (`rules::ide::rename`); the field stays open on a refusal.
    pub fn finish_rename(&mut self) -> Result<(), String> {
        let Some(r) = &self.renaming else {
            return Ok(());
        };
        if r.name.trim() == r.old {
            self.renaming = None;
            return Ok(());
        }
        let edit = rename_edit(&self.text, r.at, &r.name)?;
        self.renaming = None;
        self.apply(edit);
        Ok(())
    }

    /// Moves the cursor to the declaration of the name at `byte`, or says
    /// there is none.
    pub fn go_to_definition(&mut self, byte: usize) -> Result<(), String> {
        let at = definition_at(&self.text, byte).ok_or("No declaration found for that.")?;
        self.want_cursor = Some(at);
        self.cursor = at;
        self.menu = None;
        Ok(())
    }

    /// The uses of the name at the cursor (`rules::ide::references`),
    /// asked once per text and cursor.
    pub fn marks(&mut self) -> &[Mark] {
        let stale = self
            .marks
            .as_ref()
            .is_none_or(|(t, c, _)| *c != self.cursor || *t != self.text);
        if stale {
            let found = occurrences(&self.text, self.cursor);
            self.marks = Some((self.text.clone(), self.cursor, found));
        }
        match &self.marks {
            Some((_, _, m)) => m,
            None => &[],
        }
    }

    /// The diagnostics at a byte, for the hover.
    pub fn diagnostics_at(&self, byte: usize) -> Vec<&Diagnostic> {
        diagnostics_at(&self.analysis.diagnostics, position_at(&self.text, byte))
    }

    /// Queues a reload of the text for the game link: compiled and
    /// verified here first. A refusal here is kept as the reload state.
    pub fn queue_reload(&mut self) -> Result<String, String> {
        match reload_params(&self.text, self.mode) {
            Ok((params, hash)) => {
                self.outbox = Some(params);
                self.reload = ReloadState::Sending(hash.clone());
                Ok(hash)
            }
            Err(why) => {
                self.reload = ReloadState::Refused(why.clone());
                Err(why)
            }
        }
    }

    /// The reload row: its tone and text, or `None` before any reload.
    pub fn reload_line(&self) -> Option<(Tone, String)> {
        reload_line(&self.reload)
    }

    /// Readies the script for an outside editor ("Open in editor"): saves
    /// it to its own file (when it has unsaved edits), or, with none, to a
    /// new file under `data_root` ([`external_path`]), and watches that
    /// file for the outside editor's saves. Returns the file.
    pub fn prepare_external(&mut self, data_root: &Path) -> Result<PathBuf, String> {
        let path = match self.path.clone() {
            Some(p) => {
                if self.dirty() || !p.exists() {
                    self.save(None)?;
                }
                p
            }
            None => {
                let p = external_path(None, &self.title, data_root, |p| p.exists());
                if let Some(dir) = p.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                }
                self.save(Some(&p))?
            }
        };
        self.watch = Some(Watch::new(path.clone(), modified(&path)));
        Ok(path)
    }

    /// Checks the watched file, at most once a [`WATCH_EVERY`]: a new
    /// modification time reads the file and hands its text to
    /// [`Self::disk_text`].
    pub fn poll_disk(&mut self, now: Instant) -> DiskPoll {
        let Some(w) = &mut self.watch else {
            return DiskPoll::Nothing;
        };
        if !w.due(now) || !w.changed(modified(&w.path)) {
            return DiskPoll::Nothing;
        }
        match std::fs::read_to_string(&w.path) {
            Ok(disk) => self.disk_text(disk),
            Err(_) => DiskPoll::Nothing,
        }
    }

    /// The watched file's text after it changed on disk
    /// ([`disk_change`]): the same as the buffer marks it saved; with no
    /// unsaved edits here it replaces the buffer; otherwise it waits in
    /// [`Self::conflict`] for the window to ask.
    pub fn disk_text(&mut self, disk: String) -> DiskPoll {
        match disk_change(&self.text, &self.saved, &disk) {
            DiskChange::Same => {
                self.saved = disk;
                self.conflict = None;
                DiskPoll::Nothing
            }
            DiskChange::Take => {
                self.take_disk(disk);
                DiskPoll::Took
            }
            DiskChange::Ask => {
                self.conflict = Some(disk);
                DiskPoll::Asked
            }
        }
    }

    /// The conflict's answer "take the file's": the buffer becomes the
    /// file's text, saved. False with no conflict.
    pub fn take_conflict(&mut self) -> bool {
        match self.conflict.take() {
            Some(disk) => {
                self.take_disk(disk);
                true
            }
            None => false,
        }
    }

    /// The conflict's answer "keep mine": the buffer stays as it is,
    /// unsaved, and the next save writes it over the file.
    pub fn keep_mine(&mut self) {
        self.conflict = None;
    }

    fn take_disk(&mut self, disk: String) {
        self.conflict = None;
        self.set_text(disk.clone());
        self.saved = disk;
    }

    /// The outside editor's start, once its thread has an answer: how it
    /// was opened, or why nothing opened it.
    pub fn take_launched(&mut self) -> Option<Result<String, String>> {
        let done = self.launched.as_ref()?.lock().ok()?.take()?;
        self.launched = None;
        Some(done)
    }
}

/// How often the watched file is checked.
pub const WATCH_EVERY: Duration = Duration::from_secs(1);

/// How long a started editor has to fail before it counts as open: a
/// GUI editor keeps running, `code` and `open` exit at once with success.
pub const LAUNCH_GRACE: Duration = Duration::from_secs(3);

/// The file "Open in editor" handed out, and the modification time last
/// seen on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Watch {
    pub path: PathBuf,
    pub seen: Option<SystemTime>,
    next: Option<Instant>,
}

impl Watch {
    pub fn new(path: PathBuf, seen: Option<SystemTime>) -> Watch {
        Watch { path, seen, next: None }
    }

    /// Whether a check is due at `now`; a due check books the next one a
    /// [`WATCH_EVERY`] later.
    pub fn due(&mut self, now: Instant) -> bool {
        if self.next.is_some_and(|t| now < t) {
            return false;
        }
        self.next = Some(now + WATCH_EVERY);
        true
    }

    /// Whether the file changed: a modification time other than the last
    /// seen. A file gone is no change (an editor's save may replace it);
    /// its return is one.
    pub fn changed(&mut self, mtime: Option<SystemTime>) -> bool {
        if mtime == self.seen {
            return false;
        }
        self.seen = mtime;
        mtime.is_some()
    }
}

/// A file's modification time, `None` when it can't be read.
pub fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// What a change of the watched file does to the buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskChange {
    /// The file holds what the buffer holds.
    Same,
    /// No unsaved edits here: the file's text replaces the buffer.
    Take,
    /// Unsaved edits here and different text there: ask which to keep.
    Ask,
}

/// What a watched file's new text does, given the buffer and the text as
/// last opened or saved.
pub fn disk_change(buffer: &str, saved: &str, disk: &str) -> DiskChange {
    if disk == buffer {
        DiskChange::Same
    } else if buffer == saved {
        DiskChange::Take
    } else {
        DiskChange::Ask
    }
}

/// What one poll of the watched file did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskPoll {
    Nothing,
    /// The buffer took the file's text.
    Took,
    /// The file and the buffer both changed: [`RulesEditor::conflict`].
    Asked,
}

/// A script's file name from its title: the name before any
/// " (built in)", lower case, spaces as underscores, other characters
/// that don't belong in a file name dropped. A title that is a file name
/// already stays as it is.
pub fn script_file_name(title: &str) -> String {
    let stem = title.split(" (").next().unwrap_or("").trim();
    let suffix = format!(".{EXTENSION}");
    if stem.len() > suffix.len() && stem.ends_with(&suffix) && !stem.contains(['/', '\\']) {
        return stem.to_string();
    }
    let name: String = stem
        .to_lowercase()
        .chars()
        .map(|c| if c == ' ' { '_' } else { c })
        .filter(|c| c.is_alphanumeric() || matches!(c, '_' | '-'))
        .collect();
    let name = if name.is_empty() { "script".to_string() } else { name };
    format!("{name}{suffix}")
}

/// Where "Open in editor" writes the script: its own file, or else a new
/// one in `<data_root>/scripts` named after it ([`script_file_name`]),
/// with `-2`, `-3`... when that name is `taken`.
pub fn external_path(path: Option<&Path>, title: &str, data_root: &Path, taken: impl Fn(&Path) -> bool) -> PathBuf {
    if let Some(p) = path {
        return p.to_path_buf();
    }
    let dir = data_root.join("scripts");
    let name = script_file_name(title);
    let suffix = format!(".{EXTENSION}");
    let stem = name.strip_suffix(&suffix).unwrap_or(&name).to_string();
    let first = dir.join(&name);
    if !taken(&first) {
        return first;
    }
    (2..10_000)
        .map(|n| dir.join(format!("{stem}-{n}{suffix}")))
        .find(|p| !taken(p))
        .unwrap_or(first)
}

/// One way to open a file in an outside editor: a program and its
/// arguments, the file last.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
}

impl Launch {
    /// The command as a person would type it.
    pub fn describe(&self) -> String {
        let mut out = self.program.display().to_string();
        for a in &self.args {
            out.push(' ');
            out.push_str(a);
        }
        out
    }
}

/// Editors that need a terminal, which Studio has none to give.
const TERMINAL_EDITORS: [&str; 12] = [
    "vi", "vim", "nvim", "nano", "pico", "hx", "helix", "micro", "kak", "ed", "joe", "mg",
];

/// A program's file name names a terminal editor.
pub fn is_terminal_editor(program: &str) -> bool {
    let base = Path::new(program)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(program);
    TERMINAL_EDITORS.contains(&base)
}

/// The directories searched for a program: `PATH`'s, then Homebrew's and
/// `/usr/local/bin`, which an app started from the Finder has no `PATH`
/// entry for.
pub fn program_dirs(path_var: Option<&str>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = path_var
        .unwrap_or("")
        .split(':')
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .collect();
    for extra in ["/opt/homebrew/bin", "/usr/local/bin"] {
        let extra = PathBuf::from(extra);
        if !dirs.contains(&extra) {
            dirs.push(extra);
        }
    }
    dirs
}

/// The first of `dirs` holding the program `name`.
pub fn find_program(name: &str, dirs: &[PathBuf], is_file: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    dirs.iter().map(|d| d.join(name)).find(|p| is_file(p))
}

/// The rules extension's id in VS Code (`publisher.name` of
/// `editors/vscode/package.json`) and in Zed (`editors/zed/extension.toml`).
/// `h2 rules editors` reads the same places
/// (`crates/apps/cli/src/rules_editors.rs`).
pub const VSCODE_EXTENSION: &str = "h2recomp.h2-rules";
pub const ZED_EXTENSION: &str = "h2-rules";

/// Whether `editor` (`code` or `zed`) has the rules extension
/// (docs/editor/rules-editors.md): VS Code keeps each extension in
/// `~/.vscode/extensions/<id>-<version>`; Zed keeps each, a dev extension
/// as a link to its folder, in `extensions/installed/<id>` under its data
/// directory (`~/Library/Application Support/Zed` on macOS,
/// `~/.local/share/zed` elsewhere).
pub fn extension_installed(
    editor: &str,
    home: &Path,
    macos: bool,
    exists: impl Fn(&Path) -> bool,
    list_dir: impl Fn(&Path) -> Vec<String>,
) -> bool {
    match editor {
        "code" => {
            let prefix = format!("{VSCODE_EXTENSION}-");
            list_dir(&home.join(".vscode/extensions"))
                .iter()
                .any(|name| name.to_ascii_lowercase().starts_with(&prefix))
        }
        "zed" => {
            let data = if macos {
                home.join("Library/Application Support/Zed")
            } else {
                home.join(".local/share/zed")
            };
            exists(&data.join("extensions/installed").join(ZED_EXTENSION))
        }
        _ => false,
    }
}

/// What "Open in editor" tries, in order: VS Code (`code`) or Zed
/// (`zed`) where `find` finds it and it has the rules extension
/// (`has_extension`), then `$VISUAL` and `$EDITOR` (a terminal editor is
/// passed over: there is no terminal to run it in), then `code` and
/// `zed` where `find` finds them, then on macOS `open -t` (the default
/// text editor). The same command twice is tried once.
pub fn editor_launches(
    file: &Path,
    env: impl Fn(&str) -> Option<String>,
    find: impl Fn(&str) -> Option<PathBuf>,
    has_extension: impl Fn(&str) -> bool,
    macos: bool,
) -> Vec<Launch> {
    let file_arg = file.display().to_string();
    let mut out: Vec<Launch> = Vec::new();
    for name in ["code", "zed"] {
        if has_extension(name)
            && let Some(program) = find(name)
        {
            out.push(Launch {
                program,
                args: vec![file_arg.clone()],
            });
        }
    }
    for var in ["VISUAL", "EDITOR"] {
        let Some(value) = env(var) else {
            continue;
        };
        let mut words = value.split_whitespace();
        let Some(program) = words.next() else {
            continue;
        };
        if is_terminal_editor(program) {
            continue;
        }
        let mut args: Vec<String> = words.map(str::to_string).collect();
        args.push(file_arg.clone());
        out.push(Launch {
            program: PathBuf::from(program),
            args,
        });
    }
    for name in ["code", "zed"] {
        if let Some(program) = find(name) {
            out.push(Launch {
                program,
                args: vec![file_arg.clone()],
            });
        }
    }
    if macos {
        out.push(Launch {
            program: PathBuf::from("open"),
            args: vec!["-t".into(), file_arg],
        });
    }
    let mut unique: Vec<Launch> = Vec::with_capacity(out.len());
    for l in out {
        if !unique.contains(&l) {
            unique.push(l);
        }
    }
    unique
}

/// [`editor_launches`] with this process's environment.
pub fn editor_launches_here(file: &Path) -> Vec<Launch> {
    let dirs = program_dirs(std::env::var("PATH").ok().as_deref());
    editor_launches(
        file,
        |k| std::env::var(k).ok().filter(|v| !v.trim().is_empty()),
        |name| find_program(name, &dirs, |p| p.is_file()),
        extension_installed_here,
        cfg!(target_os = "macos"),
    )
}

/// [`extension_installed`] in this user's home.
pub fn extension_installed_here(editor: &str) -> bool {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return false;
    };
    let list_dir = |dir: &Path| -> Vec<String> {
        std::fs::read_dir(dir)
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| e.file_name().to_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    extension_installed(editor, &home, cfg!(target_os = "macos"), |p| p.exists(), list_dir)
}

/// What to do with a started editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchStep {
    /// Still starting: look again shortly.
    Wait,
    /// It opened the file: it exited with success (`code`, `open`), or
    /// it is still running after [`LAUNCH_GRACE`] (a GUI editor).
    Done,
    /// It failed (a `$VISUAL` that is not installed, say): try the next
    /// one.
    Next,
}

/// The step for a started editor: `exited` is whether its exit was a
/// success, `None` while it runs; `waited` is the time since it started.
pub fn launch_step(exited: Option<bool>, waited: Duration) -> LaunchStep {
    match exited {
        Some(true) => LaunchStep::Done,
        Some(false) => LaunchStep::Next,
        None if waited >= LAUNCH_GRACE => LaunchStep::Done,
        None => LaunchStep::Wait,
    }
}

/// An outside editor's start: filled once with how it was opened
/// ([`Launch::describe`]), or why nothing opened it.
pub type Launched = Arc<Mutex<Option<Result<String, String>>>>;

/// Tries each launch in turn on a thread of its own, so the window never
/// waits ([`launch_step`]); the answer lands in the returned slot.
pub fn launch(launches: Vec<Launch>) -> Launched {
    let slot: Launched = Arc::new(Mutex::new(None));
    let out = slot.clone();
    let spawned = std::thread::Builder::new()
        .name("rules-open-editor".into())
        .spawn(move || run_launches(&launches, &out));
    if let Err(e) = spawned {
        *slot.lock().unwrap_or_else(|p| p.into_inner()) = Some(Err(format!("could not start the editor: {e}")));
    }
    slot
}

fn run_launches(launches: &[Launch], out: &Launched) {
    let answer = |r: Result<String, String>| {
        *out.lock().unwrap_or_else(|p| p.into_inner()) = Some(r);
    };
    for l in launches {
        let Ok(mut child) = std::process::Command::new(&l.program)
            .args(&l.args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        else {
            continue;
        };
        let started = Instant::now();
        loop {
            let exited = match child.try_wait() {
                Ok(Some(status)) => Some(status.success()),
                Ok(None) => None,
                Err(_) => Some(false),
            };
            match launch_step(exited, started.elapsed()) {
                LaunchStep::Wait => std::thread::sleep(Duration::from_millis(100)),
                LaunchStep::Done => {
                    answer(Ok(l.describe()));
                    // Reaped when the editor closes.
                    if exited.is_none() {
                        let _ = child.wait();
                    }
                    return;
                }
                LaunchStep::Next => break,
            }
        }
    }
    answer(Err(
        "no editor opened the script: install VS Code or Zed with the rules extension (h2 rules editors install), or set $VISUAL or $EDITOR to a GUI editor"
            .into(),
    ));
}

/// A completion's text without snippet placeholders (`${1:name}` becomes
/// `name`, `${1|a,b|}` becomes `a`, `$0` and `$1` go).
pub fn plain_insert(item: &ide::CompletionItem) -> String {
    if item.insert_text_format != ide::SNIPPET {
        return item.insert_text.clone();
    }
    expand_snippet(&item.insert_text, "").text
}

/// A snippet expanded to plain text: where the cursor goes, and the
/// placeholder to select.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expansion {
    pub text: String,
    pub cursor: usize,
    pub select: Option<(usize, usize)>,
}

/// Expands an LSP snippet to plain text: each placeholder becomes its
/// default (`${1:name}` is `name`, `${1|a,b|}` is `a`), tab stops go, a
/// tab becomes [`INDENT`] and each new line takes `indent`. The first
/// placeholder (the lowest number from 1) is selected; with none, the
/// cursor goes to `$0`, else to the end.
pub fn expand_snippet(snippet: &str, indent: &str) -> Expansion {
    let mut out = String::with_capacity(snippet.len());
    // (number, start, end) of each tab stop's first occurrence.
    let mut stops: Vec<(u32, usize, usize)> = Vec::new();
    expand_into(snippet, indent, &mut out, &mut stops);
    let first = stops.iter().filter(|s| s.0 > 0).min_by_key(|s| s.0);
    let (cursor, select) = match first {
        Some(&(_, a, b)) if a < b => (b, Some((a, b))),
        Some(&(_, a, _)) => (a, None),
        None => match stops.iter().find(|s| s.0 == 0) {
            Some(&(_, a, _)) => (a, None),
            None => (out.len(), None),
        },
    };
    Expansion {
        text: out,
        cursor,
        select,
    }
}

fn note_stop(stops: &mut Vec<(u32, usize, usize)>, n: u32, start: usize, end: usize) {
    if !stops.iter().any(|s| s.0 == n) {
        stops.push((n, start, end));
    }
}

fn expand_into(s: &str, indent: &str, out: &mut String, stops: &mut Vec<(u32, usize, usize)>) {
    let b = s.as_bytes();
    let mut i = 0;
    while let Some(c) = s[i..].chars().next() {
        match c {
            '\\' if i + 1 < s.len() => {
                let next = s[i + 1..].chars().next().unwrap_or('\\');
                out.push(next);
                i += 1 + next.len_utf8();
            }
            '\t' => {
                out.push_str(INDENT);
                i += 1;
            }
            '\n' => {
                out.push('\n');
                out.push_str(indent);
                i += 1;
            }
            '$' if i + 1 < s.len() && b[i + 1].is_ascii_digit() => {
                let mut j = i + 1;
                while j < s.len() && b[j].is_ascii_digit() {
                    j += 1;
                }
                let n = s[i + 1..j].parse().unwrap_or(0);
                note_stop(stops, n, out.len(), out.len());
                i = j;
            }
            '$' if i + 1 < s.len() && b[i + 1] == b'{' => {
                // The matching brace, past nested ones and escapes.
                let mut depth = 0i32;
                let mut j = i + 1;
                let mut close = None;
                while j < s.len() {
                    match b[j] {
                        b'\\' => j += 1,
                        b'{' => depth += 1,
                        b'}' => {
                            depth -= 1;
                            if depth == 0 {
                                close = Some(j);
                                break;
                            }
                        }
                        _ => {}
                    }
                    j += 1;
                }
                let Some(close) = close else {
                    out.push_str(&s[i..]);
                    return;
                };
                let inner = &s[i + 2..close];
                let digits = inner.bytes().take_while(u8::is_ascii_digit).count();
                let n: u32 = inner[..digits].parse().unwrap_or(0);
                let rest = &inner[digits..];
                let start = out.len();
                if let Some(default) = rest.strip_prefix(':') {
                    expand_into(default, indent, out, stops);
                } else if let Some(choices) = rest.strip_prefix('|') {
                    let first = choices.trim_end_matches('|').split(',').next().unwrap_or("");
                    out.push_str(first);
                } else if digits == 0 {
                    // A variable (`${name}`): left as its name.
                    out.push_str(rest);
                }
                if digits > 0 {
                    note_stop(stops, n, start, out.len());
                }
                i = close + 1;
            }
            _ => {
                out.push(c);
                i += c.len_utf8();
            }
        }
    }
}

/// The completion's edit: the word before the cursor replaced by the
/// item's text (a snippet expanded, its lines indented as the cursor's).
pub fn completion_edit(text: &str, cursor: usize, item: &ide::CompletionItem) -> Edit {
    let cursor = cursor.min(text.len());
    let from = word_start(text, cursor);
    let line_start = text[..cursor].rfind('\n').map_or(0, |i| i + 1);
    let indent: String = text[line_start..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let exp = if item.insert_text_format == ide::SNIPPET {
        expand_snippet(&item.insert_text, &indent)
    } else {
        Expansion {
            cursor: item.insert_text.len(),
            text: item.insert_text.clone(),
            select: None,
        }
    };
    let mut out = String::with_capacity(text.len() + exp.text.len());
    out.push_str(&text[..from]);
    out.push_str(&exp.text);
    out.push_str(&text[cursor..]);
    Edit {
        text: out,
        cursor: from + exp.cursor,
        select: exp.select.map(|(a, b)| (from + a, from + b)),
    }
}

/// The completions that fit the word typed, best first: those whose label
/// starts with it, then those that hold it, then those that hold its
/// letters in order; each group in the language's order (`sort_text`).
/// Nothing typed keeps them all. Case is ignored.
pub fn rank<'a>(items: &'a [ide::CompletionItem], word: &str) -> Vec<&'a ide::CompletionItem> {
    let word = word.to_lowercase();
    let mut scored: Vec<(u8, &ide::CompletionItem)> = items
        .iter()
        .filter_map(|it| {
            if word.is_empty() {
                return Some((0, it));
            }
            let label = it.label.to_lowercase();
            if label.starts_with(&word) {
                Some((0, it))
            } else if label.contains(&word) {
                Some((1, it))
            } else {
                let mut rest = label.chars();
                word.chars().all(|w| rest.any(|l| l == w)).then_some((2, it))
            }
        })
        .collect();
    scored.sort_by(|a, b| (a.0, &a.1.sort_text).cmp(&(b.0, &b.1.sort_text)));
    scored.into_iter().map(|(_, it)| it).collect()
}

/// The completions at `cursor` as rows of the list: ranked by the word
/// before the cursor ([`rank`]), at most [`COMPLETIONS`], each with its
/// edit.
pub fn completion_choices(text: &str, cursor: usize, items: &[ide::CompletionItem]) -> Vec<Choice> {
    let cursor = cursor.min(text.len());
    let word = &text[word_start(text, cursor)..cursor];
    rank(items, word)
        .into_iter()
        .take(COMPLETIONS)
        .map(|it| Choice {
            label: it.label.clone(),
            detail: it.detail.clone(),
            doc: it.documentation.value.clone(),
            edit: completion_edit(text, cursor, it),
        })
        .collect()
}

/// The quick fixes as rows of the list.
pub fn fix_choices(text: &str, cursor: usize, actions: &[ide::CodeAction]) -> Vec<Choice> {
    actions
        .iter()
        .map(|a| Choice {
            label: a.title.clone(),
            detail: a.diagnostic.message.clone(),
            doc: String::new(),
            edit: apply_text_edits(text, cursor, &a.edits),
        })
        .collect()
}

/// What a typed character does to the completions: a name's character or
/// a `.` opens (or narrows) them; anything else closes them.
pub fn trigger(c: char) -> Trigger {
    if c.is_alphanumeric() || c == '_' || c == '.' {
        Trigger::Open
    } else {
        Trigger::Close
    }
}

/// LSP edits applied to the text, with the cursor moved by them: an edit
/// wholly before it shifts it, one around it puts it at the edit's end.
/// An edit overlapping the one before is left out.
pub fn apply_text_edits(text: &str, cursor: usize, edits: &[TextEdit]) -> Edit {
    let cursor = cursor.min(text.len());
    let mut spans: Vec<(usize, usize, &str)> = edits
        .iter()
        .map(|e| {
            let a = byte_at(text, e.range.start);
            let b = byte_at(text, e.range.end).max(a);
            (a, b, e.new_text.as_str())
        })
        .collect();
    spans.sort_by_key(|s| s.0);
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    let mut moved = cursor as isize;
    for &(a, b, new) in &spans {
        if a < at {
            continue;
        }
        out.push_str(&text[at..a]);
        out.push_str(new);
        if b <= cursor {
            moved += new.len() as isize - (b - a) as isize;
        } else if a < cursor {
            moved = out.len() as isize;
        }
        at = b;
    }
    out.push_str(&text[at..]);
    let mut cursor = (moved.max(0) as usize).min(out.len());
    while !out.is_char_boundary(cursor) {
        cursor -= 1;
    }
    Edit::to(out, cursor)
}

/// The rename of the name at `byte` to `new_name` as an edit, or why the
/// language refuses it (`rules::ide::rename`).
pub fn rename_edit(text: &str, byte: usize, new_name: &str) -> Result<Edit, String> {
    let edits = ide::rename(text, position_at(text, byte), new_name.trim())?;
    if edits.is_empty() {
        return Err("Nothing to rename there.".into());
    }
    Ok(apply_text_edits(text, byte, &edits))
}

/// The byte where the name at `byte` is declared.
pub fn definition_at(text: &str, byte: usize) -> Option<usize> {
    ide::definition(text, position_at(text, byte))
        .first()
        .map(|l| byte_at(text, l.range.start))
}

/// A highlighted use of a name: byte offsets within its line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mark {
    pub line: usize,
    pub start: usize,
    pub end: usize,
}

/// Every use of the name at `byte` (`rules::ide::references`), as spans
/// of their lines; none when the cursor is on no name.
pub fn occurrences(text: &str, byte: usize) -> Vec<Mark> {
    ide::references(text, position_at(text, byte))
        .iter()
        .filter(|l| l.range.start.line == l.range.end.line)
        .filter_map(|l| {
            let line_start = byte_at(text, Position::new(l.range.start.line, 0));
            let (a, b) = (byte_at(text, l.range.start), byte_at(text, l.range.end));
            (a < b).then(|| Mark {
                line: l.range.start.line as usize,
                start: a - line_start,
                end: b - line_start,
            })
        })
        .collect()
}

/// The diagnostics whose range holds `pos` (an empty range holds its own
/// place).
pub fn diagnostics_at(diags: &[Diagnostic], pos: Position) -> Vec<&Diagnostic> {
    diags
        .iter()
        .filter(|d| d.range.start <= pos && (pos < d.range.end || pos == d.range.start))
        .collect()
}

/// The name around `byte`: its start and end, or `None` off a name (or on
/// a number).
pub fn word_at(text: &str, byte: usize) -> Option<(usize, usize)> {
    let byte = byte.min(text.len());
    let start = word_start(text, byte);
    let end = byte
        + text[byte..]
            .chars()
            .take_while(|&c| c.is_alphanumeric() || c == '_')
            .map(char::len_utf8)
            .sum::<usize>();
    let number = text[start..].starts_with(|c: char| c.is_ascii_digit());
    (start < end && !number).then_some((start, end))
}

/// One block of a hover: prose, or a code fence's lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoverBlock {
    pub code: bool,
    pub text: String,
}

/// A hover's markdown as blocks to draw: code fences as code, the prose
/// between them without its inline marks (backticks, `**`).
pub fn hover_blocks(markdown: &str) -> Vec<HoverBlock> {
    fn flush(buf: &mut Vec<&str>, code: bool, out: &mut Vec<HoverBlock>) {
        let text = buf.join("\n");
        let text = if code {
            text
        } else {
            text.replace("**", "").replace('`', "")
        };
        let text = text.trim_matches('\n').to_string();
        if !text.trim().is_empty() {
            out.push(HoverBlock { code, text });
        }
        buf.clear();
    }
    let mut out = Vec::new();
    let mut code = false;
    let mut buf: Vec<&str> = Vec::new();
    for line in markdown.lines() {
        if line.trim_start().starts_with("```") {
            flush(&mut buf, code, &mut out);
            code = !code;
            continue;
        }
        buf.push(line);
    }
    flush(&mut buf, code, &mut out);
    out
}

/// The start of the identifier that ends at `byte`.
pub fn word_start(text: &str, byte: usize) -> usize {
    let byte = byte.min(text.len());
    let before = &text[..byte];
    before
        .char_indices()
        .rev()
        .take_while(|&(_, c)| c.is_alphanumeric() || c == '_')
        .last()
        .map_or(byte, |(i, _)| i)
}

/// The byte offset of an LSP position (UTF-16 columns); the end of the
/// line or text when past it.
pub fn byte_at(text: &str, pos: Position) -> usize {
    let mut start = 0;
    for _ in 0..pos.line {
        match text[start..].find('\n') {
            Some(i) => start += i + 1,
            None => return text.len(),
        }
    }
    let end = text[start..].find('\n').map_or(text.len(), |i| start + i);
    let mut units = 0u32;
    for (i, c) in text[start..end].char_indices() {
        if units >= pos.character {
            return start + i;
        }
        units += c.len_utf16() as u32;
    }
    end
}

/// The LSP position of a byte offset.
pub fn position_at(text: &str, byte: usize) -> Position {
    let byte = byte.min(text.len());
    let before = &text[..byte];
    let line = before.matches('\n').count() as u32;
    let start = before.rfind('\n').map_or(0, |i| i + 1);
    let character = before[start..].chars().map(|c| c.len_utf16() as u32).sum();
    Position { line, character }
}

/// A coloured run of one line: byte offsets within the line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub line: usize,
    pub start: usize,
    pub end: usize,
    pub kind: TokenKind,
}

/// The semantic tokens as byte spans of their lines, for the colouring.
/// A token past its line's end is cut to it; one past the text is left
/// out.
pub fn spans(text: &str, tokens: &[Token]) -> Vec<Span> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out = Vec::with_capacity(tokens.len());
    for t in tokens {
        let Some(line) = lines.get(t.line as usize) else {
            continue;
        };
        let at = |units: u32| -> usize {
            let mut n = 0u32;
            for (i, c) in line.char_indices() {
                if n >= units {
                    return i;
                }
                n += c.len_utf16() as u32;
            }
            line.len()
        };
        let (start, end) = (at(t.start), at(t.start + t.len));
        if start < end {
            out.push(Span {
                line: t.line as usize,
                start,
                end,
                kind: t.kind,
            });
        }
    }
    out
}

/// One row of the problems list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// 1-based, as people count.
    pub line: u32,
    pub col: u32,
    pub severity: &'static str,
    pub message: String,
    pub code: &'static str,
    /// Where a click puts the cursor.
    pub at: usize,
    /// The fixes the language offers for it, the likeliest first.
    pub fixes: Vec<ide::Fix>,
}

pub fn severity_name(s: u8) -> &'static str {
    match s {
        ide::severity::ERROR => "error",
        ide::severity::WARNING => "warning",
        ide::severity::INFORMATION => "info",
        _ => "hint",
    }
}

/// The diagnostics as problem rows: errors first, then by place.
pub fn problems(text: &str, diagnostics: &[Diagnostic]) -> Vec<Problem> {
    let mut out: Vec<(u8, Problem)> = diagnostics
        .iter()
        .map(|d| {
            (
                d.severity,
                Problem {
                    line: d.range.start.line + 1,
                    col: d.range.start.character + 1,
                    severity: severity_name(d.severity),
                    message: d.message.clone(),
                    code: d.code,
                    at: byte_at(text, d.range.start),
                    fixes: d.fixes.clone(),
                },
            )
        })
        .collect();
    out.sort_by_key(|a| (a.0, a.1.line, a.1.col));
    out.into_iter().map(|(_, p)| p).collect()
}

fn mode_name(m: ReloadMode) -> &'static str {
    match m {
        ReloadMode::Auto => "auto",
        ReloadMode::InPlace => "in_place",
        ReloadMode::Restart => "restart",
    }
}

/// `rules.reload`'s params for `text`: compiled and verified here, sent
/// as the blob (`ir_b64`) so the game runs exactly what the editor
/// checked. Returns them with the blob's hash, or why the text does not
/// build.
pub fn reload_params(text: &str, mode: ReloadMode) -> Result<(Value, String), String> {
    let (ir, _) = rules::build(text).map_err(|e| format!("the script does not build: {e}"))?;
    let blob = ir.encode();
    let hash = ir.id().to_string();
    Ok((
        json!({
            "ir_b64": variants::portable::base64_encode(&blob),
            "mode": mode_name(mode),
        }),
        hash,
    ))
}

/// A reload state as one row of the Game panel.
pub fn reload_line(state: &ReloadState) -> Option<(Tone, String)> {
    let short = |h: &str| h.get(..12).unwrap_or(h).to_string();
    Some(match state {
        ReloadState::Idle => return None,
        ReloadState::Sending(h) => (Tone::Busy, format!("Script {}: reloading...", short(h))),
        ReloadState::Done(r) => match r.mode {
            agent_proto::rules::Reloaded::InPlace => (
                Tone::Good,
                format!("Script {}: reloaded in place at tick {}", short(&r.hash), r.tick),
            ),
            agent_proto::rules::Reloaded::Restarted if r.reasons.is_empty() => (
                Tone::Notice,
                format!("Script {}: the game restarted with it", short(&r.hash)),
            ),
            agent_proto::rules::Reloaded::Restarted => (
                Tone::Notice,
                format!(
                    "Script {}: the game restarted with it ({})",
                    short(&r.hash),
                    r.reasons.join("; ")
                ),
            ),
        },
        ReloadState::Refused(why) => (Tone::Bad, format!("Script not reloaded: {why}")),
    })
}

#[cfg(test)]
mod tests;
