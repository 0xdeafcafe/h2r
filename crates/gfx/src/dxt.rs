//! CPU decoders for DXT1/3/5 (BC1/2/3).
//!
//! The Xbox's DXT blocks are bit-identical to the PC S3TC blocks, so on
//! hosts with `TEXTURE_COMPRESSION_BC` (macOS) they are uploaded as-is. This
//! module is the fallback for hosts without it (tvOS/iOS GPUs, some Vulkan
//! drivers) and the reference the tests compare against.
//!
//! Decoding rules follow the public S3TC/BC specifications (Microsoft's
//! "Block Compression" documentation); nothing here is derived from emulator
//! source.

use crate::format::TexelFormat;

/// Which block-compression variant to decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BcKind {
    /// DXT1: 8-byte blocks, optional 1-bit alpha.
    Bc1,
    /// DXT2/3: 16-byte blocks, explicit 4-bit alpha.
    Bc2,
    /// DXT4/5: 16-byte blocks, interpolated alpha.
    Bc3,
}

impl BcKind {
    /// Maps a [`TexelFormat`] to its block kind, if it is a DXT format.
    pub fn from_texel(t: TexelFormat) -> Option<Self> {
        match t {
            TexelFormat::Dxt1 => Some(BcKind::Bc1),
            TexelFormat::Dxt3 => Some(BcKind::Bc2),
            TexelFormat::Dxt5 => Some(BcKind::Bc3),
            _ => None,
        }
    }

    /// Bytes per 4x4 block.
    pub fn block_bytes(self) -> usize {
        match self {
            BcKind::Bc1 => 8,
            BcKind::Bc2 | BcKind::Bc3 => 16,
        }
    }
}

/// Bytes needed for a `width x height` level (rounded up to whole blocks).
pub fn level_bytes(kind: BcKind, width: u32, height: u32) -> usize {
    let bw = width.max(1).div_ceil(4) as usize;
    let bh = height.max(1).div_ceil(4) as usize;
    bw * bh * kind.block_bytes()
}

fn expand565(c: u16) -> [u8; 3] {
    let r = (c >> 11) as u8 & 0x1f;
    let g = (c >> 5) as u8 & 0x3f;
    let b = c as u8 & 0x1f;
    [(r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2)]
}

/// Decodes the colour half of a block into 16 RGBA texels (row-major).
/// `allow_alpha_mode` enables BC1's 3-colour + transparent mode.
fn decode_color(block: &[u8], allow_alpha_mode: bool) -> [[u8; 4]; 16] {
    let c0 = u16::from_le_bytes([block[0], block[1]]);
    let c1 = u16::from_le_bytes([block[2], block[3]]);
    let (p0, p1) = (expand565(c0), expand565(c1));
    let mix = |a: u8, b: u8, wa: u32, wb: u32, div: u32| ((a as u32 * wa + b as u32 * wb) / div) as u8;
    let mut pal = [[0u8; 4]; 4];
    pal[0] = [p0[0], p0[1], p0[2], 255];
    pal[1] = [p1[0], p1[1], p1[2], 255];
    if c0 > c1 || !allow_alpha_mode {
        for i in 0..3 {
            pal[2][i] = mix(p0[i], p1[i], 2, 1, 3);
            pal[3][i] = mix(p0[i], p1[i], 1, 2, 3);
        }
        pal[2][3] = 255;
        pal[3][3] = 255;
    } else {
        for i in 0..3 {
            pal[2][i] = mix(p0[i], p1[i], 1, 1, 2);
        }
        pal[2][3] = 255;
        pal[3] = [0, 0, 0, 0];
    }
    let idx = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
    let mut out = [[0u8; 4]; 16];
    for (i, px) in out.iter_mut().enumerate() {
        *px = pal[((idx >> (2 * i)) & 3) as usize];
    }
    out
}

