//! A bitmap tag taken apart into its records and laid out again, with
//! images or sequences added (docs/editor/modeling.md, "Texture editing",
//! "Adding images").
//!
//! A `bitm` is a header (`0x4c`, schema `BitmapHeader`) with two blocks:
//! sequences (`+0x3c`, `0x3c` each, each with a block of sprites at `+0x34`,
//! `0x20` each) and images (`+0x44`, `0x74` each). Nothing else in it is a
//! pointer. [`Parts::split`] reads those records off a tag's payload and
//! refuses a payload that holds any other nonzero byte (it may hold bytes
//! or pointers this crate does not know). [`Parts::layout`] writes them
//! back as one contiguous payload from offset 0, header, images, sequences
//! and sprites in that order, and names the block pointer words, which hold
//! offsets from the start: the caller adds the payload's new address to
//! each ([`Layout::rebase`]). The map's save moves the whole tag to the
//! appended metadata and points its index entry there
//! (`map::MapEditor::move_tag`), so a tag that grew still reads as one
//! range.
//!
//! [`added_image`] makes the record of a new image from an existing one:
//! the same flags, LOD bias and cache usage, no pixels until the save
//! writes them, the texture cache handles empty and no low-detail picture.

use crate::bytes::f32_at;
use crate::{Image, records::BitmapImage, wire::BinaryRecord};

/// Record sizes.
pub const HEADER_LEN: usize = 0x4c;
pub const IMAGE_LEN: usize = 0x74;
pub const SEQUENCE_LEN: usize = 0x3c;
pub const SPRITE_LEN: usize = 0x20;

/// Offsets of the blocks and fields this module writes.
pub mod at {
    /// Header: the sequences and images blocks (count, then address).
    pub const SEQUENCES: usize = 0x3c;
    pub const IMAGES: usize = 0x44;
    /// Sequence: its label (32 bytes), first image, image count, sprites.
    pub const LABEL: usize = 0;
    pub const FIRST_IMAGE: usize = 0x20;
    pub const IMAGE_COUNT: usize = 0x22;
    pub const SPRITES: usize = 0x34;
    /// Sprite: its image index.
    pub const SPRITE_IMAGE: usize = 0;
    /// Sprite: left, right, top and bottom (fractions of its image), then
    /// the registration point (two floats).
    pub const SPRITE_BOUNDS: usize = 0x8;
    pub const SPRITE_REGISTRATION: usize = 0x18;
    /// Image: registration point, pixel offset, the three texture cache
    /// handles and the low-detail picture.
    pub const REGISTRATION: usize = 0x10;
    pub const PIXEL_OFFSET: usize = 0x18;
    pub const HANDLES: std::ops::Range<usize> = 0x28..0x34;
    pub const LOW_DETAIL: std::ops::Range<usize> = 0x60..0x70;
}

/// One sequence: its record and its sprites' records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sequence {
    pub entry: Vec<u8>,
    pub sprites: Vec<Vec<u8>>,
}

impl Sequence {
    /// The label, up to its first NUL.
    pub fn label(&self) -> String {
        let l = &self.entry[at::LABEL..at::LABEL + 32];
        let n = l.iter().position(|&b| b == 0).unwrap_or(32);
        String::from_utf8_lossy(&l[..n]).into_owned()
    }
    pub fn first_image(&self) -> i16 {
        i16::from_le_bytes([self.entry[at::FIRST_IMAGE], self.entry[at::FIRST_IMAGE + 1]])
    }
    pub fn image_count(&self) -> u16 {
        u16::from_le_bytes([self.entry[at::IMAGE_COUNT], self.entry[at::IMAGE_COUNT + 1]])
    }
}

/// A sprite's fields as the sprite editor shows them: the image it cuts
/// from, its bounds on that image (fractions, `left <= right`,
/// `top <= bottom`) and its registration point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpriteView {
    pub image: i16,
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
    pub registration: [f32; 2],
}

impl SpriteView {
    /// The fields of a sprite record (`SPRITE_LEN` bytes).
    pub fn read(record: &[u8]) -> SpriteView {
        let b = at::SPRITE_BOUNDS;
        let g = at::SPRITE_REGISTRATION;
        SpriteView {
            image: i16::from_le_bytes([record[at::SPRITE_IMAGE], record[at::SPRITE_IMAGE + 1]]),
            left: f32_at(record, b),
            right: f32_at(record, b + 4),
            top: f32_at(record, b + 8),
            bottom: f32_at(record, b + 12),
            registration: [f32_at(record, g), f32_at(record, g + 4)],
        }
    }

