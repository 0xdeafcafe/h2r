//! A model shader's reflection mask baked into its base map's alpha.
//!
//! Halo 1's model shader (`soso`) masks its reflection with the red
//! channel of its multipurpose map (on the Xbox disc, `docs/halo1/
//! multiplayer.md`). Halo 2's `tex_bump_env` has no map of its own for
//! that: the reflection is masked by the colour pass's alpha, the base
//! map's times the detail's (`h1convert::shad::TEX_BUMP_ENV`). So the
//! converted base is a copy of Halo 1's with the multipurpose map's red as
//! its alpha ([`red_into_alpha`]): DXT5, the base's colour blocks
//! re-encoded ([`colour_block`]) beside an alpha block of the mask
//! ([`alpha_block`]), every mip level the base has. DXT5 keeps the base's
//! size per texel twice that of DXT1 and is a format Halo 2's decoder and
//! retail bitmaps use (format 16).

use assets::Level;
use h1map::bitmap::{
    FLAG_COMPRESSED, FLAG_LINEAR, FLAG_PALETTIZED, FLAG_SWIZZLED, Image, ImageKind, PixelFormat,
};

use crate::decode::{decode_image, texture_desc};
use crate::{Error, Result};

/// The bitmap's format word (`+0x02`, Halo 1's and Halo 2's) for DXT5:
/// "compressed with interpolated alpha" (S: the third of the import's
/// choices, after colour-key and explicit alpha; the engine does not read
/// it to draw).
pub const INTERPOLATED_ALPHA: u16 = 2;

/// The base image `base` (its stored pixels `base_px`) as DXT5 whose
/// alpha is the red channel of `mask` (pixels `mask_px`), sampled at the
/// base's size: the mask's level of the same size where it has one, else
/// its largest level, nearest texel. Every level of the base's chain is
/// written; the colours are the base's (exact where a DXT1 block's
/// endpoints are its extremes, [`colour_block`]). 2D images only.
/// Deterministic: the same inputs give the same bytes.
pub fn red_into_alpha(
    base: &Image,
    base_px: &[u8],
    mask: &Image,
    mask_px: &[u8],
) -> Result<(Image, Vec<u8>)> {
    if base.kind != ImageKind::D2 || mask.kind != ImageKind::D2 {
        return Err(Error::Unsupported(format!(
            "a reflection mask of type {:?} on a base of type {:?}",
            mask.kind, base.kind
        )));
    }
    let colours = decode_image(base, base_px)?;
    let masks = decode_image(mask, mask_px)?;
    let mut out_img = base.clone();
    out_img.format = PixelFormat::Dxt5;
    out_img.flags =
        (base.flags & !(FLAG_PALETTIZED | FLAG_SWIZZLED | FLAG_LINEAR)) | FLAG_COMPRESSED;
    out_img.depth = 1;
    let layout = texture_desc(&out_img)?.layout()?;
    let mut out = vec![0u8; layout.total_size];
    for l in &layout.levels {
        let src = colours
            .iter()
            .find(|v| v.face == 0 && v.level == l.level)
            .ok_or_else(|| Error::Invalid(format!("the base has no level {}", l.level)))?;
        let red = mask_red(&masks, src.width, src.height)?;
        let blocks = encode_dxt5(src.width, src.height, &src.rgba, &red);
        let n = blocks.len().min(l.size);
        out[l.offset..l.offset + n].copy_from_slice(&blocks[..n]);
    }
    out_img.pixel_size = out.len() as u32;
    Ok((out_img, out))
}

/// The red channel of `masks` (face 0) at `w` x `h`, row-major.
fn mask_red(masks: &[Level], w: u32, h: u32) -> Result<Vec<u8>> {
    let face0 = || masks.iter().filter(|m| m.face == 0);
    let m = face0()
        .find(|m| m.width == w && m.height == h)
        .or_else(|| face0().min_by_key(|m| m.level))
        .ok_or_else(|| Error::Invalid("a mask with no levels".into()))?;
    let mut red = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let (mx, my) = (x * m.width / w, y * m.height / h);
            red.push(m.rgba[((my * m.width + mx) * 4) as usize]);
        }
    }
    Ok(red)
}

