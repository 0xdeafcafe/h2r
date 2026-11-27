//! The effects phase of the tick, independent of guest memory
//! (docs/engine/effects-parts.md).
//!
//! - [`walk`]: the effect array walk of the phase entry `0x176e40`, in
//!   retail order. `engine::sim_effects::phase` runs it from the tick
//!   driver, so the driver makes no guest call for the phase.
//! - [`parts`]: the part creation switch `0x17a8a0` names each part's class
//!   by its tag group, and whether it touches the sim.
//! - [`damage`]: a damage part's record (`0x88` bytes), built exactly as
//!   the switch's `jpt!` arm builds it before it calls the area damage.
//! - [`arm`]: the damage arm's proof: compared with the guest until it has
//!   matched [`arm::PROVEN_AFTER`] parts, then answering, one part in
//!   [`arm::AUDIT_EVERY`] still compared, the guest on any difference.
//! - [`dispatch`]: what the switch's Rust entry does with each part.
//! - [`scene`]: decal, light and lens flare parts as events for the
//!   renderer's scene. They live in Rust, never in guest memory, so the
//!   arena hash does not see them.
//!
//! Refusals are named counters in each module's stats.

#![forbid(unsafe_code)]

pub mod arm;
pub mod damage;
pub mod dispatch;
pub mod parts;
pub mod scene;
pub mod walk;

pub use parts::{Part, PartClass};
