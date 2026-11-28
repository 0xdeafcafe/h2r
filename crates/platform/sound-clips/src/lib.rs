//! The one sound decoder. docs/engine/sound-instances.md, "Clip source".
//!
//! A `snd!` permutation is the concatenation of its chunks' encoded bytes
//! ([`tags::sound::Gestalt::read_encoded`]). [`decode`] turns them into
//! 16-bit PCM by the sound's own header:
//!
//! - Xbox ADPCM, mono or stereo (`audio::adpcm`), at the header's rate;
//! - a codec stream (WMA in ASF framing), through `wma`, which needs the
//!   game's executable for its tables ([`NEEDS_EXECUTABLE`] without them).
//!   Its output is stereo at 44.1 kHz, as `h2 sound` writes it.
//!
//! Any other compression is refused by name; nothing is guessed.
//!
//! [`ClipSource`] (in [`source`]) is the keyed cache over a [`Library`]:
//! one decode per [`ClipKey`] on a worker thread, and chunk streaming for a
//! long permutation. Studio's audition and `h2 sound` call the free
//! functions here; the mixer feed calls the source.
use std::fmt;
use std::sync::Arc;

use map::RawPointer;
use tags::sound::{Compression, Encoding, Gestalt, Permutation, ResourceReader, SoundHeader};

mod source;

pub use source::{ChunkReader, ClipKey, ClipSource, GestaltLibrary, Got, Library, PcmChunk, Stats};

/// Why a codec stream cannot be decoded without the disc's executable.
pub const NEEDS_EXECUTABLE: &str = "This sound needs the game's executable to decode.";

/// Most interleaved samples one decoded codec stream may hold.
const MAX_SAMPLES: usize = 128 * 1024 * 1024;

/// Why a sound is not decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipError {
    /// The sound is checked and cannot be decoded here: why.
    Refused(String),
    /// Something failed while reading or decoding: why.
    Failed(String),
}

impl fmt::Display for ClipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(s) | Self::Failed(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for ClipError {}

impl From<String> for ClipError {
    fn from(s: String) -> Self {
        Self::Failed(s)
    }
}

/// Decoded audio: interleaved 16-bit samples.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clip {
    pub rate: u32,
    pub channels: u16,
    pub pcm: Arc<[i16]>,
}

impl Clip {
    pub fn frames(&self) -> usize {
        self.pcm.len() / usize::from(self.channels.max(1))
    }

    pub fn duration_ms(&self) -> u64 {
        self.frames() as u64 * 1000 / u64::from(self.rate.max(1))
    }

    /// `bins` pairs of `[lowest, highest]`, each over its share of the
    /// frames, the channels averaged: the waveform's columns.
    pub fn peaks(&self, bins: usize) -> Vec<[i16; 2]> {
        let (frames, ch) = (self.frames(), usize::from(self.channels.max(1)));
        if frames == 0 || bins == 0 {
            return Vec::new();
        }
        (0..bins)
            .map(|b| {
                let lo = b * frames / bins;
                let hi = ((b + 1) * frames / bins).max(lo + 1).min(frames);
                let mut out = [i16::MAX, i16::MIN];
                for f in lo..hi {
                    let sum: i32 = self.pcm[f * ch..(f + 1) * ch]
                        .iter()
                        .map(|&s| i32::from(s))
                        .sum();
                    let v = (sum / ch as i32) as i16;
                    out[0] = out[0].min(v);
                    out[1] = out[1].max(v);
                }
                out
            })
            .collect()
    }

    /// Interleaved stereo `f32` at `rate` Hz (a mono clip on both sides).
    pub fn stereo(&self, rate: u32) -> Vec<f32> {
        let ch = usize::from(self.channels.max(1));
        let mut out = Vec::with_capacity(self.frames() * 2);
        for f in self.pcm.chunks_exact(ch) {
            let left = f32::from(f[0]) / 32768.0;
            let right = f32::from(f[ch.min(2) - 1]) / 32768.0;
            out.push(left);
            out.push(right);
        }
        audio::resample::resample_linear(&out, 2, self.rate, rate)
    }
}

/// Whether decoding `header`'s sound needs the game's executable.
pub fn needs_executable(header: &SoundHeader) -> bool {
    matches!(
        (header.compression, header.encoding),
        (Compression::Wma | Compression::XboxAdpcm, Encoding::Codec)
    )
}

/// The channel count and rate of a header's Xbox ADPCM, or why it is not.
pub(crate) fn adpcm_format(header: &SoundHeader) -> Result<(usize, u32), ClipError> {
    match (header.compression, header.encoding) {
        (Compression::XboxAdpcm, enc @ (Encoding::Mono | Encoding::Stereo)) => {
            let channels = if enc == Encoding::Mono { 1 } else { 2 };
            let rate = header.sample_rate.hz().ok_or_else(|| {
                ClipError::Refused(format!(
                    "the sample rate selector {:?} is not a known rate",
                    header.sample_rate
                ))
            })?;
            Ok((channels, rate))
        }
        (c, e) => Err(ClipError::Refused(format!(
            "compression {c:?} with encoding {e:?} is not decoded by the editor"
        ))),
    }
}

