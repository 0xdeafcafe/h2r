//! Replace glyphs in a font file image.
//!
//! [`apply`] takes the bytes of a font and a list of [`Replacement`]s and
//! returns a new font: the same file with those code units drawing new
//! bitmaps. Everything else (other glyphs, metrics, kerning) is unchanged, and
//! the result passes [`Font::validate`] whenever the input did. Nothing here
//! holds glyph data: the caller supplies the pixels.
//!
//! A unit that owns its record (no other unit maps to it) has the record
//! rewritten in place. A unit that is unmapped, or shares its record (the
//! fonts point many units at one placeholder), gets a new record at the end of
//! the record table. New pixel streams go at the end of the file; the old
//! stream of a replaced record stays where it was, unused.
use std::collections::{HashMap, HashSet};

use crate::encode::encode_pixels;
use crate::{
    CHARMAP_ENTRIES, CHARMAP_OFFSET, Error, Font, HEADER_OFFSET, MAX_GLYPH_BYTES, MAX_GLYPH_EDGE,
    RECORD_LEN, RECORDS_OFFSET, Result,
};

/// A new bitmap for one UTF-16 code unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replacement {
    pub unit: u16,
    pub width: u16,
    pub height: u16,
    pub advance: i16,
    pub x_offset: i16,
    /// `None` keeps the replaced record's value (the font's ascent for a unit
    /// that had no record).
    pub y_offset: Option<i16>,
    /// ARGB4444, row-major, `width * height` of them.
    pub pixels: Vec<u16>,
}

