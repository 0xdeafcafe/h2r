//! Halo 2 retail Xbox compiled HaloScript, decoded from the owner's cache/XBE.
//! No extracted tables or scripts are build inputs. See docs/haloscript.md.
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

mod catalog;
mod check;
mod expression;
mod scenario;
mod source;
pub mod vm;
mod wire;

pub use catalog::*;
pub use check::*;
pub use expression::*;
pub use scenario::*;
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
