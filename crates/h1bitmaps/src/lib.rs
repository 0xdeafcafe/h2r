//! Halo 1 Xbox bitmaps: decoded to RGBA8, and converted to Halo 2 `bitm`
//! tags.
//!
//! - [`h1map::bitmap`] reads the tag and finds each image's pixels in the map
//!   file.
//! - [`decode`] turns an image's pixels into RGBA8 with Halo 2's texture
//!   decoder (`assets` over `gfx`): Halo 1 uses the same pixel format
//!   numbers, the same Xbox swizzle and DXT blocks, and the same mip chain
//!   and cube face layout, so nothing is decoded twice.
//! - [`convert`] builds a Halo 2 `bitm` block tree whose image LOD points at
//!   the same bytes, in the [`h1models::Converted`] shape.
//!
//! Layouts and evidence: `docs/formats/h1-bitmaps.md`. No game data is in
//! this crate or its tests; real maps are read only by `#[ignore]` tests,
//! from `$H1_ISO`.
#![forbid(unsafe_code)]

pub mod atlas;
pub mod convert;
pub mod decode;

pub use convert::{LightmapPages, convert_bitmap, lightmap_pages};
pub use decode::{BitmapExt, Rgba8Image, decode_image, texture_desc};

/// Decode and conversion failures.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Reading the Halo 1 map failed.
    #[error("halo 1 map: {0}")]
    H1(#[from] h1map::Error),
    /// Halo 2's decoder refused the image.
    #[error("decode: {0}")]
    Decode(#[from] assets::Error),
    /// The texture layout could not be computed.
    #[error("texture: {0}")]
    Texture(#[from] gfx::texture::TextureError),
    /// Building the Halo 2 tag failed.
    #[error("map builder: {0}")]
    Build(#[from] map::Error),
    /// The image or request is out of range or contradictory.
    #[error("invalid: {0}")]
    Invalid(String),
    /// A format or kind with no Halo 2 equivalent.
    #[error("unsupported: {0}")]
    Unsupported(String),
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests;
