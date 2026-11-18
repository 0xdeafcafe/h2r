//! A new Halo 2 `bitm` from RGBA pixels: one linear A8R8G8B8 image,
//! shaped like an existing bitmap of the same map (its template).
//!
//! The template gives every field this module has no reason to choose: the
//! header (type, usage, source size), and the image's flags, LOD bias and
//! cache usage. The menu's level pictures (`ui\code_global_bitmaps\
//! campaign0`, mainmenu) are one 220x207 `ARGB8888` image, flags `linear`,
//! each row padded to 64 bytes (880 to 896) and the LOD to 128 bytes (V:
//! LOD size `0x2d480` = 896 x 207). What changes per bitmap:
//!
//! - **Image** (`0x74`, schema `BitmapImage`): width and height, the
//!   registration point (the centre), format 11, depth 1, no mips, the
//!   pixel offset 0, LOD 0 a new raw resource and its size, LODs 1 and 2
//!   and the three texture cache handles `0xffffffff` (a handle other than
//!   -1 reads as already resident: `h1bitmaps::convert::IMAGE_NONE_WORDS`),
//!   the runtime ID the tag's own datum, no low-detail image.
//! - **Header** (`0x4c`, schema `BitmapHeader`): no sequences (`+0x3c`),
//!   one image (`+0x44`).

use std::io::{Read, Seek};

use map::build::{Block, RawRef, TagPayload, TagSet};
use map::{FourCC, Map, TagId};

use crate::geometry::TagView;
use crate::rebuild::BITMAP_RAW_ALIGN;
use crate::{Error, Result};

/// Header and image record sizes.
pub const HEADER_LEN: usize = 0x4c;
pub const IMAGE_LEN: usize = 0x74;
/// `ARGB8888` (`tags::PixelFormat`).
pub const FORMAT_ARGB8888: u16 = 11;
/// Linear rows are padded to this many bytes.
pub const ROW_ALIGN: usize = 64;
/// LOD sizes are rounded up to this.
pub const LOD_ALIGN: usize = 128;

mod at {
    pub const SEQUENCES: u32 = 0x3c;
    pub const IMAGES: u32 = 0x44;
    pub const WIDTH: usize = 0x04;
    pub const HEIGHT: usize = 0x06;
    pub const DEPTH: usize = 0x08;
    pub const KIND: usize = 0x0a;
    pub const FORMAT: usize = 0x0c;
    pub const REGISTRATION: usize = 0x10;
    pub const MIPS: usize = 0x14;
    pub const PIXEL_OFFSET: usize = 0x18;
    pub const LOD_POINTERS: usize = 0x1c;
    pub const LOD_SIZES: usize = 0x34;
    pub const RUNTIME_ID: usize = 0x4c;
    pub const LOW_DETAIL: std::ops::Range<usize> = 0x60..0x70;
    /// LOD pointers 1 and 2, then the three texture cache handles.
    pub const NONE_WORDS: [usize; 5] = [0x20, 0x24, 0x28, 0x2c, 0x30];
}

/// An existing bitmap's header and first image, as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    pub header: Vec<u8>,
    pub image: Vec<u8>,
}

impl Template {
    /// The bitmap tag `id` of `map`, which must have at least one image.
    pub fn read<R: Read + Seek>(map: &mut Map<R>, id: TagId) -> Result<Self> {
        let tag = map.tag(id)?.clone();
        if tag.group != FourCC(*b"bitm") {
            return Err(Error::UnsupportedGroup(tag.group));
        }
        let loc = tag.location.ok_or(map::Error::NoTagData)?;
        let bytes = map.read_tag_bytes(id)?;
        let view = TagView::new(&bytes, loc.address);
        let header = view.at(0, HEADER_LEN)?;
        let images = view.block(header, at::IMAGES as usize, IMAGE_LEN)?;
        let image = images
            .first()
            .ok_or(Error::Invalid("template bitmap has no image"))?;
        Ok(Self {
            header: header.to_vec(),
            image: image.to_vec(),
        })
    }

