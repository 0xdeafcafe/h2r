//! Host pixels back to guest memory formats (for readbacks that must land in
//! guest-visible surfaces: `CopyRects` to system memory, `Lock` on a render
//! target, depth readback). The inverse of [`crate::convert`] for the
//! formats that can be render targets.
//!
//! These functions are pure; the GPU side lives in [`crate::surface`].

use crate::format::D3DFormat;
use crate::surface::SurfaceLayout;
use crate::swizzle;

/// Byte order of host colour pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostPixelOrder {
    /// Bytes `B, G, R, A`.
    Bgra8,
    /// Bytes `R, G, B, A`.
    Rgba8,
}

/// Errors from encoding.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EncodeError {
    /// No encoder for this guest format.
    #[error("cannot encode to {0:?}")]
    Unsupported(D3DFormat),
    /// Input shorter than `width * height` pixels.
    #[error("pixel data too short: need {need} bytes, have {have}")]
    TooShort {
        /// Bytes needed.
        need: usize,
        /// Bytes supplied.
        have: usize,
    },
    /// Swizzled layout with non power-of-two dimensions.
    #[error(transparent)]
    Swizzle(#[from] swizzle::SwizzleError),
}

/// Lays tightly packed rows out as the guest expects.
fn apply_layout(tight: Vec<u8>, bpt: usize, w: u32, h: u32, layout: &SurfaceLayout) -> Result<Vec<u8>, EncodeError> {
    match layout {
        SurfaceLayout::Swizzled => Ok(swizzle::swizzle(&tight, w, h, 1, bpt)?),
        SurfaceLayout::Linear { pitch } | SurfaceLayout::Tiled { pitch, .. } => {
            let row = w as usize * bpt;
            let pitch = (*pitch as usize).max(row);
            if pitch == row {
                return Ok(tight);
            }
            let mut out = vec![0u8; pitch * h as usize];
            for y in 0..h as usize {
                out[y * pitch..y * pitch + row].copy_from_slice(&tight[y * row..(y + 1) * row]);
            }
            Ok(out)
        }
    }
}

/// Encodes host colour pixels into the guest `format` and `layout`.
pub fn encode_color(
    format: D3DFormat,
    order: HostPixelOrder,
    pixels: &[u8],
    w: u32,
    h: u32,
    layout: &SurfaceLayout,
) -> Result<Vec<u8>, EncodeError> {
    let n = w as usize * h as usize;
    if pixels.len() < n * 4 {
        return Err(EncodeError::TooShort { need: n * 4, have: pixels.len() });
    }
    let rgba = |p: &[u8]| match order {
        HostPixelOrder::Bgra8 => (p[2], p[1], p[0], p[3]),
        HostPixelOrder::Rgba8 => (p[0], p[1], p[2], p[3]),
    };
    let texel = format.texel();
    use crate::format::TexelFormat as T;
    let mut out: Vec<u8> = Vec::new();
    let bpt = match texel {
        T::A8R8G8B8 | T::X8R8G8B8 => {
            for p in pixels[..n * 4].as_chunks::<4>().0.iter() {
                let (r, g, b, a) = rgba(p);
                let a = if texel == T::X8R8G8B8 { 255 } else { a };
                out.extend_from_slice(&[b, g, r, a]);
            }
            4
        }
        T::R5G6B5 | T::A1R5G5B5 | T::X1R5G5B5 | T::A4R4G4B4 => {
            for p in pixels[..n * 4].as_chunks::<4>().0.iter() {
                let (r, g, b, a) = rgba(p);
                let (r, g, b, a) = (r as u16, g as u16, b as u16, a as u16);
                let v = match texel {
                    T::R5G6B5 => ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3),
                    T::A1R5G5B5 => ((a >> 7) << 15) | ((r >> 3) << 10) | ((g >> 3) << 5) | (b >> 3),
                    T::X1R5G5B5 => 0x8000 | ((r >> 3) << 10) | ((g >> 3) << 5) | (b >> 3),
                    _ => ((a >> 4) << 12) | ((r >> 4) << 8) | ((g >> 4) << 4) | (b >> 4),
                };
                out.extend_from_slice(&v.to_le_bytes());
            }
            2
        }
        _ => return Err(EncodeError::Unsupported(format)),
    };
    apply_layout(out, bpt, w, h, layout)
}

