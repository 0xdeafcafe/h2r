//! Runtime the recompiled code links against.
//!
//! Nothing runs yet. This crate fixes the shapes that both the generated
//! code and the hand-written Rust overrides are written against: guest
//! memory, CPU state, and the dispatch table that decides per address
//! whether the generated or the rewritten version of a function runs.

pub mod cpu;
pub mod dispatch;
pub mod guest;

pub use cpu::Cpu;
pub use dispatch::{Dispatch, GuestFn};
pub use guest::Guest;
