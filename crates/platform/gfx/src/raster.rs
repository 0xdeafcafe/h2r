//! Conversion from guest pixel coordinates to the physical raster grid.

/// The renderer's window-coordinate shift for a window `window` pixels across on a
/// target `target` pixels across, in window pixels: `(2 s - 1) / (4 s)`
/// for `s = target / window`, 0 when they are equal.
///
/// At 1x a pixel is sampled at its centre, and D3D-convention geometry puts
/// edges (and a 1:1 copy's texel corners) exactly there, so the tie rules
/// decide. With the shift the `s x s` host pixels of a guest pixel sample
/// it at offsets `(k + 1/4) / s` from its centre, `k = 0..s`: all inside
/// `[0, 1)`, none on the tie. An edge on a guest pixel centre then covers
/// whole blocks, as it covers whole pixels at 1x (top-left rule included),
/// and a 1:1 point-sampled copy reads the texel 1x reads.
pub fn window_shift(window: u32, target: u32) -> f32 {
    if window == 0 || window >= target {
        return 0.0;
    }
    let s = target as f32 / window as f32;
    (2.0 * s - 1.0) / (4.0 * s)
}

/// Independent host / guest raster ratios. Reduced fractions make identical
/// grids compare equal across render targets of different sizes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderScale {
    numer: [u32; 2],
    denom: [u32; 2],
}

impl RenderScale {
    pub const ONE: Self = Self { numer: [1; 2], denom: [1; 2] };

    pub fn between(guest: (u32, u32), host: (u32, u32)) -> Option<Self> {
        fn gcd(mut a: u32, mut b: u32) -> u32 {
            while b != 0 { (a, b) = (b, a % b); }
            a
        }
        if [guest.0, guest.1, host.0, host.1].contains(&0) { return None; }
        let (x, y) = (gcd(guest.0, host.0), gcd(guest.1, host.1));
        Some(Self { numer: [host.0 / x, host.1 / y], denom: [guest.0 / x, guest.1 / y] })
    }

    /// Render a 640x480 Xbox frame at this physical output size.
    pub fn for_output(width: u32, height: u32) -> Result<Self, String> {
        Self::between((640, 480), (width, height)).ok_or_else(|| "zero-sized render target".into())
    }

    fn edge(self, pos: u32, axis: usize) -> u32 {
        let den = u64::from(self.denom[axis]);
        let scaled = (u64::from(pos) * u64::from(self.numer[axis]) + den / 2) / den;
        scaled.min(u64::from(u32::MAX)) as u32
    }

    pub fn size(self, width: u32, height: u32) -> (u32, u32) {
        (self.edge(width, 0).max(1), self.edge(height, 1).max(1))
    }

    /// Scale half-open edges independently; adjacent pieces still share
    /// their boundary when a guest coordinate lands between host pixels.
    pub fn rect(self, x: u32, y: u32, w: u32, h: u32) -> [u32; 4] {
        let (left, top) = (self.edge(x, 0), self.edge(y, 1));
        [left, top, self.edge(x.saturating_add(w), 0) - left,
            self.edge(y.saturating_add(h), 1) - top]
    }

    pub fn shift(self) -> [f32; 2] {
        std::array::from_fn(|i| window_shift(self.denom[i], self.numer[i]))
    }

    /// Map physical extents back to guest pixels. For a surface this grid
    /// was created from, the inverse is exact.
    pub fn inverse(self) -> Self { Self { denom: self.numer, numer: self.denom } }

    pub fn magnified(self) -> bool {
        (0..2).any(|i| self.numer[i] > self.denom[i])
    }

    pub fn uniform_integer(self) -> Option<u32> {
        (self.denom == [1, 1] && self.numer[0] == self.numer[1]).then_some(self.numer[0])
    }

    /// Shadow maps are square and follow the nearest integral vertical scale.
    pub fn vertical_rounded(self) -> u32 { self.edge(1, 1).max(1) }
}

impl From<u32> for RenderScale {
    fn from(scale: u32) -> Self { Self { numer: [scale.max(1); 2], denom: [1; 2] } }
}

impl std::fmt::Display for RenderScale {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (w, h) = self.size(640, 480);
        write!(f, "{w}x{h}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grids_round_shared_edges_instead_of_widths() {
        let grid = RenderScale::between((4, 4), (10, 7)).unwrap();
        assert_eq!(grid.rect(0, 0, 1, 1), [0, 0, 3, 2]);
        assert_eq!(grid.rect(1, 1, 1, 1), [3, 2, 2, 2]);
        assert_eq!(grid.rect(2, 2, 2, 2), [5, 4, 5, 3]);
        for (w, h) in [(5120, 2880), (3440, 1440), (1280, 800), (301, 199), (1, 1)] {
            let grid = RenderScale::for_output(w, h).unwrap();
            assert_eq!(grid.rect(0, 0, 640, 480), [0, 0, w, h]);
            assert_eq!(grid.inverse().size(w, h), (640, 480));
        }
    }

    #[test]
    fn guest_positions_take_the_half_pixel_shift_of_each_axis() {
        let grid = RenderScale::between((4, 4), (10, 7)).unwrap();
        assert_eq!(grid.shift(), [0.4, 2.5 / 7.0]);
        assert_eq!(RenderScale::between((4, 4), (8, 4)).unwrap().shift(), [0.375, 0.0]);
        assert_eq!(RenderScale::ONE.shift(), [0.0; 2]);
    }
}
