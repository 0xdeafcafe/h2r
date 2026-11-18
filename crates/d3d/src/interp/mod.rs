//! Interpolated replay: in-between frames at the display rate while the
//! game keeps its 30 Hz tick (docs/game/high-framerate.md, "Design D" and
//! "Phase 2: what was built").
//!
//! - [`model`]: finds the matrices in each draw's vertex constants (the
//!   view's camera, objects drawn with it, draws with a projection of
//!   their own, node matrices, the eye position), pairs draws across two
//!   frames and gives the constants of an in-between frame.
//! - [`pacing`]: the blend fraction from the frames' arrival times and the
//!   display schedule.
//! - [`backend`]: [`InterpBackend`], the [`crate::Backend`] wrapper that
//!   records, replays and presents, on the render thread.
//! - [`camera`] and [`math`]: perspective matrices split and rebuilt.

pub mod backend;
pub mod camera;
pub mod math;
pub mod model;
pub mod pacing;

pub use backend::{InterpBackend, InterpStats};
pub use pacing::Rate;
