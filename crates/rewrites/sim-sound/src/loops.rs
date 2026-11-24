//! The live instances of a loop track form a ring. The head (including its
//! datum salt) marks the end of a lap. A newly allocated track can also
//! carry the empty datum in its link.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrackRing {
    head: u32,
}

impl TrackRing {
    pub const EMPTY: u32 = u32::MAX;

    pub fn new(head: u32) -> Self {
        Self { head }
    }

    pub fn first(self) -> Option<u32> {
        (self.head != Self::EMPTY).then_some(self.head)
    }

    pub fn following(self, next: u32) -> Option<u32> {
        (next != Self::EMPTY && next != self.head).then_some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lap_ends_at_the_original_salted_datum_or_an_empty_link() {
        let ring = TrackRing::new(0xabcd0003);
        assert_eq!(TrackRing::new(TrackRing::EMPTY).first(), None);
        assert_eq!(ring.first(), Some(0xabcd0003));
        assert_eq!(ring.following(0xabce0003), Some(0xabce0003));
        assert_eq!(ring.following(TrackRing::EMPTY), None);
        assert_eq!(ring.following(0xabcd0003), None);
    }
}

/// Place the next detail impulse on the sound clock. The interval is sampled
/// between the tag's bounds, in extended precision through the addition of
/// the current clock. The caller uses the guest's integer conversion.
pub fn detail_deadline(
    fpu: &mut x87::abi::X87Abi,
    interval: [u32; 2],
    draw: u16,
    draw_scale: u32,
    clock_scale: u32,
    now: i32,
) {
    use x87::abi::{self as fp, arith_op as op, src_kind as src};
    fp::ld_f32(fpu, interval[1]);
    fp::ld_f32(fpu, interval[0]);
    fp::ld_int(fpu, i64::from(draw));
    fp::arith(fpu, op::MUL, 0, src::F32, draw_scale.into());
    fp::xch(fpu, 2);
    fp::arith(fpu, op::SUB, 0, src::ST, 1);
    fp::arith(fpu, op::MUL | fp::ARITH_POP, 2, src::ST, 0);
    fp::xch(fpu, 1);
    fp::arith(fpu, op::ADD, 0, src::ST, 1);
    fp::arith(fpu, op::MUL, 0, src::F32, clock_scale.into());
    fp::arith(fpu, op::ADD, 0, src::I32, u64::from(now as u32));
}
