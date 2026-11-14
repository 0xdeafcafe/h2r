//! Morton-order ("swizzled") texture addressing, as used by the NV2A.
//!
//! A swizzled texture of power-of-two size `W x H x D` stores texel
//! `(x, y, z)` at index `deposit(x, mx) | deposit(y, my) | deposit(z, mz)`.
//! The masks are built by handing out bit positions from the least
//! significant end, round-robin in the order x, y, z, skipping a dimension
//! once all of its `log2(size)` bits have been placed. For a square 2D
//! texture that is the classic bit interleave (`x0 y0 x1 y1 ...`); for a
//! non-square one the surplus high bits of the longer axis are appended
//! contiguously, so the texture is a row (or column) of square Morton tiles.
//!
//! This matches the behaviour of xemu's `swizzle.c` (commit
//! `4bdad199102b22ed0cfa998e9af6b35d76238b3e`) and the XDK's
//! `XGSwizzleRect`; the implementation here is written from that description.
//! Each mip level, and each cube face, is swizzled independently.

/// Errors from swizzling.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SwizzleError {
    /// Swizzled storage requires power-of-two dimensions.
    #[error("swizzled dimensions must be powers of two, got {0}x{1}x{2}")]
    NotPowerOfTwo(u32, u32, u32),
    /// The texel count does not fit the 32-bit Morton index.
    #[error("swizzled volume {0}x{1}x{2} is too large")]
    TooLarge(u32, u32, u32),
    /// A buffer was smaller than the texture needs.
    #[error("buffer too small: need {need} bytes, have {have}")]
    BufferTooSmall {
        /// Bytes required.
        need: usize,
        /// Bytes supplied.
        have: usize,
    },
}

/// Precomputed per-axis offset tables for one `W x H x D` swizzle pattern.
#[derive(Clone, Debug)]
pub struct Swizzler {
    width: u32,
    height: u32,
    depth: u32,
    x: Vec<u32>,
    y: Vec<u32>,
    z: Vec<u32>,
}

impl Swizzler {
    /// Builds the tables. All dimensions must be powers of two (use `1` for
    /// the unused axes of a 2D texture).
    pub fn new(width: u32, height: u32, depth: u32) -> Result<Self, SwizzleError> {
        let ok = |v: u32| v != 0 && v.is_power_of_two();
        if !(ok(width) && ok(height) && ok(depth)) {
            return Err(SwizzleError::NotPowerOfTwo(width, height, depth));
        }
        if width.trailing_zeros() + height.trailing_zeros() + depth.trailing_zeros() > 31 {
            return Err(SwizzleError::TooLarge(width, height, depth));
        }
        let mut remaining = [width.trailing_zeros(), height.trailing_zeros(), depth.trailing_zeros()];
        let mut masks = [0u32; 3];
        let mut pos = 0;
        while remaining.iter().any(|&r| r > 0) {
            for axis in 0..3 {
                if remaining[axis] > 0 {
                    masks[axis] |= 1 << pos;
                    pos += 1;
                    remaining[axis] -= 1;
                }
            }
        }
        let table = |size: u32, mask: u32| -> Vec<u32> {
            (0..size)
                .map(|c| {
                    // Software PDEP: scatter the bits of `c` into `mask`.
                    let (mut out, mut src, mut m) = (0u32, c, mask);
                    while m != 0 {
                        let low = m.isolate_lowest_one();
                        if src & 1 != 0 {
                            out |= low;
                        }
                        src >>= 1;
                        m &= m - 1;
                    }
                    out
                })
                .collect()
        };
        Ok(Self {
            width,
            height,
            depth,
            x: table(width, masks[0]),
            y: table(height, masks[1]),
            z: table(depth, masks[2]),
        })
    }

    /// Texel index (not byte offset) of `(x, y, z)` in swizzled storage.
    #[inline]
    pub fn index(&self, x: u32, y: u32, z: u32) -> usize {
        (self.x[x as usize] | self.y[y as usize] | self.z[z as usize]) as usize
    }

    /// Total texel count.
    pub fn texel_count(&self) -> usize {
        self.width as usize * self.height as usize * self.depth as usize
    }

    fn check(&self, src_len: usize, dst_len: usize, bpt: usize) -> Result<(), SwizzleError> {
        let need = self.texel_count() * bpt;
        for have in [src_len, dst_len] {
            if have < need {
                return Err(SwizzleError::BufferTooSmall { need, have });
            }
        }
        Ok(())
    }

    /// Converts swizzled `src` into tightly packed row-major `dst`
    /// (`z` slices of `y` rows of `x` texels), `bpt` bytes per texel.
    pub fn unswizzle_into(&self, src: &[u8], dst: &mut [u8], bpt: usize) -> Result<(), SwizzleError> {
        self.check(src.len(), dst.len(), bpt)?;
        let mut o = 0;
        for z in 0..self.depth {
            for y in 0..self.height {
                for x in 0..self.width {
                    let s = self.index(x, y, z) * bpt;
                    dst[o..o + bpt].copy_from_slice(&src[s..s + bpt]);
                    o += bpt;
                }
            }
        }
        Ok(())
    }

    /// Inverse of [`unswizzle_into`](Self::unswizzle_into).
    pub fn swizzle_into(&self, src: &[u8], dst: &mut [u8], bpt: usize) -> Result<(), SwizzleError> {
        self.check(src.len(), dst.len(), bpt)?;
        let mut i = 0;
        for z in 0..self.depth {
            for y in 0..self.height {
                for x in 0..self.width {
                    let d = self.index(x, y, z) * bpt;
                    dst[d..d + bpt].copy_from_slice(&src[i..i + bpt]);
                    i += bpt;
                }
            }
        }
        Ok(())
    }
}

