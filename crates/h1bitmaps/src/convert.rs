//! Halo 1 `bitm` -> Halo 2 `bitm`, as a [`Converted`] block tree.
//!
//! Halo 2's image record has room for everything a Halo 1 data element
//! holds, with the same format, type and flag numbers, and both games store
//! a chain the same way (checked on the disc by size and by decoding both).
//! So an image converts **without re-encoding**: its pixel bytes, cut to the
//! Halo 2 layout's size rounded up to 128 bytes (the size every Halo 2 LOD
//! has), become one raw resource, and LOD 0 points at it. LODs 1 and 2 (the
//! chains from mip 1 and 2) stay absent, as Halo 2 leaves them for small
//! textures. Each image's runtime ID (`+0x4c`) is the tag's own datum, as
//! in every retail Halo 2 image, and the flags gain [`IMAGE_CACHED`] as
//! every retail image has; the other runtime bytes (`+0x09`, `+0x16`,
//! `+0x17`), the pixel offset and the low-detail fields stay zero.
//!
//! Halo 2 layouts: `tags::records::{BitmapHeader, BitmapSequence,
//! BitmapSprite, BitmapImage}` (the schema in `crates/tags/schema.rs`) and
//! `tags::rebuild::relocate_bitmap`, which expresses retail Halo 2 bitmaps
//! the same way.

use h1map::bitmap::{Bitmap, Image, PixelFormat};
use h1map::bsp::StructureBsp;
use h1map::{H1Map, Tag, TagRef};
use h1models::{Converted, SELF_ID};
use map::FourCC;
use map::build::{Block, RawId, RawRef};
use tags::rebuild::BITMAP_RAW_ALIGN;
use tags::records::{BitmapHeader, BitmapImage, BitmapSequence, BitmapSprite};
use tags::wire::BinaryRecord;

use crate::decode::{h2_image, layout_len};
use crate::{Error, Result};

/// Halo 2 bitmap root offsets used here.
mod h2 {
    pub const SEQUENCES: u32 = 0x3c;
    pub const IMAGES: u32 = 0x44;
    pub const SPRITES: u32 = 0x34;
    pub const LOD_POINTERS: usize = 0x1c;
    pub const LOD_SIZES: usize = 0x34;
    pub const RUNTIME_ID: usize = 0x4c;
    /// The texture cache's handle for each LOD, `0xffffffff` (not
    /// resident) on every retail image (V: all 1,686 of Coagulation's).
    pub const CACHE_HANDLES: usize = 0x28;
}

/// Image words that are "none" (`0xffffffff`) on disc: LOD pointers 1 and
/// 2 (`+0x20`, `+0x24`; only LOD 0 is written) and the three texture cache
/// handles (`+0x28..+0x34`). The texture cache reads a handle other than
/// -1 as "this LOD is already resident in that cache slot" and never loads
/// the pixels (E `0x0012c9fb`..`0x0012ca28`, and the predictor at
/// `0x0012cc23`), so a zero there draws whatever slot 0 holds: Halo 1's
/// textures and lightmap drew as one flat colour.
pub const IMAGE_NONE_WORDS: [usize; 5] = [0x20, 0x24, h2::CACHE_HANDLES, 0x2c, 0x30];

/// Image flag `0x200`: the pixels are a resource the texture cache loads.
/// Every retail Halo 2 image has it (V: all 1,686 of Coagulation's and
/// 2,231 of Delta Approach's: `0x203` DXT, `0x1209` swizzled, `0x290`
/// linear); Halo 1's never do. Without it the cache takes the image as
/// already built and hands back its runtime texture (`+0x50`), which is
/// null, so nothing is bound (E `0x0012cd26`, `0x0012ce81`: `test byte
/// [image+0Fh], 2`). That was the dark Halo 1 lightmap: the stage stayed
/// empty and the lighting pass multiplied by zero.
pub const IMAGE_CACHED: u16 = 0x200;

/// Halo 2 LOD sizes are the layout rounded up to this.
pub const LOD_ALIGN: usize = 128;

/// What one image became.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConvertedImage {
    /// Bytes in its LOD 0 resource.
    pub lod_size: u32,
    /// Halo 1 pixel bytes beyond the Halo 2 LOD size (dropped), or short of
    /// it (zero-filled, negative).
    pub trimmed: i64,
}

