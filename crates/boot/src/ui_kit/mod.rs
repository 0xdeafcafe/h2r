//! The UI kit: screens of ours over [`crate::list_class`], declared rather
//! than hand-wired, so the mistakes earlier screens made cannot be made
//! again (docs/game/ui-screens.md).
//!
//! - [`ids`]: the one registry of string-id families, checked at compile
//!   time.
//! - [`input`]: events as inputs, named glyphs and legends, the opener's
//!   press guard.
//! - [`lifecycle`]: close as B does, and work that waits for the free.

pub mod ids;
pub mod input;
pub mod lifecycle;
