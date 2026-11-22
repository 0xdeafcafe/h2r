//! The rules language: lexer, parser, checker and lowering to IR.
//!
//! Compiling is deterministic: the same source always gives the same
//! blob, so the blob's hash names the script. The checker
//! ([`check::check`]) reports every error it finds, up to
//! [`MAX_ERRORS`]; lowering reads a checked script and makes no type
//! decision. See `docs/game/rules-engine.md`, "The language", and
//! `docs/game/rules-language-types.md`, "The front end".

pub mod ast;
pub mod check;
pub mod lex;
pub(crate) mod lower;
pub mod parse;
pub mod sig;
pub mod types;

use crate::ir::Ir;

/// The most source bytes the compiler reads.
pub const MAX_SOURCE_BYTES: usize = 256 * 1024;

/// A line and column, both from 1.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pos {
    pub line: u32,
    pub col: u32,
}

/// A compile error at a line and column.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{line}:{col}: {message}")]
pub struct CompileError {
    pub line: u32,
    pub col: u32,
    pub message: String,
}

impl CompileError {
    pub fn at(pos: Pos, message: impl Into<String>) -> CompileError {
        CompileError {
            line: pos.line,
            col: pos.col,
            message: message.into(),
        }
    }
}

/// The most errors one compile reports.
pub const MAX_ERRORS: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Error,
    /// The script compiles; the warning names something it likely does
    /// not mean.
    Warning,
}

/// One finding of a compile: a stable code (`null.use`, `label.unknown`),
/// its severity, where, and the message. The codes are those of
/// `docs/game/rules-language-types.md`, "Diagnostics".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: &'static str,
    pub severity: Severity,
    pub pos: Pos,
    pub message: String,
}

impl Diagnostic {
    fn error(code: &'static str, e: CompileError) -> Diagnostic {
        Diagnostic {
            code,
            severity: Severity::Error,
            pos: Pos {
                line: e.line,
                col: e.col,
            },
            message: e.message,
        }
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// The diagnostic as a [`CompileError`] (line, column and message).
    pub fn to_error(&self) -> CompileError {
        CompileError::at(self.pos, self.message.clone())
    }
}

/// What a compile gives: the IR when there is no error, and every
/// diagnostic, errors first in source order, then warnings in source
/// order.
#[derive(Clone, Debug)]
pub struct Compiled {
    pub ir: Option<Ir>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Compiled {
    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics.iter().filter(|d| d.is_error())
    }

    pub fn warnings(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics.iter().filter(|d| !d.is_error())
    }
}

/// Compiles source text and reports every finding: syntax errors, then
/// (when the source parses) the checker's errors and warnings. Lowering
/// runs only on a script with no error; a limit it meets is a `limit`
/// error.
pub fn diagnose(src: &str) -> Compiled {
    if src.len() > MAX_SOURCE_BYTES {
        let e = CompileError::at(
            Pos { line: 1, col: 1 },
            format!("the source is {} bytes, over the 256 KiB cap", src.len()),
        );
        return Compiled {
            ir: None,
            diagnostics: vec![Diagnostic::error("limit", e)],
        };
    }
    let (tokens, mut errors) = lex::lex_tolerant(src);
    let (script, parse_errors) = parse::parse_recovering(&tokens);
    errors.extend(parse_errors);
    if !errors.is_empty() {
        let mut diagnostics: Vec<Diagnostic> = errors.into_iter().map(|e| Diagnostic::error("syntax", e)).collect();
        diagnostics.sort_by_key(|d| d.pos);
        diagnostics.truncate(MAX_ERRORS);
        return Compiled { ir: None, diagnostics };
    }
    let checked = check::check(&script);
    let mut diagnostics = checked.diagnostics.clone();
    let ir = if checked.has_errors() {
        None
    } else {
        match lower::lower(&checked) {
            Ok(ir) => Some(ir),
            Err(e) => {
                diagnostics.push(Diagnostic::error("limit", e));
                None
            }
        }
    };
    diagnostics.sort_by_key(|d| (d.severity, d.pos));
    Compiled { ir, diagnostics }
}

/// Compiles source text to IR, or gives the first error. The result
/// still goes through [`crate::verify`] before it runs. [`diagnose`]
/// gives every error and the warnings.
pub fn compile(src: &str) -> Result<Ir, CompileError> {
    let compiled = diagnose(src);
    match compiled.ir {
        Some(ir) => Ok(ir),
        None => Err(compiled
            .errors()
            .next()
            .map(Diagnostic::to_error)
            .unwrap_or_else(|| CompileError::at(Pos { line: 1, col: 1 }, "internal: no IR and no error"))),
    }
}
