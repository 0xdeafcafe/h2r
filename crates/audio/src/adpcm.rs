//! Xbox ADPCM decoder.
//!
//! Xbox ADPCM (WAVE format tag `0x0069`, "XBOX ADPCM") is Microsoft's IMA
//! ADPCM with a fixed block size. Public descriptions used here:
//!
//! - xboxdevwiki, "Xbox ADPCM":
//!   <https://xboxdevwiki.net/Xbox_ADPCM>
//! - The IMA/DVI ADPCM step and index tables, and the MS IMA block layout
//!   (the layout Xbox ADPCM follows): Microsoft's IMA ADPCM WAVE description,
//!   as documented at the Multimedia Wiki,
//!   <https://wiki.multimedia.cx/index.php/Microsoft_IMA_ADPCM>
//!   (the tables are the original IMA Digital Audio Focus Group ones).
//!
//! Layout, per block of `36 * channels` bytes:
//!
//! ```text
//! for each channel:  i16 le  predictor (also the block's first sample)
//!                    u8      step index (0..=88)
//!                    u8      reserved
//! then 8 groups, each: 4 data bytes for ch0, 4 for ch1, ...   (8 nibbles each)
//! ```
//!
//! Within a byte the low nibble is the earlier sample. A channel therefore
//! carries 32 data bytes = 64 nibbles per block.
//!
//! # 64 samples per block
//!
//! The header predictor is itself a sample, so a generic IMA reading gives
//! 1 + 64 = 65 samples per block. Xbox ADPCM yields 64: the header sample
//! plus 63 nibbles, with the final high nibble discarded. This was confirmed
//! by review against vgmstream's `ima_decoder.c` and xemu's
//! `hw/xbox/mcpx/apu/vp/vp.c` (xemu pinned at 4bdad199); the xboxdevwiki's 65
//! describes the generic expansion. [`SamplesPerBlock::Hardware64`] is the
//! default. [`SamplesPerBlock::Ima65`] is *not* Xbox behaviour and exists
//! only to compare against generic IMA tools.

use crate::AudioError;

/// Bytes in one channel's block.
pub const BLOCK_BYTES_PER_CHANNEL: usize = 36;
/// Samples one channel's block decodes to under [`SamplesPerBlock::Hardware64`].
pub const SAMPLES_PER_BLOCK: usize = 64;

const HEADER_BYTES: usize = 4;
const GROUP_BYTES: usize = 4;
const GROUPS: usize = 8;

/// How many samples a block yields; see the module docs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SamplesPerBlock {
    /// Header sample plus 63 nibbles (64 total). What the hardware plays.
    #[default]
    Hardware64,
    /// Non-Xbox: header sample plus all 64 nibbles (65 total), as generic IMA
    /// decoders do. For comparison with other tools only.
    Ima65,
}

impl SamplesPerBlock {
    pub const fn count(self) -> usize {
        match self {
            Self::Hardware64 => 64,
            Self::Ima65 => 65,
        }
    }
}

const STEP_TABLE: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408,
    449, 494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066,
    2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630,
    9493, 10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794,
    32767,
];

const INDEX_TABLE: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];

/// Running state of one channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ChannelState {
    predictor: i32,
    index: i32,
}

impl ChannelState {
    fn step(&mut self, nibble: u8) -> i16 {
        let step = STEP_TABLE[self.index as usize];
        let mut diff = step >> 3;
        if nibble & 1 != 0 {
            diff += step >> 2;
        }
        if nibble & 2 != 0 {
            diff += step >> 1;
        }
        if nibble & 4 != 0 {
            diff += step;
        }
        if nibble & 8 != 0 {
            diff = -diff;
        }
        self.predictor = (self.predictor + diff).clamp(i16::MIN as i32, i16::MAX as i32);
        self.index = (self.index + INDEX_TABLE[(nibble & 7) as usize]).clamp(0, 88);
        self.predictor as i16
    }
}

/// Decodes whole blocks to interleaved 16-bit PCM using [`SamplesPerBlock::Hardware64`].
pub fn decode(data: &[u8], channels: usize) -> Result<Vec<i16>, AudioError> {
    decode_with(data, channels, SamplesPerBlock::default())
}

