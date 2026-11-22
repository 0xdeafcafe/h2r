//! What saving an edited bitmap image writes to the map
//! (docs/editor/modeling.md, "Texture editing", "Writing"): a plan of
//! raw byte writes, new raw blocks and tag patches, with no map open and
//! no file touched. The editor hands the plan to the map's save
//! (`MapEditor::patch_raw`, `append_raw`, `patch_metadata`).
//!
//! Three cases, from least to most written:
//!
//! 1. **Same size and format, pixels in this map**: the bytes of the units
//!    the edit touched ([`crate::encode::encode_tiles`]), patched in place
//!    in each LOD that is stored. Nothing else in the file changes.
//! 2. **Same size and format, pixels in a shared cache** (`shared.map`,
//!    `mainmenu.map`, `single_player_shared.map`, which the editor never
//!    writes): the LOD's stored bytes are read, the touched units patched
//!    in the copy, and the copy appended to this map as a new raw block
//!    ([`RawNew`]); the image's LOD pointer word is pointed at it.
//! 3. **A new size or format**: every LOD the image stores is encoded whole
//!    from the pixels and appended as a new raw block, the LOD pointers and
//!    sizes point at the new blocks, and the image's size, format, flags and
//!    mip count are patched. The old blocks stay as dead data.
//!
//! The image's low-detail picture (a separate, smaller copy for far views)
//! is not rewritten.

use map::{RawPointer, ResourceFile};
use tags::{Image, PixelFormat, TextureKind as TagKind};

use crate::bitmap::{LOD_COUNT, d3d_format, invalid, texture_desc};
use crate::encode::{Dirty, Planes, Span, encode_lod, encode_tiles, writable};
use crate::resource::ResourceFiles;
use crate::{Error, Result};

/// Alignment given to a pixel block appended to a map.
pub const RAW_ALIGN: u32 = tags::rebuild::BITMAP_RAW_ALIGN;

/// Offsets inside a `BitmapImage` record (`tags` schema).
mod at {
    pub const WIDTH: u32 = 4;
    pub const FORMAT: u32 = 0xc;
    pub const FLAGS: u32 = 0xe;
    pub const MIP_COUNT: u32 = 0x14;
    pub const LOD_POINTERS: u32 = 0x1c;
    pub const LOD_SIZES: u32 = 0x34;
}

/// A LOD pointer word with nothing behind it.
const NO_POINTER: u32 = u32::MAX;

/// A new size and format for an image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reshape {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
}

/// One image to write.
#[derive(Clone, Copy, Debug)]
pub struct Edit<'a> {
    /// The image as the map has it.
    pub image: &'a Image,
    /// The virtual address of its `BitmapImage` record in the map's
    /// metadata.
    pub image_at: u32,
    /// Its mip 0 pixels now: at the new size when `reshape` is set.
    pub planes: Planes<'a>,
    /// The regions changed since the image was read (unused with
    /// `reshape`, which writes everything).
    pub dirty: &'a [Dirty],
    pub reshape: Option<Reshape>,
}

/// Bytes to write inside a raw block of this map
/// (`MapEditor::patch_raw`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawWrite {
    pub block: RawPointer,
    pub block_size: u32,
    pub offset: u32,
    pub bytes: Vec<u8>,
}

/// A new raw block for the map (`MapEditor::append_raw`) and the metadata
/// address of the pointer word that takes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawNew {
    pub bytes: Vec<u8>,
    pub align: u32,
    pub pointer_address: u32,
}

/// How a plan writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// Bytes patched in the map's own blocks.
    InPlace,
    /// New raw blocks (a shared-cache copy, or a new size or format).
    Appended,
}

impl Class {
    pub fn name(self) -> &'static str {
        match self {
            Class::InPlace => "in_place",
            Class::Appended => "appended",
        }
    }
}

