//! Halo 2 (Xbox) packed bitmap fonts: parser, glyph decoder and CPU rasteriser.
//!
//! The on-disc layout was derived from the retail `default.xbe` and cross-checked
//! against every font file on the disc; see `docs/formats/fonts.md` for the format, the
//! engine addresses it was read from, and the evidence level of each field.
//!
//! ```text
//! 0x00000  zero padding (0x200 bytes)
//! 0x00200  header, 0x1c4 bytes (the only part the engine keeps in memory)
//! 0x00400  character map: u32 glyph-record index for each UTF-16 code unit
//!          (0x10000 entries, 0x40000 bytes)
//! 0x40400  glyph records, 16 bytes each, `glyph_count` of them
//!          glyph data (compressed ARGB4444 runs), addressed by absolute offset
//! ```
//!
//! Every access is bounds-checked; no input can make the parser panic, read out
//! of range, or allocate more than a few hundred KiB beyond the font itself.
use std::fmt;

pub mod encode;
pub mod gear;
pub mod icons;
pub mod patch;
pub mod raster;

pub use raster::{Bitmap, RenderOptions, render};

/// File offset of the header.
pub const HEADER_OFFSET: usize = 0x200;
/// Bytes of header the engine reads (`ReadFile(handle, slot, 0x1c4)`).
pub const HEADER_LEN: usize = 0x1c4;
/// Magic at the start of the header.
pub const MAGIC: u32 = 0xf000_0001;
/// File offset of the character map.
pub const CHARMAP_OFFSET: usize = 0x400;
/// Entries in the character map (one per UTF-16 code unit).
pub const CHARMAP_ENTRIES: usize = 0x1_0000;
/// File offset of the first glyph record.
pub const RECORDS_OFFSET: usize = CHARMAP_OFFSET + CHARMAP_ENTRIES * 4;
/// Size of one glyph record.
pub const RECORD_LEN: usize = 16;
/// Kerning pairs that fit in the header (`(0x1a4 - 0x24) / 4`).
pub const MAX_KERN_PAIRS: usize = 96;
/// Largest glyph bitmap edge accepted (the engine clamps to this).
pub const MAX_GLYPH_EDGE: u16 = 128;
/// Largest compressed glyph (the engine clamps to this).
pub const MAX_GLYPH_BYTES: u16 = 0x1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The file is smaller than a structure it must contain.
    Truncated {
        what: &'static str,
        need: usize,
        have: usize,
    },
    BadMagic(u32),
    /// A header field or record value is outside what the engine accepts.
    Invalid(&'static str),
    /// A glyph's compressed stream is malformed.
    BadGlyph {
        index: u32,
        why: &'static str,
    },
    /// The requested render would be larger than the safety limits.
    TooLarge,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Truncated { what, need, have } => {
                write!(
                    f,
                    "font truncated: {what} needs {need} bytes, file has {have}"
                )
            }
            Error::BadMagic(m) => write!(f, "bad font magic {m:#010x}"),
            Error::Invalid(s) => write!(f, "invalid font: {s}"),
            Error::BadGlyph { index, why } => write!(f, "bad glyph record {index}: {why}"),
            Error::TooLarge => write!(f, "render exceeds size limits"),
        }
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

/// One kerning pair: `amount` is added to the pen before drawing `second`
/// when it follows `first`. Only code units below 0x100 can be kerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KernPair {
    pub first: u8,
    pub second: u8,
    pub amount: i16,
}

/// A glyph record (16 bytes on disc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlyphRecord {
    /// Pen advance. The engine moves the pen by `x_offset + advance + kerning`.
    pub advance: i16,
    /// Compressed byte count at `data_offset` (0 for empty placeholder glyphs).
    pub data_len: u16,
    /// Bitmap width in pixels.
    pub width: u16,
    /// Bitmap height in pixels.
    pub height: u16,
    /// Horizontal offset of the bitmap from the pen (also added to the advance).
    pub x_offset: i16,
    /// The engine draws the bitmap's top row at `pen_y - y_offset`, where
    /// `pen_y` is the baseline; the value equals the header's ascent on the
    /// retail fonts.
    pub y_offset: i16,
    /// Absolute file offset of the compressed pixel stream.
    pub data_offset: u32,
}