fn put16(f: &mut [u8], o: usize, v: u16) {
    f[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
fn put32(f: &mut [u8], o: usize, v: u32) {
    f[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn get32(f: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(f[o..o + 4].try_into().unwrap())
}

/// Apply `replacements` to the font file `data`.
///
/// Errors if the font does not parse, a replacement's size is outside what
/// the engine accepts (1 to 128 pixels each way, at most 0x1000 compressed
/// bytes), its pixel count is wrong, or a unit appears twice.
pub fn apply(data: &[u8], replacements: &[Replacement]) -> Result<Vec<u8>> {
    let font = Font::parse(data)?;
    if replacements.is_empty() {
        return Ok(data.to_vec());
    }
    font.validate()?;
    let count = font.header.glyph_count as usize;
    let data_start = RECORDS_OFFSET + count * RECORD_LEN;

    let mut streams = Vec::with_capacity(replacements.len());
    let mut seen = HashSet::new();
    for r in replacements {
        if !seen.insert(r.unit) {
            return Err(Error::Invalid("replacement lists a code unit twice"));
        }
        if r.width == 0 || r.height == 0 || r.width > MAX_GLYPH_EDGE || r.height > MAX_GLYPH_EDGE {
            return Err(Error::Invalid(
                "replacement bitmap must be 1 to 128 pixels each way",
            ));
        }
        if r.pixels.len() != usize::from(r.width) * usize::from(r.height) {
            return Err(Error::Invalid(
                "replacement pixel count is not width * height",
            ));
        }
        let stream = encode_pixels(&r.pixels);
        if stream.len() > usize::from(MAX_GLYPH_BYTES) {
            return Err(Error::Invalid(
                "replacement compresses to more than 0x1000 bytes",
            ));
        }
        streams.push(stream);
    }

    // How many units map to each record a replacement touches.
    let wanted: HashSet<u32> = replacements
        .iter()
        .map(|r| font.glyph_index(r.unit))
        .collect();
    let mut users: HashMap<u32, u32> = HashMap::new();
    for unit in 0..CHARMAP_ENTRIES {
        let i = font.glyph_index(unit as u16);
        if wanted.contains(&i) {
            *users.entry(i).or_default() += 1;
        }
    }
    // Record 0 is what every unmapped unit draws, so it is never rewritten.
    let in_place = |r: &Replacement| {
        let i = font.glyph_index(r.unit);
        i != 0 && (i as usize) < count && users.get(&i) == Some(&1)
    };
    let added = replacements.iter().filter(|r| !in_place(r)).count();
    let shift = added * RECORD_LEN;
    let appended: usize = streams.iter().map(Vec::len).sum();
    let new_len = data.len() + shift + appended;
    if u32::try_from(new_len).is_err() {
        return Err(Error::Invalid("patched font would exceed 4 GiB"));
    }

    let mut out = Vec::with_capacity(new_len);
    out.extend_from_slice(&data[..data_start]);
    out.resize(data_start + shift, 0);
    out.extend_from_slice(&data[data_start..]);
    if shift > 0 {
        // Pixel data moved up by the new records; so do the offsets into it.
        for i in 0..count {
            let o = RECORDS_OFFSET + i * RECORD_LEN;
            let (len, off) = (
                u16::from_le_bytes([out[o + 2], out[o + 3]]),
                get32(&out, o + 12),
            );
            // Placeholder records (no data) keep whatever offset they carry.
            if len > 0 && off as usize >= data_start {
                put32(&mut out, o + 12, off + shift as u32);
            }
        }
    }

    let mut next_record = count;
    for (r, stream) in replacements.iter().zip(&streams) {
        let old = font.glyph_index(r.unit);
        let (index, y_offset) = if in_place(r) {
            let o = RECORDS_OFFSET + old as usize * RECORD_LEN;
            (old as usize, i16::from_le_bytes([out[o + 10], out[o + 11]]))
        } else {
            let index = next_record;
            next_record += 1;
            put32(
                &mut out,
                CHARMAP_OFFSET + usize::from(r.unit) * 4,
                index as u32,
            );
            let y = if old != 0 {
                font.record(old)?.y_offset
            } else {
                font.header.ascent
            };
            (index, y)
        };
        let o = RECORDS_OFFSET + index * RECORD_LEN;
        put16(&mut out, o, r.advance as u16);
        put16(&mut out, o + 2, stream.len() as u16);
        put16(&mut out, o + 4, r.width);
        put16(&mut out, o + 6, r.height);
        put16(&mut out, o + 8, r.x_offset as u16);
        put16(&mut out, o + 10, r.y_offset.unwrap_or(y_offset) as u16);
        let at = out.len() as u32;
        put32(&mut out, o + 12, at);
        out.extend_from_slice(stream);
    }

    // Recompute from the resulting records: replacing the largest glyph can
    // shrink the maximum, and input totals are advisory, not trusted sums.
    let mut max_bytes = 0u32;
    let mut total_pixels = 0u32;
    for i in 0..next_record {
        let o = RECORDS_OFFSET + i * RECORD_LEN;
        let word = |at| u16::from_le_bytes([out[at], out[at + 1]]);
        max_bytes = max_bytes.max(u32::from(word(o + 2)));
        total_pixels = total_pixels
            .checked_add(u32::from(word(o + 4)) * u32::from(word(o + 6)))
            .ok_or(Error::Invalid("patched pixel count overflows"))?;
    }
    let h = HEADER_OFFSET;
    put32(&mut out, h + 0xc, next_record as u32);
    put32(&mut out, h + 0x10, max_bytes);
    // Everything after the records counts as glyph data, including bytes the
    // input carried past its own declared data size.
    let data_size = (out.len() - (data_start + shift)) as u32;
    put32(&mut out, h + 0x18, data_size);
    put32(&mut out, h + 0x1c, total_pixels);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{G, build, solid};

    fn rep(unit: u16, width: u16, height: u16, pixel: u16) -> Replacement {
        Replacement {
            unit,
            width,
            height,
            advance: width as i16 + 1,
            x_offset: 0,
            y_offset: None,
            pixels: vec![pixel; usize::from(width) * usize::from(height)],
        }
    }

    /// Record 0 blank, 'A' and U+E100 with their own records, U+E200 and
    /// U+E201 sharing one.
    fn font() -> Vec<u8> {
        let mut f = build(
            10,
            3,
            1,
            &[
                G {
                    unit: 0x20,
                    advance: 4,
                    x_offset: 0,
                    y_offset: 10,
                    width: 0,
                    height: 0,
                    data: vec![],
                },
                solid(u16::from(b'A'), 3, 0),
                G {
                    unit: 0xe100,
                    advance: 2,
                    x_offset: 1,
                    y_offset: 9,
                    width: 2,
                    height: 2,
                    data: vec![0x44],
                },
                solid(0xe200, 3, 0),
            ],
            &[(b'A', b'A', -1)],
        );
        // The builder leaves the header's pixel total at zero; three 2x2 glyphs.
        f[HEADER_OFFSET + 0x1c] = 12;
        // Point a second unit at U+E200's record.
        let shared = 3u32.to_le_bytes();
        let o = CHARMAP_OFFSET + 0xe201 * 4;
        f[o..o + 4].copy_from_slice(&shared);
        f
    }

    #[test]
    fn no_replacements_leaves_the_file_identical() {
        let f = font();
        assert_eq!(apply(&f, &[]).unwrap(), f);
    }

    #[test]
    fn owned_record_is_rewritten_in_place() {
        let f = font();
        let out = apply(&f, &[rep(0xe100, 3, 4, 0xf0f0)]).unwrap();
        let (before, after) = (Font::parse(&f).unwrap(), Font::parse(&out).unwrap());
        assert_eq!(after.header.glyph_count, before.header.glyph_count);
        assert_eq!(after.glyph_index(0xe100), before.glyph_index(0xe100));
        let g = after.glyph(0xe100).unwrap();
        assert_eq!(
            (g.record.width, g.record.height, g.record.advance),
            (3, 4, 4)
        );
        // x offset comes from the replacement, y offset from the old record.
        assert_eq!((g.record.x_offset, g.record.y_offset), (0, 9));
        assert_eq!(g.pixels, vec![0xf0f0; 12]);
        // Nothing moved, so the other records are byte-identical.
        let records = RECORDS_OFFSET..RECORDS_OFFSET + 4 * RECORD_LEN;
        let touched = RECORDS_OFFSET + 2 * RECORD_LEN..RECORDS_OFFSET + 3 * RECORD_LEN;
        for i in records {
            if !touched.contains(&i) {
                assert_eq!(out[i], f[i], "record byte {i:#x}");
            }
        }
        assert_eq!(after.validate().unwrap(), 4);
    }

    #[test]
    fn other_glyphs_metrics_and_kerning_survive() {
        let f = font();
        let reps = [
            rep(0xe100, 5, 5, 0xffff),
            rep(0xe200, 2, 3, 0x8fff),
            rep(0x263a, 1, 1, 0xffff),
        ];
        let out = apply(&f, &reps).unwrap();
        let (before, after) = (Font::parse(&f).unwrap(), Font::parse(&out).unwrap());
        assert_eq!(after.glyph(u16::from(b'A')).unwrap(), {
            // The record is the same but for where its data now lives.
            let mut g = before.glyph(u16::from(b'A')).unwrap();
            g.record.data_offset += 2 * RECORD_LEN as u32;
            g
        });
        assert_eq!(after.glyph(0x20).unwrap().pixels, Vec::<u16>::new());
        assert_eq!(after.header.kerning, before.header.kerning);
        assert_eq!(after.kerning(u16::from(b'A'), u16::from(b'A')), -1);
        assert_eq!(
            (
                after.header.ascent,
                after.header.descent,
                after.header.leading
            ),
            (10, 3, 1)
        );
        after.validate().unwrap();
    }

    #[test]
    fn shared_record_gets_a_new_record_and_the_other_unit_keeps_the_old_glyph() {
        let f = font();
        let out = apply(&f, &[rep(0xe200, 4, 2, 0xffff)]).unwrap();
        let after = Font::parse(&out).unwrap();
        assert_eq!(after.header.glyph_count, 5);
        assert_eq!(after.glyph_index(0xe200), 4);
        assert_eq!(after.glyph_index(0xe201), 3);
        assert_eq!(after.glyph(0xe200).unwrap().pixels, vec![0xffff; 8]);
        // The unit still sharing the old record draws the old 2x2 glyph.
        let old = after.glyph(0xe201).unwrap();
        assert_eq!((old.record.width, old.record.height), (2, 2));
        assert_eq!(old.pixels, vec![0xffff; 4]);
        // A shared glyph retains its placement when split into a new record.
        assert_eq!(
            after.glyph(0xe200).unwrap().record.y_offset,
            Font::parse(&f)
                .unwrap()
                .glyph(0xe200)
                .unwrap()
                .record
                .y_offset
        );
        after.validate().unwrap();
    }

    #[test]
    fn unmapped_unit_gets_a_new_record_and_record_zero_is_untouched() {
        let f = font();
        let out = apply(&f, &[rep(0x263a, 2, 2, 0xf00f)]).unwrap();
        let after = Font::parse(&out).unwrap();
        assert_eq!(after.glyph_index(0x263a), 4);
        assert_eq!(after.glyph(0x263a).unwrap().pixels, vec![0xf00f; 4]);
        // Any other unmapped unit still draws the blank record 0.
        assert_eq!(after.glyph_index(0x263b), 0);
        assert_eq!(
            after.record(0).unwrap(),
            Font::parse(&f).unwrap().record(0).unwrap()
        );
        after.validate().unwrap();
    }

    #[test]
    fn header_totals_stay_consistent_with_the_file() {
        let f = font();
        let out = apply(
            &f,
            &[rep(0xe100, 20, 10, 0x5abc), rep(0x263a, 3, 3, 0xffff)],
        )
        .unwrap();
        let after = Font::parse(&out).unwrap();
        let data_start = RECORDS_OFFSET + after.header.glyph_count as usize * RECORD_LEN;
        assert_eq!(out.len(), data_start + after.header.data_size as usize);
        let (mut pixels, mut longest) = (0u32, 0u32);
        for i in 0..after.header.glyph_count {
            let r = after.record(i).unwrap();
            pixels += u32::from(r.width) * u32::from(r.height);
            longest = longest.max(u32::from(r.data_len));
        }
        assert_eq!(after.header.total_pixels, pixels);
        // 200 pixels of an alpha no run byte carries: 3 bytes for the first,
        // which sets the colour, then literals for the rest.
        assert_eq!(longest, 600);
        assert_eq!(after.header.max_glyph_bytes, 600);
    }

    #[test]
    fn bad_replacements_are_rejected() {
        let f = font();
        let invalid = |r: Replacement| matches!(apply(&f, &[r]), Err(Error::Invalid(_)));
        assert!(invalid(rep(0xe100, 0, 4, 0xffff)));
        assert!(invalid(rep(0xe100, 4, 0, 0xffff)));
        assert!(invalid(rep(0xe100, 129, 1, 0xffff)));
        assert!(invalid(Replacement {
            pixels: vec![0xffff; 3],
            ..rep(0xe100, 2, 2, 0xffff)
        }));
        // 128 x 128 pixels that each need a literal: far over 0x1000 bytes.
        assert!(invalid(rep(0xe100, 128, 128, 0x5fff)));
        assert!(matches!(
            apply(&f, &[rep(0xe100, 1, 1, 0xffff), rep(0xe100, 1, 1, 0xffff)]),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            apply(&f[..0x300], &[]),
            Err(Error::Truncated { .. })
        ));
    }
    #[test]
    fn shrinking_largest_record_recomputes_header_totals() {
        let data = font();
        let large = apply(&data, &[rep(0xe100, 20, 10, 0x5abc)]).unwrap();
        let small = apply(&large, &[rep(0xe100, 1, 1, 0xffff)]).unwrap();
        let parsed = Font::parse(&small).unwrap();
        assert_eq!(parsed.header.max_glyph_bytes, 1);
        assert_eq!(parsed.header.total_pixels, 9);
        parsed.validate().unwrap();
    }

    #[test]
    fn splitting_shared_record_keeps_its_vertical_offset() {
        let mut data = font();
        let at = RECORDS_OFFSET + 3 * RECORD_LEN + 10;
        data[at..at + 2].copy_from_slice(&(-3i16).to_le_bytes());
        let out = apply(&data, &[rep(0xe200, 2, 2, 0xffff)]).unwrap();
        let f = Font::parse(&out).unwrap();
        assert_eq!(f.glyph(0xe200).unwrap().record.y_offset, -3);
        assert_eq!(f.glyph(0xe201).unwrap().record.y_offset, -3);
    }
}
