//! Comparing techniques by their shadow masks, and laying images out.
//!
//! Every technique writes an 8-bit mask beside its colour image:
//! [`FACING_AWAY`] where the surface faces away from the light (no technique
//! has anything to say there), [`SHADOWED`] where it faces the light but is
//! occluded, [`LIT`] where it is lit. Filtered maps write fractions in between.
pub const FACING_AWAY: u8 = 0;
#[cfg(test)]
pub const SHADOWED: u8 = 128;
#[cfg(test)]
pub const LIT: u8 = 255;

/// The mask value as lit or not, for pixels that face the light.
fn lit(v: u8) -> Option<bool> {
    // Anything at or above the midpoint between shadowed and lit is lit.
    (v != FACING_AWAY).then_some(v >= 192)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MaskDiff {
    /// Pixels where both images face the light.
    pub compared: usize,
    /// Shadowed in `a`, lit in `b`.
    pub a_only: usize,
    /// Lit in `a`, shadowed in `b`.
    pub b_only: usize,
    /// Shadowed in both.
    pub both: usize,
}

impl MaskDiff {
    pub fn disagree(&self) -> usize {
        self.a_only + self.b_only
    }
    /// Disagreeing pixels as a percentage of the compared ones.
    pub fn percent(&self) -> f64 {
        if self.compared == 0 {
            0.0
        } else {
            100.0 * self.disagree() as f64 / self.compared as f64
        }
    }
}

pub fn compare(a: &[u8], b: &[u8]) -> MaskDiff {
    assert_eq!(a.len(), b.len(), "masks of different sizes");
    let mut d = MaskDiff::default();
    for (&x, &y) in a.iter().zip(b) {
        let (Some(x), Some(y)) = (lit(x), lit(y)) else {
            continue;
        };
        d.compared += 1;
        match (x, y) {
            (false, true) => d.a_only += 1,
            (true, false) => d.b_only += 1,
            (false, false) => d.both += 1,
            (true, true) => {}
        }
    }
    d
}

/// RGBA picture of a comparison: black faces away, grey lit in both, dark
/// shadowed in both, red shadowed only in `a`, cyan shadowed only in `b`.
pub fn diff_image(a: &[u8], b: &[u8]) -> Vec<u8> {
    a.iter()
        .zip(b)
        .flat_map(|(&x, &y)| match (lit(x), lit(y)) {
            (Some(true), Some(true)) => [150, 150, 150, 255],
            (Some(false), Some(false)) => [45, 45, 55, 255],
            (Some(false), Some(true)) => [230, 40, 40, 255],
            (Some(true), Some(false)) => [40, 210, 230, 255],
            _ => [0, 0, 0, 255],
        })
        .collect()
}

/// Tile equal-sized RGBA images `cols` wide, with a `gap`-pixel border.
pub fn grid(
    images: &[&[u8]],
    w: usize,
    h: usize,
    cols: usize,
    gap: usize,
) -> (usize, usize, Vec<u8>) {
    let rows = images.len().div_ceil(cols.max(1));
    let (gw, gh) = (cols * w + (cols + 1) * gap, rows * h + (rows + 1) * gap);
    let mut out = vec![0u8; gw * gh * 4];
    for px in out.as_chunks_mut::<4>().0 {
        px.copy_from_slice(&[20, 20, 24, 255]);
    }
    for (i, img) in images.iter().enumerate() {
        assert_eq!(img.len(), w * h * 4, "image {i} has the wrong size");
        let (ox, oy) = (gap + (i % cols) * (w + gap), gap + (i / cols) * (h + gap));
        for y in 0..h {
            let src = &img[y * w * 4..(y + 1) * w * 4];
            let at = ((oy + y) * gw + ox) * 4;
            out[at..at + w * 4].copy_from_slice(src);
        }
    }
    (gw, gh, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compare_counts_only_pixels_facing_the_light() {
        let a = [LIT, SHADOWED, SHADOWED, LIT, FACING_AWAY, SHADOWED];
        let b = [LIT, LIT, SHADOWED, SHADOWED, SHADOWED, FACING_AWAY];
        let d = compare(&a, &b);
        assert_eq!(
            d,
            MaskDiff {
                compared: 4,
                a_only: 1,
                b_only: 1,
                both: 1
            }
        );
        assert_eq!(d.disagree(), 2);
        assert!((d.percent() - 50.0).abs() < 1e-9);
        assert_eq!(compare(&[], &[]).percent(), 0.0);
    }

    #[test]
    fn filtered_values_split_at_the_midpoint() {
        let d = compare(&[200, 180, 1], &[LIT, LIT, LIT]);
        assert_eq!((d.compared, d.a_only), (3, 2));
    }

    #[test]
    fn diff_image_colours_each_case() {
        let px = diff_image(
            &[LIT, SHADOWED, SHADOWED, LIT, 0],
            &[LIT, SHADOWED, LIT, SHADOWED, LIT],
        );
        let at = |i: usize| &px[i * 4..i * 4 + 4];
        assert_eq!(at(0), [150, 150, 150, 255]);
        assert_eq!(at(1), [45, 45, 55, 255]);
        assert_eq!(at(2), [230, 40, 40, 255]);
        assert_eq!(at(3), [40, 210, 230, 255]);
        assert_eq!(at(4), [0, 0, 0, 255]);
    }

    #[test]
    fn grid_places_tiles_row_major_with_gaps() {
        let tile = |v: u8| vec![v; 2 * 3 * 4];
        let (a, b, c) = (tile(1), tile(2), tile(3));
        let (w, h, px) = grid(&[&a, &b, &c], 2, 3, 2, 1);
        assert_eq!((w, h), (2 * 2 + 3, 2 * 3 + 3));
        let at = |x: usize, y: usize| px[(y * w + x) * 4];
        assert_eq!(at(1, 1), 1);
        assert_eq!(at(4, 1), 2);
        assert_eq!(at(1, 5), 3);
        assert_eq!(at(0, 0), 20); // border
        assert_eq!(at(4, 5), 20); // empty fourth cell
    }
}
