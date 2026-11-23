//! What a spoken line goes through before it is a clip: read from the
//! service's reply, trimmed of silence, brought to a loudness, and written
//! as a WAV in the format our announcer clips use (44.1 kHz mono 16-bit
//! PCM, `art/announcer/README.md`). Pure: the meter is `sound.measure`'s.

use super::{Pcm, VoiceError};
use crate::api::sound_measure::meter::{self, Levels};
use audio::meter::SILENCE_DB;

/// The rate lines are asked for and saved at.
pub const RATE: u32 = 44_100;

/// Under this a sample is silence for trimming (dBFS).
pub const SILENCE_FLOOR_DBFS: f64 = -50.0;
/// What is left of the silence at the start and the end (ms).
pub const HEAD_PAD_MS: u64 = 20;
pub const TAIL_PAD_MS: u64 = 60;

/// The highest sample peak a normalised line may have (dBFS). Our
/// announcer clips keep their peaks under -0.5 dBTP; a sample peak at
/// -1 dBFS leaves the room.
pub const CEILING_DBFS: f64 = -1.0;

/// The mono 16-bit samples the service's reply holds: raw PCM at
/// [`RATE`] (`format: pcm`), or a WAV when it sent one.
pub fn from_response(bytes: &[u8]) -> Result<Pcm, VoiceError> {
    let pcm = if bytes.starts_with(b"RIFF") {
        read_wav(bytes)?
    } else {
        let whole = bytes.len() / 2 * 2;
        Pcm {
            rate: RATE,
            samples: bytes[..whole]
                .chunks_exact(2)
                .map(|b| i16::from_le_bytes([b[0], b[1]]))
                .collect(),
        }
    };
    if pcm.samples.is_empty() {
        return Err(VoiceError::Audio("Fish Audio sent no audio".into()));
    }
    Ok(pcm)
}

/// A mono (or the left of a stereo) WAV's samples, by the game's reader.
pub fn read_wav(bytes: &[u8]) -> Result<Pcm, VoiceError> {
    let c = boot::medal_announcer::parse_wav(bytes).map_err(|e| VoiceError::Audio(e.to_string()))?;
    let ch = usize::from(c.channels);
    let samples = c
        .data
        .chunks_exact(2 * ch)
        .map(|f| i16::from_le_bytes([f[0], f[1]]))
        .collect();
    Ok(Pcm {
        rate: c.rate,
        samples,
    })
}

/// A RIFF WAVE file of mono 16-bit PCM.
pub fn write_wav(pcm: &[i16], rate: u32) -> Vec<u8> {
    let data_len = (pcm.len() * 2) as u32;
    let mut b = Vec::with_capacity(44 + pcm.len() * 2);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        b.extend_from_slice(&s.to_le_bytes());
    }
    b
}

/// What trimming took off, in frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trimmed {
    pub head: usize,
    pub tail: usize,
}

/// `pcm` (mono) without the silence at its ends: samples under
/// [`SILENCE_FLOOR_DBFS`] are silence, and `head_ms` and `tail_ms` of it
/// stay. `None` when it is all silence.
pub fn trim_silence(pcm: &[i16], rate: u32, head_ms: u64, tail_ms: u64) -> Option<(Vec<i16>, Trimmed)> {
    let floor = 32768.0 * 10f64.powf(SILENCE_FLOOR_DBFS / 20.0);
    let loud = |s: &i16| f64::from(s.unsigned_abs()) >= floor;
    let first = pcm.iter().position(loud)?;
    let last = pcm.iter().rposition(loud)?;
    let frames = |ms: u64| (u64::from(rate) * ms / 1000) as usize;
    let start = first.saturating_sub(frames(head_ms));
    let end = (last + 1 + frames(tail_ms)).min(pcm.len());
    Some((
        pcm[start..end].to_vec(),
        Trimmed {
            head: start,
            tail: pcm.len() - end,
        },
    ))
}

/// A line brought to a loudness.
#[derive(Clone, Debug)]
pub struct Normalised {
    pub pcm: Vec<i16>,
    /// The gain applied (dB).
    pub gain_db: f64,
    /// The peak ceiling held the gain under what the target asked for, so
    /// the line is quieter than the target.
    pub limited: bool,
    /// The levels before the gain.
    pub before: Levels,
}

/// `pcm` (mono at `rate`) with one gain so its integrated loudness is
/// `target_lufs`, or as near as keeps its sample peak under `ceiling_dbfs`.
/// A line with nothing in it is an error.
pub fn normalise(pcm: &[i16], rate: u32, target_lufs: f64, ceiling_dbfs: f64) -> Result<Normalised, VoiceError> {
    let before = meter::measure(pcm, 1, rate);
    if before.lufs <= SILENCE_DB + 1.0 || before.peak_dbfs <= SILENCE_DB + 1.0 {
        return Err(VoiceError::Audio("the line is silent".into()));
    }
    let mut gain_db = target_lufs - before.lufs;
    let mut limited = false;
    if before.peak_dbfs + gain_db > ceiling_dbfs {
        gain_db = ceiling_dbfs - before.peak_dbfs;
        limited = true;
    }
    Ok(Normalised {
        pcm: apply_gain(pcm, gain_db),
        gain_db,
        limited,
        before,
    })
}

/// `pcm` times `gain_db`, rounded and held to 16 bits.
pub fn apply_gain(pcm: &[i16], gain_db: f64) -> Vec<i16> {
    let k = 10f64.powf(gain_db / 20.0);
    pcm.iter()
        .map(|&s| (f64::from(s) * k).round().clamp(-32768.0, 32767.0) as i16)
        .collect()
}
