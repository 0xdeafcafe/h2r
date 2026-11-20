//! Halo 1 image -> `gfx::TextureDesc` -> RGBA8, through Halo 2's decoder.
//!
//! A Halo 1 bitmap data element carries the same pixel format numbers as a
//! Halo 2 image (0..=17), the same swizzled/linear/compressed flag bits and
//! the same type numbers (2D, 3D, cube). [`h2_image`] restates it as a
//! `tags::Image` and [`assets::texture_desc`] builds the descriptor, so the
//! rules Halo 2's bitmaps were checked against (DXT chains stop at one
//! block, linear rows pad to 64 bytes, cube faces align to 128 bytes, the
//! descriptor validation) apply unchanged. The disc check
//! (`tests/real_disc.rs`) confirms each rule for Halo 1 by size: every
//! image's pixel size equals the layout's size rounded up to 128 bytes.

use assets::Level;
use gfx::texture::TextureDesc;
use h1map::H1Map;
use h1map::bitmap::{Bitmap, Image, ImageKind};
use tags::records::BitmapImage;
use tags::wire::{BinaryRecord, Flags16};

use crate::bitmaps::{Error, Result};

/// One decoded mip level, RGBA8 rows. Cube faces (+X, -X, +Y, -Y, +Z, -Z)
/// and volume slices are stacked top to bottom, so `height` is the level's
/// height times six (cube) or its depth (volume).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba8Image {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels, all faces or slices.
    pub height: u32,
    /// `width * height * 4` bytes, `R, G, B, A`.
    pub rgba: Vec<u8>,
}

/// A Halo 1 image restated as the Halo 2 image record Halo 2's decoder
/// takes: format, type, size, mip count and flags. Its LODs are empty: the
/// pixels come from the Halo 1 map.
pub fn h2_image(img: &Image) -> Result<tags::Image> {
    let kind = match img.kind {
        ImageKind::D2 => tags::TextureKind::D2,
        ImageKind::D3 => tags::TextureKind::D3,
        ImageKind::Cube => tags::TextureKind::Cube,
        k => return Err(Error::Unsupported(format!("image type {k:?}"))),
    };
    let depth =
        u8::try_from(img.depth).map_err(|_| Error::Unsupported(format!("depth {}", img.depth)))?;
    let mut wire = BitmapImage::decode(&[0u8; BitmapImage::SIZE]).expect("zeroed record");
    wire.flags = Flags16(img.flags);
    Ok(tags::Image {
        format: tags::PixelFormat::from(img.format.raw()),
        kind,
        dimensions: [img.width, img.height, u16::from(depth)],
        mip_count: img.mip_count,
        lods: [None; 3],
        low_detail: None,
        wire,
    })
}

/// The descriptor of the whole stored chain (every mip level, every face).
pub fn texture_desc(img: &Image) -> Result<TextureDesc> {
    Ok(assets::texture_desc(&h2_image(img)?, 0)?)
}

/// Bytes the descriptor's layout covers.
pub fn layout_len(img: &Image) -> Result<usize> {
    Ok(texture_desc(img)?.layout()?.total_size)
}

/// Decodes an image's stored chain from its pixel bytes: every face, then
/// every level, as Halo 2's [`assets::bitmap::decode_bytes`] returns them.
/// The pixel bytes must cover the layout.
pub fn decode_image(img: &Image, pixels: &[u8]) -> Result<Vec<Level>> {
    let desc = texture_desc(img)?;
    let need = desc.layout()?.total_size;
    if pixels.len() < need {
        return Err(Error::Invalid(format!(
            "{} pixel bytes, the layout needs {need}",
            pixels.len()
        )));
    }
    Ok(assets::bitmap::decode_bytes(&desc, &pixels[..need])?)
}

/// Stacks one mip level's faces (or a volume's slices, already stacked by
/// the decoder) into one image.
pub fn stack(levels: &[Level], mip: u32) -> Result<Rgba8Image> {
    let faces: Vec<&Level> = levels.iter().filter(|l| l.level == mip).collect();
    let first = faces
        .first()
        .ok_or_else(|| Error::Invalid(format!("no mip level {mip}")))?;
    let (width, height) = (first.width, first.height * first.depth * faces.len() as u32);
    Ok(Rgba8Image {
        width,
        height,
        rgba: faces.iter().flat_map(|l| l.rgba.iter().copied()).collect(),
    })
}

/// Decoding straight from a [`Bitmap`] read out of its map.
pub trait BitmapExt {
    /// Image `index`, mip level `mip` (0 is the largest), as RGBA8.
    fn decode(&self, map: &H1Map, index: usize, mip: u32) -> Result<Rgba8Image>;
    /// Every level and face of image `index`.
    fn decode_levels(&self, map: &H1Map, index: usize) -> Result<Vec<Level>>;
}

impl BitmapExt for Bitmap {
    fn decode(&self, map: &H1Map, index: usize, mip: u32) -> Result<Rgba8Image> {
        stack(&self.decode_levels(map, index)?, mip)
    }

    fn decode_levels(&self, map: &H1Map, index: usize) -> Result<Vec<Level>> {
        let pixels = self.pixels(map, index)?;
        decode_image(&self.images[index], pixels)
    }
}
