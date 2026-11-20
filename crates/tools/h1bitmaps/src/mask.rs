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

/// The image `img` (its stored pixels `px`) as DXT5 whose alpha is its
/// own brightest channel, `max(r, g, b)`, times `gain` (saturated at 1):
/// Halo 1's plasma `sotr` adds its picture's colour times the band, and
/// Halo 2's `plasma_alpha` shapes and dims only through its mask's alpha
/// (`h1convert::shad::plasma_shader`). Every level of the chain is
/// written; the colours are the image's (as [`red_into_alpha`] keeps
/// them). 2D images only. Deterministic.
pub fn brightest_into_alpha(img: &Image, px: &[u8], gain: f32) -> Result<(Image, Vec<u8>)> {
    if img.kind != ImageKind::D2 {
        return Err(Error::Unsupported(format!(
            "a brightness mask of type {:?}",
            img.kind
        )));
    }
    let colours = decode_image(img, px)?;
    let mut out_img = img.clone();
    out_img.format = PixelFormat::Dxt5;
    out_img.flags =
        (img.flags & !(FLAG_PALETTIZED | FLAG_SWIZZLED | FLAG_LINEAR)) | FLAG_COMPRESSED;
    out_img.depth = 1;
    let layout = texture_desc(&out_img)?.layout()?;
    let mut out = vec![0u8; layout.total_size];
    for l in &layout.levels {
        let src = colours
            .iter()
            .find(|v| v.face == 0 && v.level == l.level)
            .ok_or_else(|| Error::Invalid(format!("the image has no level {}", l.level)))?;
        let alpha: Vec<u8> = src
            .rgba
            .chunks_exact(4)
            .map(|t| brightest(t[0], t[1], t[2], gain))
            .collect();
        let blocks = encode_dxt5(src.width, src.height, &src.rgba, &alpha);
        let n = blocks.len().min(l.size);
        out[l.offset..l.offset + n].copy_from_slice(&blocks[..n]);
    }
    out_img.pixel_size = out.len() as u32;
    Ok((out_img, out))
}

/// `gain × max(r, g, b)`, rounded, saturated at 255.
pub fn brightest(r: u8, g: u8, b: u8, gain: f32) -> u8 {
    (f32::from(r.max(g).max(b)) * gain)
        .round()
        .clamp(0.0, 255.0) as u8
}

/// The Halo 1 bitmap `tag` with [`brightest_into_alpha`]'s alpha on
/// every image, its sequences kept, converted (DXT5, format word
/// [`INTERPOLATED_ALPHA`]).
pub fn convert_brightest(
    map: &h1map::H1Map,
    tag: &h1map::Tag,
    gain: f32,
) -> Result<h1models::Converted> {
    let bm = h1map::bitmap::Bitmap::read(map, tag)?;
    bake_each(map, bm, |i, img, px| brightest_into_alpha(img, px, gain).map_err(|e| at_image(i, e)))
}

/// A Halo 1 model shader's base map `base` with the red of its
/// multipurpose map `mask` as alpha on every image ([`red_into_alpha`]),
/// its sequences kept, converted (DXT5, format word
/// [`INTERPOLATED_ALPHA`]). Image `i` of the base takes image `i` of a
/// mask with as many images, or the only image of a single-image mask:
/// Halo 1's multi-image model maps come in such pairs (D: b30's `light
/// fixtures` and `light fixtures multi`, two images each, 256 and 128
/// square; the grunt's torso, arms and legs, and head and back with their
/// `multi` maps, two 256 square images each). Any other pairing is an
/// error.
pub fn convert_reflection_mask(
    map: &h1map::H1Map,
    base: &h1map::Tag,
    mask: &h1map::Tag,
) -> Result<h1models::Converted> {
    let bm = h1map::bitmap::Bitmap::read(map, base)?;
    let mm = h1map::bitmap::Bitmap::read(map, mask)?;
    let pick = paired_image(bm.images.len(), mm.images.len())?;
    let masks = (0..mm.images.len())
        .map(|k| mm.pixels(map, k))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    bake_each(map, bm, |i, img, px| {
        let k = pick(i);
        red_into_alpha(img, px, &mm.images[k], masks[k]).map_err(|e| at_image(i, e))
    })
}

/// The Halo 1 multipurpose map `mask` as a Halo 2 change-colour map
/// ([`channel_into`], channel `from` into `to`) on every image, its
/// sequences kept, converted (DXT5, format word [`INTERPOLATED_ALPHA`]).
pub fn convert_change_colour(
    map: &h1map::H1Map,
    mask: &h1map::Tag,
    from: usize,
    to: usize,
) -> Result<h1models::Converted> {
    let mm = h1map::bitmap::Bitmap::read(map, mask)?;
    bake_each(map, mm, |i, img, px| channel_into(img, px, from, to).map_err(|e| at_image(i, e)))
}

