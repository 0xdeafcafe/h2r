//! Our host settings as a schema (docs/feature-engine.md "Settings").
//!
//! A setting is one [`Def`]: its key in `settings.toml`, its label and
//! help, what values it may hold ([`Kind`]), its default, the environment
//! variable and command-line flag that override it, and whether a change
//! applies at once or at the next launch ([`Apply`]). A [`Store`] loads every
//! `Def` from the file, the environment and the flags, hands out
//! [`Setting<T>`] handles whose read is one atomic load, and is the one
//! writer of the file.
//!
//! Precedence is flag, environment, file, default. A value from a flag or
//! the environment (or one the run forces, [`Store::force`]) is locked: a
//! row shows it and a press does nothing. A bad value anywhere is a
//! [`Problem`] naming the key; the next source is used. Nothing here
//! touches guest memory.

mod def;
mod file;
mod store;

pub use def::{Apply, Choice, Def, Kind, Menu, ParseFn, Scope, Tier, Unit, Value};
pub use file::{PROFILES, SETTINGS_FILE, path_in, write_profile_value, write_value};
pub use store::{Flag, Outcome, Problem, SetError, Setting, SettingType, Source, State, Store};
