//! Shared ground for the native simulation subsystems
//! (docs/engine/sim-rewrite.md): bit-level math types, the exact SSE-order
//! `f32x4` lanes, the float rules every kernel follows, and the epoch that
//! invalidates derived state.
//!
//! No `runtime` dependency: a kernel built on this crate never sees guest
//! memory. Its adapter gathers, the kernel computes, the adapter scatters.
#![deny(clippy::disallowed_methods)]

pub mod det;
pub mod epoch;
pub mod lanes;
pub mod math;

pub use epoch::Epoch;
