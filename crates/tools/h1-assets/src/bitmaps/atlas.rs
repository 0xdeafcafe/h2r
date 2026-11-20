//! Lightmap pages rebuilt per cluster.
//!
//! A Halo 2 structure cluster names one lightmap page (its render info),
//! and every triangle in it samples that page. A Halo 1 cluster's surfaces
//! sample several pages (22 of Blood Gulch's 30 clusters use more than one),
//! and a page serves several clusters. So each converted cluster gets a page
//! of its own: the parts of the Halo 1 pages its surfaces sample (a
//! [`Piece`] each, cut to the texels bilinear filtering reads, with a
//! one-texel border repeating its edge so nothing from a neighbour bleeds
//! in), packed into one power-of-two image ([`pack`]), and its lightmap
//! coordinates moved to match ([`Piece::map`]). [`compose`] cuts and pastes
//! the texels and makes the Halo 2 bitmap.
//!
//! The pages hold half Halo 1's light ([`to_halo2_light`]). Halo 2's
//! structure lighting passes draw albedo × page × 2 with a flat bump
//! (`tex_bump`'s and `tex_bump_detail_blend_detail`'s, run on the CPU in
//! `h1convert/tests/lightmap_light.rs`, `the_lighting_pass_is_twice_page_times_albedo`),
//! so a page texel of 0.5 is the albedo as it is, and retail pages leave
//! that headroom (Coagulation's: 99 % of texels at most 0.82 in their
//! brightest channel). Halo 1's pages are the albedo's multiplier
//! itself: Blood Gulch's, the same level lit by Halo 1, have half their
//! texels at 1.0 in their brightest channel, full light where Halo 2's
//! scale has room above (`h1convert/tests/lightmap_light.rs`,
//! `halo1_pages_are_lit_on_twice_halo2s_scale`), and taken as they are,
//! Halo 2's ×2 drew Blood Gulch's ground saturated yellow.

use h1map::bitmap::{Bitmap, Image, Sequence};
use h1map::{H1Map, Tag};
use crate::models::Converted;

use crate::bitmaps::convert::convert_with;
use crate::bitmaps::decode::layout_len;
use crate::bitmaps::{Error, Result};

/// Texels of border around a piece, repeating its edge.
pub const BORDER: u32 = 1;
/// Largest page side written.
pub const MAX_SIDE: u32 = 4096;

/// A rectangle of one Halo 1 page (or of white) placed in a cluster page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Piece {
    /// The Halo 1 page, or `None` for white (light for surfaces Halo 1
    /// doesn't lightmap).
    pub page: Option<u16>,
    /// The page's size in texels.
    pub page_size: [u32; 2],
    /// The texels taken: first column and row, then width and height.
    pub from: [u32; 4],
    /// Where the piece's border starts in the cluster page.
    pub at: [u32; 2],
}

impl Piece {
    /// The texels bilinear filtering reads for lightmap coordinates within
    /// `lo..=hi` (each in [0, 1]) on a page of `size`, plus nothing more:
    /// first column and row, width and height, clamped to the page.
    pub fn footprint(size: [u32; 2], lo: [f32; 2], hi: [f32; 2]) -> [u32; 4] {
        let mut out = [0; 4];
        for k in 0..2 {
            let n = size[k].max(1);
            let texel = |c: f32| ((c * n as f32 - 0.5).floor() as i64).clamp(0, i64::from(n) - 1);
            let (a, b) = (texel(lo[k]), texel(hi[k]) + 1);
            let b = b.min(i64::from(n) - 1);
            out[k] = a as u32;
            out[2 + k] = (b - a + 1) as u32;
        }
        out
    }

    /// The piece's size in the cluster page, border included.
    pub fn size(&self) -> [u32; 2] {
        [self.from[2] + 2 * BORDER, self.from[3] + 2 * BORDER]
    }

    /// Moves a lightmap coordinate on the Halo 1 page to the cluster page
    /// of `page_size`: the same texel, now where the piece sits. A white
    /// piece maps everything to its middle.
    pub fn map(&self, uv: [f32; 2], page_size: [u32; 2]) -> [f32; 2] {
        let mut out = [0.0; 2];
        for k in 0..2 {
            let inside = match self.page {
                Some(_) => uv[k] * self.page_size[k] as f32 - self.from[k] as f32,
                None => self.from[2 + k] as f32 / 2.0,
            };
            out[k] = (self.at[k] as f32 + BORDER as f32 + inside) / page_size[k] as f32;
        }
        out
    }
}