    /// Why these fields cannot be written, or `Ok`.
    pub fn check(&self) -> Result<(), String> {
        let all = [self.left, self.right, self.top, self.bottom]
            .into_iter()
            .chain(self.registration);
        if all.into_iter().any(|v| !v.is_finite()) {
            return Err("a sprite's numbers must be finite".to_owned());
        }
        if self.left > self.right || self.top > self.bottom {
            return Err("a sprite's left and top must not pass its right and bottom".to_owned());
        }
        Ok(())
    }

    /// Writes the fields into a sprite record, keeping every other byte.
    pub fn write(&self, record: &mut [u8]) {
        record[at::SPRITE_IMAGE..at::SPRITE_IMAGE + 2].copy_from_slice(&self.image.to_le_bytes());
        let vals = [self.left, self.right, self.top, self.bottom];
        for (k, v) in vals.into_iter().enumerate() {
            let o = at::SPRITE_BOUNDS + 4 * k;
            record[o..o + 4].copy_from_slice(&v.to_le_bytes());
        }
        for (k, v) in self.registration.into_iter().enumerate() {
            let o = at::SPRITE_REGISTRATION + 4 * k;
            record[o..o + 4].copy_from_slice(&v.to_le_bytes());
        }
    }
}

/// A sprite record over all of `image`, registered at its origin, every
/// other byte zero.
pub fn new_sprite(image: i16) -> Vec<u8> {
    let mut r = vec![0; SPRITE_LEN];
    SpriteView { image, left: 0.0, right: 1.0, top: 0.0, bottom: 1.0, registration: [0.0; 2] }
        .write(&mut r);
    r
}

/// A bitmap tag's records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parts {
    pub header: Vec<u8>,
    pub sequences: Vec<Sequence>,
    pub images: Vec<Vec<u8>>,
}

/// A payload laid out from offset 0.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub bytes: Vec<u8>,
    /// Offsets of the block address words, each an offset from the start
    /// until the payload's address is added.
    pub rebase: Vec<u32>,
    /// Offset of the first image record.
    pub images_at: u32,
    /// Offset of the first sequence record.
    pub sequences_at: u32,
}

impl Layout {
    /// The bytes with `address` added to every block pointer: the payload
    /// as it is stored at `address`.
    pub fn at(&self, address: u32) -> Vec<u8> {
        let mut out = self.bytes.clone();
        for &o in &self.rebase {
            let o = o as usize;
            let v = u32::from_le_bytes(out[o..o + 4].try_into().expect("4 bytes"));
            out[o..o + 4].copy_from_slice(&v.wrapping_add(address).to_le_bytes());
        }
        out
    }
}

fn word(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("4 bytes"))
}

