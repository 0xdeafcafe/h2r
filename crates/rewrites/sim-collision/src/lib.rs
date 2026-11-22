//! The collision world for the simulation rewrite, phase 2
//! (docs/engine/sim-rewrite-collision.md; the guest contract is
//! docs/engine/collision-queries.md).
//!
//! Each collision BSP the guest names is decoded once, from the bytes the
//! guest itself would read, into flat arrays ([`Bsp`]). A query is then one
//! Rust call over those arrays and a few live guest words read through
//! [`GuestRead`]. Nothing here writes guest memory: a kernel returns plain
//! result structs, and the engine adapter (`engine::sim_collision`) writes
//! them back once the whole answer is known.
//!
//! The bar is bit-exact with retail. Arithmetic keeps the retail operation
//! order through `sim_core::det`. A query the kernel cannot reproduce
//! exactly (an operand off the fast path, data outside what the decoder
//! models, recursion past [`bsp::MAX_DEPTH`]) returns a [`Bail`] before any
//! guest write, and the lifted code runs instead.
#![deny(clippy::disallowed_methods)]

pub mod bsp;
pub mod mem;
pub mod structure;
mod world;

#[cfg(any(test, feature = "synth"))]
pub mod synth;

pub use bsp::segment::{Hit, Outcome, Segment};
pub use bsp::{Bsp, Consts};
pub use mem::{Fault, GuestRead};
pub use world::{CollisionWorld, Entry, EntryStats, Stats};

/// Why a query was left to the lifted code. Nothing was written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Bail {
    /// MXCSR or the x87 control and stack outside what the kernels
    /// reproduce (`sim_core::det::fast_env`, `x87_env`).
    Env = 0,
    /// An operand off the fast path (`sim_core::det::Fp`).
    Float,
    /// A guest read faulted.
    Fault,
    /// Data the decoder does not model: an index out of range, a plane
    /// that is not ordinary, a ring that does not close.
    Data,
    /// Recursion deeper than [`bsp::MAX_DEPTH`].
    Depth,
    /// An output or context record overlaps an input, the BSP or the
    /// stack the original would write.
    Alias,
    /// The debugger needs the original guest memory access sequence.
    Trap,
    /// The resident budget or bounded address index cannot admit this BSP.
    Budget,
}

/// The number of [`Bail`] reasons.
pub const BAILS: usize = 8;

impl Bail {
    pub const ALL: [Bail; BAILS] = [
        Bail::Env,
        Bail::Float,
        Bail::Fault,
        Bail::Data,
        Bail::Depth,
        Bail::Alias,
        Bail::Trap,
        Bail::Budget,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Bail::Env => "env",
            Bail::Float => "float",
            Bail::Fault => "fault",
            Bail::Data => "data",
            Bail::Depth => "depth",
            Bail::Alias => "alias",
            Bail::Trap => "trap",
            Bail::Budget => "budget",
        }
    }
}

impl From<Fault> for Bail {
    fn from(_: Fault) -> Self {
        Bail::Fault
    }
}

#[cfg(test)]
mod tests;
