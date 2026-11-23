//! The pogo stick's kit: three looks of one stick, each built from a
//! parametric source and a painted atlas under `art/pogo/` into the tags a
//! map needs to hold it as a weapon.
//!
//! - [`source`]: the YAML sources and atlases, embedded, and their parser.
//! - [`mesh`]: a source's lathes, sweeps and boxes as triangles.
//! - [`texture`]: an atlas as a `bitm`.
//! - [`kit`]: one look as `bitm`, `shad`, `mode`, `hlmt` and `weap` in a
//!   map's tag set.
//! - [`inject`]: the kit added to a multiplayer map's bytes.
//! - [`rewrite`]: every multiplayer map served with the kit, cached.
//! - [`placed`]: which weapon tags the map being played holds.
//!
//! Every look shares one rig ([`RIG`]): the same grips, pegs, foot and
//! piston travel, so the code that poses a rider reads no look.

pub mod inject;
pub mod kit;
pub mod mesh;
pub mod rewrite;
pub mod source;
pub mod texture;

#[cfg(test)]
mod inject_tests;
#[cfg(test)]
mod tests;

pub use pogo::*;
