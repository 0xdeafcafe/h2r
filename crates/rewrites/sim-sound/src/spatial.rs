//! Channel routing and the timestamp of a transition between direct and
//! submix output. The clock and fade interval are in milliseconds.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceSpace {
    Plain,
    World,
    Listener,
    Other(i8),
}

impl SourceSpace {
    pub fn decode(packed: u8) -> Self {
        let kind = ((packed << 4) as i8) >> 4;
        match kind {
            0 => Self::Plain,
            1 => Self::World,
            2 => Self::Listener,
            value => Self::Other(value),
        }
    }

    pub fn kind(self) -> i8 {
        match self {
            Self::Plain => 0,
            Self::World => 1,
            Self::Listener => 2,
            Self::Other(value) => value,
        }
    }
}

/// When routing changes in the middle of a fade, reflect the remaining
/// time into the new ramp. The guest clock is a wrapping signed dword.
pub fn transition_start(previous: i32, clock: i32, interval: i32) -> i32 {
    let age = interval.wrapping_sub(previous).wrapping_sub(clock);
    clock.wrapping_sub(age.max(0).min(interval))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_above_the_low_nibble_do_not_select_a_space() {
        for flags in 0..16 {
            for (nibble, kind) in [(0, 0), (1, 1), (2, 2), (3, 3), (8, -8), (15, -1)] {
                assert_eq!(SourceSpace::decode(flags << 4 | nibble).kind(), kind);
            }
        }
    }

    #[test]
    fn direction_change_clamps_or_reflects_the_transition_time() {
        assert_eq!(transition_start(10, 70, 100), 50);
        assert_eq!(transition_start(40, 70, 100), 70);
        assert_eq!(transition_start(-90, 70, 100), -30);
        assert_eq!(transition_start(i32::MIN, i32::MIN, 100), i32::MAX - 99);
    }
}

/// Endpoints of the ramp shared by the distance and cone evaluators.
#[derive(Clone, Copy)]
pub struct Ramp {
    pub start: u32,
    pub end: u32,
}

/// Results of the scalar SSE calculation. The caller can restore the
/// upper bound, span and comparison flags for other guest users of the ramp.
pub struct Fraction {
    pub value: u32,
    pub bound: u32,
    pub span: u32,
    pub flags: u32,
}

impl Ramp {
    /// Interpolate and saturate using the original operation order and
    /// NaN behavior. Nearly equal endpoints use a separate step function.
    pub fn fraction(self, input: u32, unity: u32, mxcsr: &mut u32) -> Fraction {
        let numerator = simd::subss(mxcsr, u128::from(input), self.start).expect("masked SSE");
        let span = simd::subss(mxcsr, u128::from(self.end), self.start).expect("masked SSE") as u32;
        let ratio = simd::divss(mxcsr, numerator, span).expect("masked SSE") as u32;
        let negative = simd::comiss(mxcsr, 0, ratio).expect("masked SSE");
        if negative & 0x41 == 0 {
            return Fraction { value: 0, bound: 0, span, flags: negative };
        }
        let flags = simd::comiss(mxcsr, ratio, unity).expect("masked SSE");
        let value = if flags & 0x41 == 0 { unity } else { ratio };
        Fraction { value, bound: unity, span, flags }
    }
}

#[cfg(test)]
mod ramps {
    use super::*;

    #[test]
    fn clamped_linear_ramps_can_run_in_either_direction() {
        for (start, end) in [(2.0f32, 6.0f32), (6.0, 2.0)] {
            let ramp = Ramp { start: start.to_bits(), end: end.to_bits() };
            for (fraction, expected) in [(-1.0f32, 0.0f32), (0.25, 0.25), (1.25, 1.0)] {
                let sample = start + fraction * (end - start);
                let mut mxcsr = 0x1f80;
                assert_eq!(ramp.fraction(sample.to_bits(), 1.0f32.to_bits(), &mut mxcsr).value,
                           expected.to_bits());
            }
        }
    }
}