/// Places pieces of `sizes` (border included) in the smallest power-of-two
/// page that holds them, by shelves of descending height. Returns the page
/// size and each piece's top-left corner, or `None` when no page up to
/// [`MAX_SIDE`] square holds them.
pub fn pack(sizes: &[[u32; 2]]) -> Option<([u32; 2], Vec<[u32; 2]>)> {
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by_key(|&i| {
        (
            std::cmp::Reverse(sizes[i][1]),
            std::cmp::Reverse(sizes[i][0]),
        )
    });
    let widest = sizes.iter().map(|s| s[0]).max().unwrap_or(1).max(1);
    let mut best: Option<([u32; 2], Vec<[u32; 2]>)> = None;
    let mut width = widest.next_power_of_two();
    while width <= MAX_SIDE {
        let mut at = vec![[0, 0]; sizes.len()];
        let (mut x, mut y, mut shelf) = (0u32, 0u32, 0u32);
        for &i in &order {
            let [w, h] = sizes[i];
            if x + w > width {
                y += shelf;
                x = 0;
                shelf = 0;
            }
            at[i] = [x, y];
            x += w;
            shelf = shelf.max(h);
        }
        let height = (y + shelf).max(1).next_power_of_two();
        if height <= MAX_SIDE {
            let area = |s: [u32; 2]| (u64::from(s[0]) * u64::from(s[1]), s[0].max(s[1]));
            if best
                .as_ref()
                .is_none_or(|(b, _)| area([width, height]) < area(*b))
            {
                best = Some(([width, height], at));
            }
        }
        width *= 2;
    }
    best
}

/// One converted cluster's page: its size and pieces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterPage {
    /// Width and height, powers of two.
    pub size: [u32; 2],
    /// The pieces, placed.
    pub pieces: Vec<Piece>,
}

/// Bytes per texel of an uncompressed, single-level 2D image; refuses
/// anything else.
fn texel_bytes(img: &Image) -> Result<usize> {
    let texels = usize::from(img.width) * usize::from(img.height);
    let len = layout_len(img)?;
    if img.depth > 1 || img.mip_count != 0 || texels == 0 || len % texels != 0 {
        return Err(Error::Unsupported(format!(
            "lightmap page {}x{}x{} with {} mips, {len} bytes",
            img.width, img.height, img.depth, img.mip_count
        )));
    }
    Ok(len / texels)
}

/// Builds the Halo 2 lightmap bitmap of `pages` (one image each, in order)
/// from the Halo 1 lightmap bitmap `tag`: every piece's texels copied from
/// its Halo 1 page, its border repeating its edge, white pieces all ones,
/// the rest of the page black. Each image keeps the format, type and
/// swizzling of the Halo 1 pages.
pub fn compose(map: &H1Map, tag: &Tag, pages: &[ClusterPage]) -> Result<(Bitmap, Converted)> {
    let source = Bitmap::read(map, tag)?;
    let first = source
        .images
        .first()
        .ok_or_else(|| Error::Invalid("a lightmap bitmap with no pages".into()))?;
    let bpt = texel_bytes(first)?;
    let mut linear = Vec::with_capacity(source.images.len());
    for (i, img) in source.images.iter().enumerate() {
        if img.format != first.format || texel_bytes(img)? != bpt {
            return Err(Error::Unsupported("lightmap pages of mixed formats".into()));
        }
        let len = layout_len(img)?;
        let bytes = source.pixels(map, i)?;
        let bytes = bytes
            .get(..len)
            .ok_or_else(|| Error::Invalid(format!("lightmap page {i} is short")))?;
        let (w, h) = (u32::from(img.width), u32::from(img.height));
        linear.push(if img.swizzled() {
            gfx::swizzle::unswizzle(bytes, w, h, 1, bpt)
                .map_err(|e| Error::Invalid(format!("lightmap page {i}: {e:?}")))?
        } else {
            bytes.to_vec()
        });
    }
    let mut images = Vec::with_capacity(pages.len());
    let mut pixels = Vec::with_capacity(pages.len());
    for p in pages {
        let [w, h] = p.size;
        let mut out = vec![0u8; w as usize * h as usize * bpt];
        for piece in &p.pieces {
            paste(&mut out, w, bpt, piece, &linear, &source.images)?;
        }
        to_halo2_light(&mut out, first.format)?;
        if first.swizzled() {
            out = gfx::swizzle::swizzle(&out, w, h, 1, bpt)
                .map_err(|e| Error::Invalid(format!("cluster page: {e:?}")))?;
        }
        let mut img = first.clone();
        img.width = u16::try_from(w).map_err(|_| Error::Invalid("page too wide".into()))?;
        img.height = u16::try_from(h).map_err(|_| Error::Invalid("page too tall".into()))?;
        img.registration = [0; 2];
        img.pixel_offset = 0;
        img.pixel_size = out.len() as u32;
        images.push(img);
        pixels.push(out);
    }
    let bitmap = Bitmap {
        sequences: (0..pages.len())
            .map(|k| Sequence {
                name: String::new(),
                first_image: k as i16,
                image_count: 1,
                sprites: Vec::new(),
            })
            .collect(),
        images,
        ..source
    };
    let (converted, _) = convert_with(&bitmap, |i| Ok(pixels[i].clone()))?;
    Ok((bitmap, converted))
}