/// Decode whole Xbox ADPCM blocks (`channels` 1 or 2) at `rate`.
pub fn decode_adpcm(data: &[u8], channels: usize, rate: u32) -> Result<Clip, ClipError> {
    let pcm = audio::adpcm::decode(data, channels)
        .map_err(|e| ClipError::Failed(format!("Xbox ADPCM: {e}")))?;
    Ok(Clip {
        rate,
        channels: channels as u16,
        pcm: pcm.into(),
    })
}

/// Decode one WMA stream in ASF framing: stereo at 44.1 kHz.
pub fn decode_wma(tables: &wma::Tables, data: &[u8]) -> Result<Clip, ClipError> {
    let decoded = wma::decode_asf(tables, data, MAX_SAMPLES)
        .map_err(|e| ClipError::Failed(format!("WMA: {e}")))?;
    Ok(Clip {
        rate: 44100,
        channels: 2,
        pcm: decoded.pcm.into(),
    })
}

/// Decode one permutation's encoded bytes by `header`. `tables` are the
/// codec tables from the executable ([`load_tables`]); a codec stream
/// without them is refused.
pub fn decode(
    header: &SoundHeader,
    data: &[u8],
    tables: Option<&wma::Tables>,
) -> Result<Clip, ClipError> {
    match (header.compression, header.encoding) {
        (Compression::XboxAdpcm, Encoding::Mono | Encoding::Stereo) => {
            let (channels, rate) = adpcm_format(header)?;
            decode_adpcm(data, channels, rate)
        }
        (Compression::Wma | Compression::XboxAdpcm, Encoding::Codec) => {
            let tables = tables.ok_or_else(|| ClipError::Refused(NEEDS_EXECUTABLE.into()))?;
            decode_wma(tables, data)
        }
        _ => Err(adpcm_format(header).expect_err("not ADPCM")),
    }
}

/// The codec tables from the executable's bytes, after `wma` has verified
/// the image.
pub fn load_tables(xbe: &[u8]) -> Result<wma::Tables, ClipError> {
    let image = wma::VerifiedImage::load(xbe)
        .map_err(|e| ClipError::Refused(format!("the game's executable: {e}")))?;
    wma::Tables::load(&image)
        .map_err(|e| ClipError::Failed(format!("the game's executable's codec tables: {e}")))
}

/// Decode permutation `p` of a sound: read its chunks from `reader`, then
/// [`decode`].
pub fn decode_permutation(
    header: &SoundHeader,
    gestalt: &Gestalt,
    p: &Permutation,
    reader: &mut dyn ResourceReader,
    tables: Option<&wma::Tables>,
) -> Result<Clip, ClipError> {
    if needs_executable(header) && tables.is_none() {
        return Err(ClipError::Refused(NEEDS_EXECUTABLE.into()));
    }
    let data = gestalt
        .read_encoded(p, reader)
        .map_err(|e| ClipError::Failed(e.to_string()))?;
    decode(header, &data, tables)
}

/// [`ResourceReader`] over the map's raw-resource files.
pub struct Raw<'a>(pub &'a mut assets::ResourceFiles);

/// [`Raw`] owning its files: the reader a [`GestaltLibrary`] keeps.
pub struct OwnedRaw(pub assets::ResourceFiles);

impl ResourceReader for OwnedRaw {
    fn check(&self, p: RawPointer, size: u32) -> tags::Result<()> {
        let Some(length) = self.0.length(p.file) else {
            return Err(tags::Error::MissingResource(p.file));
        };
        if u64::from(p.offset) + u64::from(size) > length {
            return Err(tags::Error::ResourceBounds {
                file: p.file,
                offset: p.offset,
                size,
                length,
            });
        }
        Ok(())
    }

    fn read_raw(&mut self, p: RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        self.check(p, size)?;
        self.0
            .read(p.file, p.offset, size)
            .map_err(|_| tags::Error::Invalid("a raw resource file did not read"))
    }
}

impl ResourceReader for Raw<'_> {
    fn check(&self, p: RawPointer, size: u32) -> tags::Result<()> {
        let Some(length) = self.0.length(p.file) else {
            return Err(tags::Error::MissingResource(p.file));
        };
        if u64::from(p.offset) + u64::from(size) > length {
            return Err(tags::Error::ResourceBounds {
                file: p.file,
                offset: p.offset,
                size,
                length,
            });
        }
        Ok(())
    }

    fn read_raw(&mut self, p: RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        self.check(p, size)?;
        self.0
            .read(p.file, p.offset, size)
            .map_err(|_| tags::Error::Invalid("a raw resource file did not read"))
    }
}

#[cfg(test)]
mod tests;
