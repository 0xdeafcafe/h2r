//! The co-op datagram.
//!
//! One kind of datagram goes each way. It says what the sender has from the
//! receiver (`ack_frame`, `ack_tick`: the next frame and tick it expects),
//! and repeats every pad frame and tick hash of its own from the receiver's
//! last acknowledgement on, up to [`MAX_FRAMES`] and [`MAX_HASHES`]. So
//! frames and hashes arrive in order whatever the link drops or reorders,
//! and nothing is ever retransmitted on a timer.
//!
//! Layout, little endian: magic `h2co`, version (u8), session (u32),
//! `ack_frame` (u32), `ack_tick` (u32), first frame (u32), frame count (u8),
//! that many 18-byte pads, first tick (u32), hash count (u8), that many u64.
//! Version 2 appends the next cinematic frame acknowledged, the first host
//! decision's frame (both u32), then a u8 count and that many bounded u8 decisions.

use crate::cinematic::CinematicTicks;
use crate::schedule::{PAD, Pad};

pub const MAGIC: [u8; 4] = *b"h2co";
pub const VERSION: u8 = 2;
/// Pad frames repeated in one datagram.
pub const MAX_FRAMES: usize = 32;
/// Tick hashes repeated in one datagram.
pub const MAX_HASHES: usize = 32;
const HEADER: usize = 4 + 1 + 4 + 4 + 4;
/// The largest datagram [`Datagram::encode`] makes. The relay's size cap
/// and a UDP payload under the usual MTU both hold it.
pub const MAX_DATAGRAM: usize = HEADER + 5 + MAX_FRAMES * PAD + 5 + MAX_HASHES * 8 + 9 + MAX_FRAMES;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Datagram {
    /// Both peers derive it from the Live session; anything else is not ours.
    pub session: u32,
    /// The next frame the sender needs from the receiver.
    pub ack_frame: u32,
    /// The next tick hash the sender needs from the receiver.
    pub ack_tick: u32,
    pub first_frame: u32,
    pub frames: Vec<Pad>,
    pub ack_cinematic: u32,
    pub first_cinematic: u32,
    pub cinematic: Vec<CinematicTicks>,
    pub first_tick: u32,
    pub hashes: Vec<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireError {
    /// Not a co-op datagram, or cut short.
    Malformed,
    /// A co-op datagram of a version this build does not speak.
    Version(u8),
}

impl Datagram {
    /// The bytes on the wire. Frames and hashes beyond the per-datagram
    /// limits are left out (the next datagram carries them once the first
    /// ones are acknowledged).
    pub fn encode(&self) -> Vec<u8> {
        let frames = &self.frames[..self.frames.len().min(MAX_FRAMES)];
        let hashes = &self.hashes[..self.hashes.len().min(MAX_HASHES)];
        let mut out = Vec::with_capacity(MAX_DATAGRAM);
        out.extend_from_slice(&MAGIC);
        out.push(VERSION);
        for v in [
            self.session,
            self.ack_frame,
            self.ack_tick,
            self.first_frame,
        ] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.push(frames.len() as u8);
        for f in frames {
            out.extend_from_slice(f);
        }
        out.extend_from_slice(&self.first_tick.to_le_bytes());
        out.push(hashes.len() as u8);
        for h in hashes {
            out.extend_from_slice(&h.to_le_bytes());
        }
        out.extend_from_slice(&self.ack_cinematic.to_le_bytes());
        out.extend_from_slice(&self.first_cinematic.to_le_bytes());
        let ticks = &self.cinematic[..self.cinematic.len().min(MAX_FRAMES)];
        out.push(ticks.len() as u8);
        out.extend(ticks.iter().map(|t| t.count()));
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Datagram, WireError> {
        let mut r = Reader { bytes, at: 0 };
        if r.take(4)? != MAGIC {
            return Err(WireError::Malformed);
        }
        let version = r.u8()?;
        if version != VERSION {
            return Err(WireError::Version(version));
        }
        let (session, ack_frame, ack_tick, first_frame) = (r.u32()?, r.u32()?, r.u32()?, r.u32()?);
        let n = usize::from(r.u8()?);
        if n > MAX_FRAMES {
            return Err(WireError::Malformed);
        }
        let mut frames = Vec::with_capacity(n);
        for _ in 0..n {
            frames.push(r.take(PAD)?.try_into().expect("PAD bytes"));
        }
        let first_tick = r.u32()?;
        let n = usize::from(r.u8()?);
        if n > MAX_HASHES {
            return Err(WireError::Malformed);
        }
        let mut hashes = Vec::with_capacity(n);
        for _ in 0..n {
            hashes.push(u64::from_le_bytes(r.take(8)?.try_into().expect("8 bytes")));
        }
        let ack_cinematic = r.u32()?;
        let first_cinematic = r.u32()?;
        let n = usize::from(r.u8()?);
        if n > MAX_FRAMES || first_cinematic.checked_add(n as u32).is_none() { return Err(WireError::Malformed); }
        let cinematic = r.take(n)?.iter().map(|&value| CinematicTicks::new(value).ok_or(WireError::Malformed))
            .collect::<Result<_, _>>()?;
        if r.at != bytes.len() {
            return Err(WireError::Malformed);
        }
        Ok(Datagram {
            session,
            ack_frame,
            ack_tick,
            first_frame,
            frames,
            ack_cinematic,
            first_cinematic,
            cinematic,
            first_tick,
            hashes,
        })
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        let end = self.at.checked_add(n).ok_or(WireError::Malformed)?;
        let s = self.bytes.get(self.at..end).ok_or(WireError::Malformed)?;
        self.at = end;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8, WireError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, WireError> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Datagram {
        Datagram {
            session: 0xfeed_beef,
            ack_frame: 7,
            ack_tick: 5,
            first_frame: 9,
            frames: vec![[1; PAD], [2; PAD], [3; PAD]],
            first_tick: 4,
            ack_cinematic: 3,
            first_cinematic: 5,
            cinematic: vec![CinematicTicks::new(15).unwrap()],
            hashes: vec![u64::MAX, 0, 0x0123_4567_89ab_cdef],
        }
    }

    #[test]
    fn round_trips() {
        let d = sample();
        assert_eq!(Datagram::decode(&d.encode()), Ok(d));
        let empty = Datagram::default();
        assert_eq!(Datagram::decode(&empty.encode()), Ok(empty));
    }

    #[test]
    fn never_exceeds_the_cap() {
        let d = Datagram {
            frames: vec![[9; PAD]; 100],
            hashes: vec![1; 100],
            cinematic: vec![CinematicTicks::default(); 100],
            ..sample()
        };
        let bytes = d.encode();
        assert_eq!(bytes.len(), MAX_DATAGRAM);
        let back = Datagram::decode(&bytes).unwrap();
        assert_eq!(
            (back.frames.len(), back.hashes.len()),
            (MAX_FRAMES, MAX_HASHES)
        );
    }

    #[test]
    fn refuses_what_is_not_ours() {
        let good = sample().encode();
        assert_eq!(Datagram::decode(b""), Err(WireError::Malformed));
        assert_eq!(Datagram::decode(b"nope"), Err(WireError::Malformed));
        for cut in 0..good.len() {
            assert_eq!(
                Datagram::decode(&good[..cut]),
                Err(WireError::Malformed),
                "{cut}"
            );
        }
        let mut long = good.clone();
        long.push(0);
        assert_eq!(Datagram::decode(&long), Err(WireError::Malformed));
        let mut other = good.clone();
        other[4] = 1;
        assert_eq!(Datagram::decode(&other), Err(WireError::Version(1)));
        let mut excessive = good.clone();
        *excessive.last_mut().unwrap() = 16;
        assert_eq!(Datagram::decode(&excessive), Err(WireError::Malformed));
        // A frame count over the limit is refused before anything is read.
        let mut many = good;
        many[HEADER + 4] = MAX_FRAMES as u8 + 1;
        assert_eq!(Datagram::decode(&many), Err(WireError::Malformed));
    }
}
