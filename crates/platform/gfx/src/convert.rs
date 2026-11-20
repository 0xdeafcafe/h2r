//! Per-texel conversion of linear (already unswizzled, tightly packed)
//! Xbox texture data to host formats.
//!
//! The channel semantics are the ones the NV2A applies when sampling, as
//! recorded by xemu's per-format swizzle table (see [`crate::format`]):
//!
//! | Xbox format | sampled as `(r, g, b, a)` |
//! |---|---|
//! | `L8` | `(l, l, l, 1)` |
//! | `AL8` | `(v, v, v, v)` |
//! | `A8` | `(1, 1, 1, a)` |
//! | `A8L8` | `(l, l, l, a)`, byte 0 = `l`, byte 1 = `a` |
//! | `X*` formats | alpha forced to 1 |
//! | `G8B8` | `(b0, b1, b0, b1)`: raw bytes 0 and 1 |
//! | `R8B8` | `(b1, b0, b0, b1)` |
//!
//! | `D24S8`, `F24S8` | `(z[23:16], z[15:8], z[7:0], stencil)` (raw bits) |
//! | `D16`, `F16` | `(z[15:8], z[7:0], 0, 0)` (raw bits) |
//!
//! Everything is expanded to RGBA8 except `A8R8G8B8`, which is passed
//! through as BGRA8.
//!
//! Depth formats have no colour meaning: the NV2A samples them as shadow
//! maps, comparing the stored depth with the texture coordinate (xemu's
//! `psh_append_shadowmap`, with `SHADOWCTL`'s function). They are handed
//! to the shader as their raw bits, most significant depth byte in red, so
//! a shader that compares can rebuild the stored value exactly
//! ([`depth_channels`]); float depth (`F24S8`, `F16`) is left as its bits.

use crate::format::TexelFormat;

/// Errors from converting texel data.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConvertError {
    /// Input slice too short.
    #[error("texel data too short: need {need} bytes, have {have}")]
    TooShort {
        /// Bytes required.
        need: usize,
        /// Bytes available.
        have: usize,
    },
    /// This format cannot be converted to a colour texture.
    #[error("unsupported texture format {0:?}")]
    Unsupported(TexelFormat),
    /// `P8` needs a palette.
    #[error("palettised texture without a palette")]
    MissingPalette,
}

#[inline]
fn e5(v: u16) -> u8 {
    let v = (v & 0x1f) as u8;
    (v << 3) | (v >> 2)
}
#[inline]
fn e6(v: u16) -> u8 {
    let v = (v & 0x3f) as u8;
    (v << 2) | (v >> 4)
}
#[inline]
fn e4(v: u16) -> u8 {
    ((v & 0xf) as u8) * 17
}
#[inline]
fn e1(v: u16) -> u8 {
    if v & 1 != 0 { 255 } else { 0 }
}

fn yuv_to_rgb(y: u8, u: u8, v: u8) -> [u8; 3] {
    // BT.601 limited range, the encoding the Xbox video path uses.
    let c = y as i32 - 16;
    let d = u as i32 - 128;
    let e = v as i32 - 128;
    let clamp = |x: i32| x.clamp(0, 255) as u8;
    [
        clamp((298 * c + 409 * e + 128) >> 8),
        clamp((298 * c - 100 * d - 208 * e + 128) >> 8),
        clamp((298 * c + 516 * d + 128) >> 8),
    ]
}

