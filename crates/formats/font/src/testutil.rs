//! Builders for synthetic fonts used by the tests.
use crate::*;

/// One glyph to place in a synthetic font.
pub struct G {
    pub unit: u16,
    pub advance: i16,
    pub x_offset: i16,
    pub y_offset: i16,
    pub width: u16,
    pub height: u16,
    /// Compressed stream.
    pub data: Vec<u8>,
}

pub fn build(
    ascent: i16,
    descent: i16,
    leading: i16,
    glyphs: &[G],
    kern: &[(u8, u8, i16)],
) -> Vec<u8> {
    let mut f = vec![0u8; RECORDS_OFFSET + glyphs.len() * RECORD_LEN];
    let put16 = |f: &mut Vec<u8>, o: usize, v: u16| f[o..o + 2].copy_from_slice(&v.to_le_bytes());
    let put32 = |f: &mut Vec<u8>, o: usize, v: u32| f[o..o + 4].copy_from_slice(&v.to_le_bytes());
    let h = HEADER_OFFSET;
    put32(&mut f, h, MAGIC);
    put16(&mut f, h + 4, ascent as u16);
    put16(&mut f, h + 6, descent as u16);
    put16(&mut f, h + 8, leading as u16);
    put32(&mut f, h + 0xc, glyphs.len() as u32);
    put32(&mut f, h + 0x20, kern.len() as u32);
    for (i, (a, b, k)) in kern.iter().enumerate() {
        let o = h + 0x24 + i * 4;
        f[o] = *a;
        f[o + 1] = *b;
        put16(&mut f, o + 2, *k as u16);
        let bit = h + 0x1a4 + usize::from(*a >> 5) * 4;
        let w = u32::from_le_bytes(f[bit..bit + 4].try_into().unwrap()) | 1 << (a & 31);
        put32(&mut f, bit, w);
    }
    let mut data_off = f.len();
    let mut data = Vec::new();
    for (i, g) in glyphs.iter().enumerate() {
        put32(&mut f, CHARMAP_OFFSET + usize::from(g.unit) * 4, i as u32);
        let o = RECORDS_OFFSET + i * RECORD_LEN;
        put16(&mut f, o, g.advance as u16);
        put16(&mut f, o + 2, g.data.len() as u16);
        put16(&mut f, o + 4, g.width);
        put16(&mut f, o + 6, g.height);
        put16(&mut f, o + 8, g.x_offset as u16);
        put16(&mut f, o + 10, g.y_offset as u16);
        put32(&mut f, o + 12, data_off as u32);
        data_off += g.data.len();
        data.extend_from_slice(&g.data);
    }
    put32(&mut f, h + 0x18, data.len() as u32);
    f.extend_from_slice(&data);
    f
}

/// A 2x2 glyph, all pixels full alpha: `01 000100`.
pub fn solid(unit: u16, advance: i16, x: i16) -> G {
    G {
        unit,
        advance,
        x_offset: x,
        y_offset: 1,
        width: 2,
        height: 2,
        data: vec![0x44],
    }
}
