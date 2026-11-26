//! Developer textures for Forge block-out, drawn by code.
//!
//! A measured grid in the colour of a face's role ([`tile`]), a font atlas
//! ([`font`]) and the layout of the plate that says what a face is and how
//! big it is ([`label::layout`]). Nothing here reads an image or a font
//! file: the rasters are a pure function of [`VERSION`]. The design is in
//! `docs/game/forge-blockout.md`, "The dev textures".
#![forbid(unsafe_code)]

pub mod family;
pub mod font;
mod image;
pub mod label;
mod tile;

pub use family::Family;
pub use image::Image;
pub use tile::{line_colours, tile, TEXELS_PER_UNIT, TILE_SIZE, TILE_UNITS};

/// Bumped by any change to a raster or to the label layout. A cache of
/// encoded images is keyed by it.
pub const VERSION: u32 = 1;