/// The 0x1c4-byte font header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub ascent: i16,
    pub descent: i16,
    pub leading: i16,
    pub glyph_count: u32,
    /// Total bytes of compressed glyph data (file length = data start + this).
    pub data_size: u32,
    /// `+0x10`: largest `data_len` of any record (holds on every disc font; the
    /// engine does not read it in any code reached).
    pub max_glyph_bytes: u32,
    /// `+0x1c`: total `width * height` over all records (holds on every disc font).
    pub total_pixels: u32,
    /// `+0x0a` (always 0 on the disc) and `+0x14` (1710, 2100 or 3240): meaning
    /// unknown, not read by any reached engine code; kept for study.
    pub unknown: [u32; 2],
    pub kerning: Vec<KernPair>,
    /// 256-bit set of first characters that have kerning pairs.
    pub kern_first: [u32; 8],
}

/// A parsed font borrowing the file bytes.
#[derive(Debug, Clone)]
pub struct Font<'a> {
    data: &'a [u8],
    pub header: Header,
}

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        d.get(o..o.checked_add(2)?)?.try_into().ok()?,
    ))
}
fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        d.get(o..o.checked_add(4)?)?.try_into().ok()?,
    ))
}

impl<'a> Font<'a> {
    /// Parse the header and check that the fixed tables fit. Glyph data is
    /// checked lazily by [`Font::glyph`] or all at once by [`Font::validate`].
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let need = HEADER_OFFSET + HEADER_LEN;
        if data.len() < need {
            return Err(Error::Truncated {
                what: "header",
                need,
                have: data.len(),
            });
        }
        let h = &data[HEADER_OFFSET..HEADER_OFFSET + HEADER_LEN];
        let magic = u32_at(h, 0).ok_or(Error::Invalid("header"))?;
        if magic != MAGIC {
            return Err(Error::BadMagic(magic));
        }
        let s16 = |o| {
            u16_at(h, o)
                .map(|v| v as i16)
                .ok_or(Error::Invalid("header"))
        };
        let u32h = |o| u32_at(h, o).ok_or(Error::Invalid("header"));
        let kern_count = u32h(0x20)? as usize;
        if kern_count > MAX_KERN_PAIRS {
            return Err(Error::Invalid("kerning pair count exceeds header capacity"));
        }
        let mut kerning = Vec::with_capacity(kern_count);
        for i in 0..kern_count {
            let o = 0x24 + i * 4;
            kerning.push(KernPair {
                first: h[o],
                second: h[o + 1],
                amount: s16(o + 2)?,
            });
        }
        let mut kern_first = [0u32; 8];
        for (i, w) in kern_first.iter_mut().enumerate() {
            *w = u32h(0x1a4 + i * 4)?;
        }
        let glyph_count = u32h(0xc)?;
        let records_end = (glyph_count as usize)
            .checked_mul(RECORD_LEN)
            .and_then(|n| n.checked_add(RECORDS_OFFSET))
            .ok_or(Error::Invalid("glyph count overflows"))?;
        if data.len() < records_end {
            return Err(Error::Truncated {
                what: "character map and glyph records",
                need: records_end,
                have: data.len(),
            });
        }
        if (data.len() - records_end) < u32h(0x18)? as usize {
            return Err(Error::Truncated {
                what: "glyph data",
                need: records_end.saturating_add(u32h(0x18)? as usize),
                have: data.len(),
            });
        }
        let header = Header {
            ascent: s16(4)?,
            descent: s16(6)?,
            leading: s16(8)?,
            glyph_count,
            data_size: u32h(0x18)?,
            max_glyph_bytes: u32h(0x10)?,
            total_pixels: u32h(0x1c)?,
            unknown: [
                u32::from(u16_at(h, 0xa).ok_or(Error::Invalid("header"))?),
                u32h(0x14)?,
            ],
            kerning,
            kern_first,
        };
        if data.len() < RECORDS_OFFSET {
            return Err(Error::Truncated {
                what: "character map",
                need: RECORDS_OFFSET,
                have: data.len(),
            });
        }
        Ok(Font { data, header })
    }

    /// Line height the engine uses (`ascent + descent + leading`, `0x122540`).
    pub fn line_height(&self) -> i32 {
        i32::from(self.header.ascent)
            + i32::from(self.header.descent)
            + i32::from(self.header.leading)
    }

    /// Glyph-record index for a UTF-16 code unit. Unmapped units read as 0, which
    /// is what the engine does (it then draws record 0, normally a blank space).
    pub fn glyph_index(&self, unit: u16) -> u32 {
        // CHARMAP_OFFSET + 4 * unit is below RECORDS_OFFSET <= data.len() (checked in parse).
        u32_at(self.data, CHARMAP_OFFSET + usize::from(unit) * 4).unwrap_or(0)
    }

    /// The record with this index, checked for sanity against the file.
    pub fn record(&self, index: u32) -> Result<GlyphRecord> {
        let bad = |why| Error::BadGlyph { index, why };
        if index >= self.header.glyph_count {
            return Err(bad("index out of range"));
        }
        let o = RECORDS_OFFSET + index as usize * RECORD_LEN;
        let d = self.data;
        let rd = |off: usize| u16_at(d, o + off).ok_or(bad("record truncated"));
        let r = GlyphRecord {
            advance: rd(0)? as i16,
            data_len: rd(2)?,
            width: rd(4)?,
            height: rd(6)?,
            x_offset: rd(8)? as i16,
            y_offset: rd(10)? as i16,
            data_offset: u32_at(d, o + 12).ok_or(bad("record truncated"))?,
        };
        if r.width > MAX_GLYPH_EDGE || r.height > MAX_GLYPH_EDGE {
            return Err(bad("bitmap larger than 128 pixels"));
        }
        if r.data_len > MAX_GLYPH_BYTES {
            return Err(bad("compressed size above 0x1000"));
        }
        let empty = r.width == 0 || r.height == 0;
        if r.data_len == 0 && !empty {
            return Err(bad("non-empty bitmap without pixel data"));
        }
        if !empty || r.data_len > 0 {
            let end = (r.data_offset as usize).checked_add(usize::from(r.data_len));
            let data_start = RECORDS_OFFSET + self.header.glyph_count as usize * RECORD_LEN;
            let data_end = data_start + self.header.data_size as usize;
            if (r.data_offset as usize) < data_start || end.is_none_or(|e| e > data_end) {
                return Err(bad("pixel data outside file"));
            }
        }
        Ok(r)
    }

    /// Record and decoded pixels for a UTF-16 code unit.
    pub fn glyph(&self, unit: u16) -> Result<Glyph> {
        self.glyph_by_index(self.glyph_index(unit))
    }

    pub fn glyph_by_index(&self, index: u32) -> Result<Glyph> {
        let record = self.record(index)?;
        if record.width == 0 || record.height == 0 {
            // Placeholder records (width or height 0, usually data_len 0) have no pixels.
            return Ok(Glyph {
                record,
                pixels: Vec::new(),
            });
        }
        let start = record.data_offset as usize;
        let src = &self.data[start..start + usize::from(record.data_len)];
        let want = usize::from(record.width) * usize::from(record.height);
        let pixels = decode_pixels(src, want).map_err(|why| Error::BadGlyph { index, why })?;
        Ok(Glyph { record, pixels })
    }

    /// Kerning amount for `first` followed by `second` (0 when none). Mirrors
    /// the engine routine at `0x122570`: both units must be 1..=0xff and `first`
    /// must be in the header's bit set.
    pub fn kerning(&self, first: u16, second: u16) -> i16 {
        if first == 0 || second == 0 || first > 0xff || second > 0xff {
            return 0;
        }
        if self.header.kern_first[usize::from(first >> 5)] & (1 << (first & 31)) == 0 {
            return 0;
        }
        self.header
            .kerning
            .iter()
            .find(|k| u16::from(k.first) == first && u16::from(k.second) == second)
            .map_or(0, |k| k.amount)
    }

    /// Full structural check: every character-map entry points at a real record,
    /// and every record's pixel stream decodes to exactly `width * height`
    /// pixels. Returns the number of records decoded.
    pub fn validate(&self) -> Result<u32> {
        for unit in 0..=u16::MAX {
            let i = self.glyph_index(unit);
            if i >= self.header.glyph_count {
                return Err(Error::BadGlyph {
                    index: i,
                    why: "character map entry out of range",
                });
            }
        }
        for i in 0..self.header.glyph_count {
            self.glyph_by_index(i)?;
        }
        Ok(self.header.glyph_count)
    }
}

