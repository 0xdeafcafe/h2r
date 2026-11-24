//! Listener-relative sound propagation and exact conversion to linear gain.
//! The guest supplies tuning values and the x87 control word for each mix.
use x87::abi::{self, ARITH_POP, X87Abi, arith_op, src_kind, unary_op};

/// Scale factors in the dB-to-linear path, kept separate because each
/// multiply rounds at the current precision.
#[derive(Clone, Copy)]
pub struct GainCurve {
    pub decibel_scale: u32,
    pub exponent_scale: u32,
}

impl GainCurve {
    /// Push the linear gain in extended precision. Both the precision of
    /// the intermediate results and the sticky exceptions are observable.
    /// The sound entry runs only with all exceptions masked.
    pub fn linear(self, decibels: u32, stack: &mut X87Abi) {
        abi::ld_f32(stack, decibels);
        for scale in [self.decibel_scale, self.exponent_scale] {
            abi::arith(stack, arith_op::MUL, 0, src_kind::F32, u64::from(scale));
        }
        abi::ld_const(stack, 3); // log2(e)
        abi::arith(stack, arith_op::MUL | ARITH_POP, 1, src_kind::ST, 0);
        // 2^x = 2^round(x) * (1 + (2^(x-round(x)) - 1)).
        abi::ld_st(stack, 0);
        abi::unary(stack, unary_op::RNDINT);
        abi::xch(stack, 1);
        abi::arith(stack, arith_op::SUB, 0, src_kind::ST, 1);
        abi::unary(stack, unary_op::X2M1);
        abi::ld_const(stack, 0); // one
        abi::arith(stack, arith_op::ADD | ARITH_POP, 1, src_kind::ST, 0);
        abi::unary(stack, unary_op::SCALE);
        abi::st_st(stack, 1, true);
    }
}

/// A portal has two masks, one for each group of clusters it connects.
/// The listener uses the other side; a listener present on both sides
/// takes the first match. The listener's own environment never accumulates
/// its door gain.
#[derive(Clone)]
pub struct PortalSides(pub [[u32; 16]; 2]);

impl PortalSides {
    pub fn opposite(&self, listener: usize) -> Option<usize> {
        (0..2)
            .find(|&side| self.contains(side, listener))
            .map(|side| side ^ 1)
    }

    pub fn contains(&self, side: usize, cluster: usize) -> bool {
        self.0[side][cluster / 32] & (1 << (cluster % 32)) != 0
    }
}

/// The operand order matters for signed zeros and unordered comparisons.
/// COMISS raises IE for either kind of NaN.
pub fn portal_max(mxcsr: &mut u32, previous: u32, door: u32) -> u32 {
    let flags = simd::comiss(mxcsr, previous, door).expect("masked SSE");
    if flags & 0x41 == 0 { previous } else { door }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_side_wins_when_both_contain_listener() {
        let mut p = PortalSides([[0; 16]; 2]);
        p.0[1][1] = 1;
        assert_eq!(p.opposite(32), Some(0));
        assert_eq!(p.opposite(31), None);
        p.0[0][1] = 1;
        assert_eq!(p.opposite(32), Some(1));
        p.0[0][15] = 1 << 31;
        assert!(p.contains(0, 511));
    }

    #[test]
    fn equal_and_unordered_select_the_new_gain() {
        let mut mxcsr = 0x1f80;
        assert_eq!(
            portal_max(&mut mxcsr, 0, (-0.0f32).to_bits()),
            (-0.0f32).to_bits()
        );
        assert_eq!(portal_max(&mut mxcsr, f32::NAN.to_bits(), 0), 0);
        assert_ne!(mxcsr & 1, 0);
    }

    #[test]
    fn zero_db_pushes_unity_above_the_previous_gain() {
        let mut stack = X87Abi::default();
        abi::ld_f32(&mut stack, 7.5f32.to_bits());
        let curve = GainCurve {
            decibel_scale: 0.25f32.to_bits(),
            exponent_scale: 3.0f32.to_bits(),
        };
        curve.linear(0, &mut stack);
        assert_eq!(stack.st(0), Some(x87::F80::ONE));
        assert_eq!(abi::st_f32(&stack, 1).1.value, 7.5f32.to_bits());
    }
}