/// Decodes `data`, which must be a whole number of `36 * channels`-byte
/// blocks, to interleaved 16-bit PCM. Only 1 or 2 channels exist on Xbox.
pub fn decode_with(
    data: &[u8],
    channels: usize,
    per_block: SamplesPerBlock,
) -> Result<Vec<i16>, AudioError> {
    if !(1..=2).contains(&channels) {
        return Err(AudioError::UnsupportedFormat("Xbox ADPCM is mono or stereo"));
    }
    let block_bytes = BLOCK_BYTES_PER_CHANNEL * channels;
    if data.len() % block_bytes != 0 {
        return Err(AudioError::BadLength {
            len: data.len(),
            unit: block_bytes,
            what: "ADPCM block",
        });
    }
    let n = per_block.count();
    let mut out = Vec::with_capacity(data.len() / block_bytes * n * channels);
    let mut cols = vec![[0i16; 65]; channels];
    for block in data.chunks_exact(block_bytes) {
        decode_block(block, channels, &mut cols);
        for i in 0..n {
            for col in &cols {
                out.push(col[i]);
            }
        }
    }
    Ok(out)
}

/// Decodes one block into per-channel columns of 65 samples.
fn decode_block(block: &[u8], channels: usize, cols: &mut [[i16; 65]]) {
    let mut states = [ChannelState { predictor: 0, index: 0 }; 2];
    for (ch, st) in states.iter_mut().enumerate().take(channels) {
        let h = &block[ch * HEADER_BYTES..][..HEADER_BYTES];
        st.predictor = i16::from_le_bytes([h[0], h[1]]) as i32;
        // A corrupt index is clamped rather than rejected; the hardware has no
        // error path either.
        st.index = (h[2] as i32).min(88);
        cols[ch][0] = st.predictor as i16;
    }
    let body = &block[channels * HEADER_BYTES..];
    for group in 0..GROUPS {
        for ch in 0..channels {
            let off = (group * channels + ch) * GROUP_BYTES;
            for (b, &byte) in body[off..off + GROUP_BYTES].iter().enumerate() {
                let s = 1 + group * 8 + b * 2;
                cols[ch][s] = states[ch].step(byte & 0x0f);
                cols[ch][s + 1] = states[ch].step(byte >> 4);
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Test-only IMA encoder producing Xbox block layout. Not shipped: the
    /// game only ever decodes.
    pub fn encode(pcm: &[i16], channels: usize) -> Vec<u8> {
        let per_ch = pcm.len() / channels;
        assert_eq!(per_ch % SAMPLES_PER_BLOCK, 0, "pad input to whole blocks");
        let mut out = Vec::new();
        let mut index = [0i32; 2];
        for blk in 0..per_ch / SAMPLES_PER_BLOCK {
            // Block b of channel c covers samples [b*64 .. b*64+64) with the
            // 65th decode slot unused; header sample is sample 0.
            let base = blk * SAMPLES_PER_BLOCK;
            let mut codes = vec![[0u8; 64]; channels];
            for ch in 0..channels {
                let s = |i: usize| pcm[(base + i) * channels + ch] as i32;
                let mut st = ChannelState { predictor: s(0), index: index[ch] };
                out.extend_from_slice(&(s(0) as i16).to_le_bytes());
                out.push(st.index as u8);
                out.push(0);
                for i in 1..64 {
                    let code = encode_nibble(&st, s(i));
                    st.step(code);
                    codes[ch][i - 1] = code;
                }
                // Slot 64 is padding; use a zero-delta code.
                codes[ch][63] = 0;
                index[ch] = st.index;
            }
            for group in 0..GROUPS {
                for codes in codes.iter().take(channels) {
                    for b in 0..4 {
                        let i = group * 8 + b * 2;
                        out.push(codes[i] | (codes[i + 1] << 4));
                    }
                }
            }
        }
        out
    }

    fn encode_nibble(st: &ChannelState, target: i32) -> u8 {
        let step = STEP_TABLE[st.index as usize];
        let mut diff = target - st.predictor;
        let mut code = 0u8;
        if diff < 0 {
            code = 8;
            diff = -diff;
        }
        if diff >= step {
            code |= 4;
            diff -= step;
        }
        if diff >= step >> 1 {
            code |= 2;
            diff -= step >> 1;
        }
        if diff >= step >> 2 {
            code |= 1;
        }
        code
    }

    #[test]
    fn hand_computed_block() {
        // predictor 1000, index 0 (step 7), then nibbles 0x7 and 0xF.
        // 0x7: diff = 0 + 1 + 3 + 7 = 7+... compute with step 7:
        //   step>>3 = 0, +step>>2 = 1, +step>>1 = 3, +step = 7  => 11.
        //   predictor 1011; index 0+8 = 8 (step 16).
        // 0xF: sign set, bits 111, step 16: 2 + 4 + 8 + 16 = 30 => -30.
        //   predictor 981; index 8+8 = 16.
        let mut block = vec![0u8; 36];
        block[..2].copy_from_slice(&1000i16.to_le_bytes());
        block[4] = 0xF7; // low nibble first
        let pcm = decode_with(&block, 1, SamplesPerBlock::Ima65).unwrap();
        assert_eq!(pcm.len(), 65);
        assert_eq!(&pcm[..3], &[1000, 1011, 981]);
        // The remaining zero nibbles each add step>>3 with index sinking.
        assert!(pcm[3] > 981);
    }

    #[test]
    fn silence_stays_near_zero() {
        // Zero predictor, zero codes: each step adds step>>3 = 0 at index 0
        // (7>>3 = 0), and the index stays clamped at 0.
        let block = vec![0u8; 36];
        assert_eq!(decode(&block, 1).unwrap(), vec![0i16; 64]);
    }

    #[test]
    fn hardware64_is_prefix_of_ima65() {
        let block: Vec<u8> = (0..36u8).map(|i| i.wrapping_mul(37)).collect();
        let a = decode_with(&block, 1, SamplesPerBlock::Hardware64).unwrap();
        let b = decode_with(&block, 1, SamplesPerBlock::Ima65).unwrap();
        assert_eq!(a, b[..64]);
    }

    #[test]
    fn mono_roundtrip_is_close() {
        let pcm: Vec<i16> = (0..256)
            .map(|i| ((i as f32 * 0.15).sin() * 12000.0) as i16)
            .collect();
        let enc = encode(&pcm, 1);
        assert_eq!(enc.len(), 4 * 36);
        let dec = decode(&enc, 1).unwrap();
        assert_eq!(dec.len(), pcm.len());
        for (i, (&a, &b)) in pcm.iter().zip(&dec).enumerate() {
            // The step index starts at 0 and climbs at most 8 per sample, so
            // the first few samples slew-limit; skip that warm-up.
            if i < 16 {
                continue;
            }
            let tol = 1500;
            assert!((a as i32 - b as i32).abs() <= tol, "sample {i}: {a} vs {b}");
        }
        // First sample of every block is exact (it is the header).
        for blk in 0..4 {
            assert_eq!(dec[blk * 64], pcm[blk * 64]);
        }
    }

    #[test]
    fn stereo_interleave_keeps_channels_apart() {
        // Left: slow ramp up. Right: constant -5000. Distinct enough that a
        // channel swap or wrong group interleave would show up.
        let mut pcm = Vec::new();
        for i in 0..128 {
            pcm.push((i * 40) as i16);
            pcm.push(-5000);
        }
        let enc = encode(&pcm, 2);
        assert_eq!(enc.len(), 2 * 72);
        let dec = decode(&enc, 2).unwrap();
        assert_eq!(dec.len(), pcm.len());
        for blk in 0..2 {
            let i = blk * 64;
            assert_eq!(dec[i * 2], pcm[i * 2]);
            assert_eq!(dec[i * 2 + 1], -5000);
        }
        for f in 0..128 {
            assert!((dec[f * 2 + 1] as i32 + 5000).abs() < 1200, "R frame {f}");
            assert!((dec[f * 2] as i32 - pcm[f * 2] as i32).abs() < 1500, "L frame {f}");
        }
    }

    #[test]
    fn stereo_group_layout_is_4_bytes_per_channel() {
        // Header: L predictor 0, R predictor 0. Put one nonzero nibble in the
        // very first right-channel data byte (offset 8 + 4) and check it is
        // sample 1 of the right channel only.
        let mut block = vec![0u8; 72];
        block[8 + 4] = 0x07; // R group 0, byte 0, low nibble
        let pcm = decode_with(&block, 2, SamplesPerBlock::Ima65).unwrap();
        assert_eq!(pcm[2], 0); // L sample 1
        assert_ne!(pcm[3], 0); // R sample 1
    }

    #[test]
    fn rejects_bad_input() {
        assert!(matches!(decode(&[0; 35], 1), Err(AudioError::BadLength { .. })));
        assert!(matches!(decode(&[0; 36], 2), Err(AudioError::BadLength { .. })));
        assert!(decode(&[0; 36], 3).is_err());
        assert_eq!(decode(&[], 1).unwrap(), Vec::<i16>::new());
    }

    #[test]
    fn extreme_codes_clamp() {
        // Max predictor, all-positive max codes must saturate not wrap.
        let mut block = vec![0x77u8; 36];
        block[..2].copy_from_slice(&i16::MAX.to_le_bytes());
        block[2] = 88;
        let pcm = decode(&block, 1).unwrap();
        assert!(pcm.iter().all(|&s| s == i16::MAX));
        // Corrupt step index is clamped, not a panic.
        block[2] = 255;
        decode(&block, 1).unwrap();
    }
}