/// Converts a tightly packed `width x height` image (all slices of a volume
/// concatenated count as extra rows) to RGBA8 bytes `R, G, B, A`.
///
/// `palette` supplies 32-bit `A8R8G8B8` entries for [`TexelFormat::P8`].
/// DXT formats are handled by [`crate::dxt`], not here.
pub fn to_rgba8(
    fmt: TexelFormat,
    src: &[u8],
    texels: usize,
    palette: Option<&[u32]>,
) -> Result<Vec<u8>, ConvertError> {
    use TexelFormat::*;
    if let Unsupported(f) = fmt
        && f.is_depth()
    {
        return depth_channels(f, src, texels);
    }
    let bpt = match fmt {
        Unsupported(_) | Dxt1 | Dxt3 | Dxt5 => return Err(ConvertError::Unsupported(fmt)),
        _ => fmt.bytes_per_texel().expect("uncompressed"),
    } as usize;
    let need = texels * bpt;
    if src.len() < need {
        return Err(ConvertError::TooShort { need, have: src.len() });
    }
    let src = &src[..need];
    let mut out = Vec::with_capacity(texels * 4);

    if matches!(fmt, Yuy2 | Uyvy) {
        // Two texels per 4-byte macropixel; an odd trailing texel reuses the
        // last chroma pair.
        for pair in src.chunks(4) {
            let (y0, u, y1, v) = match (fmt, pair) {
                (Yuy2, [y0, u, y1, v]) => (*y0, *u, *y1, *v),
                (Uyvy, [u, y0, v, y1]) => (*y0, *u, *y1, *v),
                _ => break,
            };
            let a = yuv_to_rgb(y0, u, v);
            let b = yuv_to_rgb(y1, u, v);
            out.extend_from_slice(&[a[0], a[1], a[2], 255, b[0], b[1], b[2], 255]);
        }
        out.truncate(texels * 4);
        return Ok(out);
    }

    if fmt == P8 {
        let pal = palette.ok_or(ConvertError::MissingPalette)?;
        for &i in src {
            // Out-of-range indices read as transparent black, like an
            // unpopulated palette slot.
            let argb = pal.get(i as usize).copied().unwrap_or(0);
            out.extend_from_slice(&[(argb >> 16) as u8, (argb >> 8) as u8, argb as u8, (argb >> 24) as u8]);
        }
        return Ok(out);
    }

    for t in src.chunks_exact(bpt) {
        let px: [u8; 4] = match fmt {
            L8 => [t[0], t[0], t[0], 255],
            Al8 => [t[0], t[0], t[0], t[0]],
            A8 => [255, 255, 255, t[0]],
            A8L8 => [t[0], t[0], t[0], t[1]],
            G8B8 => [t[0], t[1], t[0], t[1]],
            R8B8 => [t[1], t[0], t[0], t[1]],
            A8R8G8B8 => [t[2], t[1], t[0], t[3]],
            X8R8G8B8 => [t[2], t[1], t[0], 255],
            // Memory bytes of A8B8G8R8 are R, G, B, A.
            A8B8G8R8 => [t[0], t[1], t[2], t[3]],
            // B8G8R8A8: dword = B<<24 | G<<16 | R<<8 | A -> bytes A, R, G, B.
            B8G8R8A8 => [t[1], t[2], t[3], t[0]],
            // R8G8B8A8: dword = R<<24 | G<<16 | B<<8 | A -> bytes A, B, G, R.
            R8G8B8A8 => [t[3], t[2], t[1], t[0]],
            // xemu's SZ_Y16 uploads the two bytes as RG8 with swizzle
            // (ONE, GREEN, RED, ONE): output (1, byte1, byte0, 1).
            L16 => [255, t[1], t[0], 255],
            _ => {
                let v = u16::from_le_bytes([t[0], t[1]]);
                match fmt {
                    A1R5G5B5 => [e5(v >> 10), e5(v >> 5), e5(v), e1(v >> 15)],
                    X1R5G5B5 => [e5(v >> 10), e5(v >> 5), e5(v), 255],
                    A4R4G4B4 => [e4(v >> 8), e4(v >> 4), e4(v), e4(v >> 12)],
                    R5G6B5 => [e5(v >> 11), e6(v >> 5), e5(v), 255],
                    // R6G5B5: 6 bits red at the top, then 5 green, 5 blue.
                    R6G5B5 => [e6(v >> 10), e5(v >> 5), e5(v), 255],
                    R5G5B5A1 => [e5(v >> 11), e5(v >> 6), e5(v >> 1), e1(v)],
                    R4G4B4A4 => [e4(v >> 12), e4(v >> 8), e4(v >> 4), e4(v)],
                    _ => unreachable!("handled above: {fmt:?}"),
                }
            }
        };
        out.extend_from_slice(&px);
    }
    Ok(out)
}