/// Depth (and optionally stencil) values read back from a surface.
#[derive(Clone, Debug, PartialEq)]
pub struct DepthPlane {
    /// Normalised depth in `[0, 1]`, row-major.
    pub depth: Vec<f32>,
    /// Stencil values, row-major, if the surface has a stencil aspect.
    pub stencil: Option<Vec<u8>>,
}

/// Encodes depth/stencil to the guest format. Float-depth formats
/// (`F16`, `F24S8`) are not supported: they store a non-linear value that the
/// fixed-depth host surfaces cannot represent.
pub fn encode_depth(
    format: D3DFormat,
    plane: &DepthPlane,
    w: u32,
    h: u32,
    layout: &SurfaceLayout,
) -> Result<Vec<u8>, EncodeError> {
    let n = w as usize * h as usize;
    if plane.depth.len() < n {
        return Err(EncodeError::TooShort { need: n, have: plane.depth.len() });
    }
    let mut out = Vec::new();
    let bpt = match format {
        D3DFormat::D16 | D3DFormat::LinD16 => {
            for d in &plane.depth[..n] {
                out.extend_from_slice(&((d.clamp(0.0, 1.0) * 65535.0).round() as u16).to_le_bytes());
            }
            2
        }
        D3DFormat::D24S8 | D3DFormat::LinD24S8 => {
            for (i, d) in plane.depth[..n].iter().enumerate() {
                let z = (d.clamp(0.0, 1.0) * 16_777_215.0).round() as u32;
                let s = plane.stencil.as_ref().and_then(|s| s.get(i)).copied().unwrap_or(0) as u32;
                out.extend_from_slice(&((z << 8) | s).to_le_bytes());
            }
            4
        }
        _ => return Err(EncodeError::Unsupported(format)),
    };
    apply_layout(out, bpt, w, h, layout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::texture::{DecodeOptions, TextureDesc, TextureKind, decode_texture};

    #[test]
    fn a8r8g8b8_from_bgra_and_rgba() {
        let bgra = [10, 20, 30, 40];
        let rgba = [30, 20, 10, 40];
        let lin = SurfaceLayout::Linear { pitch: 4 };
        let a = encode_color(D3DFormat::A8R8G8B8, HostPixelOrder::Bgra8, &bgra, 1, 1, &lin).unwrap();
        let b = encode_color(D3DFormat::A8R8G8B8, HostPixelOrder::Rgba8, &rgba, 1, 1, &lin).unwrap();
        assert_eq!(a, vec![10, 20, 30, 40]);
        assert_eq!(a, b);
        let x = encode_color(D3DFormat::X8R8G8B8, HostPixelOrder::Bgra8, &bgra, 1, 1, &lin).unwrap();
        assert_eq!(x, vec![10, 20, 30, 255]);
    }

    #[test]
    fn r5g6b5_known_values() {
        let px = [255u8, 0, 0, 255, 0, 255, 0, 255]; // red, green (RGBA)
        let lin = SurfaceLayout::Linear { pitch: 4 };
        let out = encode_color(D3DFormat::R5G6B5, HostPixelOrder::Rgba8, &px, 2, 1, &lin).unwrap();
        assert_eq!(out, vec![0x00, 0xF8, 0xE0, 0x07]);
    }

    #[test]
    fn pitch_padding_and_swizzle() {
        let px: Vec<u8> = (0..16).flat_map(|i| [i as u8, 0, 0, 255]).collect(); // 4x4 BGRA
        let padded = encode_color(
            D3DFormat::A8R8G8B8,
            HostPixelOrder::Bgra8,
            &px,
            4,
            4,
            &SurfaceLayout::Linear { pitch: 32 },
        )
        .unwrap();
        assert_eq!(padded.len(), 32 * 4);
        assert_eq!(padded[32], 4); // row 1, pixel 0
        let sw = encode_color(D3DFormat::A8R8G8B8, HostPixelOrder::Bgra8, &px, 4, 4, &SurfaceLayout::Swizzled).unwrap();
        let back = swizzle::unswizzle(&sw, 4, 4, 1, 4).unwrap();
        assert_eq!(back, px);
        assert_ne!(sw, px);
    }

    #[test]
    fn encode_then_decode_roundtrips_through_texture_path() {
        // 8x4 pixels through every 16-bit format and the swizzled layout.
        let px: Vec<u8> = (0..32).flat_map(|i| [(i * 8) as u8, (255 - i * 8) as u8, (i * 3) as u8, 255]).collect();
        for fmt in [D3DFormat::R5G6B5, D3DFormat::A1R5G5B5, D3DFormat::X1R5G5B5, D3DFormat::A4R4G4B4, D3DFormat::A8R8G8B8] {
            let bytes = encode_color(fmt, HostPixelOrder::Rgba8, &px, 8, 4, &SurfaceLayout::Swizzled).unwrap();
            let desc = TextureDesc::new(fmt, TextureKind::D2, 8, 4, 1, 1);
            let t = decode_texture(&desc, &bytes, None, DecodeOptions::default()).unwrap();
            let got = &t.levels[0].data;
            let bgra = matches!(fmt, D3DFormat::A8R8G8B8);
            for (i, (g, e)) in got.chunks(4).zip(px.chunks(4)).enumerate() {
                let (r, gr, b) = if bgra { (g[2], g[1], g[0]) } else { (g[0], g[1], g[2]) };
                let tol = if matches!(fmt, D3DFormat::A8R8G8B8) { 0 } else { 16 };
                for (a, b) in [(r, e[0]), (gr, e[1]), (b, e[2])] {
                    assert!((a as i32 - b as i32).abs() <= tol, "{fmt:?} px {i}: {a} vs {b}");
                }
            }
        }
    }

    #[test]
    fn depth_encodings() {
        let plane = DepthPlane { depth: vec![0.0, 1.0, 0.5, 0.25], stencil: Some(vec![0, 1, 2, 255]) };
        let lin = SurfaceLayout::Linear { pitch: 4 };
        let d16 = encode_depth(D3DFormat::D16, &plane, 2, 2, &lin).unwrap();
        assert_eq!(d16, vec![0, 0, 0xFF, 0xFF, 0x00, 0x80, 0x00, 0x40]);
        let lin = SurfaceLayout::Linear { pitch: 8 };
        let d24 = encode_depth(D3DFormat::D24S8, &plane, 2, 2, &lin).unwrap();
        let word = |i: usize| u32::from_le_bytes(d24[i * 4..i * 4 + 4].try_into().unwrap());
        assert_eq!(word(0), 0);
        assert_eq!(word(1), (0xFFFFFF << 8) | 1);
        assert_eq!(word(2), (0x800000 << 8) | 2);
        assert_eq!(word(3) & 0xFF, 255);
    }

    #[test]
    fn errors() {
        let lin = SurfaceLayout::Linear { pitch: 4 };
        assert!(matches!(
            encode_color(D3DFormat::L8, HostPixelOrder::Rgba8, &[0; 4], 1, 1, &lin),
            Err(EncodeError::Unsupported(_))
        ));
        assert!(matches!(
            encode_color(D3DFormat::A8R8G8B8, HostPixelOrder::Rgba8, &[0; 3], 1, 1, &lin),
            Err(EncodeError::TooShort { .. })
        ));
        assert!(matches!(
            encode_color(D3DFormat::A8R8G8B8, HostPixelOrder::Rgba8, &[0; 12], 3, 1, &SurfaceLayout::Swizzled),
            Err(EncodeError::Swizzle(_))
        ));
        let plane = DepthPlane { depth: vec![0.0], stencil: None };
        assert!(encode_depth(D3DFormat::F24S8, &plane, 1, 1, &lin).is_err());
    }
}