/// Halves every texel's colour in `texels` (linear, `format`): Halo 1's
/// light (the albedo's multiplier) on Halo 2's scale (half of it, which
/// the lighting pass doubles; module docs). Each channel is rounded to
/// the nearest step, halves up, so Halo 1's full light (31 or 63) is a
/// step above half and the pass's ×2 clamps to full. 16-bit R5G6B5 and
/// 32-bit (A|X)8R8G8B8 pages; alpha is kept.
pub fn to_halo2_light(texels: &mut [u8], format: h1map::bitmap::PixelFormat) -> Result<()> {
    use h1map::bitmap::PixelFormat as F;
    let half = |v: u16| v.div_ceil(2);
    match format {
        F::R5G6B5 => {
            for t in texels.chunks_exact_mut(2) {
                let v = u16::from_le_bytes([t[0], t[1]]);
                let (r, g, b) = (half(v >> 11), half((v >> 5) & 0x3f), half(v & 0x1f));
                t.copy_from_slice(&((r << 11) | (g << 5) | b).to_le_bytes());
            }
        }
        F::A8R8G8B8 | F::X8R8G8B8 => {
            for t in texels.chunks_exact_mut(4) {
                for c in &mut t[..3] {
                    *c = half(u16::from(*c)) as u8;
                }
            }
        }
        f => {
            return Err(Error::Unsupported(format!("lightmap pages of format {f:?}")));
        }
    }
    Ok(())
}

