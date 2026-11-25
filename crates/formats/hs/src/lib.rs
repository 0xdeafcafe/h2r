//! Halo 2 retail Xbox compiled HaloScript, decoded from the owner's cache/XBE.
//! No extracted tables or scripts are build inputs. See docs/formats/haloscript.md.
//!
//! Invariant-bearing storage cannot be changed through the public API:
//! ```compile_fail
//! fn mutate(scenario: &mut hs::Scenario) { scenario.nodes.clear(); }
//! ```
//! ```compile_fail
//! fn mutate(node: &mut hs::Node) { node.data = 0; }
//! ```
//! ```compile_fail
//! fn mutate(catalog: &mut hs::Catalog) { catalog.functions.clear(); }
//! ```
#![forbid(unsafe_code)]

mod cast;
mod catalog;
mod check;
mod compiler;
mod expression;
#[cfg(any(test, feature = "fixture"))]
pub mod fixture;
pub mod h1_conversation;
pub mod ide;
pub mod recording;
pub mod recording_edit;
mod scenario;
mod script_doc;
mod source;
pub mod table_writes;
pub mod vm;
mod wire;

pub use cast::{Unrunnable, castable};
pub mod h1_ai_slots;
pub mod casts {
    pub use crate::cast::types;
}
pub use catalog::*;
pub use check::*;
pub use compiler::{
    Compiled, MAX_DEPTH, Message, Options, Resolve, compile, first_difference,
};
pub use expression::*;
pub use scenario::*;
pub use script_doc::*;
pub use source::*;
pub use wire::*;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Map(#[from] map::Error),
    #[error(transparent)]
    Xbe(#[from] xbe::Error),
    #[error("HaloScript: {0}")]
    Invalid(String),
}
pub type Result<T> = std::result::Result<T, Error>;

fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}

#[cfg(test)]
mod tests;
