//! The host lanes now live in `sim_core::lanes`, shared with the native
//! subsystems (docs/engine/sim-rewrite.md section 3). The per-function
//! ports keep using them through this path.
pub use sim_core::lanes::*;

#[cfg(test)]
mod tests;