/// Which mask image each of `base` images takes: the same index when the
/// counts agree, else the mask's only image.
pub(crate) fn paired_image(base: usize, mask: usize) -> Result<impl Fn(usize) -> usize> {
    if base == 0 || mask == 0 || (mask != base && mask != 1) {
        return Err(Error::Unsupported(format!(
            "a mask of {mask} images on a base of {base}"
        )));
    }
    Ok(move |i| if mask == base { i } else { 0 })
}

fn at_image(i: usize, e: Error) -> Error {
    Error::Invalid(format!("image {i}: {e}"))
}

/// `bm` with each image `i` (and its stored pixels) replaced by what
/// `bake` makes of it, converted with Halo 1's format word for DXT5.
fn bake_each(
    map: &h1map::H1Map,
    mut bm: h1map::bitmap::Bitmap,
    mut bake: impl FnMut(usize, &Image, &[u8]) -> Result<(Image, Vec<u8>)>,
) -> Result<h1models::Converted> {
    if bm.images.is_empty() {
        return Err(Error::Invalid("a bitmap with no images".into()));
    }
    let mut pixels = Vec::with_capacity(bm.images.len());
    for i in 0..bm.images.len() {
        let (image, bytes) = bake(i, &bm.images[i], bm.pixels(map, i)?)?;
        bm.images[i] = image;
        pixels.push(bytes);
    }
    bm.encoding = INTERPOLATED_ALPHA;
    let (converted, _) = crate::convert::convert_with(&bm, |i| Ok(std::mem::take(&mut pixels[i])))?;
    Ok(converted)
}

/// A Halo 2 change-colour map from one channel of a Halo 1 model
/// shader's multipurpose map: DXT5 of `mask`'s size and mip chain, whose
/// channel `to` (0 red .. 3 alpha) is `mask`'s channel `from` and every
/// other channel 0. Halo 2's change-colour stems tint by each channel of
/// this map in turn (red the object's change colour 0, green 1, blue 2,
/// alpha 3; `h1convert::shad::TEX_BUMP_ENV_FOUR_CHANGE_COLOR`), so a
/// channel left 0 tints nothing. 2D images only; deterministic.
pub fn channel_into(
    mask: &Image,
    mask_px: &[u8],
    from: usize,
    to: usize,
) -> Result<(Image, Vec<u8>)> {
    if mask.kind != ImageKind::D2 || from > 3 || to > 3 {
        return Err(Error::Unsupported(format!(
            "a change-colour mask of type {:?}, channel {from} into {to}",
            mask.kind
        )));
    }
    let levels = decode_image(mask, mask_px)?;
    let mut out_img = mask.clone();
    out_img.format = PixelFormat::Dxt5;
    out_img.flags =
        (mask.flags & !(FLAG_PALETTIZED | FLAG_SWIZZLED | FLAG_LINEAR)) | FLAG_COMPRESSED;
    out_img.depth = 1;
    let layout = texture_desc(&out_img)?.layout()?;
    let mut out = vec![0u8; layout.total_size];
    for l in &layout.levels {
        let src = levels
            .iter()
            .find(|v| v.face == 0 && v.level == l.level)
            .ok_or_else(|| Error::Invalid(format!("the mask has no level {}", l.level)))?;
        let n = (src.width * src.height) as usize;
        let mut rgba = vec![0u8; n * 4];
        let mut alpha = vec![0u8; n];
        for t in 0..n {
            let v = src.rgba[t * 4 + from];
            if to == 3 {
                alpha[t] = v;
            } else {
                rgba[t * 4 + to] = v;
            }
        }
        let blocks = encode_dxt5(src.width, src.height, &rgba, &alpha);
        let n = blocks.len().min(l.size);
        out[l.offset..l.offset + n].copy_from_slice(&blocks[..n]);
    }
    out_img.pixel_size = out.len() as u32;
    Ok((out_img, out))
}

/// The base alpha at and above which an alpha-tested surface is kept
/// ([`cutout_bump`]): Halo 1 tests its model shaders' base at a half.
/// Retail Halo 2's alpha-tested bump maps agree (V, coagulation's
/// `tree_cliff_leaf_bump`: its cut-out index covers its leaf's texels
/// with alpha below 128 and almost none above).
pub const CUTOUT_THRESHOLD: u8 = 128;

/// A DXT colour block every texel of which is the flat tangent-space
/// normal, `(129, 128, 255)`: endpoints `0x841f` (132, 130, 255) and
/// `0x7bff` (123, 125, 255), the larger first (four-colour mode), every
/// texel index 2, two thirds of the first and one of the second. Within
/// one step of `default_vector`'s `(128, 128, 255)`, which no single
/// R5G6B5 endpoint is (128 lies between 5-bit red's 123 and 132).
pub const FLAT_NORMAL_BLOCK: [u8; 8] = [0x1f, 0x84, 0xff, 0x7b, 0xaa, 0xaa, 0xaa, 0xaa];

