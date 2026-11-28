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
//! - [`object`], [`sound`], [`character`], [`coln`], [`decal`], [`light`]:
//!   each part's leaf calls and draws, as the switch's `obje`, `snd!`,
//!   `char`, `coln`, `deca` and `ligh` arms make them.
//! - [`random`]: the shared stream's two draws, run on the guest's x87
//!   state with the recompiler's x87 helpers.
//! - [`dispatch`]: what the switch's Rust entry does with each part.
//! - [`scene`]: decal, light and lens flare parts as events for the
//!   renderer's scene. They live in Rust, never in guest memory, so the
//!   arena hash does not see them.
//!
//! A part Rust cannot answer panics with its cause; nothing runs the
//! guest's arm.

#![forbid(unsafe_code)]

pub mod character;
pub mod coln;
pub mod damage;
pub mod decal;
pub mod dispatch;
pub mod light;
pub mod object;
pub mod parts;
pub mod random;
pub mod scene;
pub mod sound;
pub mod walk;

pub use parts::{Part, PartClass};
