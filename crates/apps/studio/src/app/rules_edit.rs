//! The game type script editor (docs/editor/editor-guide.md, "Game type
//! scripts"): one `.rules` file open, its language intelligence from
//! `rules::ide` and its check report from `rules::workbench`, and its
//! reload into the paired game (`rules.reload`, docs/game/rules-engine.md,
//! "Reloading a running script").
//!
//! No toolkit here: `ui::rules` draws it, the agent API's `rules.*`
//! (`api::rules`) drives it, and tests read the pure helpers ([`spans`],
//! [`problems`], [`byte_at`], [`position_at`], [`reload_params`]).

use std::path::{Path, PathBuf};

use agent_proto::rules::{ReloadMode, ReloadResult};
use rules::ide::{self, Diagnostic, Position, Token, TokenKind};
use rules::workbench::{self, Check};
use serde_json::{Value, json};

/// A new script's text.
pub const BLANK: &str = "rules \"New game type\" 1\ncarrier slayer\n\noption kill_points: int 1..5 = 1 \"Kill points\" \"Points for each kill\"\n\non kill(victim, killer) {\n    if killer != none and killer != victim {\n        score(killer, kill_points)\n    }\n}\n";

/// The file extension of a script.
pub const EXTENSION: &str = "rules";

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
    /// Where the widget should put the cursor next (a problem clicked).
    pub want_cursor: Option<usize>,
    /// A completion to apply: replace from this byte to the cursor.
    pub insert: Option<(usize, String)>,
    /// The text the analysis is for.
    analysed: Option<String>,
    pub analysis: Analysis,
    /// Saving reloads the script into the paired game.
    pub reload_on_save: bool,
    pub mode: ReloadMode,
    pub reload: ReloadState,
    /// The reload waiting for the game link (`play` takes it).
    pub outbox: Option<Value>,
    /// The completions list is open under the editor.
    pub completing: bool,
    /// The last hover asked for: its byte and its text.
    pub hover: Option<(usize, Option<String>)>,
    /// The text widget was active last frame (`ui::rules`).
    pub typing: bool,
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
            insert: None,
            analysed: None,
            analysis: Analysis::default(),
            reload_on_save: true,
            mode: ReloadMode::Auto,
            reload: ReloadState::Idle,
            outbox: None,
            completing: false,
            hover: None,
            typing: false,
        }
    }
}

/// The scripts to start from: every built-in, then the examples, as
/// `(label, source)`.
pub fn starters() -> Vec<(String, &'static str)> {
    let mut out: Vec<(String, &'static str)> = rules::builtin::ALL
        .iter()
        .map(|b| (format!("{} (built in)", b.name), b.source))
        .collect();
    out.extend(
        rules::builtin::examples()
            .iter()
            .map(|&(name, src)| (format!("{name} (example)"), src)),
    );
    out
}

impl RulesEditor {
    pub fn dirty(&self) -> bool {
        self.text != self.saved
    }

    fn replace(&mut self, text: String, path: Option<PathBuf>, title: String) {
        self.saved = text.clone();
        self.text = text;
        self.path = path;
        self.title = title;
        self.cursor = 0;
        self.want_cursor = Some(0);
        self.insert = None;
        self.analysed = None;
        self.completing = false;
    }

    /// A blank script.
    pub fn new_blank(&mut self) {
        self.replace(BLANK.to_string(), None, "Untitled".into());
    }

    /// A built-in or example by its [`starters`] label or its name, any
    /// case. Unsaved: it has no path.
    pub fn open_starter(&mut self, name: &str) -> Result<(), String> {
        let want = name.trim().to_ascii_lowercase();
        let found = starters().into_iter().find(|(label, _)| {
            let label = label.to_ascii_lowercase();
            label == want || label.split(" (").next() == Some(want.as_str())
        });
        let Some((label, src)) = found else {
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
            self.text = out;
            self.cursor = self.cursor.min(self.text.len());
            while !self.text.is_char_boundary(self.cursor) {
                self.cursor -= 1;
            }
            self.want_cursor = Some(self.cursor);
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
        let from = word_start(&self.text, self.cursor);
        self.insert = Some((from, plain_insert(item)));
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
}

/// A completion's text without snippet placeholders (`${1:name}` becomes
/// `name`, `$0` and `$1` go).
pub fn plain_insert(item: &ide::CompletionItem) -> String {
    if item.insert_text_format != ide::SNIPPET {
        return item.insert_text.clone();
    }
    let s = &item.insert_text;
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('{') => {
                chars.next();
                // ${n:default} → default
                while chars.peek().is_some_and(|c| c.is_ascii_digit()) {
                    chars.next();
                }
                if chars.peek() == Some(&':') {
                    chars.next();
                }
                for c in chars.by_ref() {
                    if c == '}' {
                        break;
                    }
                    out.push(c);
                }
            }
            Some(d) if d.is_ascii_digit() => {
                while chars.peek().is_some_and(|c| c.is_ascii_digit()) {
                    chars.next();
                }
            }
            _ => out.push('$'),
        }
    }
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
                },
            )
        })
        .collect();
    out.sort_by(|a, b| (a.0, a.1.line, a.1.col).cmp(&(b.0, b.1.line, b.1.col)));
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
