//! Halo 1 Xbox bitmap (`bitm`) tags: the header, sequences with their
//! sprites, and the bitmap data elements (one per image), with each image's
//! pixel bytes.
//!
//! On Xbox the pixels are in the map file itself: each image's pixel data
//! offset is a file offset in the decompressed map, outside the tag data (see
//! `docs/halo1/bitmaps.md`, with evidence). This module only finds and
//! describes the bytes; decoding them to RGBA (Xbox swizzle, DXT, cube
//! faces) is `crates/tools/h1-assets`, which reuses Halo 2's texture decoder.

use std::ops::Range;

use crate::format::*;
use crate::reader::H1Map;
use crate::space::{Space, f32_at};
use crate::{Error, Result};

/// `bitm`.
pub const BITM: Group = Group::new(b"bitm");
/// Bytes in the `bitm` header struct.
pub const HEADER_LEN: usize = 0x6c;
/// Bytes per sequence.
pub const SEQUENCE_LEN: usize = 0x40;
/// Bytes per sprite.
pub const SPRITE_LEN: usize = 0x20;
/// Bytes per bitmap data element (one image).
pub const IMAGE_LEN: usize = 0x30;
/// The sequences block in the header.
pub const SEQUENCES: usize = 0x54;
/// The bitmap data block in the header.
pub const IMAGES: usize = 0x60;
/// The sprites block in a sequence.
pub const SPRITES: usize = 0x34;
/// Image flags: dimensions are powers of two.
pub const FLAG_POWER_OF_TWO: u16 = 1;
/// Image flags: block-compressed (DXT).
pub const FLAG_COMPRESSED: u16 = 2;
/// Image flags: palettized (P8).
pub const FLAG_PALETTIZED: u16 = 4;
/// Image flags: Xbox swizzled.
pub const FLAG_SWIZZLED: u16 = 8;
/// Image flags: linear (row-major, padded rows).
pub const FLAG_LINEAR: u16 = 0x10;

/// An image's texture type (`+0x0a`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    /// A 2D texture.
    D2,
    /// A volume: `depth` slices.
    D3,
    /// A cube map: six square faces.
    Cube,
    /// "White": a placeholder the engine replaces with white.
    White,
    /// Anything else, as stored.
    Other(u16),
}

impl ImageKind {
    /// From the stored value.
    pub fn from_raw(v: u16) -> Self {
        match v {
            0 => Self::D2,
            1 => Self::D3,
            2 => Self::Cube,
            3 => Self::White,
            v => Self::Other(v),
        }
    }

    /// The stored value.
    pub fn raw(self) -> u16 {
        match self {
            Self::D2 => 0,
            Self::D3 => 1,
            Self::Cube => 2,
            Self::White => 3,
            Self::Other(v) => v,
        }
    }
}

/// An image's pixel format (`+0x0c`). The numbering is Halo 2's for the
/// same formats (Halo 2 adds P8 and float formats after `P8Bump`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PixelFormat {
    /// 0: alpha only.
    A8,
    /// 1: luminance.
    Y8,
    /// 2: one byte for both luminance and alpha.
    Ay8,
    /// 3: alpha and luminance, two bytes.
    A8Y8,
    /// 6: 16-bit colour.
    R5G6B5,
    /// 8: 16-bit colour, 1-bit alpha.
    A1R5G5B5,
    /// 9: 16-bit colour, 4-bit alpha.
    A4R4G4B4,
    /// 10: 32-bit colour, no alpha.
    X8R8G8B8,
    /// 11: 32-bit colour and alpha.
    A8R8G8B8,
    /// 14: DXT1.
    Dxt1,
    /// 15: DXT3.
    Dxt3,
    /// 16: DXT5.
    Dxt5,
    /// 17: 8-bit palette indices into a bump (normal) palette.
    P8Bump,
    /// Anything else, as stored.
    Other(u16),
}

impl PixelFormat {
    /// From the stored value.
    pub fn from_raw(v: u16) -> Self {
        match v {
            0 => Self::A8,
            1 => Self::Y8,
            2 => Self::Ay8,
            3 => Self::A8Y8,
            6 => Self::R5G6B5,
            8 => Self::A1R5G5B5,
            9 => Self::A4R4G4B4,
            10 => Self::X8R8G8B8,
            11 => Self::A8R8G8B8,
            14 => Self::Dxt1,
            15 => Self::Dxt3,
            16 => Self::Dxt5,
            17 => Self::P8Bump,
            v => Self::Other(v),
        }
    }

