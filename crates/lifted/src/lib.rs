//! The lifted original: the image's own code, lifted and run on the IR
//! interpreter (docs/engine/shadow-exec.md).
//!
//! One crate for both places a port is checked against it:
//!
//! - **Differential tests** (`crates/engine/tests/*_differential.rs`):
//!   [`image::Image`] loads the user's XBE once per test binary (pinned by
//!   hash, skipped without it), [`code::Original`] lifts what the
//!   interpreter reaches, [`cpu`] converts between the runtime's and the
//!   interpreter's register files, and [`code::step`], [`code::segment`]
//!   and [`code::run_until`] drive it.
//! - **Shadow checks in a running game** ([`shadow`]): a port's call is
//!   run on a copy of guest memory and the original on another, from the
//!   same entry state the game reached, and compared. The agent API turns
//!   it on (`shadow.enable`) and reads it (`shadow.status`).

pub mod code;
pub mod cpu;
pub mod image;
pub mod shadow;

pub use code::{Assembled, Code, Original, StepError};