/// Everything saving one image writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub raw: Vec<RawWrite>,
    pub appended: Vec<RawNew>,
    /// Patches to the map's metadata: `(virtual address, bytes)`.
    pub tag: Vec<(u32, Vec<u8>)>,
    pub class: Class,
}

/// The wire id of a pixel format.
fn format_id(f: PixelFormat) -> Option<u16> {
    use PixelFormat::*;
    Some(match f {
        A8 => 0,
        Y8 => 1,
        Ay8 => 2,
        A8Y8 => 3,
        Rgb565 => 6,
        Argb1555 => 8,
        Argb4444 => 9,
        Xrgb8888 => 10,
        Argb8888 => 11,
        Dxt1 => 14,
        Dxt3 => 15,
        Dxt5 => 16,
        _ => return None,
    })
}

fn compressed(f: PixelFormat) -> bool {
    matches!(f, PixelFormat::Dxt1 | PixelFormat::Dxt3 | PixelFormat::Dxt5)
}

/// `image` at a new size and format, and the field patches that make the
/// record say so (offsets in the record): size, format, the storage flags (compressed, swizzled or
/// linear, power of two) and the mip count. Mips stay a full chain when the
/// image had mips, else none; a size that is not a power of two is linear
/// (no mips), as the hardware needs. The image's depth, kind, LOD pointers
/// and everything else are kept.
pub fn reshaped(image: &Image, r: Reshape) -> Result<(Image, Vec<(u32, Vec<u8>)>)> {
    let id = format_id(r.format)
        .filter(|_| writable(r.format))
        .ok_or_else(|| Error::UnsupportedFormat(format!("{:?} (not written)", r.format)))?;
    let (w, h) = (
        u16::try_from(r.width).map_err(|_| invalid("width over 65535"))?,
        u16::try_from(r.height).map_err(|_| invalid("height over 65535"))?,
    );
    if w == 0 || h == 0 {
        return Err(invalid("zero size"));
    }
    if image.kind == TagKind::Cube && w != h {
        return Err(invalid("a cube map stays square"));
    }
    let depth = if image.kind == TagKind::D3 {
        u32::from(image.dimensions[2]).max(1)
    } else {
        1
    };
    let pow2 = w.is_power_of_two() && h.is_power_of_two() && depth.is_power_of_two();
    let packed = compressed(r.format);
    if packed && !pow2 {
        return Err(invalid("a DXT image needs power of two sides"));
    }
    let linear = !packed && (!pow2 || (image.is_linear() && d3d_format(r.format, true).is_ok()));
    if linear && image.kind != TagKind::D2 {
        return Err(invalid("a cube map or volume needs a power of two size"));
    }
    if linear && d3d_format(r.format, true).is_err() {
        return Err(invalid(format!(
            "{:?} has no linear storage, and {w}x{h} is not a power of two",
            r.format
        )));
    }
    let mut flags = image.wire.flags.0 & !(1 | 2 | 4 | 8 | 16);
    if pow2 {
        flags |= 1;
    }
    flags |= if packed {
        2
    } else if linear {
        16
    } else {
        8
    };
    let mips = if linear || image.mip_count == 0 {
        0
    } else {
        (gfx::texture::max_levels(u32::from(w), u32::from(h), depth) - 1) as u16
    };
    let mut out = image.clone();
    out.format = r.format;
    out.dimensions = [w, h, image.dimensions[2]];
    out.mip_count = mips;
    out.wire.width = w;
    out.wire.height = h;
    out.wire.format.0 = id;
    out.wire.flags.0 = flags;
    out.wire.mip_count = mips;
    // Every LOD the new shape cannot hold is gone; the rest are rewritten
    // by the caller.
    texture_desc(&out, 0)?;
    let mut size = w.to_le_bytes().to_vec();
    size.extend_from_slice(&h.to_le_bytes());
    let p = |at: u32, v: u16| (at, v.to_le_bytes().to_vec());
    let tag = vec![
        (at::WIDTH, size),
        p(at::FORMAT, id),
        p(at::FLAGS, flags),
        p(at::MIP_COUNT, mips),
    ];
    Ok((out, tag))
}