    /// The stored value.
    pub fn raw(self) -> u16 {
        match self {
            Self::A8 => 0,
            Self::Y8 => 1,
            Self::Ay8 => 2,
            Self::A8Y8 => 3,
            Self::R5G6B5 => 6,
            Self::A1R5G5B5 => 8,
            Self::A4R4G4B4 => 9,
            Self::X8R8G8B8 => 10,
            Self::A8R8G8B8 => 11,
            Self::Dxt1 => 14,
            Self::Dxt3 => 15,
            Self::Dxt5 => 16,
            Self::P8Bump => 17,
            Self::Other(v) => v,
        }
    }

    /// Whether this is a DXT format.
    pub fn is_dxt(self) -> bool {
        matches!(self, Self::Dxt1 | Self::Dxt3 | Self::Dxt5)
    }
}

/// One sprite in a sequence.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sprite {
    /// Image (bitmap data element) index (`+0x00`).
    pub image: i16,
    /// Left, right, top, bottom in [0, 1] of the image (`+0x08`).
    pub bounds: [f32; 4],
    /// Registration point (`+0x18`).
    pub registration: [f32; 2],
}

/// A sequence: a run of images, or sprites inside one.
#[derive(Debug, Clone, PartialEq)]
pub struct Sequence {
    /// Name (`+0x00`, 32 bytes).
    pub name: String,
    /// First image (`+0x20`).
    pub first_image: i16,
    /// Image count (`+0x22`).
    pub image_count: i16,
    /// Sprites (`+0x34`).
    pub sprites: Vec<Sprite>,
}

/// One bitmap data element: an image and where its pixels are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// Width (`+0x04`).
    pub width: u16,
    /// Height (`+0x06`).
    pub height: u16,
    /// Depth (`+0x08`): volume slices, 1 otherwise.
    pub depth: u16,
    /// Texture type (`+0x0a`).
    pub kind: ImageKind,
    /// Pixel format (`+0x0c`).
    pub format: PixelFormat,
    /// Flags (`+0x0e`): `FLAG_*`.
    pub flags: u16,
    /// Registration point (`+0x10`).
    pub registration: [i16; 2],
    /// Mipmap levels below the base (`+0x14`).
    pub mip_count: u16,
    /// File offset of the pixels in the decompressed map (`+0x18`).
    pub pixel_offset: u32,
    /// Bytes of pixels (`+0x1c`).
    pub pixel_size: u32,
    /// The element as stored.
    pub raw: [u8; IMAGE_LEN],
}

impl Image {
    /// Parses one element; checks its `bitm` signature.
    pub fn parse(e: &[u8]) -> Result<Self> {
        let raw: [u8; IMAGE_LEN] = e
            .get(..IMAGE_LEN)
            .and_then(|s| s.try_into().ok())
            .ok_or(Error::Invalid("short bitmap data element".into()))?;
        let u16a = |at| u16_at(&raw, at).unwrap();
        let u32a = |at| u32_at(&raw, at).unwrap();
        if u32a(0) != BITM.0 {
            return Err(Error::Invalid(format!(
                "bitmap data signature {:#010x}, want 'bitm'",
                u32a(0)
            )));
        }
        Ok(Self {
            width: u16a(4),
            height: u16a(6),
            depth: u16a(8),
            kind: ImageKind::from_raw(u16a(0xa)),
            format: PixelFormat::from_raw(u16a(0xc)),
            flags: u16a(0xe),
            registration: [u16a(0x10) as i16, u16a(0x12) as i16],
            mip_count: u16a(0x14),
            pixel_offset: u32a(0x18),
            pixel_size: u32a(0x1c),
            raw,
        })
    }

    /// Whether the swizzled flag is set.
    pub fn swizzled(&self) -> bool {
        self.flags & FLAG_SWIZZLED != 0
    }

    /// Whether the linear flag is set.
    pub fn linear(&self) -> bool {
        self.flags & FLAG_LINEAR != 0
    }

    /// The pixels' range in the decompressed file.
    pub fn pixel_range(&self) -> Range<usize> {
        let at = self.pixel_offset as usize;
        at..at.saturating_add(self.pixel_size as usize)
    }
}

