//! 48 kHz to and from the packet rate, for rates that divide 48 kHz
//! (8, 16 and 24 kHz; the engine asks for 16). Down is a box average over
//! each output sample's span, so it does not alias the way plain
//! decimation would; up is linear between samples, carrying the last
//! sample of the previous frame so frame edges do not click.

use crate::{HOST_FRAME, HOST_RATE};

/// The integer factor between 48 kHz and `rate`, or `None` if `rate` does
/// not divide 48 kHz.
pub fn factor(rate: u32) -> Option<usize> {
    (rate != 0 && HOST_RATE.is_multiple_of(rate) && rate <= HOST_RATE)
        .then(|| (HOST_RATE / rate) as usize)
}

/// Samples of `rate` in one 20 ms frame.
pub fn frame_len(rate: u32) -> usize {
    (rate / 50) as usize
}

/// One host frame down to `out.len() == HOST_FRAME / factor` samples.
pub fn down(input: &[i16; HOST_FRAME], factor: usize, out: &mut [i16]) {
    debug_assert_eq!(out.len() * factor, HOST_FRAME);
    for (o, span) in out.iter_mut().zip(input.chunks_exact(factor)) {
        let sum: i32 = span.iter().map(|&s| i32::from(s)).sum();
        *o = (sum / factor as i32) as i16;
    }
}

/// Linear up-sampling with the previous frame's last sample carried.
#[derive(Clone, Debug, Default)]
pub struct Up {
    last: i16,
}

impl Up {
    /// `input` (one frame at the packet rate) up by `factor` into `out`.
    pub fn run(&mut self, input: &[i16], factor: usize, out: &mut [i16; HOST_FRAME]) {
        debug_assert_eq!(input.len() * factor, HOST_FRAME);
        let mut previous = i32::from(self.last);
        for (chunk, &next) in out.chunks_exact_mut(factor).zip(input) {
            let next = i32::from(next);
            for (k, o) in chunk.iter_mut().enumerate() {
                let k = k as i32 + 1;
                *o = (previous + (next - previous) * k / factor as i32) as i16;
            }
            previous = next;
        }
        self.last = previous as i16;
    }
}
