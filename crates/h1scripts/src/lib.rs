//! Halo 1 campaign scripts, decoded from the scenario tag and carried over to
//! Halo 2.
//!
//! - [`layout`] reads Halo 1's compiled script tables (syntax nodes, string
//!   data, scripts, globals, references) from a scenario.
//! - [`catalog`] reads Halo 1's builtin function, value type and script type
//!   tables from Halo 1's own XBE.
//! - [`decompile`] renders the tables as HSC text and checks every node.
//! - [`translate`] maps Halo 1 builtins and types to Halo 2's.
//! - [`compile`] builds Halo 2 syntax nodes and scenario blocks from the
//!   translation, for the converter to drop in.
//!
//! No game data is in this crate or its tests: tables and scripts are read
//! from the owner's disc at run time. See `docs/halo1/scripts.md`.
#![forbid(unsafe_code)]

pub mod catalog;
pub mod compile;
pub mod decompile;
pub mod hud;
pub mod identified;
pub mod layout;
pub mod level;
pub mod seats;
pub mod translate;

pub use catalog::Catalog;
pub use layout::Tables;

/// A malformed table, an unsupported XBE, or a script that will not compile.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Reading the Halo 1 map failed.
    #[error(transparent)]
    Map(#[from] h1map::Error),
    /// Reading an XBE failed.
    #[error(transparent)]
    Xbe(#[from] xbe::Error),
    /// The Halo 2 side rejected something.
    #[error(transparent)]
    Hs(#[from] hs::Error),
    /// A structural rule was broken.
    #[error("Halo 1 scripts: {0}")]
    Invalid(String),
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}

pub(crate) fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

pub(crate) fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

#[cfg(test)]
mod tests;