fn decode_bc3_alpha(block: &[u8]) -> [u8; 16] {
    let (a0, a1) = (block[0] as u32, block[1] as u32);
    let mut pal = [0u8; 8];
    pal[0] = a0 as u8;
    pal[1] = a1 as u8;
    if a0 > a1 {
        for i in 1..7u32 {
            pal[1 + i as usize] = (((7 - i) * a0 + i * a1) / 7) as u8;
        }
    } else {
        for i in 1..5u32 {
            pal[1 + i as usize] = (((5 - i) * a0 + i * a1) / 5) as u8;
        }
        pal[6] = 0;
        pal[7] = 255;
    }
    let mut bits = 0u64;
    for (i, b) in block[2..8].iter().enumerate() {
        bits |= (*b as u64) << (8 * i);
    }
    let mut out = [0u8; 16];
    for (i, a) in out.iter_mut().enumerate() {
        *a = pal[((bits >> (3 * i)) & 7) as usize];
    }
    out
}

/// Decodes a single block to 16 RGBA texels (row-major, `[R,G,B,A]`).
///
/// `block` must be at least `kind.block_bytes()` long.
pub fn decode_block(kind: BcKind, block: &[u8]) -> [[u8; 4]; 16] {
    match kind {
        BcKind::Bc1 => decode_color(&block[0..8], true),
        BcKind::Bc2 => {
            let mut px = decode_color(&block[8..16], false);
            for (i, p) in px.iter_mut().enumerate() {
                let byte = block[i / 2];
                let nib = if i % 2 == 0 { byte & 0xf } else { byte >> 4 };
                p[3] = nib * 17;
            }
            px
        }
        BcKind::Bc3 => {
            let mut px = decode_color(&block[8..16], false);
            let a = decode_bc3_alpha(&block[0..8]);
            for (p, a) in px.iter_mut().zip(a) {
                p[3] = a;
            }
            px
        }
    }
}