/// DXT5 blocks of a `w` x `h` level: `rgba` its colours (RGBA8 rows),
/// `alpha` one byte per texel. A block past the level's edge repeats the
/// edge texels.
pub fn encode_dxt5(w: u32, h: u32, rgba: &[u8], alpha: &[u8]) -> Vec<u8> {
    let (bw, bh) = (w.div_ceil(4).max(1), h.div_ceil(4).max(1));
    let mut out = Vec::with_capacity((bw * bh * 16) as usize);
    for by in 0..bh {
        for bx in 0..bw {
            let mut c = [[0u8; 3]; 16];
            let mut a = [0u8; 16];
            for j in 0..4 {
                for i in 0..4 {
                    let x = (bx * 4 + i).min(w - 1);
                    let y = (by * 4 + j).min(h - 1);
                    let t = (y * w + x) as usize;
                    let k = (j * 4 + i) as usize;
                    c[k] = [rgba[t * 4], rgba[t * 4 + 1], rgba[t * 4 + 2]];
                    a[k] = alpha[t];
                }
            }
            out.extend_from_slice(&alpha_block(&a));
            out.extend_from_slice(&colour_block(&c));
        }
    }
    out
}

/// A DXT5 alpha block of 16 texels (row-major): the extremes as the
/// endpoints, the larger first (the 8-value mode: 6 steps between), each
/// texel the nearest of the 8 (ties to the lower index); one value is
/// exact.
pub fn alpha_block(a: &[u8; 16]) -> [u8; 8] {
    let (hi, lo) = (*a.iter().max().unwrap(), *a.iter().min().unwrap());
    let mut out = [0u8; 8];
    out[0] = hi;
    out[1] = lo;
    if hi == lo {
        return out;
    }
    let (h, l) = (u32::from(hi), u32::from(lo));
    let mut palette = [h, l, 0, 0, 0, 0, 0, 0];
    for (i, p) in palette.iter_mut().enumerate().skip(2) {
        let i = i as u32;
        *p = ((8 - i) * h + (i - 1) * l) / 7;
    }
    let mut bits = 0u64;
    for (k, &v) in a.iter().enumerate() {
        let v = u32::from(v);
        let idx = (0..8).min_by_key(|&i| palette[i].abs_diff(v)).unwrap() as u64;
        bits |= idx << (3 * k);
    }
    out[2..8].copy_from_slice(&bits.to_le_bytes()[..6]);
    out
}

/// R5G6B5 of an RGB8 colour.
fn to_565([r, g, b]: [u8; 3]) -> u16 {
    ((u16::from(r) >> 3) << 11) | ((u16::from(g) >> 2) << 5) | (u16::from(b) >> 3)
}

/// RGB8 of an R5G6B5 colour, as the decoder expands it.
fn from_565(v: u16) -> [u32; 3] {
    let (r, g, b) = (u32::from(v >> 11), u32::from((v >> 5) & 0x3f), u32::from(v & 0x1f));
    [(r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2)]
}

fn distance(a: [u32; 3], b: [u32; 3]) -> u32 {
    (0..3).map(|i| a[i].abs_diff(b[i]).pow(2)).sum()
}

/// A DXT colour block (the four-colour mode DXT5 always uses) of 16
/// texels (row-major): the endpoints are the two texels farthest apart
/// (in R5G6B5; the first such pair), the larger first, and each texel
/// the nearest of the four colours (ties to the lower index). A block
/// decoded from DXT1's four-colour mode whose endpoints both appear in it
/// comes back as it was.
pub fn colour_block(c: &[[u8; 3]; 16]) -> [u8; 8] {
    let q: Vec<u16> = c.iter().map(|&t| to_565(t)).collect();
    let (mut e0, mut e1, mut far) = (q[0], q[0], 0);
    for (i, &a) in q.iter().enumerate() {
        for &b in &q[i + 1..] {
            let d = distance(from_565(a), from_565(b));
            if d > far {
                (e0, e1, far) = (a, b, d);
            }
        }
    }
    let (e0, e1) = if e0 >= e1 { (e0, e1) } else { (e1, e0) };
    let (p0, p1) = (from_565(e0), from_565(e1));
    let mix = |a: u32, b: u32| -> [u32; 3] { std::array::from_fn(|i| (a * p0[i] + b * p1[i]) / 3) };
    let palette = [p0, p1, mix(2, 1), mix(1, 2)];
    let mut out = [0u8; 8];
    out[0..2].copy_from_slice(&e0.to_le_bytes());
    out[2..4].copy_from_slice(&e1.to_le_bytes());
    if e0 == e1 {
        return out;
    }
    let mut bits = 0u32;
    for (k, &t) in c.iter().enumerate() {
        let t = [u32::from(t[0]), u32::from(t[1]), u32::from(t[2])];
        let idx = (0..4).min_by_key(|&i| distance(palette[i], t)).unwrap() as u32;
        bits |= idx << (2 * k);
    }
    out[4..8].copy_from_slice(&bits.to_le_bytes());
    out
}