/// A Halo 1 bitmap tag.
#[derive(Debug, Clone, PartialEq)]
pub struct Bitmap {
    /// Type (`+0x00`): 2D textures, 3D textures, cube maps, sprites,
    /// interface bitmaps.
    pub kind: u16,
    /// Format (`+0x02`): the import's compression choice.
    pub encoding: u16,
    /// Usage (`+0x04`): alpha blend, default, height map, detail map, light
    /// map, vector map.
    pub usage: u16,
    /// Flags (`+0x06`).
    pub flags: u16,
    /// Detail fade factor, sharpen amount, bump height (`+0x08`..`+0x10`).
    pub detail_fade: f32,
    /// Sharpen amount.
    pub sharpen: f32,
    /// Bump height.
    pub bump_height: f32,
    /// Sprite budget size and count (`+0x14`, `+0x16`).
    pub sprite_budget: [u16; 2],
    /// Colour plate width and height (`+0x18`).
    pub color_plate: [u16; 2],
    /// Blur filter size (`+0x44`).
    pub blur: f32,
    /// Alpha bias (`+0x48`).
    pub alpha_bias: f32,
    /// Requested mipmap levels (`+0x4c`).
    pub mipmap_count: u16,
    /// Sprite usage and spacing (`+0x4e`, `+0x50`).
    pub sprite_usage: u16,
    /// Sprite spacing.
    pub sprite_spacing: u16,
    /// Sequences.
    pub sequences: Vec<Sequence>,
    /// Images (bitmap data elements).
    pub images: Vec<Image>,
}

impl Bitmap {
    /// Reads the `bitm` tag `tag` of `map`, and checks every image's pixels
    /// lie inside the file.
    pub fn read(map: &H1Map, tag: &Tag) -> Result<Self> {
        if tag.group != BITM {
            return Err(Error::Invalid(format!(
                "tag is a {}, not a bitm",
                tag.group
            )));
        }
        map.tag_data(tag)?;
        let b = Self::parse(&map.metadata(), tag.data_address)?;
        for img in &b.images {
            map.bytes().get(img.pixel_range()).ok_or(Error::Bounds {
                what: "bitmap pixels",
                offset: img.pixel_offset as u64,
                size: img.pixel_size as u64,
                limit: map.bytes().len() as u64,
            })?;
        }
        Ok(b)
    }

    /// Parses the tag whose header is at `address` in `space`.
    pub fn parse(space: &Space, address: u32) -> Result<Self> {
        let h = space.at(address, HEADER_LEN)?;
        let u = |at| u16_at(h, at).unwrap();
        let mut sequences = Vec::new();
        for s in space.block_at(h, SEQUENCES, SEQUENCE_LEN)? {
            let sprites = space
                .block_at(s, SPRITES, SPRITE_LEN)?
                .into_iter()
                .map(|p| Sprite {
                    image: u16_at(p, 0).unwrap() as i16,
                    bounds: std::array::from_fn(|i| f32_at(p, 8 + 4 * i)),
                    registration: [f32_at(p, 0x18), f32_at(p, 0x1c)],
                })
                .collect();
            sequences.push(Sequence {
                name: fixed_str(&s[..0x20]),
                first_image: u16_at(s, 0x20).unwrap() as i16,
                image_count: u16_at(s, 0x22).unwrap() as i16,
                sprites,
            });
        }
        let images = space
            .block_at(h, IMAGES, IMAGE_LEN)?
            .into_iter()
            .map(Image::parse)
            .collect::<Result<_>>()?;
        Ok(Self {
            kind: u(0),
            encoding: u(2),
            usage: u(4),
            flags: u(6),
            detail_fade: f32_at(h, 8),
            sharpen: f32_at(h, 0xc),
            bump_height: f32_at(h, 0x10),
            sprite_budget: [u(0x14), u(0x16)],
            color_plate: [u(0x18), u(0x1a)],
            blur: f32_at(h, 0x44),
            alpha_bias: f32_at(h, 0x48),
            mipmap_count: u(0x4c),
            sprite_usage: u(0x4e),
            sprite_spacing: u(0x50),
            sequences,
            images,
        })
    }

    /// Image `index`'s pixel bytes in `map` (the whole stored chain: every
    /// face and mip level).
    pub fn pixels<'m>(&self, map: &'m H1Map, index: usize) -> Result<&'m [u8]> {
        let img = self.images.get(index).ok_or(Error::Invalid(format!(
            "image {index} of {}",
            self.images.len()
        )))?;
        pixels(map.bytes(), img)
    }
}

/// An image's pixels in a decompressed file.
pub fn pixels<'f>(file: &'f [u8], img: &Image) -> Result<&'f [u8]> {
    file.get(img.pixel_range()).ok_or(Error::Bounds {
        what: "bitmap pixels",
        offset: img.pixel_offset as u64,
        size: img.pixel_size as u64,
        limit: file.len() as u64,
    })
}
