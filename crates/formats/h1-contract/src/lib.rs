//! The names both Halo 1 halves share: what the converters (`h1text`,
//! `h1scripts`, `h1convert`, in `crates/tools`) write into a converted map,
//! and what the game's Halo 1 hooks (`boot`) read back from guest memory.
//!
//! Only names, offsets, markers, slot tables and small layout types live
//! here, so the in-game half needs no converter crate
//! (docs/dev/crate-structure.md, "W3. The Halo 1 merge"). The converters
//! re-export each item from the module it was written beside, so
//! `h1text::hud::SCENARIO_HELP` and `h1_contract::hud::SCENARIO_HELP` are
//! the same constant.
//!
//! - [`hud`]: the converted `hmt ` tag's runtime fields and the controller
//!   glyphs of a mission message (docs/halo1/text.md).
//! - [`hud_builtins`]: the retired Halo 2 script slots the HUD controls
//!   and the scenario help list are installed on.
//! - [`cinematics`]: the checkpointed scenario globals of the cinematic
//!   effects.
//! - [`unit_builtins`]: the retail slots the host answers for converted
//!   campaigns (docs/halo1/script-builtins.md).
//! - [`script_fields`]: the Halo 2 scenario fields the script tables go in.
//! - [`shader`]: object function names and the model reflection marker.
//! - [`campaign`]: the campaign's levels in play order.
//!
//! No game data is in this crate.
#![forbid(unsafe_code)]

pub mod campaign;
pub mod cinematics;
pub mod hud;
pub mod hud_builtins;
pub mod script_fields;
pub mod shader;
pub mod unit_builtins;
