//! The collision world for the simulation rewrite, phase 2
//! (docs/engine/sim-rewrite-collision.md; the guest contract is
//! docs/engine/collision-queries.md).
//!
//! Each collision BSP the guest names is decoded once, from the bytes the
//! guest itself would read, into flat arrays ([`Bsp`]), and each MOPP
//! program into a flat tree ([`mopp::Tree`]). A query is then one
//! Rust call over those arrays and a few live guest words read through
//! [`GuestRead`]. The structure's portal graph is decoded the same way
//! ([`Topology`]) for the cluster flood and the object visit. Nothing here
//! writes guest memory: a kernel returns plain
//! result structs, and the engine adapter (`engine::sim_collision`) writes
//! them back once the whole answer is known.
//!
//! The bar is bit-exact with retail. Arithmetic keeps the retail operation
//! order through `sim_core::det`. A query the kernel cannot reproduce
//! exactly (an operand off the fast path, data outside what the decoder
//! models, recursion past [`bsp::MAX_DEPTH`]) returns a [`Bail`] before any
//! guest write, and the lifted code runs instead.
#![deny(clippy::disallowed_methods)]

pub mod batch;
pub mod bsp;
pub mod flood;
pub mod instance;
pub mod mem;
pub mod model;
pub mod objects;
pub mod mopp;
pub mod sphere;
pub mod structure;
mod store;
mod world;

#[cfg(any(test, feature = "synth"))]
pub mod synth;

pub use bsp::segment::{Hit, Outcome, Segment};
pub use bsp::{Bsp, Consts};
pub use flood::{Flood, FloodQuery, Topology, TopologyKey};
pub use mem::{Fault, GuestRead};
pub use objects::{Visit, VisitQuery};
pub use world::{
    CollisionWorld, Entry, EntryStats, RayParity, SourceFingerprint, Stats,
    VectorParity, take_bail,
};

/// Which of a region's float units rounded: the PE (precision) flag each
/// sets in the guest, MXCSR bit 5 for SSE and status bit 5 for the x87.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Inexact {
    pub sse: bool,
    pub x87: bool,
}

impl Inexact {
    pub fn of(r: &sim_core::det::Checked) -> Self {
        Self {
            sse: r.sse_inexact(),
            x87: r.x87_inexact(),
        }
    }
}

/// Why a query was left to the lifted code. Nothing was written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Bail {
    /// MXCSR or the x87 control and stack outside what the kernels
    /// reproduce: MXCSR not masked and round-to-nearest
    /// (`sim_core::det::guest_env`; PE need not be sticky), or the x87
    /// outside `x87_env`.
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
    /// The resident budget or bounded address index cannot admit this BSP
    /// or MOPP program.
    Budget,
    /// The guest's output array is too small for the answer: the original
    /// grows it with a guest call (entry F's `hkArray`).
    Grow,
}

/// The number of [`Bail`] reasons.
pub const BAILS: usize = 9;

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
        Bail::Grow,
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
            Bail::Grow => "grow",
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
