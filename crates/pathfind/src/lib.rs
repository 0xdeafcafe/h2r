//! Halo 2 AI pathfinding data: generate it from a structure BSP's collision
//! geometry, compare it against retail data, and emit it as a
//! `map::build` block tree. Format: docs/formats/pathfinding.md.
//!
//! ```no_run
//! # fn f(bsp: &tags::collision::Bsp) {
//! let opts = pathfind::generate::Options::default();
//! let pf = pathfind::generate::generate(bsp, 0, 0, &opts).unwrap();
//! assert!(pf.validate(Some(bsp.surfaces.len())).is_empty());
//! let element = pathfind::build::element(&pf).unwrap();
//! // sbsp_block.block(0xc4, 1, element);
//! # }
//! ```
pub mod build;
pub mod compare;
pub mod generate;
pub mod geom;