fn merge(spans: &[Span], lod: u32) -> impl Iterator<Item = &Span> {
    spans.iter().filter(move |s| s.lod == lod)
}

/// Plans the write of one image.
///
/// Reads `files` only for the pixels of a LOD that lives in a shared cache
/// (case 2). Errors name what cannot be written: a format that is not a
/// picture, an image with no LOD 0 in this map, bytes the image's LOD
/// does not hold.
pub fn plan_image(edit: &Edit, files: &mut ResourceFiles) -> Result<Plan> {
    let _scope = mem_watch::scope("assets.write");
    let img = edit.image;
    if img.lods[0].is_none() {
        return Err(invalid(
            "only a lower mip of the image is stored: it is not written",
        ));
    }
    if let Some(r) = edit.reshape {
        return plan_reshape(edit, r);
    }
    let spans = encode_tiles(img, &edit.planes, edit.dirty)?;
    let mut plan = Plan {
        raw: Vec::new(),
        appended: Vec::new(),
        tag: Vec::new(),
        class: Class::InPlace,
    };
    for lod in 0..LOD_COUNT {
        let Some(range) = img.lods[lod as usize] else {
            continue;
        };
        let mine: Vec<&Span> = merge(&spans, lod).collect();
        if mine.is_empty() {
            continue;
        }
        if mine
            .iter()
            .any(|s| s.offset + s.bytes.len() > range.size as usize)
        {
            return Err(invalid(format!(
                "an edit lies past the {} bytes LOD {lod} stores",
                range.size
            )));
        }
        if range.pointer.file == ResourceFile::Local {
            plan.raw.extend(mine.iter().map(|s| RawWrite {
                block: range.pointer,
                block_size: range.size,
                offset: s.offset as u32,
                bytes: s.bytes.clone(),
            }));
        } else {
            let mut bytes = files.read(range.pointer.file, range.pointer.offset, range.size)?;
            for s in mine {
                bytes[s.offset..s.offset + s.bytes.len()].copy_from_slice(&s.bytes);
            }
            plan.appended.push(RawNew {
                bytes,
                align: RAW_ALIGN,
                pointer_address: edit.image_at + at::LOD_POINTERS + 4 * lod,
            });
            plan.class = Class::Appended;
        }
    }
    Ok(plan)
}

fn plan_reshape(edit: &Edit, r: Reshape) -> Result<Plan> {
    let (new, fields) = reshaped(edit.image, r)?;
    let tag = fields
        .into_iter()
        .map(|(o, b)| (edit.image_at + o, b))
        .collect();
    let mut plan = Plan {
        raw: Vec::new(),
        appended: Vec::new(),
        tag,
        class: Class::Appended,
    };
    let total = u32::from(new.mip_count) + 1;
    for lod in 0..LOD_COUNT {
        let pointer = edit.image_at + at::LOD_POINTERS + 4 * lod;
        let size = edit.image_at + at::LOD_SIZES + 4 * lod;
        // A LOD the new size cannot hold, or the image never stored, has no
        // block.
        if edit.image.lods[lod as usize].is_none() || lod >= total {
            if edit.image.lods[lod as usize].is_some() {
                plan.tag.push((pointer, NO_POINTER.to_le_bytes().to_vec()));
                plan.tag.push((size, 0u32.to_le_bytes().to_vec()));
            }
            continue;
        }
        let bytes = encode_lod(&new, lod, &edit.planes)?;
        plan.tag
            .push((size, (bytes.len() as u32).to_le_bytes().to_vec()));
        plan.appended.push(RawNew {
            bytes,
            align: RAW_ALIGN,
            pointer_address: pointer,
        });
    }
    Ok(plan)
}
