//! The UI kit: screens of ours over [`crate::list_class`], declared rather
//! than hand-wired, so the mistakes earlier screens made cannot be made
//! again (docs/game/ui-screens.md).
//!
//! - [`choice`]: a row of many choices over a setting ([`choice::ChoiceRow`]).
//! - [`dialog`]: modal dialogs over the screen beneath ([`dialog::DialogSpec`]).
//! - [`dialog_tag`]: a dialog's tag pieces and tag copy, and the request
//!   that opens it as the game opens its dialogs.
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
//! - [`picker`]: the value picker, retail's `low_level_settings`.
//! - [`page`]: a screen's behaviour as a pure state machine, table-tested.

pub mod choice;
pub mod dialog;
pub mod dialog_tag;
pub mod ids;
pub mod input;
pub mod layout;
pub(crate) mod level_art;
pub mod lifecycle;
pub mod page;
pub mod picker;
pub mod preview;
pub mod screen;
pub mod screens;
pub mod spec;
pub(crate) mod text;
pub(crate) mod text_fit;
pub mod view;