/// An unordered pair of cluster IDs addresses one byte in the BSP's
/// upper-triangular table, with the diagonal omitted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClusterPair {
    pub upper: i32,
    pub skipped: i32,
    pub row_bias: i32,
}

impl ClusterPair {
    pub fn new(a: i32, b: i32, count: i32) -> Self {
        let (lower, upper) = (a.min(b), a.max(b));
        let skipped = lower.wrapping_add(1).wrapping_mul(lower) / 2;
        let row_bias = count
            .wrapping_sub(1)
            .wrapping_mul(lower)
            .wrapping_sub(skipped);
        Self {
            upper,
            skipped,
            row_bias,
        }
    }
}

#[cfg(test)]
mod distance_tests {
    use super::*;

    #[test]
    fn triangular_storage_visits_each_distinct_pair_once() {
        for count in 2..33 {
            let mut offset = 0;
            for first in 0..count {
                for second in first + 1..count {
                    let p = ClusterPair::new(first, second, count);
                    assert_eq!(p, ClusterPair::new(second, first, count));
                    assert_eq!(p.upper - 1 + p.row_bias, offset);
                    offset += 1;
                }
            }
        }
    }
}

/// Fractions along the listener-to-source ray and the reverse ray where
/// solid geometry first intercepts each one.
#[derive(Clone, Copy)]
pub struct ObstructionSpan {
    pub forward: u32,
    pub reverse: u32,
}

impl ObstructionSpan {
    /// Thickness divided by segment length. Degenerate or unordered hits
    /// leave the cached contact points alone.
    pub fn depth(self, mxcsr: &mut u32, unit: u32) -> (u32, u32, bool) {
        let clear = simd::subss(mxcsr, u128::from(unit), self.reverse).expect("masked SSE");
        let raw = simd::subss(mxcsr, clear, self.forward).expect("masked SSE") as u32;
        let above = |flags: u32| flags & 0x41 == 0;
        let compare = |m: &mut u32, a, b| simd::comiss(m, a, b).expect("masked SSE");
        if above(compare(mxcsr, 0, raw)) {
            (raw, 0, false)
        } else if above(compare(mxcsr, raw, unit)) {
            (raw, unit, true)
        } else {
            (raw, raw, above(compare(mxcsr, raw, 0)))
        }
    }
}

#[cfg(test)]
mod ray_tests {
    use super::*;

    #[test]
    fn valid_overlapping_and_reversed_ray_contacts() {
        for (a, b, d, write) in [
            (0.25f32, 0.5f32, 0.25f32, true),
            (0.375, 0.625, 0., false),
            (0.75, 0.75, 0., false),
            (-0.75, 0., 1., true),
        ] {
            let span = ObstructionSpan {
                forward: a.to_bits(),
                reverse: b.to_bits(),
            };
            let (_, depth, points) = span.depth(&mut 0x1f80, 1.0f32.to_bits());
            assert_eq!((depth, points), (d.to_bits(), write));
        }
    }
}

/// Convert a portal distance to obstruction with the mixer's reciprocal
/// curve. All intermediate operations use the current x87 precision.
pub fn portal_obstruction(stack: &mut X87Abi, distance: u32, unit: u32, offset: u64) -> u32 {
    abi::ld_f32(stack, distance);
    abi::arith(stack, arith_op::ADD, 0, src_kind::F32, unit.into());
    abi::arith(stack, arith_op::DIVR, 0, src_kind::F32, unit.into());
    abi::arith(stack, arith_op::SUBR, 0, src_kind::F64, offset);
    let (_, result) = abi::st_f32(stack, 0);
    abi::store_commit(stack, result.status, true);
    result.value
}
