//! Guest memory to `FrameScene` (docs/graphics/framescene.md, section
//! 4.1): the only render code with guest offsets.
//!
//! [`layout`] holds the offsets, one file per guest record, moved
//! unchanged from `native-render`; [`extract`] builds a scene's views from
//! them on the sim thread. Until the stages that still read guest memory
//! at their hooks have moved here (slices 3 to 6), `native-render`
//! re-exports the layout modules at their old paths and [`GuestRead`] is
//! shared with them.
//!
//! ## Writes the tick reads
//!
//! Section 5.2 of the spec: each slice that replaces a guest entry adds a
//! row here naming the tick-visible writes it reproduces and the test
//! that proves each. Slice 1 replaces no entry: extraction only reads,
//! the guest's front end runs as before, and the stages that draw from
//! the scene skip guest items by the same masks as before.
//!
//! | Entry replaced | Writes reproduced | Test |
//! |---|---|---|
//! | (none yet) | | |

pub mod decorators;
pub mod extract;
mod guest;
pub mod late;
pub mod layout;
pub mod light_groups;
pub mod testing;

pub use guest::{GuestRead, GuestWrite};