/// Converts a Halo 1 bitmap. `pixels(i)` gives image `i`'s stored bytes
/// ([`Bitmap::pixels`] over its map).
pub fn convert_with(
    bm: &Bitmap,
    mut pixels: impl FnMut(usize) -> Result<Vec<u8>>,
) -> Result<(Converted, Vec<ConvertedImage>)> {
    let mut root = Block::zeroed(BitmapHeader::SIZE);
    root.put_u16(0, bm.kind)
        .put_u16(2, bm.encoding)
        .put_u16(4, bm.usage)
        .put_u16(6, bm.flags)
        .put_f32(8, bm.detail_fade)
        .put_f32(0xc, bm.sharpen)
        .put_f32(0x10, bm.bump_height)
        .put_u16(0x14, bm.sprite_budget[0])
        .put_u16(0x16, bm.sprite_budget[1])
        .put_u16(0x18, bm.color_plate[0])
        .put_u16(0x1a, bm.color_plate[1])
        .put_f32(0x2c, bm.blur)
        .put_f32(0x30, bm.alpha_bias)
        .put_u16(0x34, bm.mipmap_count)
        .put_u16(0x36, bm.sprite_usage)
        .put_u16(0x38, bm.sprite_spacing);

    let mut sequences = Block::zeroed(bm.sequences.len() * BitmapSequence::SIZE);
    for (i, s) in bm.sequences.iter().enumerate() {
        let at = i * BitmapSequence::SIZE;
        let name = s.name.as_bytes();
        let n = name.len().min(31);
        sequences.bytes[at..at + n].copy_from_slice(&name[..n]);
        sequences
            .put_u16(at + 0x20, s.first_image as u16)
            .put_u16(at + 0x22, s.image_count as u16);
        let mut sprites = Block::zeroed(s.sprites.len() * BitmapSprite::SIZE);
        for (j, p) in s.sprites.iter().enumerate() {
            let at = j * BitmapSprite::SIZE;
            sprites.put_u16(at, p.image as u16);
            for (k, v) in p.bounds.iter().chain(&p.registration).enumerate() {
                sprites.put_f32(at + 8 + 4 * k, *v);
            }
        }
        sequences.block(at as u32 + h2::SPRITES, s.sprites.len() as u32, sprites);
    }
    root.block(h2::SEQUENCES, bm.sequences.len() as u32, sequences);

    let mut raw = Vec::new();
    let mut report = Vec::new();
    let mut images = Block::zeroed(bm.images.len() * BitmapImage::SIZE);
    for (i, img) in bm.images.iter().enumerate() {
        let at = i * BitmapImage::SIZE;
        let (bytes, r) = lod_bytes(img, pixels(i)?)?;
        put_image(&mut images, at, img);
        for w in IMAGE_NONE_WORDS {
            images.put_u32(at + w, u32::MAX);
        }
        images.put_u32(at + h2::LOD_SIZES, r.lod_size);
        // Every retail Halo 2 image holds its own tag's datum here.
        images.tag_id((at + h2::RUNTIME_ID) as u32, SELF_ID);
        images.raw(
            (at + h2::LOD_POINTERS) as u32,
            RawRef::Local(RawId(raw.len())),
        );
        raw.push((bytes, BITMAP_RAW_ALIGN));
        report.push(r);
    }
    root.block(h2::IMAGES, bm.images.len() as u32, images);
    Ok((
        Converted {
            group: FourCC(*b"bitm"),
            root,
            raw,
        },
        report,
    ))
}

/// Converts the Halo 1 `bitm` tag `tag` of `map`.
pub fn convert_bitmap(map: &H1Map, tag: &Tag) -> Result<(Bitmap, Converted, Vec<ConvertedImage>)> {
    let bm = Bitmap::read(map, tag)?;
    let (out, report) = convert_with(&bm, |i| Ok(bm.pixels(map, i)?.to_vec()))?;
    Ok((bm, out, report))
}

/// [`convert_bitmap`] with every image's alpha made opaque
/// ([`force_opaque`]): for a picture whose stored alpha is not what Halo 1
/// draws it with (`h1convert::sky_shaders`, Blood Gulch's clear blue sky).
pub fn convert_bitmap_opaque(
    map: &H1Map,
    tag: &Tag,
) -> Result<(Bitmap, Converted, Vec<ConvertedImage>)> {
    let bm = Bitmap::read(map, tag)?;
    let (out, report) = convert_with(&bm, |i| {
        let mut px = bm.pixels(map, i)?.to_vec();
        force_opaque(bm.images[i].format, &mut px);
        Ok(px)
    })?;
    Ok((bm, out, report))
}

