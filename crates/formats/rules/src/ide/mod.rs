//! Language intelligence for the rules language: diagnostics with their
//! fixes, code actions, completion, hover, signature help, definitions,
//! references, rename, semantic tokens, the outline and a formatter.
//!
//! Pure over source text, like the rest of the crate: Live's
//! `POST /v1/rules/ide`, `h2 rules lsp` and the web editor share it.
//! Positions are the Language Server Protocol's: a 0-based line and a
//! UTF-16 `character`, and the result types serialize with LSP field names.
//! Every feature tolerates broken source: the lexer and parser recover
//! (`lex::lex_tolerant`, `parse::parse_recovering`) and the features work
//! from what parsed. Types, labels and diagnostics are the checker's
//! (`lang::check::Checked`), read once per request in `model`. See
//! `docs/game/rules-engine.md`, "Tooling", and
//! `docs/game/rules-language-types.md`, "Diagnostics and the editor".

use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};

mod complete;
mod fix;
mod format;
mod info;
mod model;
mod scan;
#[cfg(test)]
mod tests;

pub use format::format;

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

/// Markdown text, as LSP's `MarkupContent`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Markup {
    /// Always `"markdown"`.
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

/// LSP's `DiagnosticSeverity`.
pub mod severity {
    pub const ERROR: u8 = 1;
    pub const WARNING: u8 = 2;
    pub const INFORMATION: u8 = 3;
    pub const HINT: u8 = 4;
}

/// A text replacement: LSP's `TextEdit`. An insertion has an empty
/// range.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextEdit {
    pub range: Range,
    pub new_text: String,
}

/// A fix of a diagnostic: the edits that remove it, and what they do.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Fix {
    /// "Change to `barrier`", "Declare label `barier`".
    pub title: String,
    /// In source order; none overlaps another.
    pub edits: Vec<TextEdit>,
}

/// A problem in the source.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub range: Range,
    /// A [`severity`] number.
    pub severity: u8,
    pub message: String,
    /// The checker's stable code (`label.unknown`, `null.use`, `syntax`,
    /// `limit`: `crate::Diagnostic::code`), or the editor's own: `verify`
    /// (a refusal, the tick budget or the IR size, about the whole
    /// script), `unused` or `unhostable`.
    pub code: &'static str,
    /// Always `"rules"`.
    pub source: &'static str,
    /// The fixes the editor offers, the likeliest first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fixes: Vec<Fix>,
}

/// A fix offered at a range: LSP's `CodeAction` of kind `quickfix`,
/// without the document's URI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CodeAction {
    pub title: String,
    /// Always `"quickfix"`.
    pub kind: &'static str,
    /// The diagnostic it fixes, without its fixes.
    pub diagnostic: Diagnostic,
    pub edits: Vec<TextEdit>,
}

/// LSP's `CompletionItemKind` numbers this module uses.
pub mod kind {
    pub const FUNCTION: u8 = 3;
    pub const FIELD: u8 = 5;
    pub const VARIABLE: u8 = 6;
    pub const MODULE: u8 = 9;
    pub const PROPERTY: u8 = 10;
    pub const VALUE: u8 = 12;
    pub const ENUM: u8 = 13;
    pub const KEYWORD: u8 = 14;
    pub const SNIPPET: u8 = 15;
    pub const ENUM_MEMBER: u8 = 20;
    pub const CONSTANT: u8 = 21;
    pub const STRUCT: u8 = 22;
    pub const EVENT: u8 = 23;
    pub const TYPE_PARAMETER: u8 = 25;
}

/// LSP's `InsertTextFormat`.
pub const PLAIN_TEXT: u8 = 1;
pub const SNIPPET: u8 = 2;

/// One completion.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionItem {
    pub label: String,
    /// A [`kind`] number.
    pub kind: u8,
    pub detail: String,
    pub documentation: Markup,
    pub insert_text: String,
    /// [`PLAIN_TEXT`] or [`SNIPPET`].
    pub insert_text_format: u8,
    /// The item's place in the list as text that sorts: the list is
    /// ranked (what the place wants first), so a client keeps its order.
    pub sort_text: String,
}

/// What hovering a place shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hover {
    pub markdown: String,
    /// The token hovered.
    pub range: Range,
}

/// LSP's `Hover`: `{contents: {kind, value}, range}`.
impl Serialize for Hover {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("Hover", 2)?;
        st.serialize_field("contents", &Markup::markdown(self.markdown.clone()))?;
        st.serialize_field("range", &self.range)?;
        st.end()
    }
}

/// A parameter of a signature.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Parameter {
    pub label: String,
    /// Where `label` sits in the signature's label, in UTF-16 units, for
    /// LSP's label offsets.
    #[serde(skip)]
    pub offsets: [u32; 2],
}

/// One signature of a call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Signature {
    pub label: String,
    pub documentation: Markup,
    pub parameters: Vec<Parameter>,
}

/// The call around a place and the argument the place is in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureHelp {
    pub signatures: Vec<Signature>,
    pub active_signature: u32,
    pub active_parameter: u32,
}

/// A span of the same document (LSP's `Location` without the URI).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Location {
    pub range: Range,
}

/// A semantic token type; its index in [`LEGEND`] is its number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TokenKind {
    Keyword,
    Event,
    Function,
    Api,
    Variable,
    Parameter,
    Enum,
    EnumMember,
    Type,
    Number,
    String,
    Comment,
    Operator,
    Label,
    Capability,
}

