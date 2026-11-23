//! Language intelligence for a scenario's HaloScript as source: the
//! working copy Studio writes for an outside editor
//! (docs/editor/rules-editors.md, "HaloScript"). Problems, completion of
//! builtins, scripts, globals and the scenario's names, hover with the
//! builtin's signature, usage text and documentation, signature help,
//! definitions, references and highlights across the file, rename of a
//! script or global, the outline, folding, semantic tokens, a quick fix
//! for a misspelt call, and a formatter.
//!
//! Pure over the text and a [`Context`] (the map's builtins, types and
//! names, which Studio writes beside the file as [`CONTEXT_FILE`]). With
//! an empty context only the syntax is checked. Positions are LSP's: a
//! 0-based line and a UTF-16 `character`; results serialize with LSP
//! field names, as `rules::ide`'s do. `h2 hs lsp`
//! (`crates/apps/h2-tools/src/hs_lsp.rs`) serves them, and Studio checks
//! a saved working copy with [`diagnostics`].

use serde::{Deserialize, Serialize};

mod context;
mod features;
mod format;
mod model;
mod syntax;
#[cfg(test)]
mod tests;

pub use context::{BuiltinInfo, CONTEXT_FILE, CONTEXT_VERSION, Context, NamedType};
pub use features::{
    actions, complete, definition, highlights, hover, name_at, prepare_rename, references, rename,
    semantic_tokens, signature_help, symbols, folding_ranges,
};
pub use format::format;
pub use model::DefItem;

/// The file extension of a working copy.
pub const EXTENSION: &str = "hsc";

/// A place in the source: a 0-based line and a UTF-16 column.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

impl Position {
    pub fn new(line: u32, character: u32) -> Position {
        Position { line, character }
    }
}

/// A half-open span of the source.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

/// LSP's `DiagnosticSeverity`.
pub mod severity {
    pub const ERROR: u8 = 1;
    pub const WARNING: u8 = 2;
    pub const INFORMATION: u8 = 3;
}

/// A problem: LSP's `Diagnostic`, with a stable code (`syntax.open`,
/// `call.unknown`, `call.arity`, `name.unknown`, ...).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub range: Range,
    pub severity: u8,
    pub code: String,
    pub source: &'static str,
    pub message: String,
}

/// Markdown text, as LSP's `MarkupContent`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Markup {
    pub kind: &'static str,
    pub value: String,
}

impl Markup {
    pub fn markdown(value: impl Into<String>) -> Markup {
        Markup {
            kind: "markdown",
            value: value.into(),
        }
    }
}

/// LSP's `TextEdit`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextEdit {
    pub range: Range,
    pub new_text: String,
}

/// LSP's `CompletionItemKind` values used here.
pub mod completion_kind {
    pub const METHOD: u8 = 2;
    pub const FUNCTION: u8 = 3;
    pub const VARIABLE: u8 = 6;
    pub const KEYWORD: u8 = 14;
    pub const ENUM_MEMBER: u8 = 20;
    pub const CONSTANT: u8 = 21;
    pub const TYPE_PARAMETER: u8 = 25;
}

/// LSP's `CompletionItem`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionItem {
    pub label: String,
    pub kind: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentation: Option<Markup>,
    pub sort_text: String,
}

/// LSP's `Hover`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Hover {
    pub contents: Markup,
    pub range: Range,
}

/// One signature: its label and each parameter's UTF-16 offsets in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    pub label: String,
    pub documentation: Option<String>,
    pub parameters: Vec<[u32; 2]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureHelp {
    pub signature: Signature,
    pub active_parameter: u32,
}

/// LSP's `DocumentHighlightKind`.
pub mod highlight_kind {
    pub const READ: u8 = 2;
    pub const WRITE: u8 = 3;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Highlight {
    pub range: Range,
    pub kind: u8,
}

/// LSP's `DocumentSymbol` (no children: definitions are flat).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Symbol {
    pub name: String,
    pub detail: String,
    /// 12 a script (function), 13 a global (variable).
    pub kind: u8,
    pub range: Range,
    pub selection_range: Range,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FoldingRange {
    pub start_line: u32,
    pub end_line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<&'static str>,
}

/// `prepareRename`'s answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PrepareRename {
    pub range: Range,
    pub placeholder: String,
}

/// A quick fix: its title, the problem it fixes, and its edits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeAction {
    pub title: String,
    pub kind: &'static str,
    pub diagnostic: Diagnostic,
    pub edits: Vec<TextEdit>,
}

/// The semantic token types, in the order the server's legend lists them.
pub const LEGEND: [&str; 11] = [
    "keyword",
    "type",
    "function",
    "method",
    "macro",
    "variable",
    "enumMember",
    "string",
    "number",
    "comment",
    "parameter",
];

/// A semantic token's type: its index in [`LEGEND`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Keyword,
    Type,
    /// A builtin call.
    Function,
    /// A script: a call, a reference or its name.
    Method,
    /// A builtin that reads its own arguments (`begin`, `if`, `set`).
    Macro,
    /// A global.
    Variable,
    /// A scenario name.
    EnumMember,
    String,
    Number,
    Comment,
    Parameter,
}

impl TokenKind {
    pub fn index(self) -> u32 {
        self as u32
    }
}

/// One semantic token on one line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub line: u32,
    /// UTF-16 column.
    pub start: u32,
    /// UTF-16 length.
    pub len: u32,
    pub kind: TokenKind,
}

/// Every problem of the text against the context.
pub fn diagnostics(src: &str, ctx: &Context) -> Vec<Diagnostic> {
    model::Model::new(src, ctx).diagnostics
}

/// One definition of the text, for an outside reader (Studio compares a
/// working copy with the map's scripts by these).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Definition {
    pub item: DefItem,
    pub name: String,
    /// The script kind, or `global`.
    pub kind: String,
    /// The 0-based line it starts on.
    pub line: u32,
    /// Its text, `(` to `)`.
    pub text: String,
}

/// The text's definitions in order.
pub fn definitions(src: &str, ctx: &Context) -> Vec<Definition> {
    let m = model::Model::new(src, ctx);
    m.defs
        .iter()
        .map(|d| Definition {
            item: d.item,
            name: d.name.clone(),
            kind: d.kind.clone(),
            line: m.lines.position(d.span.0).line,
            text: src[d.span.0..d.span.1].to_owned(),
        })
        .collect()
}

/// A definition's text without its layout and comments: tokens joined by
/// one space, none inside the parentheses. Two texts that differ only in
/// layout or comments normalise the same.
pub fn normalise(text: &str) -> String {
    let (lexemes, _) = syntax::lex(text);
    let mut out = String::with_capacity(text.len());
    let mut last: Option<syntax::Tok> = None;
    for lx in lexemes {
        if lx.kind == syntax::Tok::Comment {
            continue;
        }
        let tight = matches!(
            (last, lx.kind),
            (None, _) | (Some(syntax::Tok::Open), _) | (_, syntax::Tok::Close)
        );
        if !tight {
            out.push(' ');
        }
        out.push_str(&text[lx.start..lx.end]);
        last = Some(lx.kind);
    }
    out
}

/// A UTF-16 position's byte offset in `src`.
pub fn offset(src: &str, pos: Position) -> usize {
    syntax::Lines::new(src).offset(pos)
}

/// A byte offset's UTF-16 position in `src`.
pub fn position(src: &str, byte: usize) -> Position {
    syntax::Lines::new(src).position(byte)
}