/// Unswizzles one 2D/3D level into a new tightly packed buffer.
pub fn unswizzle(src: &[u8], width: u32, height: u32, depth: u32, bpt: usize) -> Result<Vec<u8>, SwizzleError> {
    let s = Swizzler::new(width, height, depth)?;
    let mut out = vec![0u8; s.texel_count() * bpt];
    s.unswizzle_into(src, &mut out, bpt)?;
    Ok(out)
}

/// Swizzles one 2D/3D level from tightly packed row-major data.
pub fn swizzle(src: &[u8], width: u32, height: u32, depth: u32, bpt: usize) -> Result<Vec<u8>, SwizzleError> {
    let s = Swizzler::new(width, height, depth)?;
    let mut out = vec![0u8; s.texel_count() * bpt];
    s.swizzle_into(src, &mut out, bpt)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_2d_is_classic_interleave() {
        // 4x4: index = y1 x1 y0 x0 (x in even bits, y in odd bits).
        let s = Swizzler::new(4, 4, 1).unwrap();
        let expect = |x: u32, y: u32| {
            (x & 1) | ((y & 1) << 1) | ((x & 2) << 1) | ((y & 2) << 2)
        } as usize;
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(s.index(x, y, 0), expect(x, y), "({x},{y})");
            }
        }
        // Known values.
        assert_eq!(s.index(1, 0, 0), 1);
        assert_eq!(s.index(0, 1, 0), 2);
        assert_eq!(s.index(2, 0, 0), 4);
        assert_eq!(s.index(3, 3, 0), 15);
    }

    #[test]
    fn non_square_appends_surplus_bits() {
        // 8x2: bits x0 y0 x1 x2.
        let s = Swizzler::new(8, 2, 1).unwrap();
        assert_eq!(s.index(1, 0, 0), 1);
        assert_eq!(s.index(0, 1, 0), 2);
        assert_eq!(s.index(2, 0, 0), 4);
        assert_eq!(s.index(4, 0, 0), 8);
        assert_eq!(s.index(7, 1, 0), 15);
        // 2x8: bits x0 y0 y1 y2.
        let s = Swizzler::new(2, 8, 1).unwrap();
        assert_eq!(s.index(1, 0, 0), 1);
        assert_eq!(s.index(0, 1, 0), 2);
        assert_eq!(s.index(0, 2, 0), 4);
        assert_eq!(s.index(0, 4, 0), 8);
    }

    #[test]
    fn volume_interleaves_xyz() {
        let s = Swizzler::new(4, 4, 4).unwrap();
        assert_eq!(s.index(1, 0, 0), 1);
        assert_eq!(s.index(0, 1, 0), 2);
        assert_eq!(s.index(0, 0, 1), 4);
        assert_eq!(s.index(2, 0, 0), 8);
        assert_eq!(s.index(3, 3, 3), 63);
    }

    #[test]
    fn index_is_a_bijection() {
        for &(w, h, d) in &[(1, 1, 1), (16, 4, 1), (4, 16, 1), (8, 8, 2), (2, 4, 8), (1, 32, 1)] {
            let s = Swizzler::new(w, h, d).unwrap();
            let mut seen = vec![false; s.texel_count()];
            for z in 0..d {
                for y in 0..h {
                    for x in 0..w {
                        let i = s.index(x, y, z);
                        assert!(!seen[i], "duplicate index for {w}x{h}x{d}");
                        seen[i] = true;
                    }
                }
            }
            assert!(seen.iter().all(|&b| b));
        }
    }

    #[test]
    fn roundtrip_various_sizes_and_bpt() {
        for &(w, h, d) in &[(8u32, 8u32, 1u32), (16, 4, 1), (2, 32, 1), (4, 4, 4), (1, 1, 1)] {
            for bpt in [1usize, 2, 4] {
                let n = (w * h * d) as usize * bpt;
                let linear: Vec<u8> = (0..n).map(|i| (i * 7 + 3) as u8).collect();
                let sw = swizzle(&linear, w, h, d, bpt).unwrap();
                if (w, h) == (8, 8) {
                    assert_ne!(sw, linear, "8x8 must actually permute");
                }
                let back = unswizzle(&sw, w, h, d, bpt).unwrap();
                assert_eq!(back, linear);
            }
        }
    }

    #[test]
    fn rejects_non_pow2_and_short_buffers() {
        assert_eq!(Swizzler::new(3, 4, 1).unwrap_err(), SwizzleError::NotPowerOfTwo(3, 4, 1));
        assert!(matches!(
            unswizzle(&[0u8; 10], 4, 4, 1, 1),
            Err(SwizzleError::BufferTooSmall { need: 16, have: 10 })
        ));
    }

    #[test]
    fn unswizzle_known_4x4_u8() {
        // Swizzled memory holds 0..16 in order; row-major result follows the
        // interleave table.
        let sw: Vec<u8> = (0..16).collect();
        let lin = unswizzle(&sw, 4, 4, 1, 1).unwrap();
        assert_eq!(lin, vec![0, 1, 4, 5, 2, 3, 6, 7, 8, 9, 12, 13, 10, 11, 14, 15]);
    }

    #[test]
    fn huge_volumes_are_rejected_not_overflowed() {
        assert!(matches!(Swizzler::new(1 << 16, 1 << 16, 1 << 16), Err(SwizzleError::TooLarge(..))));
    }
}