/// Sets the alpha of every texel of `px`, an image's stored bytes in
/// `format`, to opaque, in place: the bytes are per texel or per block,
/// so the swizzle and the mip chain don't matter. True when every texel
/// now samples opaque: the formats with an alpha byte, nibble or bit
/// (A8, A8R8G8B8, X8R8G8B8, A1R5G5B5, A4R4G4B4), DXT3's explicit and DXT5's
/// interpolated alpha (both endpoints 255, every code the first), and those with none (Y8,
/// R5G6B5, P8 bump). False, unchanged, for AY8 and A8Y8 (alpha shares or
/// sits beside the luminance) and for DXT1, whose transparent texels are a
/// colour code (one more than the block's opaque ones only when its first
/// endpoint is not above its second) that can't be made opaque without
/// re-encoding.
pub fn force_opaque(format: PixelFormat, px: &mut [u8]) -> bool {
    match format {
        PixelFormat::A8 => px.fill(0xff),
        PixelFormat::A8R8G8B8 | PixelFormat::X8R8G8B8 => {
            // B, G, R, A per texel.
            px.iter_mut().skip(3).step_by(4).for_each(|a| *a = 0xff);
        }
        PixelFormat::A1R5G5B5 | PixelFormat::A4R4G4B4 => {
            let top = if format == PixelFormat::A1R5G5B5 {
                0x80
            } else {
                0xf0
            };
            px.iter_mut().skip(1).step_by(2).for_each(|b| *b |= top);
        }
        PixelFormat::Dxt3 => px.chunks_mut(16).for_each(|b| {
            let n = b.len().min(8);
            b[..n].fill(0xff);
        }),
        PixelFormat::Dxt5 => px.chunks_mut(16).for_each(|b| {
            // Endpoints 255 and 255, every texel code 0 (the first
            // endpoint; with equal endpoints code 6 would be 0).
            for (k, v) in b.iter_mut().take(8).enumerate() {
                *v = if k < 2 { 0xff } else { 0 };
            }
        }),
        PixelFormat::Y8 | PixelFormat::R5G6B5 | PixelFormat::P8Bump => {}
        PixelFormat::Ay8 | PixelFormat::A8Y8 | PixelFormat::Dxt1 | PixelFormat::Other(_) => {
            return false;
        }
    }
    true
}

/// The image record's fields, Halo 1's values in Halo 2's places.
fn put_image(b: &mut Block, at: usize, img: &Image) {
    b.put_u32(at, FourCC(*b"bitm").as_u32())
        .put_u16(at + 4, img.width)
        .put_u16(at + 6, img.height)
        .put_u16(at + 0xa, img.kind.raw())
        .put_u16(at + 0xc, img.format.raw())
        .put_u16(at + 0xe, img.flags | IMAGE_CACHED)
        .put_u16(at + 0x10, img.registration[0] as u16)
        .put_u16(at + 0x12, img.registration[1] as u16)
        .put_u16(at + 0x14, img.mip_count);
    // Halo 2's depth is a byte; `h2_image` has refused anything larger.
    b.bytes[at + 8] = img.depth as u8;
}

/// LOD 0's bytes: the stored chain, cut or zero-padded to the Halo 2 layout
/// rounded up to [`LOD_ALIGN`].
pub fn lod_bytes(img: &Image, mut pixels: Vec<u8>) -> Result<(Vec<u8>, ConvertedImage)> {
    h2_image(img)?;
    let need = layout_len(img)?;
    if pixels.len() < need {
        return Err(Error::Invalid(format!(
            "{} pixel bytes, the layout needs {need}",
            pixels.len()
        )));
    }
    let size = need.next_multiple_of(LOD_ALIGN);
    let trimmed = pixels.len() as i64 - size as i64;
    pixels.resize(size, 0);
    let lod_size = u32::try_from(size).map_err(|_| Error::Invalid("LOD over 4 GiB".into()))?;
    Ok((pixels, ConvertedImage { lod_size, trimmed }))
}

/// What a structure BSP's lightmaps need from its lightmap bitmap: the
/// `bitm` tag, and per Halo 1 lightmap (the `sbsp` lightmaps block, in
/// order) the image (page) it samples, `None` when it has no lightmap.
#[derive(Debug, Clone, PartialEq)]
pub struct LightmapPages {
    /// The `sbsp`'s lightmap bitmap reference.
    pub bitmap: TagRef,
    /// That tag, when the reference names one.
    pub tag: Option<Tag>,
    /// Per lightmap, its image in [`Self::tag`]'s bitmap.
    pub pages: Vec<Option<u16>>,
}

/// The lightmap bitmap of a structure BSP and the page each lightmap uses.
/// Fails when a page index is outside the bitmap's images.
pub fn lightmap_pages(map: &H1Map, bsp: &StructureBsp) -> Result<LightmapPages> {
    let bitmap = bsp.lightmaps_bitmap();
    let tag = if bitmap.id.is_none() {
        None
    } else {
        Some(map.tag(bitmap.id)?.clone())
    };
    let images = match &tag {
        Some(t) => Bitmap::read(map, t)?.images.len(),
        None => 0,
    };
    let mut pages = Vec::new();
    for l in bsp.lightmaps()? {
        pages.push(match l.bitmap {
            i if i < 0 => None,
            i if (i as usize) < images => Some(i as u16),
            i => {
                return Err(Error::Invalid(format!(
                    "lightmap page {i} of {images} images"
                )));
            }
        });
    }
    Ok(LightmapPages { bitmap, tag, pages })
}
