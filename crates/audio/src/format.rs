//! Wave formats DirectSound on Xbox accepts, and decoding to float.
//!
//! The Xbox `WAVEFORMATEX` handled by `DirectSoundCreateBuffer` carries
//! `WAVE_FORMAT_PCM` (0x0001, 8 or 16 bit) or `WAVE_FORMAT_XBOX_ADPCM`
//! (0x0069). Both are mono or stereo. (Multichannel and WMA go through other
//! paths, see `docs/audio.md`.)

use crate::{AudioError, adpcm};

/// WAVE format tag for integer PCM.
pub const WAVE_FORMAT_PCM: u16 = 0x0001;
/// WAVE format tag for Xbox ADPCM.
pub const WAVE_FORMAT_XBOX_ADPCM: u16 = 0x0069;

/// Sample encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Codec {
    /// Unsigned 8-bit PCM, silence is 0x80.
    Pcm8,
    /// Signed 16-bit little-endian PCM.
    Pcm16,
    /// Xbox ADPCM, see [`crate::adpcm`].
    XboxAdpcm,
}

/// Typed stand-in for `WAVEFORMATEX`.
///
/// Always valid: fields are private and every constructor validates, so
/// `block_align` and friends never see zero channels or a zero rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WaveFormat {
    codec: Codec,
    channels: u16,
    sample_rate: u32,
}

impl WaveFormat {
    /// `channels` must be 1 or 2 and `sample_rate` non-zero.
    pub fn new(codec: Codec, channels: u16, sample_rate: u32) -> Result<Self, AudioError> {
        if !(1..=2).contains(&channels) {
            return Err(AudioError::UnsupportedFormat("only mono and stereo"));
        }
        if sample_rate == 0 {
            return Err(AudioError::UnsupportedFormat("zero sample rate"));
        }
        Ok(Self { codec, channels, sample_rate })
    }

    pub fn pcm16(channels: u16, sample_rate: u32) -> Result<Self, AudioError> {
        Self::new(Codec::Pcm16, channels, sample_rate)
    }

    pub fn pcm8(channels: u16, sample_rate: u32) -> Result<Self, AudioError> {
        Self::new(Codec::Pcm8, channels, sample_rate)
    }

    pub fn xbox_adpcm(channels: u16, sample_rate: u32) -> Result<Self, AudioError> {
        Self::new(Codec::XboxAdpcm, channels, sample_rate)
    }

    pub const fn codec(&self) -> Codec {
        self.codec
    }
    /// 1 or 2.
    pub const fn channels(&self) -> u16 {
        self.channels
    }
    /// Frames per second, non-zero.
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Builds from the raw `WAVEFORMATEX` fields a guest passes in.
    pub fn from_wfx(
        format_tag: u16,
        channels: u16,
        sample_rate: u32,
        bits_per_sample: u16,
    ) -> Result<Self, AudioError> {
        let codec = match (format_tag, bits_per_sample) {
            (WAVE_FORMAT_PCM, 8) => Codec::Pcm8,
            (WAVE_FORMAT_PCM, 16) => Codec::Pcm16,
            (WAVE_FORMAT_XBOX_ADPCM, _) => Codec::XboxAdpcm,
            (WAVE_FORMAT_PCM, _) => {
                return Err(AudioError::UnsupportedFormat("PCM must be 8 or 16 bit"));
            }
            _ => return Err(AudioError::UnsupportedFormat("unknown wave format tag")),
        };
        Self::new(codec, channels, sample_rate)
    }

    /// Bytes per frame for PCM, or per block (all channels) for ADPCM.
    pub const fn block_align(&self) -> usize {
        match self.codec {
            Codec::Pcm8 => self.channels as usize,
            Codec::Pcm16 => 2 * self.channels as usize,
            Codec::XboxAdpcm => adpcm::BLOCK_BYTES_PER_CHANNEL * self.channels as usize,
        }
    }

    /// Frames each `block_align` bytes decode to.
    pub const fn frames_per_block(&self) -> usize {
        match self.codec {
            Codec::XboxAdpcm => adpcm::SAMPLES_PER_BLOCK,
            _ => 1,
        }
    }

    /// Frames in `bytes` of data (partial blocks are not counted).
    pub const fn frames_in(&self, bytes: usize) -> usize {
        bytes / self.block_align() * self.frames_per_block()
    }

    /// Decodes `data` to interleaved `f32` in -1.0..1.0. `data` must be a
    /// whole number of blocks.
    pub fn decode(&self, data: &[u8]) -> Result<Vec<f32>, AudioError> {
        if data.len() % self.block_align() != 0 {
            return Err(AudioError::BadLength {
                len: data.len(),
                unit: self.block_align(),
                what: "frame or block",
            });
        }
        Ok(match self.codec {
            Codec::Pcm8 => data.iter().map(|&b| (b as f32 - 128.0) / 128.0).collect(),
            Codec::Pcm16 => data
                .chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
                .collect(),
            Codec::XboxAdpcm => adpcm::decode(data, self.channels as usize)?
                .into_iter()
                .map(|s| s as f32 / 32768.0)
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm8_is_unsigned_centered() {
        let f = WaveFormat::pcm8(1, 22050).unwrap();
        let d = f.decode(&[0, 128, 255]).unwrap();
        assert_eq!(d[0], -1.0);
        assert_eq!(d[1], 0.0);
        assert!((d[2] - 127.0 / 128.0).abs() < 1e-6);
    }

    #[test]
    fn pcm16_stereo_is_interleaved() {
        let f = WaveFormat::pcm16(2, 48000).unwrap();
        let mut bytes = Vec::new();
        for s in [i16::MIN, 16384, 0, i16::MAX] {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        let d = f.decode(&bytes).unwrap();
        assert_eq!(d, vec![-1.0, 0.5, 0.0, 32767.0 / 32768.0]);
        assert_eq!(f.frames_in(bytes.len()), 2);
    }

    #[test]
    fn adpcm_frame_counts() {
        let f = WaveFormat::xbox_adpcm(2, 44100).unwrap();
        assert_eq!(f.block_align(), 72);
        assert_eq!(f.frames_in(72 * 3), 192);
        assert_eq!(f.decode(&vec![0u8; 144]).unwrap().len(), 128 * 2);
    }

    #[test]
    fn rejects_partial_and_bad_formats() {
        assert!(WaveFormat::pcm16(2, 48000).unwrap().decode(&[0; 3]).is_err());
        // Invalid formats cannot be constructed, so no divide-by-zero later.
        assert!(WaveFormat::pcm16(3, 48000).is_err());
        assert!(WaveFormat::pcm16(0, 8000).is_err());
        assert!(WaveFormat::pcm16(1, 0).is_err());
        assert!(WaveFormat::new(Codec::XboxAdpcm, 0, 8000).is_err());
        assert!(WaveFormat::from_wfx(1, 0, 8000, 16).is_err());
        assert!(WaveFormat::from_wfx(1, 1, 8000, 24).is_err());
        assert!(WaveFormat::from_wfx(0x55, 1, 8000, 0).is_err());
        assert_eq!(
            WaveFormat::from_wfx(0x69, 2, 44100, 4).unwrap(),
            WaveFormat::xbox_adpcm(2, 44100).unwrap()
        );
        assert_eq!(
            WaveFormat::from_wfx(1, 1, 8000, 8).unwrap().codec(),
            Codec::Pcm8
        );
    }
}