fn put(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

impl Parts {
    /// The records of the bitmap payload `bytes`, stored at `address`.
    /// Refused, with the reason, when a block lies outside the payload, two
    /// records share bytes, or a byte that no record holds is not zero.
    pub fn split(bytes: &[u8], address: u32) -> Result<Parts, String> {
        if bytes.len() < HEADER_LEN {
            return Err(format!(
                "the payload is {} bytes, shorter than the {HEADER_LEN:#x}-byte header",
                bytes.len()
            ));
        }
        let mut owned = vec![false; bytes.len()];
        owned[..HEADER_LEN].fill(true);
        let mut take = |pair: usize, from: &[u8], stride: usize, what: &str| {
            let count = word(from, pair) as usize;
            if count == 0 {
                return Ok(Vec::new());
            }
            let addr = word(from, pair + 4);
            let start = addr
                .checked_sub(address)
                .map(|s| s as usize)
                .filter(|s| s % 4 == 0)
                .ok_or_else(|| format!("its {what} block at {addr:#x} is outside the tag"))?;
            let end = count
                .checked_mul(stride)
                .and_then(|n| n.checked_add(start))
                .filter(|e| *e <= bytes.len())
                .ok_or_else(|| format!("its {what} block runs past the tag"))?;
            if owned[start..end].iter().any(|&o| o) {
                return Err(format!("its {what} block shares bytes with another record"));
            }
            owned[start..end].fill(true);
            Ok(bytes[start..end]
                .chunks_exact(stride)
                .map(<[u8]>::to_vec)
                .collect::<Vec<_>>())
        };
        let header = bytes[..HEADER_LEN].to_vec();
        let images = take(at::IMAGES, &header, IMAGE_LEN, "images")?;
        let entries = take(at::SEQUENCES, &header, SEQUENCE_LEN, "sequences")?;
        let mut sequences = Vec::with_capacity(entries.len());
        for mut entry in entries {
            let sprites = take(at::SPRITES, &entry, SPRITE_LEN, "sprites")?;
            // The sprite block's address is the layout's to write: a
            // sequence is the same wherever the bitmap sits.
            entry[at::SPRITES + 4..at::SPRITES + 8].fill(0);
            sequences.push(Sequence { entry, sprites });
        }
        let stray = owned
            .iter()
            .zip(bytes)
            .filter(|(o, b)| !**o && **b != 0)
            .count();
        if stray != 0 {
            return Err(format!(
                "it holds {stray} nonzero bytes outside its header, sequences, sprites and images"
            ));
        }
        Ok(Parts {
            header,
            sequences,
            images,
        })
    }

    /// The records as one payload from offset 0: header, images,
    /// sequences, then each sequence's sprites. An empty block is
    /// `(0, 0)`.
    pub fn layout(&self) -> Layout {
        let mut bytes = self.header.clone();
        let mut rebase = Vec::new();
        let images_at = bytes.len() as u32;
        for i in &self.images {
            bytes.extend_from_slice(i);
        }
        let sequences_at = bytes.len() as u32;
        for s in &self.sequences {
            bytes.extend_from_slice(&s.entry);
        }
        let pair = |bytes: &mut Vec<u8>, rebase: &mut Vec<u32>, at: usize, n: usize, to: u32| {
            put(bytes, at, n as u32);
            put(bytes, at + 4, if n == 0 { 0 } else { to });
            if n != 0 {
                rebase.push(at as u32 + 4);
            }
        };
        pair(&mut bytes, &mut rebase, at::IMAGES, self.images.len(), images_at);
        pair(
            &mut bytes,
            &mut rebase,
            at::SEQUENCES,
            self.sequences.len(),
            sequences_at,
        );
        for (k, s) in self.sequences.iter().enumerate() {
            let to = bytes.len() as u32;
            for sp in &s.sprites {
                bytes.extend_from_slice(sp);
            }
            let entry = sequences_at as usize + k * SEQUENCE_LEN;
            pair(&mut bytes, &mut rebase, entry + at::SPRITES, s.sprites.len(), to);
        }
        Layout {
            bytes,
            rebase,
            images_at,
            sequences_at,
        }
    }
}

/// A sequence of `count` images from `first`, labelled `label` (up to 31
/// bytes), shaped like `like` when the bitmap has a sequence: its other
/// fields, and when it has sprites, one sprite per image copied from its
/// first sprite with the image index set (the bounds and registration of
/// that sprite are kept). With no `like`, every other byte is zero and the
/// sequence has no sprites.
pub fn sequence(like: Option<&Sequence>, label: &str, first: i16, count: u16) -> Sequence {
    let mut entry = like.map_or_else(|| vec![0; SEQUENCE_LEN], |l| l.entry.clone());
    let l = &mut entry[at::LABEL..at::LABEL + 32];
    l.fill(0);
    let n = label.len().min(31);
    l[..n].copy_from_slice(&label.as_bytes()[..n]);
    entry[at::FIRST_IMAGE..at::FIRST_IMAGE + 2].copy_from_slice(&first.to_le_bytes());
    entry[at::IMAGE_COUNT..at::IMAGE_COUNT + 2].copy_from_slice(&count.to_le_bytes());
    let sprites = match like.and_then(|l| l.sprites.first()) {
        Some(sp) => (0..count)
            .map(|k| {
                let mut s = sp.clone();
                let i = first.saturating_add(k as i16);
                s[at::SPRITE_IMAGE..at::SPRITE_IMAGE + 2].copy_from_slice(&i.to_le_bytes());
                s
            })
            .collect(),
        None => Vec::new(),
    };
    Sequence { entry, sprites }
}

/// The record of a new image made from `record` (an existing image's
/// `0x74` bytes, decoded as `image`): its fields kept, its pixel offset
/// zero, its registration point the centre, the three texture cache
/// handles `0xffffffff` (a handle other than -1 reads as already
/// resident), no low-detail picture. Its LOD pointers and sizes are the
/// source's until the save writes the new image's pixels and points them
/// (`assets::write`, a reshape writes every LOD the record stores). The
/// decoded image says the same, with no low-detail range.
pub fn added_image(record: &[u8], image: &Image) -> Result<(Vec<u8>, Image), String> {
    if record.len() != IMAGE_LEN {
        return Err(format!("an image record is {IMAGE_LEN:#x} bytes, not {:#x}", record.len()));
    }
    let mut r = record.to_vec();
    put(&mut r, at::PIXEL_OFFSET, 0);
    let (w, h) = (image.dimensions[0] / 2, image.dimensions[1] / 2);
    r[at::REGISTRATION..at::REGISTRATION + 2].copy_from_slice(&w.to_le_bytes());
    r[at::REGISTRATION + 2..at::REGISTRATION + 4].copy_from_slice(&h.to_le_bytes());
    r[at::HANDLES].fill(0xff);
    r[at::LOW_DETAIL].fill(0);
    let wire = BitmapImage::decode(&r).map_err(|e| e.to_string())?;
    let mut out = image.clone();
    out.wire = wire;
    out.low_detail = None;
    Ok((r, out))
}

#[cfg(test)]
#[path = "bitmap_grow_tests.rs"]
mod tests;
