//! The Textures tool's canvas of h2-editor (docs/editor/modeling.md,
//! "Texture editing"). It knows nothing of Halo formats: `studio`
//! decodes a bitmap's images into [`Canvas`]es, performs [`BitmapOp`]s on
//! a [`Bitmap`] and (a later slice) encodes the changed tiles back.
//!
//! # The model
//!
//! - A **canvas** is one RGBA8 image in tiles of 256 pixels, each an
//!   `Arc`. Cloning it clones the tile table, so an undo snapshot shares
//!   every tile an edit did not write; [`Canvas::changed_tiles`] is a
//!   pointer comparison.
//! - A **bitmap** holds a layer per image face or depth slice at mip 0
//!   ([`LayerId`]), each beside the canvas it was opened with, so what
//!   has been edited is the tiles that differ.
//! - **Edits** are [`BitmapOp`] values (paint, fill, adjust, import), one
//!   undo step each, all or nothing. A drag is [`Bitmap::begin_stroke`],
//!   [`Bitmap::stroke_to`] and [`Bitmap::end_stroke`]: many touched
//!   rectangles, one undo step.
//! - **Dirty rectangles.** Every edit, undo and redo adds the pixels it
//!   touched to a per-layer log ([`Bitmap::take_dirty`]) and reports them
//!   as [`Applied`] with the tiles they overlap. The live preview uploads
//!   those rectangles, and the encoder (a later slice) re-encodes only
//!   those tiles' blocks and their mips.
//! - **Colour** is sRGB bytes with straight alpha; brush blends and
//!   resampling are done in linear light.

mod adjust;
mod bitmap;
mod brush;
mod canvas;
mod color;
mod error;
mod fill;
mod png_io;
mod rect;
mod resample;

pub use adjust::{Adjust, apply as apply_adjust};
pub use bitmap::{Applied, Bitmap, BitmapOp, Dirty, Layer, LayerId, UNDO_LIMIT, paint_canvas};
pub use brush::{Brush, Stroke, paint, stamp};
pub use canvas::{Canvas, MAX_SIZE, TILE, check_size};
pub use color::{Channels, blend, replace, to_linear, to_srgb};
pub use error::Error;
pub use fill::{fill_rect, flood};
pub use png_io::{decode_png, encode_png};
pub use rect::Rect;
pub use resample::resample;
