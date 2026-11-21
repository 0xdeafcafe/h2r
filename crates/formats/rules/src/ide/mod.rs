//! Language intelligence for the rules language: diagnostics, completion,
//! hover, signature help, definitions and references, semantic tokens, the
//! outline and a formatter.
//!
//! Pure over source text, like the rest of the crate: Live's
//! `POST /v1/rules/ide`, `h2 rules lsp` and the web editor share it.
//! Positions are the Language Server Protocol's: a 0-based line and a
//! UTF-16 `character`, and the result types serialize with LSP field names.
//! Every feature tolerates broken source: the lexer and parser recover
//! (`lex::lex_tolerant`, `parse::parse_recovering`) and the features work
//! from what parsed. See `docs/game/rules-engine.md`, "Tooling".

use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};

mod complete;
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

/// A problem in the source.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub range: Range,
    /// A [`severity`] number.
    pub severity: u8,
    pub message: String,
    /// `syntax` (lexer or parser), `compile` (types and names), `verify`
    /// (a refusal, the tick budget or the IR size, about the whole
    /// script), `unused` or `unhostable`.
    pub code: &'static str,
    /// Always `"rules"`.
    pub source: &'static str,
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

/// Every problem: syntax errors (all of them, from the recovering
/// parser), then, when the source parses, the compiler's first error or
/// the verifier's, budget and size problems; warnings for a `let`, a set
/// member or a `fn` never used; and a note at each call of an entry no
/// host can make yet.
pub fn diagnostics(src: &str) -> Vec<Diagnostic> {
    let m = model::analyze(src);
    info::diagnostics(&m)
}

/// The completions at `pos`, by context: declarations at the top level,
/// event names after `on`, capabilities after `requires`, carriers after
/// `carrier`, types after `:`, fields after `.`, and names, API entries
/// and statements in a body.
pub fn complete(src: &str, pos: Position) -> Vec<CompletionItem> {
    let m = model::analyze(src);
    let off = m.li.offset(pos);
    complete::complete(&m, off)
}

/// What the token at `pos` is: a name's declaration and type, an API
/// entry's signature, capability and status, an event, a keyword.
pub fn hover(src: &str, pos: Position) -> Option<Hover> {
    let m = model::analyze(src);
    let off = m.li.offset(pos);
    info::hover(&m, off)
}

/// The signature of the call (or `on` handler) around `pos`.
pub fn signature_help(src: &str, pos: Position) -> Option<SignatureHelp> {
    let m = model::analyze(src);
    let off = m.li.offset(pos);
    info::signature_help(&m, off)
}

/// Where the name at `pos` is declared (a label's first use).
pub fn definition(src: &str, pos: Position) -> Vec<Location> {
    let m = model::analyze(src);
    let off = m.li.offset(pos);
    info::definition(&m, off)
}

/// Every use of the name at `pos`, its declaration first.
pub fn references(src: &str, pos: Position) -> Vec<Location> {
    let m = model::analyze(src);
    let off = m.li.offset(pos);
    info::references(&m, off)
}

/// Every comment, literal, keyword, name and operator, classified, in
/// source order.
pub fn semantic_tokens(src: &str) -> Vec<Token> {
    let m = model::analyze(src);
    info::semantic_tokens(&m)
}

/// The document outline: the header, then each declaration and handler.
pub fn symbols(src: &str) -> Vec<Symbol> {
    let m = model::analyze(src);
    info::symbols(&m)
}