    /// The template image's width and height.
    pub fn size(&self) -> (u16, u16) {
        let w = |o: usize| u16::from_le_bytes([self.image[o], self.image[o + 1]]);
        (w(at::WIDTH), w(at::HEIGHT))
    }
}

/// `width` x `height` RGBA rows as linear `ARGB8888` (bytes B, G, R, A),
/// each row padded to [`ROW_ALIGN`], the whole to [`LOD_ALIGN`].
pub fn argb8888_linear(width: u16, height: u16, rgba: &[u8]) -> Result<Vec<u8>> {
    let (w, h) = (usize::from(width), usize::from(height));
    if rgba.len() != w * h * 4 {
        return Err(Error::Invalid("RGBA length is not width x height x 4"));
    }
    let pitch = (w * 4).next_multiple_of(ROW_ALIGN);
    let mut out = vec![0u8; (pitch * h).next_multiple_of(LOD_ALIGN)];
    for y in 0..h {
        for x in 0..w {
            let s = &rgba[(y * w + x) * 4..][..4];
            out[y * pitch + x * 4..][..4].copy_from_slice(&[s[2], s[1], s[0], s[3]]);
        }
    }
    Ok(out)
}

/// Adds `name`, a bitmap shaped like `template` holding `rgba`
/// (`width` x `height`), to `set`. Returns its datum. The set's index must
/// have room for one more tag (for a decomposed retail map:
/// `PinnedImage::clear_index_tail` and `index_entries_end`).
pub fn install(
    set: &mut TagSet,
    template: &Template,
    name: &str,
    width: u16,
    height: u16,
    rgba: &[u8],
) -> Result<TagId> {
    if template.header.len() != HEADER_LEN || template.image.len() != IMAGE_LEN {
        return Err(Error::Invalid("template records have the wrong size"));
    }
    let pixels = argb8888_linear(width, height, rgba)?;
    let lod_size = u32::try_from(pixels.len()).map_err(|_| Error::Invalid("LOD over 4 GiB"))?;
    let slot = set.slots.len();
    let id = TagId(u32::from(set.salt_base.wrapping_add(slot as u16)) << 16 | slot as u32);
    let raw = set.add_raw(pixels, BITMAP_RAW_ALIGN);

    let mut image = Block::new(template.image.clone());
    image
        .put_u16(at::WIDTH, width)
        .put_u16(at::HEIGHT, height)
        .put_u16(at::KIND, 0)
        .put_u16(at::FORMAT, FORMAT_ARGB8888)
        .put_u16(at::REGISTRATION, width / 2)
        .put_u16(at::REGISTRATION + 2, height / 2)
        .put_u16(at::MIPS, 0)
        .put_u32(at::PIXEL_OFFSET, 0)
        .put_u32(at::LOD_SIZES, lod_size)
        .put_u32(at::LOD_SIZES + 4, 0)
        .put_u32(at::LOD_SIZES + 8, 0);
    image.bytes[at::DEPTH] = 1;
    image.bytes[at::LOW_DETAIL].fill(0);
    for w in at::NONE_WORDS {
        image.put_u32(w, u32::MAX);
    }
    image.raw(at::LOD_POINTERS as u32, RawRef::Local(raw));
    image.tag_id(at::RUNTIME_ID as u32, id);

    let mut root = Block::new(template.header.clone());
    root.block(at::SEQUENCES, 0, Block::zeroed(0));
    root.block(at::IMAGES, 1, image);
    set.add_group(FourCC(*b"bitm"), None, None);
    let got = set.add_tag(FourCC(*b"bitm"), name, TagPayload::Built(root))?;
    if got != id {
        return Err(Error::Invalid("the set gave the new bitmap another datum"));
    }
    Ok(id)
}

#[cfg(test)]
#[path = "rgba_bitmap_tests.rs"]
mod tests;