/// The semantic token legend, by [`TokenKind`] index. `api`, `label` and
/// `capability` are this language's own; the rest are LSP's standard
/// names.
pub const LEGEND: [&str; 15] = [
    "keyword",
    "event",
    "function",
    "api",
    "variable",
    "parameter",
    "enum",
    "enumMember",
    "type",
    "number",
    "string",
    "comment",
    "operator",
    "label",
    "capability",
];

impl TokenKind {
    pub fn index(self) -> u32 {
        self as u32
    }

    pub fn name(self) -> &'static str {
        LEGEND[self as usize]
    }
}

/// A classified token: a line, a UTF-16 start and length, one line long.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub line: u32,
    pub start: u32,
    pub len: u32,
    pub kind: TokenKind,
}

/// `{line, start, length, type, typeName}`: `type` is the legend index.
impl Serialize for Token {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("Token", 5)?;
        st.serialize_field("line", &self.line)?;
        st.serialize_field("start", &self.start)?;
        st.serialize_field("length", &self.len)?;
        st.serialize_field("type", &self.kind.index())?;
        st.serialize_field("typeName", self.kind.name())?;
        st.end()
    }
}

/// LSP's `SymbolKind` numbers this module uses.
pub mod symbol_kind {
    pub const MODULE: u8 = 2;
    pub const PROPERTY: u8 = 7;
    pub const FIELD: u8 = 8;
    pub const ENUM: u8 = 10;
    pub const FUNCTION: u8 = 12;
    pub const VARIABLE: u8 = 13;
    pub const CONSTANT: u8 = 14;
    pub const ARRAY: u8 = 18;
    pub const ENUM_MEMBER: u8 = 22;
    pub const STRUCT: u8 = 23;
    pub const EVENT: u8 = 24;
}

/// An entry of the document outline (LSP's `DocumentSymbol`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Symbol {
    pub name: String,
    pub detail: String,
    /// A [`symbol_kind`] number.
    pub kind: u8,
    pub range: Range,
    pub selection_range: Range,
    pub children: Vec<Symbol>,
}

/// Reads `src` once and hands the analysis to `f`.
fn with<R>(src: &str, f: impl FnOnce(&model::Model) -> R) -> R {
    let parsed = model::read(src);
    let m = model::analyze(src, &parsed);
    f(&m)
}

/// Every problem: syntax errors (all of them, from the recovering
/// parser), or, when the source parses, every diagnostic of the checker
/// (as `crate::diagnose` gives them: errors and warnings, each with its
/// code), then the verifier's, budget and size problems; warnings for a
/// `let`, a set member or a `fn` never used; and a note at each call of
/// an entry no host can make yet. Each carries the fixes the editor
/// offers.
pub fn diagnostics(src: &str) -> Vec<Diagnostic> {
    with(src, info::diagnostics)
}

/// The fixes of every diagnostic whose range meets `range`, in the
/// diagnostics' order (LSP's `textDocument/codeAction`).
pub fn actions(src: &str, range: Range) -> Vec<CodeAction> {
    with(src, |m| fix::actions(m, range))
}

/// The edits that rename the name at `pos` and every use of it. A label
/// keeps its map text: renaming `goal` to `finish` writes `label finish:
/// zone = "goal"`. An error says why the rename is refused: nothing to
/// rename there, a name that is not an identifier or is a keyword, or a
/// name the script declares already.
pub fn rename(src: &str, pos: Position, new_name: &str) -> Result<Vec<TextEdit>, String> {
    with(src, |m| fix::rename(m, m.li.offset(pos), new_name))
}

/// The completions at `pos`, by context, ranked: declarations at the top
/// level, event names after `on`, capabilities after `requires`, carriers
/// after `carrier`, types after `:`, label kinds and requirements on a
/// `label` line, fields after `.`, labels after `is`, and in a body names,
/// API entries and statements, those of the type the place wants first.
pub fn complete(src: &str, pos: Position) -> Vec<CompletionItem> {
    with(src, |m| complete::complete(m, m.li.offset(pos)))
}

/// What the token at `pos` is: a name's declaration and its type there,
/// an API entry's signature, capability and status, an event, a label and
/// what a map needs for it, a keyword.
pub fn hover(src: &str, pos: Position) -> Option<Hover> {
    with(src, |m| info::hover(m, m.li.offset(pos)))
}

/// The signature of the call (or `on` handler) around `pos`.
pub fn signature_help(src: &str, pos: Position) -> Option<SignatureHelp> {
    with(src, |m| info::signature_help(m, m.li.offset(pos)))
}

/// Where the name at `pos` is declared (a label's `label` line).
pub fn definition(src: &str, pos: Position) -> Vec<Location> {
    with(src, |m| info::definition(m, m.li.offset(pos)))
}

/// Every use of the name at `pos`, its declaration first.
pub fn references(src: &str, pos: Position) -> Vec<Location> {
    with(src, |m| info::references(m, m.li.offset(pos)))
}

/// Every comment, literal, keyword, name and operator, classified, in
/// source order.
pub fn semantic_tokens(src: &str) -> Vec<Token> {
    with(src, info::semantic_tokens)
}

/// The document outline: the header, then each declaration and handler.
pub fn symbols(src: &str) -> Vec<Symbol> {
    with(src, info::symbols)
}
