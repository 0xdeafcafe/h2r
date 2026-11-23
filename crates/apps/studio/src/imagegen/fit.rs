//! A generated picture made to fit a bitmap slot: its size, its pixel
//! format and its alpha (docs/editor/agent-api.md, "Generating images").
//!
//! [`fit`] resamples with the Textures tool's own filter
//! ([`bitmap_edit::resample`]: box down, bilinear up, in linear light,
//! colour weighted by alpha), then rounds every pixel to what the slot's
//! format stores and its decoder gives back: the uncompressed formats by
//! the encoder's quantisation (`assets::encode`) and the decoder's
//! expansion (`gfx::convert`), the DXT formats by encoding each 4 by 4
//! block with `assets::encode::encode_block` and decoding it with
//! `gfx::dxt::decode_block`. So the preview is the picture the game
//! shows, and importing it changes nothing more when the map saves.

use bitmap_edit::resample;
use gfx::dxt::{BcKind, decode_block};
use tags::PixelFormat;

/// Where the slot's alpha comes from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Alpha {
    /// The picture's own alpha, except that a fully opaque picture going
    /// into an alpha-only format (`A8`, `AY8`) takes its brightness.
    #[default]
    Auto,
    /// The picture's own alpha.
    Image,
    /// The picture's brightness (Rec. 601 luma): a white-on-black
    /// picture becomes a cut-out.
    Luma,
    /// Fully opaque.
    Opaque,
}

impl Alpha {
    pub fn parse(s: &str) -> Option<Alpha> {
        Some(match s {
            "auto" => Alpha::Auto,
            "image" => Alpha::Image,
            "luma" => Alpha::Luma,
            "opaque" => Alpha::Opaque,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Alpha::Auto => "auto",
            Alpha::Image => "image",
            Alpha::Luma => "luma",
            Alpha::Opaque => "opaque",
        }
    }
}

/// The picture as the slot holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fitted {
    pub width: u32,
    pub height: u32,
    /// Row-major RGBA8.
    pub rgba: Vec<u8>,
    /// The alpha used: `image`, `luma` or `opaque` (never `auto`).
    pub alpha: Alpha,
}

/// The formats [`fit`] rounds to: the ones the Textures tool writes
/// (`assets::encode::writable`).
pub fn writable(format: PixelFormat) -> bool {
    assets::encode::writable(format)
}

fn luma(p: &[u8]) -> u8 {
    ((77 * u32::from(p[0]) + 150 * u32::from(p[1]) + 29 * u32::from(p[2]) + 128) >> 8) as u8
}

/// Whether the format keeps any alpha.
pub fn has_alpha(format: PixelFormat) -> bool {
    use PixelFormat as F;
    matches!(
        format,
        F::A8 | F::Ay8 | F::A8Y8 | F::Argb1555 | F::Argb4444 | F::Argb8888 | F::Dxt1 | F::Dxt3 | F::Dxt5
    )
}

/// Fits `src` (`sw` by `sh`, RGBA8) to a `width` by `height` slot of
/// `format` (`None`: 32-bit RGBA, only resampled). A picture whose aspect
/// is within a tenth of the slot's is stretched to it; otherwise it is
/// scaled to fit inside and centred on transparent pixels.
pub fn fit(
    src: &[u8],
    sw: u32,
    sh: u32,
    width: u32,
    height: u32,
    format: Option<PixelFormat>,
    alpha: Alpha,
) -> Result<Fitted, String> {
    if sw == 0 || sh == 0 || src.len() != sw as usize * sh as usize * 4 {
        return Err(format!("the picture is not {sw} by {sh} RGBA pixels"));
    }
    if width == 0 || height == 0 {
        return Err("the slot has no size".into());
    }
    if let Some(f) = format
        && !writable(f)
    {
        return Err(format!(
            "the slot's format {f:?} is not a picture format Studio writes"
        ));
    }
    let opaque = src.chunks_exact(4).all(|p| p[3] == 255);
    let alpha = match alpha {
        Alpha::Auto if opaque && matches!(format, Some(PixelFormat::A8 | PixelFormat::Ay8)) => {
            Alpha::Luma
        }
        Alpha::Auto => Alpha::Image,
        a => a,
    };
    let mut px = src.to_vec();
    match alpha {
        Alpha::Luma => {
            for p in px.chunks_exact_mut(4) {
                p[3] = luma(p);
            }
        }
        Alpha::Opaque => {
            for p in px.chunks_exact_mut(4) {
                p[3] = 255;
            }
        }
        _ => {}
    }
    let rgba = place(&px, sw, sh, width, height)?;
    let rgba = match format {
        Some(f) => quantise(&rgba, width, height, f),
        None => rgba,
    };
    Ok(Fitted {
        width,
        height,
        rgba,
        alpha,
    })
}