/// A decoded glyph: ARGB4444 pixels, row-major, `width * height` of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Glyph {
    pub record: GlyphRecord,
    pub pixels: Vec<u16>,
}

/// Expand a 3-bit coverage value to a 4-bit alpha the way the engine does
/// (`(a << 1) | (a & 1)`, so 7 becomes 15).
fn alpha3(a: u8) -> u16 {
    u16::from((a << 1) | (a & 1)) << 12
}

/// Decode a glyph's compressed stream into exactly `want` ARGB4444 pixels.
///
/// Each byte (`0x122610`) is one of, by its top two bits:
/// * `00 000000`, then 2 bytes: one literal pixel word, big-endian; its low 12
///   bits become the current colour for later runs.
/// * `00 nnnnnn` (n > 0): `n` pixels of the current colour, alpha 0.
/// * `01 nnnnnn`: `n` pixels of the current colour, alpha 15 (n may be 0).
/// * `10 aaa rrr`: one pixel with 3-bit alpha `aaa`, then a run chosen by `rrr`:
///   0 none, 1/2/3 = 4/3/2 more opaque pixels, 4/5/6/7 = 5/4/3/2 transparent.
/// * `11 aaa bbb`: two pixels with 3-bit alphas `aaa`, `bbb`.
///
/// The colour starts at 0xfff (white). Returns an error if the stream is
/// truncated or does not yield exactly `want` pixels.
pub fn decode_pixels(src: &[u8], want: usize) -> std::result::Result<Vec<u16>, &'static str> {
    if want > usize::from(MAX_GLYPH_EDGE) * usize::from(MAX_GLYPH_EDGE) {
        return Err("requested glyph larger than 128x128");
    }
    let mut out: Vec<u16> = Vec::with_capacity(want);
    let mut color: u16 = 0xfff;
    let mut i = 0;
    let push = |out: &mut Vec<u16>, v: u16, n: usize| -> std::result::Result<(), &'static str> {
        if out.len() + n > want {
            return Err("pixel stream longer than width*height");
        }
        out.resize(out.len() + n, v);
        Ok(())
    };
    while i < src.len() {
        let b = src[i];
        let (class, low) = (b >> 6, b & 0x3f);
        match class {
            0 if low == 0 => {
                let w = src.get(i + 1..i + 3).ok_or("truncated literal")?;
                let word = u16::from_be_bytes([w[0], w[1]]);
                color = word & 0xfff;
                push(&mut out, word, 1)?;
                i += 3;
                continue;
            }
            0 => push(&mut out, color, usize::from(low))?,
            1 => push(&mut out, color | 0xf000, usize::from(low))?,
            2 => {
                push(&mut out, color | alpha3((b >> 3) & 7), 1)?;
                let (n, opaque) = match b & 7 {
                    0 => (0, false),
                    1 => (4, true),
                    2 => (3, true),
                    3 => (2, true),
                    4 => (5, false),
                    5 => (4, false),
                    6 => (3, false),
                    _ => (2, false),
                };
                push(&mut out, if opaque { color | 0xf000 } else { color }, n)?;
            }
            _ => {
                push(&mut out, color | alpha3((b >> 3) & 7), 1)?;
                push(&mut out, color | alpha3(b & 7), 1)?;
            }
        }
        i += 1;
    }
    if out.len() != want {
        return Err("pixel stream shorter than width*height");
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) mod testutil;
