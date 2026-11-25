//! RGBA8 pixels back into a bitmap's stored bytes: the inverse of
//! [`crate::bitmap::decode_image`] for the formats the Textures tool edits
//! (docs/editor/modeling.md, "Texture editing").
//!
//! # What is written
//!
//! * **Layout** is the decoder's own ([`TextureDesc::layout`]): a swizzled
//!   level is Morton ordered ([`gfx::swizzle`]), a linear one has rows padded
//!   to the descriptor's pitch, a DXT one is blocks in row-major order, and
//!   cube faces sit one face stride (`TextureLayout::face_stride`) apart. A LOD's bytes
//!   are its layout rounded up to 128 ([`LOD_ALIGN`]), as on the disc.
//! * **Texels** invert `gfx::convert`: each channel is quantised with
//!   rounding, so `decode(encode(decode(x))) == decode(x)` exactly for the
//!   uncompressed formats. `L8` and `AY8` take the Rec. 601 luma of the
//!   pixel (a grey pixel gives its own value back); `A8` takes alpha.
//! * **DXT blocks** are range fitted: the colour endpoints are the inset
//!   bounding box of the block's colours along the diagonal their
//!   covariance picks, quantised to 5:6:5, and each texel takes the nearest
//!   of the four palette colours the decoder (`gfx::dxt`) builds. DXT1
//!   blocks with a texel under half alpha use the three colour mode with a
//!   transparent index. DXT3 alpha is 4 bits, DXT5 alpha is the 8-value or
//!   6-value interpolated set. The result is lossy within the format's
//!   usual error.
//! * **Mips** are box filtered from mip 0 straight into each level (every
//!   level texel is the mean of its `2^k x 2^k` source box, per channel,
//!   in the stored encoding, depth included for volumes). The same filter
//!   serves a whole chain and a single dirty region, so a region written
//!   alone equals the same region of a whole-chain write.
//!
//! # Tiles
//!
//! [`encode_tiles`] writes only the units (texels, or 4x4 blocks) a set of
//! dirty rectangles touches, in every LOD chain the image stores and in
//! every level of each, and returns them as byte [`Span`]s. A rectangle at
//! mip 0 maps to the rectangle of texels that read it at each mip; a block
//! is re-encoded whole from the pixels it covers. Nothing else in the
//! stored bytes changes, which is what lets an edit be written in place
//! (`MapEditor::patch_raw`) or sent live (docs/editor/realtime-edits.md)
//! with a few bytes.

use std::collections::BTreeMap;

use gfx::D3DFormat;
use gfx::dxt::BcKind;
use gfx::format::{Layout, TexelFormat};
use gfx::swizzle::Swizzler;
use gfx::texture::{LevelLayout, TextureDesc, TextureKind};
use tags::Image;

use crate::bitmap::{LOD_COUNT, invalid, texture_desc};
use crate::{Error, Result};

/// A LOD's byte length is its layout rounded up to this many bytes.
pub const LOD_ALIGN: usize = 128;

/// A rectangle of pixels: top left and size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl PixelRect {
    pub const fn new(x: u32, y: u32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    pub const fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }

    /// The part of `self` inside a `w` x `h` image.
    fn clip(self, w: u32, h: u32) -> PixelRect {
        let x = self.x.min(w);
        let y = self.y.min(h);
        let r = self.x.saturating_add(self.w).min(w);
        let b = self.y.saturating_add(self.h).min(h);
        PixelRect {
            x,
            y,
            w: r - x,
            h: b - y,
        }
    }
}

/// The mip 0 pixels of an image: row-major `R,G,B,A`, one plane per cube
/// face (`depth` is 1) or per volume slice (one face), so a plane is
/// `planes[face * depth + slice]`. A 2D image has the one plane.
#[derive(Clone, Copy, Debug)]
pub struct Planes<'a> {
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub planes: &'a [&'a [u8]],
}