/// A depth texture's raw bits as RGBA8: for 32-bit formats (`D24S8`,
/// `F24S8`, word = `z << 8 | stencil`) red, green and blue are the depth
/// bytes from most to least significant and alpha the stencil; for 16-bit
/// formats (`D16`, `F16`) red and green are the depth bytes and blue and
/// alpha zero. Sampled with point filtering, `round(c * 255)` gives the
/// bytes back. `format` must be a depth format.
pub fn depth_channels(format: crate::format::D3DFormat, src: &[u8], texels: usize) -> Result<Vec<u8>, ConvertError> {
    let bpt = if format.bytes_per_texel() == Some(2) { 2 } else { 4 };
    let need = texels * bpt;
    if src.len() < need {
        return Err(ConvertError::TooShort { need, have: src.len() });
    }
    let mut out = Vec::with_capacity(texels * 4);
    for t in src[..need].chunks_exact(bpt) {
        out.extend_from_slice(&match t {
            [lo, hi] => [*hi, *lo, 0, 0],
            [s, z0, z1, z2] => [*z2, *z1, *z0, *s],
            _ => unreachable!("chunks of {bpt}"),
        });
    }
    Ok(out)
}

/// `A8R8G8B8`/`X8R8G8B8` pass-through to BGRA8: copies, forcing alpha to 255
/// for the `X` variant.
pub fn to_bgra8(fmt: TexelFormat, src: &[u8], texels: usize) -> Result<Vec<u8>, ConvertError> {
    let need = texels * 4;
    if src.len() < need {
        return Err(ConvertError::TooShort { need, have: src.len() });
    }
    let mut out = src[..need].to_vec();
    if fmt == TexelFormat::X8R8G8B8 {
        for p in out.as_chunks_mut::<4>().0.iter_mut() {
            p[3] = 255;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use TexelFormat::*;

    fn one(fmt: TexelFormat, bytes: &[u8]) -> [u8; 4] {
        let v = to_rgba8(fmt, bytes, 1, None).unwrap();
        [v[0], v[1], v[2], v[3]]
    }

    #[test]
    fn eight_bit_formats() {
        assert_eq!(one(L8, &[0x80]), [0x80, 0x80, 0x80, 255]);
        assert_eq!(one(Al8, &[0x40]), [0x40; 4]);
        assert_eq!(one(A8, &[0x33]), [255, 255, 255, 0x33]);
        assert_eq!(one(A8L8, &[0x10, 0x20]), [0x10, 0x10, 0x10, 0x20]);
    }

    #[test]
    fn thirty_two_bit_formats() {
        // A8R8G8B8 bytes are B, G, R, A.
        assert_eq!(one(A8R8G8B8, &[1, 2, 3, 4]), [3, 2, 1, 4]);
        assert_eq!(one(X8R8G8B8, &[1, 2, 3, 0]), [3, 2, 1, 255]);
        assert_eq!(one(A8B8G8R8, &[1, 2, 3, 4]), [1, 2, 3, 4]);
        assert_eq!(one(B8G8R8A8, &[4, 1, 2, 3]), [1, 2, 3, 4]);
        assert_eq!(one(R8G8B8A8, &[4, 3, 2, 1]), [1, 2, 3, 4]);
    }

    #[test]
    fn sixteen_bit_formats_hit_extremes() {
        assert_eq!(one(R5G6B5, &0xFFFFu16.to_le_bytes()), [255, 255, 255, 255]);
        assert_eq!(one(R5G6B5, &0xF800u16.to_le_bytes()), [255, 0, 0, 255]);
        assert_eq!(one(R5G6B5, &0x07E0u16.to_le_bytes()), [0, 255, 0, 255]);
        assert_eq!(one(R5G6B5, &0x001Fu16.to_le_bytes()), [0, 0, 255, 255]);
        assert_eq!(one(A1R5G5B5, &0x8000u16.to_le_bytes()), [0, 0, 0, 255]);
        assert_eq!(one(A1R5G5B5, &0x7C00u16.to_le_bytes()), [255, 0, 0, 0]);
        assert_eq!(one(X1R5G5B5, &0x0000u16.to_le_bytes()), [0, 0, 0, 255]);
        assert_eq!(one(A4R4G4B4, &0xF0F0u16.to_le_bytes()), [0, 255, 0, 255]);
        assert_eq!(one(A4R4G4B4, &0x8421u16.to_le_bytes()), [0x44, 0x22, 0x11, 0x88]);
        assert_eq!(one(R5G5B5A1, &0xF801u16.to_le_bytes()), [255, 0, 0, 255]);
        assert_eq!(one(R4G4B4A4, &0x000Fu16.to_le_bytes()), [0, 0, 0, 255]);
        assert_eq!(one(R6G5B5, &0xFC00u16.to_le_bytes()), [255, 0, 0, 255]);
    }

    #[test]
    fn bump_style_two_channel_swizzles() {
        assert_eq!(one(G8B8, &[10, 20]), [10, 20, 10, 20]);
        assert_eq!(one(R8B8, &[10, 20]), [20, 10, 10, 20]);
    }

    #[test]
    fn palette_lookup() {
        let pal = [0xFF112233u32, 0x80445566];
        let v = to_rgba8(P8, &[1, 0, 9], 3, Some(&pal)).unwrap();
        assert_eq!(v, vec![0x44, 0x55, 0x66, 0x80, 0x11, 0x22, 0x33, 0xFF, 0, 0, 0, 0]);
        assert_eq!(to_rgba8(P8, &[0], 1, None), Err(ConvertError::MissingPalette));
    }

    #[test]
    fn yuy2_and_uyvy_neutral_grey_and_extremes() {
        // Y=235 (white), U=V=128.
        let v = to_rgba8(Yuy2, &[235, 128, 235, 128], 2, None).unwrap();
        assert_eq!(&v[..4], &[255, 255, 255, 255]);
        assert_eq!(&v[4..], &[255, 255, 255, 255]);
        // Y=16 (black) in UYVY order.
        let v = to_rgba8(Uyvy, &[128, 16, 128, 16], 2, None).unwrap();
        assert_eq!(&v[..4], &[0, 0, 0, 255]);
    }

    #[test]
    fn bgra_passthrough_forces_alpha_for_x() {
        let src = [1, 2, 3, 0, 4, 5, 6, 7];
        assert_eq!(to_bgra8(X8R8G8B8, &src, 2).unwrap(), vec![1, 2, 3, 255, 4, 5, 6, 255]);
        assert_eq!(to_bgra8(A8R8G8B8, &src, 2).unwrap(), src.to_vec());
    }

    #[test]
    fn errors() {
        assert_eq!(
            to_rgba8(L8, &[0], 2, None),
            Err(ConvertError::TooShort { need: 2, have: 1 })
        );
        assert!(matches!(to_rgba8(Dxt1, &[], 0, None), Err(ConvertError::Unsupported(_))));
    }

    #[test]
    fn l16_uses_xemu_y16_swizzle() {
        let px = to_rgba8(TexelFormat::L16, &[0x34, 0x12], 1, None).unwrap();
        assert_eq!(px, vec![255, 0x12, 0x34, 255]);
    }

    #[test]
    fn depth_formats_give_their_raw_bits() {
        use crate::format::D3DFormat as F;
        // D24S8 word 0xABCDEF_5A: depth 0xABCDEF, stencil 0x5A.
        let word = 0xABCD_EF5Au32.to_le_bytes();
        for f in [F::D24S8, F::LinD24S8, F::F24S8, F::LinF24S8] {
            assert_eq!(one(Unsupported(f), &word), [0xAB, 0xCD, 0xEF, 0x5A], "{f:?}");
        }
        for f in [F::D16, F::LinD16, F::F16, F::LinF16] {
            assert_eq!(one(Unsupported(f), &0x1234u16.to_le_bytes()), [0x12, 0x34, 0, 0], "{f:?}");
        }
        assert!(matches!(depth_channels(F::D24S8, &[0; 3], 1), Err(ConvertError::TooShort { need: 4, have: 3 })));
        // Not depth: still unsupported.
        assert!(to_rgba8(Unsupported(F::V16U16), &[0; 4], 1, None).is_err());
    }
}
