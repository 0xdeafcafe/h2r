//! Halo 1 samples to 16-bit PCM, with the audio crate's Xbox ADPCM decoder
//! (the one Halo 2's mixer uses), and PCM to WAV.

use crate::sounds::h1::{Compression, Sound, samples};
use crate::sounds::{Error, Result};

/// A decoded playable permutation: interleaved 16-bit PCM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcm {
    /// Hertz.
    pub rate: u32,
    /// 1 or 2.
    pub channels: usize,
    /// Interleaved samples.
    pub samples: Vec<i16>,
}

impl Pcm {
    /// Frames (samples per channel).
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels
    }

    /// Duration in seconds.
    pub fn seconds(&self) -> f64 {
        self.frames() as f64 / f64::from(self.rate)
    }

    /// A 16-bit PCM WAV file.
    pub fn wav(&self) -> Vec<u8> {
        let data = (self.samples.len() * 2) as u32;
        let ch = self.channels as u16;
        let mut w = Vec::with_capacity(44 + data as usize);
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&ch.to_le_bytes());
        w.extend_from_slice(&self.rate.to_le_bytes());
        w.extend_from_slice(&(self.rate * 2 * u32::from(ch)).to_le_bytes());
        w.extend_from_slice(&(2 * ch).to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&data.to_le_bytes());
        for s in &self.samples {
            w.extend_from_slice(&s.to_le_bytes());
        }
        w
    }
}

/// Decodes encoded bytes of `compression` with `channels` channels.
pub fn decode_bytes(compression: Compression, channels: usize, data: &[u8]) -> Result<Vec<i16>> {
    match compression {
        Compression::XboxAdpcm => Ok(audio::adpcm::decode(data, channels)?),
        Compression::None => {
            if !data.len().is_multiple_of(2 * channels) {
                return Err(Error::Invalid(format!(
                    "{} PCM bytes for {channels} channels",
                    data.len()
                )));
            }
            Ok(data
                .chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]))
                .collect())
        }
        c => Err(Error::Unsupported(format!("compression {c:?}"))),
    }
}

/// The encoded bytes of one playable permutation: the samples of each
/// permutation in its chain ([`Sound::chains`]), in order.
pub fn chain_bytes(sound: &Sound, file: &[u8], range: usize, chain: &[usize]) -> Result<Vec<u8>> {
    let perms = &sound.pitch_ranges[range].permutations;
    let mut out = Vec::new();
    for &i in chain {
        out.extend_from_slice(samples(file, &perms[i])?);
    }
    Ok(out)
}

/// Decodes one playable permutation. Every permutation in the chain must
/// use the sound's compression.
pub fn decode_chain(sound: &Sound, file: &[u8], range: usize, chain: &[usize]) -> Result<Pcm> {
    let channels = sound.channels()?;
    let perms = &sound.pitch_ranges[range].permutations;
    if let Some(&i) = chain
        .iter()
        .find(|&&i| perms[i].compression != sound.compression)
    {
        return Err(Error::Invalid(format!(
            "permutation {i} is {:?}, the sound {:?}",
            perms[i].compression, sound.compression
        )));
    }
    let samples = decode_bytes(
        sound.compression,
        channels,
        &chain_bytes(sound, file, range, chain)?,
    )?;
    Ok(Pcm {
        rate: sound.hz()?,
        channels,
        samples,
    })
}

/// Every playable permutation of every range: `(range, first permutation,
/// pcm)`.
pub fn decode_all(sound: &Sound, file: &[u8]) -> Result<Vec<(usize, usize, Pcm)>> {
    let mut out = Vec::new();
    for r in 0..sound.pitch_ranges.len() {
        for chain in sound.chains(r)? {
            let pcm = decode_chain(sound, file, r, &chain)?;
            out.push((r, chain[0], pcm));
        }
    }
    Ok(out)
}
