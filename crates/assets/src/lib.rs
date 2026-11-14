//! Glue between decoded game tags and the `gfx` texture decoder.
//!
//! Currently covers bitmap (`bitm`) tags: [`bitmap::texture_desc`] maps a
//! decoded `tags::Image` onto a `gfx::TextureDesc`, [`resource`] fetches the
//! raw pixel bytes from the map files named by the image's raw pointers, and
//! [`bitmap::decode_image`] runs the CPU decode to RGBA8. [`png`] writes the
//! results. No game data is embedded; see the module docs of [`bitmap`] for the
//! mapping rules and their evidence.
#![forbid(unsafe_code)]

pub mod bitmap;
pub mod png;
pub mod resource;

pub use bitmap::{DecodedImage, Level, d3d_format, decode_image, texture_desc};
pub use resource::{Region, ResourceFiles};

/// Why a bitmap image could not be decoded.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The tag-level decode failed.
    #[error("tag: {0}")]
    Tag(#[from] tags::Error),
    /// The map reader failed.
    #[error("map: {0}")]
    Map(#[from] map::Error),
    /// The bitmap's pixel format has no Xbox texture equivalent we decode.
    #[error("unsupported bitmap format {0}")]
    UnsupportedFormat(String),
    /// Image fields contradict each other or the data.
    #[error("invalid bitmap: {0}")]
    Invalid(String),
    /// The image has no pixel data (all LOD sizes are zero).
    #[error("image has no pixel data")]
    NoData,
    /// A resource file needed for pixel data was not supplied.
    #[error("resource file {0:?} not available")]
    MissingResource(map::ResourceFile),
    /// Reading resource bytes failed.
    #[error("resource I/O: {0}")]
    Io(#[from] std::io::Error),
    /// The texture decoder rejected the data.
    #[error("texture: {0}")]
    Texture(#[from] gfx::texture::TextureError),
}

/// Result type for this crate.
pub type Result<T> = std::result::Result<T, Error>;
