//! Ambience palette selection across a BSP portal and its distance fade.

use crate::environment::GainCurve;
use x87::abi::{self as fp, X87Abi, src_kind, unary_op};

/// A sound-cluster list stores the side of the portal in its index.
#[derive(Clone, Copy)]
pub struct PortalLink {
    pub index: u32,
    pub side: u32,
}

impl From<u16> for PortalLink {
    fn from(word: u16) -> Self {
        Self {
            index: u32::from(word & 0x7fff),
            side: u32::from(word >> 15),
        }
    }
}

/// Coefficients come from the image. Both endpoint gains are rounded to
/// binary32 before the cubic interpolates between them.
#[derive(Clone, Copy)]
pub struct FadeCurve {
    pub minimum_radius: u32,
    pub epsilon: u32,
    pub one: u32,
    pub quadratic: u32,
    pub cubic: u32,
    pub db_threshold: u32,
    pub silent_db: u32,
    pub gain: GainCurve,
}

pub struct Fade {
    pub radius: u32,
    pub low: u32,
    pub high: u32,
    pub weight: u32,
    pub linear: u32,
    pub flags: u32,
    pub high_db: u32,
}

impl FadeCurve {
    /// Evaluate a listener distance in the same float environments as the
    /// caller. Comparisons retain the unordered behaviour and MXCSR flags.
    pub fn at(&self, radius: u32, distance: u32, mxcsr: &mut u32, fpu: &mut X87Abi) -> Fade {
        let cmp = |csr: &mut u32, a: u32, b: u32| simd::comiss(csr, a, b).expect("masked SSE");
        let above = |flags: u32| flags & 0x41 == 0;
        let sub = |csr: &mut u32, a: u32, b: u32| {
            simd::subss(csr, u128::from(a), b).expect("masked SSE") as u32
        };
        let mul = |csr: &mut u32, a: u32, b: u32| {
            simd::mulss(csr, u128::from(a), b).expect("masked SSE") as u32
        };
        let radius = if above(cmp(mxcsr, radius, self.minimum_radius)) {
            radius
        } else {
            self.minimum_radius
        };
        fp::ld_f32(fpu, radius);
        fp::unary(fpu, unary_op::ABS);
        fp::ld_f32(fpu, self.epsilon);
        let tiny = above(fp::cmp(fpu, 0, src_kind::ST, 1, 1, true));
        fp::st_st(fpu, 0, true);

        let mut position = if tiny {
            if above(cmp(mxcsr, radius, distance)) {
                self.one
            } else {
                0
            }
        } else {
            let n = sub(mxcsr, distance, radius);
            let d = sub(mxcsr, 0, radius);
            simd::divss(mxcsr, u128::from(n), d).expect("masked SSE") as u32
        };
        if above(cmp(mxcsr, 0, position)) {
            position = 0;
        } else if above(cmp(mxcsr, position, self.one)) {
            position = self.one;
        }

        // For each endpoint the caller clamps a dB value before converting it.
        let low_db = if !above(cmp(mxcsr, self.db_threshold, self.silent_db))
            && above(cmp(mxcsr, self.silent_db, 0))
        {
            0
        } else {
            self.silent_db
        };
        let endpoint = |fpu: &mut X87Abi, db| {
            self.gain.linear(db, fpu);
            let (_, store) = fp::st_f32(fpu, 0);
            fp::store_commit(fpu, store.status, true);
            store.value
        };
        let low = endpoint(fpu, low_db);
        let high_db = if above(cmp(mxcsr, self.db_threshold, 0)) {
            self.silent_db
        } else {
            0
        };
        let high = endpoint(fpu, high_db);

        let square = mul(mxcsr, position, position);
        let cube = mul(mxcsr, square, position);
        let q = mul(mxcsr, square, self.quadratic);
        let r = mul(mxcsr, cube, self.cubic);
        let mut weight = sub(mxcsr, q, r);
        let mut flags = cmp(mxcsr, 0, weight);
        if above(flags) {
            weight = 0;
        } else {
            flags = cmp(mxcsr, weight, self.one);
            if above(flags) {
                weight = self.one;
            }
        }
        let delta = sub(mxcsr, high, low);
        let scaled = mul(mxcsr, delta, weight);
        let linear = simd::addss(mxcsr, u128::from(scaled), low).expect("masked SSE") as u32;
        Fade {
            radius,
            low,
            high,
            weight,
            linear,
            flags,
            high_db,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_portal_index_selects_the_second_endpoint() {
        let link = PortalLink::from(0x8015);
        assert_eq!((link.index, link.side), (21, 1));
    }

    #[test]
    fn cubic_fade_preserves_both_endpoints_and_the_halfway_gain() {
        let curve = FadeCurve {
            minimum_radius: 0,
            epsilon: 0.01f32.to_bits(),
            one: 1f32.to_bits(),
            quadratic: 3f32.to_bits(),
            cubic: 2f32.to_bits(),
            db_threshold: (-1.0f32).to_bits(),
            silent_db: (-4.0f32).to_bits(),
            gain: GainCurve {
                decibel_scale: 1f32.to_bits(),
                exponent_scale: 1f32.to_bits(),
            },
        };
        for (d, w) in [
            (-2.0f32, 1f32),
            (0.0, 1.0),
            (2.0, 0.5),
            (4.0, 0.0),
            (6.0, 0.0),
        ] {
            let result = curve.at(
                4f32.to_bits(),
                d.to_bits(),
                &mut 0x1f80,
                &mut X87Abi::default(),
            );
            assert_eq!(result.weight, w.to_bits());
        }
    }
}
