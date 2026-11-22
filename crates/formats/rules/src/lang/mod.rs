//! The rules language: lexer, parser, type checker and lowering to IR.
//!
//! Compiling is deterministic: the same source always gives the same
//! blob, so the blob's hash names the script. See
//! `docs/game/rules-engine.md`, "The language".

pub mod ast;
pub mod lex;
pub(crate) mod lower;
pub mod parse;

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

/// Compiles source text to IR. The result still goes through
/// [`crate::verify`] before it runs.
pub fn compile(src: &str) -> Result<Ir, CompileError> {
    if src.len() > MAX_SOURCE_BYTES {
        return Err(CompileError::at(
            Pos { line: 1, col: 1 },
            format!("the source is {} bytes, over the 256 KiB cap", src.len()),
        ));
    }
    let tokens = lex::lex(src)?;
    let script = parse::parse(&tokens)?;
    lower::lower(&script)
}