/// A bump map for Halo 2's alpha-tested model stem made from the base
/// image `base` (its stored pixels `base_px`): DXT5 of the base's size
/// and chain, every texel the flat normal ([`FLAT_NORMAL_BLOCK`]), its
/// alpha 0 where the base's level is below [`CUTOUT_THRESHOLD`] and 255
/// elsewhere.
///
/// `opaque\tex_bump_alpha_test_single_pass` cuts its depth and shadow
/// passes by its bump map's alpha, not the base's: its layer 10
/// (`shadow\shadow_generate`, whose depth-only implementations lay down
/// the depth the later passes test equal against) samples bitmap 0, the
/// bump, at stage 0 with `D3DTSS_ALPHAKILL` on, while the colour pass
/// (layer 3) alpha-tests the base. Retail's alpha-tested bump maps are
/// P8 bump with palette index 255 exactly where their colour map is clear
/// (V, coagulation's `bush_vine_bmp` and `tree_cliff_leaf_bump`). With
/// `default_vector` there (opaque), a Halo 1 leaf card's depth covered
/// the whole card and the cut-out parts drew as the dark colour behind
/// it. A binary alpha keeps the depth cut where the colour pass cuts.
/// 2D images only; deterministic.
pub fn cutout_bump(base: &Image, base_px: &[u8]) -> Result<(Image, Vec<u8>)> {
    if base.kind != ImageKind::D2 {
        return Err(Error::Unsupported(format!(
            "a cut-out bump of a base of type {:?}",
            base.kind
        )));
    }
    let levels = decode_image(base, base_px)?;
    let mut out_img = base.clone();
    out_img.format = PixelFormat::Dxt5;
    out_img.flags =
        (base.flags & !(FLAG_PALETTIZED | FLAG_SWIZZLED | FLAG_LINEAR)) | FLAG_COMPRESSED;
    out_img.depth = 1;
    let layout = texture_desc(&out_img)?.layout()?;
    let mut out = vec![0u8; layout.total_size];
    for l in &layout.levels {
        let src = levels
            .iter()
            .find(|v| v.face == 0 && v.level == l.level)
            .ok_or_else(|| Error::Invalid(format!("the base has no level {}", l.level)))?;
        let alpha: Vec<u8> = src
            .rgba
            .chunks_exact(4)
            .map(|t| if t[3] >= CUTOUT_THRESHOLD { 255 } else { 0 })
            .collect();
        let blocks = encode_alpha_dxt5(src.width, src.height, &alpha, FLAT_NORMAL_BLOCK);
        let n = blocks.len().min(l.size);
        out[l.offset..l.offset + n].copy_from_slice(&blocks[..n]);
    }
    out_img.pixel_size = out.len() as u32;
    Ok((out_img, out))
}

/// A flat bump with the same cutout and image sequences as the source bitmap.
pub fn convert_cutout_bump(map: &h1map::H1Map, tag: &h1map::Tag) -> Result<h1models::Converted> {
    let mut baked = h1map::bitmap::Bitmap::read(map, tag)?;
    if baked.images.is_empty() {
        return Err(Error::Invalid("empty cutout bitmap".into()));
    }
    let mut pixels = Vec::with_capacity(baked.images.len());
    for i in 0..baked.images.len() {
        let (image, bytes) = cutout_bump(&baked.images[i], baked.pixels(map, i)?)?;
        baked.images[i] = image;
        pixels.push(bytes);
    }
    baked.encoding = INTERPOLATED_ALPHA;
    let (converted, _) = crate::convert::convert_with(&baked, |i| Ok(std::mem::take(&mut pixels[i])))?;
    Ok(converted)
}

/// DXT5 blocks of a `w` x `h` level whose colour half is `colour` in
/// every block and whose alpha is `alpha` (one byte per texel). A block
/// past the level's edge repeats the edge texels.
fn encode_alpha_dxt5(w: u32, h: u32, alpha: &[u8], colour: [u8; 8]) -> Vec<u8> {
    let (bw, bh) = (w.div_ceil(4).max(1), h.div_ceil(4).max(1));
    let mut out = Vec::with_capacity((bw * bh * 16) as usize);
    for by in 0..bh {
        for bx in 0..bw {
            let a: [u8; 16] = std::array::from_fn(|k| {
                let (i, j) = (k as u32 % 4, k as u32 / 4);
                let x = (bx * 4 + i).min(w - 1);
                let y = (by * 4 + j).min(h - 1);
                alpha[(y * w + x) as usize]
            });
            out.extend_from_slice(&alpha_block(&a));
            out.extend_from_slice(&colour);
        }
    }
    out
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
