//! Playback decisions between animation manager callbacks. The engine reads
//! a fresh channel on each resume. Only the decision and its arithmetic
//! effects live here. All SSE exceptions must be masked on entry.

/// The interval to dispatch when advancing a looping clip past its end.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WrappedInterval {
    pub start: u32,
    pub end: u32,
}

impl WrappedInterval {
    /// Advance from the loop frame by the excess over the outgoing clip's
    /// end. Each scalar operation observes the guest rounding mode.
    pub fn new(target: u32, clip_end: u32, loop_frame: i16, mxcsr: &mut u32) -> Self {
        let excess = simd::subss(mxcsr, target.into(), clip_end).expect("masked SSE");
        // Every signed 16-bit frame is exactly representable.
        let start = sim_core::det::from_int(i32::from(loop_frame));
        let end = simd::addss(mxcsr, excess, start).expect("masked SSE") as u32;
        Self { start, end }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mix {
    Primary,
    Secondary,
    Both,
}

/// Selection for a bound secondary channel. The final comparison's
/// left operand and result survive into the next sampling or motion call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChannelMix {
    pub channels: Mix,
    pub compared: u32,
    pub flags: u32,
}

impl ChannelMix {
    /// Read the lower threshold only for a weight below full or unordered.
    /// Unordered comparisons set carry at each branch and select both channels.
    pub fn select<E>(
        weight: u32,
        full: u32,
        lower_bound: impl FnOnce() -> Result<u32, E>,
        mxcsr: &mut u32,
    ) -> Result<Self, E> {
        let high = simd::comiss(mxcsr, weight, full).expect("masked SSE");
        if high & 1 == 0 {
            return Ok(Self {
                channels: Mix::Secondary,
                compared: weight,
                flags: high,
            });
        }
        let small = lower_bound()?;
        let low = simd::comiss(mxcsr, small, weight).expect("masked SSE");
        if low & 1 == 0 {
            return Ok(Self {
                channels: Mix::Primary,
                compared: small,
                flags: low,
            });
        }
        // The loop tests weight against full again. Its flags and operands
        // are identical to the first comparison. Both sticky effects stand.
        Ok(Self {
            channels: Mix::Both,
            compared: weight,
            flags: high,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlaybackRate {
    Stopped,
    Unit,
    Other,
}

/// The base-pose sampler preserves its requested interpolation for a stopped
/// channel, checks the game clock at unit speed, and forces interpolation
/// otherwise. Return the final UCOMISS flags for its LAHF/TEST sequence.
pub fn playback_rate<E>(
    speed: u32,
    zero: u32,
    unit: impl FnOnce() -> Result<u32, E>,
    mxcsr: &mut u32,
) -> Result<(PlaybackRate, u32), E> {
    let stopped = simd::ucomiss(mxcsr, speed, zero).expect("masked SSE");
    if stopped == 0x40 {
        return Ok((PlaybackRate::Stopped, stopped));
    }
    let normal = simd::ucomiss(mxcsr, speed, unit()?).expect("masked SSE");
    Ok((
        if normal == 0x40 {
            PlaybackRate::Unit
        } else {
            PlaybackRate::Other
        },
        normal,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secondary_mix_matches_the_three_guest_comparisons() {
        let weights = [
            0.0f32.to_bits(),
            (-0.0f32).to_bits(),
            0.25f32.to_bits(),
            0.5f32.to_bits(),
            0.75f32.to_bits(),
            1.0f32.to_bits(),
            1,
            f32::INFINITY.to_bits(),
            f32::NAN.to_bits(),
            0x7f80_0011,
        ];
        for weight in weights {
            for full in [0.75f32.to_bits(), f32::NAN.to_bits()] {
                for small in [0.25f32.to_bits(), f32::NAN.to_bits()] {
                    let mut reference = 0x1f80;
                    let mut flags = simd::comiss(&mut reference, weight, full).unwrap();
                    let mut compared = weight;
                    let mut consulted_small = false;
                    let channels = if flags & 1 == 0 {
                        Mix::Secondary
                    } else {
                        consulted_small = true;
                        compared = small;
                        flags = simd::comiss(&mut reference, small, weight).unwrap();
                        if flags & 1 == 0 {
                            Mix::Primary
                        } else {
                            compared = weight;
                            flags = simd::comiss(&mut reference, weight, full).unwrap();
                            if flags & 1 != 0 {
                                Mix::Both
                            } else {
                                Mix::Primary
                            }
                        }
                    };
                    let mut actual = 0x1f80;
                    let mut read_small = false;
                    let mix = ChannelMix::select(
                        weight,
                        full,
                        || {
                            read_small = true;
                            Ok::<_, ()>(small)
                        },
                        &mut actual,
                    )
                    .unwrap();
                    assert_eq!(
                        mix,
                        ChannelMix {
                            channels,
                            compared,
                            flags
                        }
                    );
                    assert_eq!((read_small, actual), (consulted_small, reference));
                }
            }
        }
    }

    #[test]
    fn rate_preserves_signed_zero_and_nan_comparison_effects() {
        for (speed, mode, flags, invalid) in [
            ((-0.0f32).to_bits(), PlaybackRate::Stopped, 0x40, 0),
            (1.0f32.to_bits(), PlaybackRate::Unit, 0x40, 0),
            (0.75f32.to_bits(), PlaybackRate::Other, 1, 0),
            (f32::NAN.to_bits(), PlaybackRate::Other, 0x45, 0),
            (0x7f80_0011, PlaybackRate::Other, 0x45, 1),
        ] {
            let mut mxcsr = 0x1f80;
            let result = playback_rate(
                speed,
                0,
                || {
                    assert_ne!(mode, PlaybackRate::Stopped);
                    Ok::<_, ()>(1.0f32.to_bits())
                },
                &mut mxcsr,
            )
            .unwrap();
            assert_eq!(result, (mode, flags));
            assert_eq!(mxcsr & 1, invalid);
        }
    }

    #[test]
    fn wrapped_interval_preserves_rounding_and_signed_loop_frame() {
        for round in [0, 1, 2, 3] {
            let mut mxcsr = 0x1f80 | (round << 13);
            let mut reference = mxcsr;
            let target = 13.25f32.to_bits();
            let end = 1.0e-8f32.to_bits();
            let expected_start = simd::cvtsi2ss(&mut reference, 0, (-7i32) as u32).unwrap() as u32;
            let excess = simd::subss(&mut reference, target.into(), end).unwrap();
            let expected_end = simd::addss(&mut reference, excess, expected_start).unwrap() as u32;
            let interval = WrappedInterval::new(target, end, -7, &mut mxcsr);
            assert_eq!(
                (interval.start, interval.end, mxcsr),
                (expected_start, expected_end, reference)
            );
        }
    }
}