/// Copies one piece (border included) into the row-major page `out`.
fn paste(
    out: &mut [u8],
    width: u32,
    bpt: usize,
    piece: &Piece,
    linear: &[Vec<u8>],
    images: &[Image],
) -> Result<()> {
    let [pw, ph] = piece.size();
    let [x0, y0, fw, fh] = piece.from;
    let source = match piece.page {
        Some(p) => {
            let i = usize::from(p);
            let img = images
                .get(i)
                .ok_or_else(|| Error::Invalid(format!("lightmap page {p} of {}", images.len())))?;
            if x0 + fw > u32::from(img.width) || y0 + fh > u32::from(img.height) {
                return Err(Error::Invalid(format!("piece past lightmap page {p}")));
            }
            Some((&linear[i], u32::from(img.width)))
        }
        None => None,
    };
    for dy in 0..ph {
        for dx in 0..pw {
            let (x, y) = (piece.at[0] + dx, piece.at[1] + dy);
            let to = (y as usize * width as usize + x as usize) * bpt;
            let dst = out
                .get_mut(to..to + bpt)
                .ok_or_else(|| Error::Invalid("piece past its cluster page".into()))?;
            match source {
                None => dst.fill(0xff),
                Some((src, sw)) => {
                    // The border repeats the nearest edge texel.
                    let sx = x0 + dx.saturating_sub(BORDER).min(fw - 1);
                    let sy = y0 + dy.saturating_sub(BORDER).min(fh - 1);
                    let from = (sy as usize * sw as usize + sx as usize) * bpt;
                    dst.copy_from_slice(&src[from..from + bpt]);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halo1_light_halves_onto_halo2s_scale() {
        use h1map::bitmap::PixelFormat as F;
        // R5G6B5: full white, black, and (17, 40, 3).
        let px = |r: u16, g: u16, b: u16| ((r << 11) | (g << 5) | b).to_le_bytes();
        let mut t: Vec<u8> = [px(31, 63, 31), px(0, 0, 0), px(17, 40, 3)].concat();
        to_halo2_light(&mut t, F::R5G6B5).unwrap();
        assert_eq!(t, [px(16, 32, 16), px(0, 0, 0), px(9, 20, 2)].concat());
        // Each channel within half a step of half Halo 1's value, and
        // twice it (the lighting pass) within a step of Halo 1's.
        for v in 0u16..32 {
            let mut t = px(v, 0, 0).to_vec();
            to_halo2_light(&mut t, F::R5G6B5).unwrap();
            let h = u16::from_le_bytes([t[0], t[1]]) >> 11;
            assert!((f32::from(h) - f32::from(v) / 2.0).abs() <= 0.5, "{v} -> {h}");
            assert!((f32::from(2 * h).min(31.0) - f32::from(v)).abs() <= 1.0, "{v} -> {h}");
        }
        // 32-bit: colour halved, alpha kept.
        let mut t = vec![255u8, 128, 7, 200];
        to_halo2_light(&mut t, F::A8R8G8B8).unwrap();
        assert_eq!(t, [128, 64, 4, 200]);
        assert!(to_halo2_light(&mut [0u8; 8], F::Dxt1).is_err());
    }

    #[test]
    fn the_footprint_is_the_texels_bilinear_filtering_reads() {
        // One coordinate at a texel corner reads the four texels around it.
        assert_eq!(Piece::footprint([8, 8], [0.5; 2], [0.5; 2]), [3, 3, 2, 2]);
        // The whole page, clamped at its edges.
        assert_eq!(Piece::footprint([8, 4], [0.0; 2], [1.0; 2]), [0, 0, 8, 4]);
        // Texel 2's centre to texel 5's: the filter reads texel 6 (with no
        // weight) at the last.
        assert_eq!(
            Piece::footprint([16, 16], [2.5 / 16.0; 2], [5.5 / 16.0; 2]),
            [2, 2, 5, 5]
        );
    }

    #[test]
    fn packed_pieces_fit_a_power_of_two_page_without_overlapping() {
        let sizes = [[10, 6], [34, 3], [4, 4], [4, 4], [18, 18], [3, 40]];
        let (size, at) = pack(&sizes).unwrap();
        assert!(size.iter().all(|s| s.is_power_of_two()));
        for (i, (a, s)) in at.iter().zip(&sizes).enumerate() {
            assert!(
                a[0] + s[0] <= size[0] && a[1] + s[1] <= size[1],
                "piece {i}"
            );
            for (j, (b, t)) in at.iter().zip(&sizes).enumerate().skip(i + 1) {
                let apart = a[0] + s[0] <= b[0]
                    || b[0] + t[0] <= a[0]
                    || a[1] + s[1] <= b[1]
                    || b[1] + t[1] <= a[1];
                assert!(apart, "pieces {i} and {j} overlap");
            }
        }
        assert!(pack(&[[MAX_SIDE + 1, 1]]).is_none());
    }

    #[test]
    fn a_coordinate_lands_on_the_same_texel_in_the_cluster_page() {
        let piece = Piece {
            page: Some(0),
            page_size: [16, 8],
            from: [4, 2, 6, 3],
            at: [9, 5],
        };
        // Texel (5, 3)'s centre on the Halo 1 page...
        let uv = [5.5 / 16.0, 3.5 / 8.0];
        let page = [32, 16];
        let moved = piece.map(uv, page);
        // ...is texel (9 + 1 + 1, 5 + 1 + 1)'s centre on the cluster page.
        assert_eq!(moved, [11.5 / 32.0, 7.5 / 16.0]);
        let white = Piece {
            page: None,
            page_size: [2, 2],
            from: [0, 0, 2, 2],
            at: [0, 4],
        };
        assert_eq!(white.map([0.9, 0.1], page), [2.0 / 32.0, 6.0 / 16.0]);
    }

    #[test]
    fn a_pasted_piece_repeats_its_edge_in_its_border() {
        use h1map::bitmap::{ImageKind, PixelFormat};
        // A 4x2 one-byte-per-texel page: texel value = 10 * row + column.
        let page: Vec<u8> = (0..2)
            .flat_map(|y| (0..4).map(move |x| 10 * y + x))
            .collect();
        let image = Image {
            width: 4,
            height: 2,
            depth: 1,
            kind: ImageKind::D2,
            format: PixelFormat::from_raw(0),
            flags: 0,
            registration: [0; 2],
            mip_count: 0,
            pixel_offset: 0,
            pixel_size: 8,
            raw: [0; h1map::bitmap::IMAGE_LEN],
        };
        let piece = Piece {
            page: Some(0),
            page_size: [4, 2],
            from: [1, 0, 2, 2],
            at: [0, 0],
        };
        let mut out = vec![0u8; 4 * 4];
        paste(&mut out, 4, 1, &piece, &[page], &[image]).unwrap();
        assert_eq!(
            out,
            [
                1, 1, 2, 2, //
                1, 1, 2, 2, //
                11, 11, 12, 12, //
                11, 11, 12, 12,
            ]
        );
        let white = Piece {
            page: None,
            page_size: [2, 2],
            from: [0, 0, 2, 2],
            at: [0, 0],
        };
        let mut out = vec![0u8; 4 * 4];
        paste(&mut out, 4, 1, &white, &[], &[]).unwrap();
        assert!(out.iter().all(|&b| b == 0xff));
    }
}