/// Decodes a whole `width x height` level into tightly packed RGBA8.
///
/// `data` holds blocks in row-major block order, as on the Xbox. Missing
/// trailing blocks are an error; extra bytes are ignored. Partial blocks on
/// the right/bottom edge are cropped.
pub fn decode_level(kind: BcKind, data: &[u8], width: u32, height: u32) -> Result<Vec<u8>, usize> {
    let need = level_bytes(kind, width, height);
    if data.len() < need {
        return Err(need);
    }
    let (w, h) = (width as usize, height as usize);
    let bw = w.div_ceil(4);
    let bh = h.div_ceil(4);
    let bb = kind.block_bytes();
    let mut out = vec![0u8; w * h * 4];
    for by in 0..bh {
        for bx in 0..bw {
            let off = (by * bw + bx) * bb;
            let px = decode_block(kind, &data[off..off + bb]);
            for (i, p) in px.iter().enumerate() {
                let (x, y) = (bx * 4 + i % 4, by * 4 + i / 4);
                if x < w && y < h {
                    out[(y * w + x) * 4..][..4].copy_from_slice(p);
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bc1_block(c0: u16, c1: u16, idx: u32) -> [u8; 8] {
        let mut b = [0u8; 8];
        b[0..2].copy_from_slice(&c0.to_le_bytes());
        b[2..4].copy_from_slice(&c1.to_le_bytes());
        b[4..8].copy_from_slice(&idx.to_le_bytes());
        b
    }

    #[test]
    fn bc1_four_color_mode() {
        // c0 = pure red (0xF800), c1 = pure blue (0x001F), c0 > c1.
        // Indices cycle 0,1,2,3 across the first row.
        let idx = 0b11_10_01_00u32;
        let px = decode_block(BcKind::Bc1, &bc1_block(0xF800, 0x001F, idx));
        assert_eq!(px[0], [255, 0, 0, 255]);
        assert_eq!(px[1], [0, 0, 255, 255]);
        assert_eq!(px[2], [170, 0, 85, 255]); // (2*red + blue) / 3
        assert_eq!(px[3], [85, 0, 170, 255]); // (red + 2*blue) / 3
    }

    #[test]
    fn bc1_three_color_mode_has_transparent_black() {
        // c0 <= c1 selects the punch-through mode.
        let idx = 0b11_10_01_00u32;
        let px = decode_block(BcKind::Bc1, &bc1_block(0x001F, 0xF800, idx));
        assert_eq!(px[0], [0, 0, 255, 255]);
        assert_eq!(px[1], [255, 0, 0, 255]);
        assert_eq!(px[2], [127, 0, 127, 255]);
        assert_eq!(px[3], [0, 0, 0, 0]);
    }

    #[test]
    fn expand565_extremes_are_exact() {
        assert_eq!(expand565(0xFFFF), [255, 255, 255]);
        assert_eq!(expand565(0x0000), [0, 0, 0]);
    }

    #[test]
    fn bc2_explicit_alpha() {
        let mut b = [0u8; 16];
        // Alpha nibbles: pixel0 = 0xF, pixel1 = 0x8, pixel2 = 0x0.
        b[0] = 0x8F;
        b[1] = 0x00;
        // Colour block: solid white, all indices 0.
        b[8..10].copy_from_slice(&0xFFFFu16.to_le_bytes());
        b[10..12].copy_from_slice(&0xFFFFu16.to_le_bytes());
        let px = decode_block(BcKind::Bc2, &b);
        assert_eq!(px[0], [255, 255, 255, 255]);
        assert_eq!(px[1], [255, 255, 255, 136]);
        assert_eq!(px[2], [255, 255, 255, 0]);
    }

    #[test]
    fn bc2_ignores_three_color_mode() {
        // c0 < c1 would be punch-through in BC1 but BC2/3 are always 4-colour.
        let mut b = [0u8; 16];
        b[8..10].copy_from_slice(&0x001Fu16.to_le_bytes());
        b[10..12].copy_from_slice(&0xF800u16.to_le_bytes());
        b[12] = 0b10; // texel 0 -> palette[2]
        let px = decode_block(BcKind::Bc2, &b);
        assert_eq!(px[0][..3], [85, 0, 170]);
    }

    #[test]
    fn bc3_interpolated_alpha_8_step() {
        let mut b = [0u8; 16];
        b[0] = 255; // a0 > a1: 8-step ramp
        b[1] = 0;
        // 3-bit indices for texels 0..7: 0,1,2,3,4,5,6,7
        let mut bits = 0u64;
        for i in 0..8u64 {
            bits |= i << (3 * i);
        }
        b[2..8].copy_from_slice(&bits.to_le_bytes()[..6]);
        b[8..10].copy_from_slice(&0xFFFFu16.to_le_bytes());
        b[10..12].copy_from_slice(&0xFFFFu16.to_le_bytes());
        let px = decode_block(BcKind::Bc3, &b);
        let a: Vec<u8> = px[..8].iter().map(|p| p[3]).collect();
        assert_eq!(a, vec![255, 0, 218, 182, 145, 109, 72, 36]);
    }

    #[test]
    fn bc3_six_step_mode_has_zero_and_255() {
        let mut b = [0u8; 16];
        b[0] = 100; // a0 <= a1: 6-step ramp + explicit 0 and 255
        b[1] = 200;
        let mut bits = 0u64;
        for (i, idx) in [0u64, 1, 2, 5, 6, 7].iter().enumerate() {
            bits |= idx << (3 * i);
        }
        b[2..8].copy_from_slice(&bits.to_le_bytes()[..6]);
        let px = decode_block(BcKind::Bc3, &b);
        let a: Vec<u8> = px[..6].iter().map(|p| p[3]).collect();
        assert_eq!(a, vec![100, 200, 120, 180, 0, 255]);
    }

    #[test]
    fn level_decode_crops_and_checks_size() {
        // 2x2 texture is one block; only the top-left 2x2 is kept.
        let block = bc1_block(0xF800, 0xF800, 0);
        let rgba = decode_level(BcKind::Bc1, &block, 2, 2).unwrap();
        assert_eq!(rgba.len(), 16);
        assert!(rgba.chunks(4).all(|p| p == [255, 0, 0, 255]));
        assert_eq!(decode_level(BcKind::Bc1, &block[..4], 2, 2), Err(8));
    }

    #[test]
    fn level_decode_block_order_is_row_major() {
        // 8x4 = two blocks: red then blue.
        let mut data = Vec::new();
        data.extend(bc1_block(0xF800, 0xF800, 0));
        data.extend(bc1_block(0x001F, 0x001F, 0));
        let rgba = decode_level(BcKind::Bc1, &data, 8, 4).unwrap();
        assert_eq!(&rgba[0..4], &[255, 0, 0, 255]);
        assert_eq!(&rgba[(4 * 4)..(4 * 4 + 4)], &[0, 0, 255, 255]);
        assert_eq!(level_bytes(BcKind::Bc1, 8, 4), 16);
        assert_eq!(level_bytes(BcKind::Bc3, 1, 1), 16);
    }
}
