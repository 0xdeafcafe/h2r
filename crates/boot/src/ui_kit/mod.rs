//! The UI kit: screens of ours over [`crate::list_class`], declared rather
//! than hand-wired, so the mistakes earlier screens made cannot be made
//! again (docs/game/ui-screens.md).
//!
//! - [`ids`]: the one registry of string-id families, checked at compile
//!   time.
//! - [`input`]: events as inputs, named glyphs and legends, the opener's
//!   press guard.
//! - [`lifecycle`]: close as B does, and work that waits for the free.
//! - [`spec`]: a screen as data ([`spec::Describe`]) and the static checks.
//! - [`screen`]: a declared screen ([`screen::KitScreen`]) on the list
//!   class.
//! - [`screens`]: our existing screens described, with samples.
//! - [`preview`]: a screen drawn offline from the disc's tags and fonts.

pub mod ids;
pub mod input;
pub mod layout;
pub mod lifecycle;
pub mod preview;
pub mod screen;
pub mod screens;
pub mod spec;
pub mod view;
