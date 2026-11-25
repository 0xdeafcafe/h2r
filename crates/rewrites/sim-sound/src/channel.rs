//! Pitch and gain inputs for one mixer channel. The guest rounds each
//! intermediate to binary32 and leaves any SSE exceptions sticky in MXCSR.

#[derive(Clone, Copy, Debug)]
pub struct ChannelLevels {
    pub pitch_range: [i16; 2],
    pub fraction: u32,
    pub pitch_offset: u32,
    pub gain: u32,
    pub gain_offset: u32,
    pub gain_delta: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EvaluatedLevels {
    pub pitch: u32,
    pub gain_offset: u32,
    pub gain: u32,
}

impl ChannelLevels {
    /// The caller supplies an MXCSR with masked exceptions, as for the
    /// simulation's other scalar SSE kernels.
    pub fn evaluate(self, mxcsr: &mut u32) -> EvaluatedLevels {
        let [low, high] = self.pitch_range.map(|v| u128::from(f32::from(v).to_bits()));
        let width = simd::subss(mxcsr, high, low as u32).expect("masked SSE");
        let scaled = simd::mulss(mxcsr, width, self.fraction).expect("masked SSE");
        let offset = simd::addss(mxcsr, scaled, self.pitch_offset).expect("masked SSE");
        let pitch = simd::addss(mxcsr, offset, low as u32).expect("masked SSE");
        let gain_offset = simd::addss(mxcsr, self.gain_offset.into(), self.gain_delta)
            .expect("masked SSE");
        let gain = simd::addss(mxcsr, self.gain.into(), gain_offset as u32).expect("masked SSE");
        EvaluatedLevels { pitch: pitch as u32, gain_offset: gain_offset as u32, gain: gain as u32 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolates_pitch_and_adds_gain_in_guest_order() {
        let input = ChannelLevels {
            pitch_range: [-10, 6], fraction: 0.25_f32.to_bits(),
            pitch_offset: 3_f32.to_bits(), gain: (-16_777_216_f32).to_bits(),
            gain_offset: 16_777_216_f32.to_bits(), gain_delta: 1_f32.to_bits(),
        };
        let mut mxcsr = 0x1f80;
        let levels = input.evaluate(&mut mxcsr);
        assert_eq!(levels.pitch, (-3.0_f32).to_bits());
        assert_eq!(levels.gain, 0);
        assert_ne!(mxcsr & 0x20, 0);
    }
}