/// `src` resampled to `w` by `h`: stretched when the aspects are within a
/// tenth, else scaled inside and centred on transparent pixels.
fn place(src: &[u8], sw: u32, sh: u32, w: u32, h: u32) -> Result<Vec<u8>, String> {
    let a = f64::from(sw) / f64::from(sh);
    let b = f64::from(w) / f64::from(h);
    if (a / b - 1.0).abs() <= 0.1 {
        return resample(src, sw, sh, w, h).map_err(|e| e.to_string());
    }
    let (iw, ih) = if a > b {
        (w, ((f64::from(w) / a).round() as u32).clamp(1, h))
    } else {
        (((f64::from(h) * a).round() as u32).clamp(1, w), h)
    };
    let inner = resample(src, sw, sh, iw, ih).map_err(|e| e.to_string())?;
    let mut out = vec![0u8; w as usize * h as usize * 4];
    let (ox, oy) = ((w - iw) / 2, (h - ih) / 2);
    for y in 0..ih {
        let from = (y * iw * 4) as usize;
        let to = (((oy + y) * w + ox) * 4) as usize;
        out[to..to + iw as usize * 4].copy_from_slice(&inner[from..from + iw as usize * 4]);
    }
    Ok(out)
}

/// `v` stored in a field of `max` and read back to 8 bits, as the encoder
/// and the decoder do.
fn round_trip(v: u8, bits: u32) -> u8 {
    let max = (1u32 << bits) - 1;
    let q = (u32::from(v) * max + 127) / 255;
    match bits {
        1 => {
            if q != 0 {
                255
            } else {
                0
            }
        }
        4 => (q * 17) as u8,
        5 => ((q << 3) | (q >> 2)) as u8,
        6 => ((q << 2) | (q >> 4)) as u8,
        _ => v,
    }
}

/// Every pixel as `format` stores it and its decoder returns it.
fn quantise(rgba: &[u8], w: u32, h: u32, format: PixelFormat) -> Vec<u8> {
    use PixelFormat as F;
    let kind = match format {
        F::Dxt1 => Some(BcKind::Bc1),
        F::Dxt3 => Some(BcKind::Bc2),
        F::Dxt5 => Some(BcKind::Bc3),
        _ => None,
    };
    if let Some(kind) = kind {
        return dxt(rgba, w, h, kind);
    }
    let mut out = rgba.to_vec();
    for p in out.chunks_exact_mut(4) {
        let px: [u8; 4] = match format {
            F::A8 => [255, 255, 255, p[3]],
            F::Y8 => {
                let l = luma(p);
                [l, l, l, 255]
            }
            F::Ay8 => {
                let l = luma(p);
                [l, l, l, l]
            }
            F::A8Y8 => {
                let l = luma(p);
                [l, l, l, p[3]]
            }
            F::Rgb565 => [round_trip(p[0], 5), round_trip(p[1], 6), round_trip(p[2], 5), 255],
            F::Argb1555 => [
                round_trip(p[0], 5),
                round_trip(p[1], 5),
                round_trip(p[2], 5),
                if p[3] >= 128 { 255 } else { 0 },
            ],
            F::Argb4444 => [
                round_trip(p[0], 4),
                round_trip(p[1], 4),
                round_trip(p[2], 4),
                round_trip(p[3], 4),
            ],
            F::Xrgb8888 => [p[0], p[1], p[2], 255],
            _ => [p[0], p[1], p[2], p[3]],
        };
        p.copy_from_slice(&px);
    }
    out
}

/// Each 4 by 4 block encoded as the map stores it and decoded back; edge
/// blocks of a side that is not a multiple of 4 repeat the last row and
/// column, and only the pixels inside are kept.
fn dxt(rgba: &[u8], w: u32, h: u32, kind: BcKind) -> Vec<u8> {
    let mut out = rgba.to_vec();
    let at = |x: u32, y: u32| -> [u8; 4] {
        let i = ((y.min(h - 1) * w + x.min(w - 1)) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    };
    for by in (0..h).step_by(4) {
        for bx in (0..w).step_by(4) {
            let mut block = [[0u8; 4]; 16];
            for (i, t) in block.iter_mut().enumerate() {
                *t = at(bx + (i as u32 % 4), by + (i as u32 / 4));
            }
            let bytes = assets::encode::encode_block(kind, &block);
            let back = decode_block(kind, &bytes);
            for (i, t) in back.iter().enumerate() {
                let (x, y) = (bx + i as u32 % 4, by + i as u32 / 4);
                if x < w && y < h {
                    let o = ((y * w + x) * 4) as usize;
                    out[o..o + 4].copy_from_slice(t);
                }
            }
        }
    }
    out
}