impl Planes<'_> {
    /// Checks the planes against an image's size and kind.
    fn check(&self, desc0: &TextureDesc) -> Result<()> {
        let depth = if desc0.kind == TextureKind::Volume {
            desc0.depth
        } else {
            1
        };
        if (self.width, self.height, self.depth) != (desc0.width, desc0.height, depth) {
            return Err(invalid(format!(
                "pixels are {}x{}x{}, the image is {}x{}x{}",
                self.width, self.height, self.depth, desc0.width, desc0.height, depth
            )));
        }
        let want = (desc0.faces() * depth) as usize;
        if self.planes.len() != want {
            return Err(invalid(format!(
                "{} planes of pixels, the image needs {want}",
                self.planes.len()
            )));
        }
        let bytes = self.width as usize * self.height as usize * 4;
        if self.planes.iter().any(|p| p.len() != bytes) {
            return Err(invalid("a plane of pixels is not width x height x 4 bytes"));
        }
        Ok(())
    }
}

/// A run of stored bytes to write: `offset` into LOD `lod`'s bytes (the
/// face stride of a cube face included).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub lod: u32,
    pub offset: usize,
    pub bytes: Vec<u8>,
}

/// A changed region: a cube face or volume slice (0 otherwise) and the
/// mip 0 pixels in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dirty {
    pub face: u32,
    pub slice: u32,
    pub rect: PixelRect,
}

/// Whether [`encode_lod`] and [`encode_tiles`] write `format`: the colour
/// formats of `FORMAT_TABLE` with known channels. The data formats (P8,
/// bump, V8U8, G8B8) and the float formats are not pictures.
pub fn writable(format: tags::PixelFormat) -> bool {
    use tags::PixelFormat as P;
    matches!(
        format,
        P::A8
            | P::Y8
            | P::Ay8
            | P::A8Y8
            | P::Rgb565
            | P::Argb1555
            | P::Argb4444
            | P::Xrgb8888
            | P::Argb8888
            | P::Dxt1
            | P::Dxt3
            | P::Dxt5
    )
}

fn luma(p: [u8; 4]) -> u8 {
    ((77 * u32::from(p[0]) + 150 * u32::from(p[1]) + 29 * u32::from(p[2]) + 128) >> 8) as u8
}

/// `v` of 255 as a `max`-valued field, rounded.
fn quant(v: u8, max: u32) -> u16 {
    ((u32::from(v) * max + 127) / 255) as u16
}

/// One texel as stored (`out` is the format's bytes per texel long).
fn put_texel(texel: TexelFormat, p: [u8; 4], out: &mut [u8]) {
    use TexelFormat::*;
    let word = |out: &mut [u8], v: u16| out.copy_from_slice(&v.to_le_bytes());
    match texel {
        L8 | Al8 => out[0] = luma(p),
        A8 => out[0] = p[3],
        A8L8 => out.copy_from_slice(&[luma(p), p[3]]),
        A1R5G5B5 => word(
            out,
            u16::from(p[3] >= 128) << 15
                | quant(p[0], 31) << 10
                | quant(p[1], 31) << 5
                | quant(p[2], 31),
        ),
        A4R4G4B4 => word(
            out,
            quant(p[3], 15) << 12 | quant(p[0], 15) << 8 | quant(p[1], 15) << 4 | quant(p[2], 15),
        ),
        R5G6B5 => word(
            out,
            quant(p[0], 31) << 11 | quant(p[1], 63) << 5 | quant(p[2], 31),
        ),
        A8R8G8B8 => out.copy_from_slice(&[p[2], p[1], p[0], p[3]]),
        X8R8G8B8 => out.copy_from_slice(&[p[2], p[1], p[0], 255]),
        // `writable` keeps every other format out.
        _ => out.fill(0),
    }
}

/// The error for a format this module does not write.
fn refuse(format: D3DFormat) -> Error {
    Error::UnsupportedFormat(format!("{format:?} (not written)"))
}

fn check_format(image: &Image) -> Result<()> {
    use TexelFormat::*;
    let format = d3d_of(image)?;
    if !writable(image.format) {
        return Err(refuse(format));
    }
    match format.texel() {
        L8 | Al8 | A8 | A8L8 | A1R5G5B5 | A4R4G4B4 | R5G6B5 | A8R8G8B8 | X8R8G8B8 | Dxt1 | Dxt3
        | Dxt5 => Ok(()),
        _ => Err(refuse(format)),
    }
}

