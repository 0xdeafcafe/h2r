//! Samples and the WAV file they come from and go to.
use audio::meter::{self, Levels};

/// 16-bit PCM, interleaved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wave {
    /// 1 or 2.
    pub channels: u16,
    /// Frames per second.
    pub rate: u32,
    pub pcm: Vec<i16>,
}

fn word(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn half(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

impl Wave {
    pub fn frames(&self) -> usize {
        self.pcm.len() / usize::from(self.channels.max(1))
    }

    pub fn duration_ms(&self) -> u64 {
        self.frames() as u64 * 1000 / u64::from(self.rate.max(1))
    }

    /// The meter's levels of these samples.
    pub fn levels(&self) -> Levels {
        meter::measure(&self.pcm, self.channels, self.rate)
    }

    /// `bins` columns of (lowest, highest) sample over all channels.
    pub fn peaks(&self, bins: usize) -> Vec<[i16; 2]> {
        let n = self.pcm.len();
        if bins == 0 || n == 0 {
            return Vec::new();
        }
        (0..bins)
            .map(|b| {
                let (from, to) = (b * n / bins, ((b + 1) * n / bins).max(b * n / bins + 1));
                let part = &self.pcm[from..to.min(n)];
                [
                    part.iter().copied().min().unwrap_or(0),
                    part.iter().copied().max().unwrap_or(0),
                ]
            })
            .collect()
    }

    /// A 16-bit PCM WAV of one or two channels, or why it is not one.
    pub fn from_wav(bytes: &[u8]) -> Result<Wave, String> {
        if bytes.starts_with(b"version https://git-lfs") {
            return Err("a Git LFS pointer: run `git lfs pull`".into());
        }
        if bytes.get(..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
            return Err("not a RIFF WAVE file".into());
        }
        let (mut fmt, mut data) = (None, None);
        let mut at = 12;
        while at + 8 <= bytes.len() {
            let id = &bytes[at..at + 4];
            let len = word(bytes, at + 4).ok_or("a chunk with no length")? as usize;
            let body = at + 8;
            match id {
                b"fmt " => fmt = Some(body),
                b"data" => data = Some((body, len.min(bytes.len().saturating_sub(body)))),
                _ => {}
            }
            at = body.saturating_add(len).saturating_add(len & 1);
        }
        let f = fmt.ok_or("no fmt chunk")?;
        let short = "a short fmt chunk";
        let (tag, channels, rate, bits) = (
            half(bytes, f).ok_or(short)?,
            half(bytes, f + 2).ok_or(short)?,
            word(bytes, f + 4).ok_or(short)?,
            half(bytes, f + 14).ok_or(short)?,
        );
        if tag != 1 || bits != 16 {
            return Err(format!("format {tag}, {bits} bits: a clip is 16-bit PCM"));
        }
        if !(1..=2).contains(&channels) || rate == 0 {
            return Err(format!("{channels} channels at {rate} Hz"));
        }
        let (start, len) = data.ok_or("no data chunk")?;
        let width = 2 * usize::from(channels);
        let frames = len / width;
        if frames == 0 {
            return Err("the clip is empty".into());
        }
        Ok(Wave {
            channels,
            rate,
            pcm: bytes[start..start + frames * width]
                .chunks_exact(2)
                .map(|b| i16::from_le_bytes([b[0], b[1]]))
                .collect(),
        })
    }

    /// The samples as a RIFF WAVE file.
    pub fn to_wav(&self) -> Vec<u8> {
        let data = (self.pcm.len() * 2) as u32;
        let width = u32::from(self.channels) * 2;
        let mut b = Vec::with_capacity(44 + self.pcm.len() * 2);
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&self.channels.to_le_bytes());
        b.extend_from_slice(&self.rate.to_le_bytes());
        b.extend_from_slice(&(self.rate * width).to_le_bytes());
        b.extend_from_slice(&(width as u16).to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data.to_le_bytes());
        for s in &self.pcm {
            b.extend_from_slice(&s.to_le_bytes());
        }
        b
    }
}
