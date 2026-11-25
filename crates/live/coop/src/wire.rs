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
//! Version 3 puts the Forge lane before the cinematic section: the next
//! Forge byte the sender needs (u32), the offset of the first Forge byte
//! carried (u32), a u16 count and that many bytes, then a u8 that is 0 or
//! the length of the Forge beat that follows (`BEAT` bytes). The Forge bytes
//! are a reliable ordered stream like the frames: repeated from the
//! receiver's acknowledgement on until acknowledged. The beat is the sender's
//! Forge log head and stream position, sent in every datagram.
//! The version also holds the next cinematic frame acknowledged, the first host
//! decision's frame (both u32), then a u8 count and that many bounded u8 decisions.

use crate::cinematic::CinematicTicks;
use crate::schedule::{PAD, Pad};

pub const MAGIC: [u8; 4] = *b"h2co";
pub const VERSION: u8 = 3;
/// Pad frames repeated in one datagram.
pub const MAX_FRAMES: usize = 32;
/// Tick hashes repeated in one datagram.
pub const MAX_HASHES: usize = 32;
/// The most Forge stream bytes one datagram may carry. A peer carrying
/// pads and hashes too fits fewer ([`forge_room`]); both machines of a
/// session run one build, so the ceiling they accept is the same.
pub const MAX_FORGE: usize = 1024;
/// The largest datagram a sender puts on the wire: the relay's message
/// cap and a UDP payload under the usual MTU both hold it. [`Peer::flush`]
/// keeps within it by carrying less Forge ([`forge_room`]).
pub const WIRE_CAP: usize = 1200;
/// The Forge beat: log head sequence and tick (u32 each), its hash and the
/// stream position (u64).
pub const BEAT: usize = 4 + 4 + 32 + 8;
const HEADER: usize = 4 + 1 + 4 + 4 + 4;
/// The largest datagram [`Datagram::encode`] can make, every section
/// full: the buffer bound, not what a sender sends (see [`WIRE_CAP`]).
pub const MAX_DATAGRAM: usize =
    HEADER + 5 + MAX_FRAMES * PAD + 5 + MAX_HASHES * 8 + 4 + 4 + 2 + MAX_FORGE + 1 + BEAT + 9 + MAX_FRAMES;

/// The Forge bytes one datagram carries beside the rest of it: the wire
/// cap less the other sections as they encode. A datagram full of pads
/// and hashes carries a short run; a Forge-only peer's, with none,
/// carries nearly the whole cap.
pub fn forge_room(frames: usize, hashes: usize, cinematic: usize, beat: bool) -> usize {
    let rest = HEADER
        + 5
        + frames * PAD
        + 5
        + hashes * 8
        + 4
        + 4
        + 2
        + 1
        + usize::from(beat) * BEAT
        + 9
        + cinematic;
    WIRE_CAP.saturating_sub(rest).min(MAX_FORGE)
}

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
    /// The next Forge stream byte the sender needs from the receiver.
    pub ack_forge: u32,
    /// The stream offset of `forge[0]`.
    pub first_forge: u32,
    pub forge: Vec<u8>,
    /// The sender's Forge position (`forge::net::Beat`'s bytes), if any.
    pub beat: Option<[u8; BEAT]>,
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
        let forge = &self.forge[..self.forge.len().min(MAX_FORGE)];
        out.extend_from_slice(&self.ack_forge.to_le_bytes());
        out.extend_from_slice(&self.first_forge.to_le_bytes());
        out.extend_from_slice(&(forge.len() as u16).to_le_bytes());
        out.extend_from_slice(forge);
        match &self.beat {
            Some(beat) => {
                out.push(BEAT as u8);
                out.extend_from_slice(beat);
            }
            None => out.push(0),
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
        let ack_forge = r.u32()?;
        let first_forge = r.u32()?;
        let n = usize::from(u16::from_le_bytes(r.take(2)?.try_into().expect("2 bytes")));
        if n > MAX_FORGE || first_forge.checked_add(n as u32).is_none() {
            return Err(WireError::Malformed);
        }
        let forge = r.take(n)?.to_vec();
        let beat = match r.u8()? {
            0 => None,
            n if usize::from(n) == BEAT => Some(r.take(BEAT)?.try_into().expect("BEAT bytes")),
            _ => return Err(WireError::Malformed),
        };
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
            ack_forge,
            first_forge,
            forge,
            beat,
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
            ack_forge: 11,
            first_forge: 6,
            forge: vec![1, 2, 3, 4],
            beat: Some([7; BEAT]),
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
            forge: vec![5; 2000],
            ..sample()
        };
        let bytes = d.encode();
        assert_eq!(bytes.len(), MAX_DATAGRAM);
        let back = Datagram::decode(&bytes).unwrap();
        assert_eq!(
            (back.frames.len(), back.hashes.len(), back.forge.len()),
            (MAX_FRAMES, MAX_HASHES, MAX_FORGE)
        );
    }

    #[test]
    fn a_budgeted_datagram_fits_the_wire_cap() {
        // The relay closes a socket over 1200 bytes (docs/live/live-coop.md,
        // "The coop crate"), and a UDP payload wants the usual MTU.
        for (frames, hashes, cinematic, beat) in [
            (0, 0, 0, false),
            (0, 0, 0, true),
            (MAX_FRAMES, MAX_HASHES, MAX_FRAMES, true),
            (7, 3, 2, true),
        ] {
            let forge = forge_room(frames, hashes, cinematic, beat);
            let d = Datagram {
                frames: vec![[0; PAD]; frames],
                hashes: vec![0; hashes],
                cinematic: vec![CinematicTicks::default(); cinematic],
                forge: vec![0; forge],
                beat: beat.then_some([0; BEAT]),
                ..sample()
            };
            assert!(d.encode().len() <= WIRE_CAP, "{frames} {hashes} {cinematic}");
            assert_eq!(
                Datagram::decode(&d.encode()).unwrap().forge.len(),
                forge,
                "{frames} {hashes} {cinematic}"
            );
        }
        // A datagram full of pads and hashes still carries a run the old
        // per-datagram ceiling sized.
        assert!(forge_room(MAX_FRAMES, MAX_HASHES, MAX_FRAMES, true) >= 200);
        // A Forge-only peer's datagram carries nearly the whole cap.
        assert!(forge_room(0, 0, 0, true) > 1000);
    }

    #[test]
    fn a_beat_of_another_length_or_a_forge_run_over_the_cap_is_refused() {
        let mut d = sample();
        d.beat = None;
        let mut bytes = d.encode();
        // The beat length byte follows the forge bytes; the cinematic
        // section (9 bytes and one decision) follows it.
        let at = bytes.len() - 1 - 8 - 1 - 1;
        assert_eq!(bytes[at], 0);
        bytes[at] = 5;
        assert_eq!(Datagram::decode(&bytes), Err(WireError::Malformed));
        let mut d = sample();
        d.forge = vec![0; MAX_FORGE];
        let mut bytes = d.encode();
        let count_at = HEADER + 5 + 3 * PAD + 4 + 1 + 3 * 8 + 4 + 4;
        bytes[count_at..count_at + 2].copy_from_slice(&(MAX_FORGE as u16 + 1).to_le_bytes());
        assert_eq!(Datagram::decode(&bytes), Err(WireError::Malformed));
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