// ---- mips ---------------------------------------------------------------

/// Level `k`'s pixels over `r` (inside the level), box filtered from mip
/// 0: row-major `R,G,B,A`. `z` is the level's slice (0 for a non-volume);
/// `face` the cube face.
fn box_region(src: &Planes, face: u32, k: u32, z: u32, r: PixelRect) -> Vec<u8> {
    let (sw, sh, sd) = (src.width as usize, src.height as usize, src.depth as usize);
    let k = k.min(31);
    let z0 = (z as usize) << k;
    let z1 = (((z as usize) + 1) << k).min(sd).max(z0 + 1);
    let mut out = Vec::with_capacity(r.w as usize * r.h as usize * 4);
    for y in r.y..r.y + r.h {
        let y0 = (y as usize) << k;
        let y1 = (((y as usize) + 1) << k).min(sh).max(y0 + 1);
        for x in r.x..r.x + r.w {
            let x0 = (x as usize) << k;
            let x1 = (((x as usize) + 1) << k).min(sw).max(x0 + 1);
            if k == 0 {
                let plane = src.planes[face as usize * sd + z0];
                out.extend_from_slice(&plane[(y0 * sw + x0) * 4..][..4]);
                continue;
            }
            let mut sum = [0u32; 4];
            let mut n = 0u32;
            for zi in z0..z1 {
                let plane = src.planes[face as usize * sd + zi];
                for yy in y0..y1 {
                    for p in plane[(yy * sw + x0) * 4..(yy * sw + x1) * 4].chunks_exact(4) {
                        for c in 0..4 {
                            sum[c] += u32::from(p[c]);
                        }
                        n += 1;
                    }
                }
            }
            out.extend(sum.map(|s| ((s + n / 2) / n) as u8));
        }
    }
    out
}

// ---- DXT ----------------------------------------------------------------

fn expand565(c: u16) -> [u8; 3] {
    let r = (c >> 11) as u8 & 0x1f;
    let g = (c >> 5) as u8 & 0x3f;
    let b = c as u8 & 0x1f;
    [
        (r << 3) | (r >> 2),
        (g << 2) | (g >> 4),
        (b << 3) | (b >> 2),
    ]
}

fn pack565(c: [u8; 3]) -> u16 {
    quant(c[0], 31) << 11 | quant(c[1], 63) << 5 | quant(c[2], 31)
}

fn dist(a: [u8; 3], b: [u8; 3]) -> u32 {
    (0..3)
        .map(|i| (i32::from(a[i]) - i32::from(b[i])).pow(2) as u32)
        .sum()
}

