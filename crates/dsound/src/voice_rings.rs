//! The XHV per-port voice effects (effects `0xb..=0xe` of the effects
//! image), run on the host.
//!
//! On an Xbox the title's voice engine plays each remote talker into a
//! DirectSound stream (`0x335ed3`, 8 kHz mono) whose one mix bin is its
//! port's voice effect (`0x333330`: `(state +0x14 - 0x1400) >> 5` of effect
//! `0xb + port`). That DSP effect writes the port's mix into an output ring
//! as 16 kHz 32-bit samples, and the headphone feed (`0x334d1f`, through
//! `0x332c9c`) reads the ring: base at map +0x18, size at map +0x1c, the
//! effect's write offset in bytes at state +0x34 (docs/live/live-voice.md,
//! 3.3 item 7).
//!
//! Here the descriptor gives each of the four effects its own invented DSP
//! base, so each port's streams land on a bin of their own ([`bin`]); the
//! mixer captures those bins instead of playing them
//! (`audio::Mixer::set_taps`); and after every render [`VoiceRings::pump`]
//! converts what was captured to 16 kHz, writes it into the port's ring
//! with the sample in the high 16 bits, and moves the write offset on. The
//! rings advance by exactly the frames the mixer rendered, voice or not, so
//! the feed's clock is the mixer's.

use audio::Mixer;

use crate::host::{GuestMemory, MemFault, at};

/// The ring's sample rate: the headset's 16 kHz (`0x484290`).
pub const RING_RATE: u32 = 16_000;

/// The first port's bin. The four are the top FX sends (`DSMIXBIN_FXSEND_16`
/// to `_19`), clear of the mix-ins' input bins (`0x17..=0x19`). Invented: the
/// real value is the address of the effect's input in DSP memory.
const FIRST_BIN: u32 = 0x1b;

/// The mix bin port `port`'s voice streams feed.
pub fn bin(port: u32) -> u32 {
    FIRST_BIN + port
}

/// The value at state +0x14 that makes `0x333330` derive `bin`.
pub fn dsp_base(bin: u32) -> u32 {
    0x1400 + (bin << 5)
}

/// Offset of the write offset in the effect's state (`0x332cd7`).
const STATE_WRITE: u32 = 0x34;

/// Linear rate conversion of mono samples with the position kept as an
/// exact fraction, so the output count is exactly `input * to / from` over
/// time, however the input is split. Output `k` sits between the previous
/// call's last sample and the next: a one-sample delay.
#[derive(Clone, Debug)]
struct Converter {
    from: u64,
    to: u64,
    /// The next output's position, in 1/`to` input samples after `last`.
    pos: u64,
    last: f32,
}

impl Converter {
    fn new(from: u32, to: u32) -> Self {
        Self {
            from: u64::from(from),
            to: u64::from(to),
            pos: 0,
            last: 0.0,
        }
    }

    fn run(&mut self, input: &[f32], mut out: impl FnMut(f32)) {
        let end = input.len() as u64 * self.to;
        while self.pos < end {
            let i = (self.pos / self.to) as usize;
            let t = (self.pos % self.to) as f32 / self.to as f32;
            let a = if i == 0 { self.last } else { input[i - 1] };
            let b = input[i];
            out(a + (b - a) * t);
            self.pos += self.from;
        }
        if let Some(&l) = input.last() {
            self.last = l;
        }
        self.pos -= end;
    }
}

/// One port's ring.
#[derive(Debug)]
struct Ring {
    bin: u32,
    base: u32,
    size: u32,
    state: u32,
    /// Byte offset of the next sample, `< size`, 4-aligned.
    write: u32,
    /// The converter and the mixer rate it was made for.
    conv: Option<(u32, Converter)>,
}

/// The four rings and where they are in guest memory.
#[derive(Debug, Default)]
pub struct VoiceRings {
    rings: Vec<Ring>,
    bytes: Vec<u8>,
}

impl VoiceRings {
    /// `rings[port] = (ring base, ring size, effect state)`.
    pub fn new(rings: &[(u32, u32, u32)]) -> Self {
        Self {
            rings: rings
                .iter()
                .enumerate()
                .map(|(port, &(base, size, state))| Ring {
                    bin: bin(port as u32),
                    base,
                    size,
                    state,
                    write: 0,
                    conv: None,
                })
                .collect(),
            bytes: Vec::new(),
        }
    }

    /// The bins the mixer must capture.
    pub fn bins(&self) -> Vec<u32> {
        self.rings.iter().map(|r| r.bin).collect()
    }

    /// Moves what the mixer captured since the last call into the rings.
    pub fn pump(&mut self, h: &mut dyn GuestMemory, mixer: &mut Mixer) -> Result<(), MemFault> {
        let rate = mixer.sample_rate();
        for ring in &mut self.rings {
            let input = mixer.take_tap(ring.bin);
            if input.is_empty() {
                continue;
            }
            let conv = match &mut ring.conv {
                Some((r, c)) if *r == rate => c,
                slot => &mut slot.insert((rate, Converter::new(rate, RING_RATE))).1,
            };
            self.bytes.clear();
            conv.run(&input, |s| {
                let v = (s * 32768.0).round().clamp(-32768.0, 32767.0) as i16;
                self.bytes.extend((i32::from(v) << 16).to_le_bytes());
            });
            // At most the ring's size is worth writing: older samples would
            // be overwritten in the same call.
            let len = self.bytes.len() as u32;
            let skip = len.saturating_sub(ring.size);
            ring.write = (ring.write + skip) % ring.size;
            let mut rest = &self.bytes[skip as usize..];
            while !rest.is_empty() {
                let n = rest.len().min((ring.size - ring.write) as usize);
                h.write(at(ring.base, ring.write)?, &rest[..n])?;
                ring.write = (ring.write + n as u32) % ring.size;
                rest = &rest[n..];
            }
            h.write_u32(at(ring.state, STATE_WRITE)?, ring.write)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversion_counts_are_exact_however_the_input_is_split() {
        for (from, to) in [(48_000, 16_000), (44_100, 16_000), (8_000, 16_000)] {
            let input: Vec<f32> = (0..4410).map(|i| (i as f32 * 0.01).sin()).collect();
            let mut whole = Vec::new();
            Converter::new(from, to).run(&input, |s| whole.push(s));
            let mut parts = Vec::new();
            let mut c = Converter::new(from, to);
            for chunk in input.chunks(37) {
                c.run(chunk, |s| parts.push(s));
            }
            assert_eq!(whole, parts);
            let want = input.len() as u64 * u64::from(to) / u64::from(from);
            assert!((whole.len() as u64).abs_diff(want) <= 1, "{from}->{to}");
        }
    }

    #[test]
    fn the_dsp_base_gives_back_the_bin_as_0x333330_derives_it() {
        for port in 0..4 {
            assert_eq!((dsp_base(bin(port)) - 0x1400) >> 5, bin(port));
        }
    }
}
