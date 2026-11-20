//! Typed guest layouts: one definition per Halo 2 structure, and the one
//! table of guest global addresses (docs/engine/guest-layout.md).
//!
//! A layout is declared once with [`layout!`]: a field's offset, type and
//! evidence sit on one line, and readers name the field (`Unit::PLAYER`,
//! `unit.player(m)`) instead of adding `0x13c`. Views are an address and
//! nothing else; every getter reads at the moment it is called, through
//! any [`Mem`]. The getters are generic, so through a concrete memory a
//! field read is a bounds check and a load; through `dyn` memory (what
//! engine code holds) it is one virtual call, as before.
//!
//! The guest-memory boundary ([`memory`], [`testing`]), datum arrays and
//! handles ([`datum`], [`handle`]), the tag index ([`tags`]), typed views
//! ([`view`], `record!`) and the one table of named guest functions and
//! globals ([`points`]) live here too; `engine` re-exports them.
//!
//! What sits on top: `sim-state` (docs/engine/sim-review.md
//! lane 6) builds `ObjectView`/`UnitView` and `SimRead` over [`object`],
//! [`unit`], [`player`] and [`game`] (docs/engine/sim-rewrite-objects.md
//! section 8). Bots, telemetry and the agent API read the same
//! definitions, so a layout fix lands once.
//!
//! No game data here: offsets and addresses are research notes, each with
//! its evidence. Tests use synthetic memory.

// The moved engine modules say `guest_layout::` (S2, docs/dev/crate-structure.md).
extern crate self as guest_layout;

#[macro_use]
mod macros;
mod field;
mod mem;

// The guest boundary and the named points, from `engine`, which
// re-exports them (docs/dev/crate-structure.md, S2).
pub mod datum;
pub mod handle;
pub mod memory;
pub mod points;
pub mod tags;
pub mod testing;
pub mod view;

pub use handle::DatumHandle;
pub use memory::{FlatMemory, GuestMemory, MemFault};

pub mod damage;
pub mod datum_array;
pub mod game;
pub mod global;
pub mod object;
pub mod player;
pub mod sound;
pub mod tag;
pub mod unit;

pub use field::{Datum, Field, FieldInfo, MAX_FIELD, Off, View};
pub use mem::{Fault, FlatRam, Mem, MemMut};

pub use damage::DamageReport;
pub use datum_array::DatumArray;
pub use game::{GameEngine, GameGlobals, GameOptions, GameTime, GameVariant, PlayerGlobals};
pub use object::{Object, ObjectDefinition, ObjectHeader, ObjectType};
pub use player::Player;
pub use tag::{TagEntry, TagIndex};
pub use unit::{Biped, Device, Unit, Weapon};

#[cfg(test)]
mod tests;