/// The 8 bytes of colour: two 5:6:5 endpoints and 2 bits a texel.
/// `punch` is DXT1's: texels under half alpha become the transparent index
/// of the three colour mode.
fn color_block(px: &[[u8; 4]; 16], punch: bool) -> [u8; 8] {
    let clear = |p: &[u8; 4]| punch && p[3] < 128;
    let any_clear = px.iter().any(clear);
    let opaque: Vec<[u8; 3]> = px
        .iter()
        .filter(|p| !clear(p))
        .map(|p| [p[0], p[1], p[2]])
        .collect();
    let (mut lo, mut hi) = ([255u8; 3], [0u8; 3]);
    for c in &opaque {
        for i in 0..3 {
            lo[i] = lo[i].min(c[i]);
            hi[i] = hi[i].max(c[i]);
        }
    }
    if opaque.is_empty() {
        (lo, hi) = ([0; 3], [0; 3]);
    }
    // The covariance of red and green with blue says which diagonal of the
    // box the colours lie along.
    let n = opaque.len().max(1) as i32;
    let mean = |i: usize| opaque.iter().map(|c| i32::from(c[i])).sum::<i32>() / n;
    let m = [mean(0), mean(1), mean(2)];
    let cov = |i: usize| {
        opaque
            .iter()
            .map(|c| (i32::from(c[i]) - m[i]) * (i32::from(c[2]) - m[2]))
            .sum::<i32>()
    };
    for i in 0..2 {
        if cov(i) < 0 {
            std::mem::swap(&mut lo[i], &mut hi[i]);
        }
    }
    // Inset the box: the extremes are rare, the palette's ends are not.
    let inset = |a: u8, b: u8| -> (u8, u8) {
        let d = (i32::from(a) - i32::from(b)) / 16;
        (
            (i32::from(a) - d).clamp(0, 255) as u8,
            (i32::from(b) + d).clamp(0, 255) as u8,
        )
    };
    let (mut e0, mut e1) = ([0u8; 3], [0u8; 3]);
    for i in 0..3 {
        // `hi` is the end the diagonal pairs with the larger blue.
        (e0[i], e1[i]) = inset(hi[i], lo[i]);
    }
    let (mut c0, mut c1) = (pack565(e0), pack565(e1));
    // Four colour mode needs c0 > c1, three colour mode c0 <= c1.
    let three = any_clear;
    if (c0 > c1) == three {
        std::mem::swap(&mut c0, &mut c1);
    }
    let (p0, p1) = (expand565(c0), expand565(c1));
    let mix = |a: u8, b: u8, wa: u32, wb: u32, div: u32| {
        ((u32::from(a) * wa + u32::from(b) * wb) / div) as u8
    };
    let mut pal = [p0, p1, [0; 3], [0; 3]];
    let mut used = 4;
    if three {
        used = 3;
        for i in 0..3 {
            pal[2][i] = mix(p0[i], p1[i], 1, 1, 2);
        }
    } else {
        for i in 0..3 {
            pal[2][i] = mix(p0[i], p1[i], 2, 1, 3);
            pal[3][i] = mix(p0[i], p1[i], 1, 2, 3);
        }
    }
    let mut idx = 0u32;
    for (i, p) in px.iter().enumerate() {
        let best = if clear(p) {
            3
        } else {
            let c = [p[0], p[1], p[2]];
            let mut best = 0;
            for j in 1..used {
                if dist(c, pal[j]) < dist(c, pal[best]) {
                    best = j;
                }
            }
            best
        };
        idx |= (best as u32) << (2 * i);
    }
    let mut out = [0u8; 8];
    out[0..2].copy_from_slice(&c0.to_le_bytes());
    out[2..4].copy_from_slice(&c1.to_le_bytes());
    out[4..8].copy_from_slice(&idx.to_le_bytes());
    out
}

/// The 8 bytes of DXT5 alpha: two endpoints and 3 bits a texel.
fn alpha_block(px: &[[u8; 4]; 16]) -> [u8; 8] {
    let a0 = px.iter().map(|p| p[3]).max().unwrap_or(0);
    let a1 = px.iter().map(|p| p[3]).min().unwrap_or(0);
    // a0 >= a1; equal endpoints select the 6-value mode, whose index 0 is
    // still a0.
    let mut pal = [0u8; 8];
    pal[0] = a0;
    pal[1] = a1;
    if a0 > a1 {
        for i in 1..7u32 {
            pal[1 + i as usize] = (((7 - i) * u32::from(a0) + i * u32::from(a1)) / 7) as u8;
        }
    }
    let used = if a0 > a1 { 8 } else { 2 };
    let mut bits = 0u64;
    for (i, p) in px.iter().enumerate() {
        let mut best = 0;
        for j in 1..used {
            if i32::from(p[3]).abs_diff(i32::from(pal[j]))
                < i32::from(p[3]).abs_diff(i32::from(pal[best]))
            {
                best = j;
            }
        }
        bits |= (best as u64) << (3 * i);
    }
    let mut out = [0u8; 8];
    out[0] = a0;
    out[1] = a1;
    out[2..8].copy_from_slice(&bits.to_le_bytes()[..6]);
    out
}

/// One 4x4 block of `kind` (8 or 16 bytes) from 16 texels, row-major.
pub fn encode_block(kind: BcKind, px: &[[u8; 4]; 16]) -> Vec<u8> {
    match kind {
        BcKind::Bc1 => color_block(px, true).to_vec(),
        BcKind::Bc2 => {
            let mut out = vec![0u8; 16];
            for (i, p) in px.iter().enumerate() {
                let nib = quant(p[3], 15) as u8;
                out[i / 2] |= if i % 2 == 0 { nib } else { nib << 4 };
            }
            out[8..].copy_from_slice(&color_block(px, false));
            out
        }
        BcKind::Bc3 => {
            let mut out = alpha_block(px).to_vec();
            out.extend_from_slice(&color_block(px, false));
            out
        }
    }
}

// ---- levels -------------------------------------------------------------

/// Encodes `region` of level `lv` (absolute mip `mip`, the level's slices
/// `zs`) of cube face `face`, and hands each unit written to `sink` with
/// its byte offset in the face. A DXT region grows to whole blocks.
#[allow(clippy::too_many_arguments)]
fn encode_region(
    desc: &TextureDesc,
    lv: &LevelLayout,
    mip: u32,
    src: &Planes,
    face: u32,
    zs: std::ops::Range<u32>,
    region: PixelRect,
    sink: &mut dyn FnMut(usize, &[u8]),
) -> Result<()> {
    let region = region.clip(lv.width, lv.height);
    if region.is_empty() {
        return Ok(());
    }
    let texel = desc.format.texel();
    match desc.format.layout() {
        Layout::Compressed => {
            let kind = BcKind::from_texel(texel).ok_or_else(|| refuse(desc.format))?;
            let bw = lv.width.div_ceil(4);
            let (bx0, bx1) = (region.x / 4, (region.x + region.w).div_ceil(4));
            let (by0, by1) = (region.y / 4, (region.y + region.h).div_ceil(4));
            let area = PixelRect::new(
                bx0 * 4,
                by0 * 4,
                (bx1 * 4).min(lv.width) - bx0 * 4,
                (by1 * 4).min(lv.height) - by0 * 4,
            );
            for z in zs {
                let pixels = box_region(src, face, mip, z, area);
                let at = |x: u32, y: u32| -> [u8; 4] {
                    let (x, y) = (x.min(area.w - 1), y.min(area.h - 1));
                    let o = (y * area.w + x) as usize * 4;
                    [pixels[o], pixels[o + 1], pixels[o + 2], pixels[o + 3]]
                };
                for by in by0..by1 {
                    for bx in bx0..bx1 {
                        let mut block = [[0u8; 4]; 16];
                        for (i, p) in block.iter_mut().enumerate() {
                            *p = at(
                                (bx - bx0) * 4 + (i as u32 % 4),
                                (by - by0) * 4 + (i as u32 / 4),
                            );
                        }
                        let bytes = encode_block(kind, &block);
                        let slice = gfx::dxt::level_bytes(kind, lv.width, lv.height) * z as usize;
                        sink(
                            lv.offset + slice + ((by * bw + bx) as usize * kind.block_bytes()),
                            &bytes,
                        );
                    }
                }
            }
        }
        layout => {
            let bpt = desc
                .format
                .bytes_per_texel()
                .ok_or_else(|| refuse(desc.format))? as usize;
            let swz = match layout {
                Layout::Swizzled => Some(
                    Swizzler::new(lv.width, lv.height, lv.depth)
                        .map_err(|e| invalid(e.to_string()))?,
                ),
                _ => None,
            };
            let mut unit = [0u8; 4];
            for z in zs {
                let pixels = box_region(src, face, mip, z, region);
                for ry in 0..region.h {
                    for rx in 0..region.w {
                        let o = (ry * region.w + rx) as usize * 4;
                        let p = [pixels[o], pixels[o + 1], pixels[o + 2], pixels[o + 3]];
                        let (x, y) = (region.x + rx, region.y + ry);
                        let at = match &swz {
                            Some(s) => lv.offset + s.index(x, y, z) * bpt,
                            None => lv.offset + y as usize * desc.pitch as usize + x as usize * bpt,
                        };
                        put_texel(texel, p, &mut unit[..bpt]);
                        sink(at, &unit[..bpt]);
                    }
                }
            }
        }
    }
    Ok(())
}

/// The whole of LOD `lod` of `image` as it is stored: every level of every
/// face, in the image's format and layout, padded to [`LOD_ALIGN`]. Mip
/// levels come from `planes` (the image's mip 0, whatever LOD is asked
/// for), so a chain written here reads back through
/// [`crate::bitmap::decode_image`].
pub fn encode_lod(image: &Image, lod: u32, planes: &Planes) -> Result<Vec<u8>> {
    let _scope = mem_watch::scope("assets.encode");
    check_format(image)?;
    planes.check(&texture_desc(image, 0)?)?;
    let desc = texture_desc(image, lod)?;
    let layout = desc.layout()?;
    let mut out = vec![0u8; layout.total_size.next_multiple_of(LOD_ALIGN)];
    for face in 0..layout.faces {
        let base = face as usize * layout.face_stride;
        for lv in &layout.levels {
            let whole = PixelRect::new(0, 0, lv.width, lv.height);
            let depth = if desc.kind == TextureKind::Volume {
                lv.depth
            } else {
                1
            };
            encode_region(
                &desc,
                lv,
                lod + lv.level,
                planes,
                face,
                0..depth,
                whole,
                &mut |off, b| {
                    out[base + off..base + off + b.len()].copy_from_slice(b);
                },
            )?;
        }
    }
    Ok(out)
}

fn d3d_of(image: &Image) -> Result<D3DFormat> {
    crate::bitmap::d3d_format(image.format, image.is_linear())
}

/// The stored bytes the dirty regions change, as byte [`Span`]s, in every
/// LOD of `image` that is present (a LOD's chain starts at mip `lod`).
/// Spans are sorted by LOD and offset, never overlap, and adjacent units
/// are one span. The image keeps its size and format: this is the write
/// for a painted region, in place or live.
///
/// `planes` are the image's mip 0 pixels as they are now.
pub fn encode_tiles(image: &Image, planes: &Planes, dirty: &[Dirty]) -> Result<Vec<Span>> {
    let _scope = mem_watch::scope("assets.encode");
    check_format(image)?;
    let desc0 = texture_desc(image, 0)?;
    planes.check(&desc0)?;
    let volume = desc0.kind == TextureKind::Volume;
    let mut units: BTreeMap<(u32, usize), Vec<u8>> = BTreeMap::new();
    for lod in 0..LOD_COUNT {
        if image.lods[lod as usize].is_none() {
            continue;
        }
        let desc = texture_desc(image, lod)?;
        let layout = desc.layout()?;
        for d in dirty {
            if d.face >= layout.faces || (volume && d.slice >= desc0.depth) {
                return Err(invalid(format!(
                    "face {} slice {} is outside the image",
                    d.face, d.slice
                )));
            }
            let rect = d.rect.clip(planes.width, planes.height);
            if rect.is_empty() {
                continue;
            }
            let base = d.face as usize * layout.face_stride;
            for lv in &layout.levels {
                let m = lod + lv.level;
                // The texels of mip `m` that read the rectangle.
                let (x0, y0) = (rect.x >> m, rect.y >> m);
                let (x1, y1) = (
                    ((rect.x + rect.w - 1) >> m) + 1,
                    ((rect.y + rect.h - 1) >> m) + 1,
                );
                let region = PixelRect::new(x0, y0, x1 - x0, y1 - y0);
                // A volume's slices halve with each level: the slice's level
                // slice is the only one that reads it.
                let z = if volume {
                    (d.slice >> m).min(lv.depth - 1)
                } else {
                    0
                };
                let zs = z..z + 1;
                encode_region(&desc, lv, m, planes, d.face, zs, region, &mut |off, b| {
                    units.insert((lod, base + off), b.to_vec());
                })?;
            }
        }
    }
    let mut spans: Vec<Span> = Vec::new();
    for ((lod, offset), bytes) in units {
        match spans.last_mut() {
            Some(s) if s.lod == lod && s.offset + s.bytes.len() == offset => {
                s.bytes.extend_from_slice(&bytes)
            }
            _ => spans.push(Span { lod, offset, bytes }),
        }
    }
    Ok(spans)
}

#[cfg(test)]
#[path = "encode_tests.rs"]
mod tests;
