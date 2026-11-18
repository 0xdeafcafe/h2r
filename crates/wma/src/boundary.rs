//! The game's WMA codec boundary, answered by the native decoder.
//!
//! Halo 2's `XWMAXMO` container (`0x388533` steps it, `0x3886b0` drains PCM)
//! drives a codec through six stdcall entries. [`Session`] is that codec's
//! state machine with no runtime attached; the guest adapter (in `dsound`)
//! reads the arguments, pulls input through the game's callback and calls it.
//!
//! | entry | arguments | here |
//! |---|---|---|
//! | create `0x38a0c5` | `(&handle, context, callback, version, samples, rate, channels, byte rate, block align, flags, 0)` | [`Session::new`] |
//! | destroy `0x38a13f` | `(&handle)` | drop |
//! | reset `0x38a16b` | `(handle)` | [`Session::reset`] |
//! | begin packet `0x38a189` | `(handle)` | [`Session::feed`] then [`Session::begin`] |
//! | decode `0x38a1a7` | `(handle)` | [`Session::decode`] |
//! | PCM `0x38a1cc` | `(handle, &frames, buffer, bytes)` | [`Session::take_pcm`] |
//!
//! Statuses, from the container's handling (`0x388557..0x3886a5`): begin
//! returns 0, or [`NO_DATA`] when the input runs dry (the container then
//! checks its reader for the end of the file). Decode returns 0 after a frame
//! and [`PACKET_DONE`] when the packet is used up, after which the container
//! drains PCM and begins the next packet. A negative decode status makes the
//! container reset the codec.
//!
//! The native decoder needs whole packets (one packet of lookahead for the
//! superframe reservoir), so begin pulls a complete packet and decodes it at
//! once, and decode reports the packet done. PCM is the same, it is just
//! produced a packet earlier. Where the guest decoder would carry on through
//! damage, this one resynchronises instead of failing the stream:
//!
//! - **Starvation is not the end.** The game's reader is asynchronous: a
//!   dry callback usually means the next disc read has not landed. Begin then
//!   returns [`NO_DATA`] and keeps everything (the partial packet, the held
//!   packet and the decoder's overlap and reservoir), and the container tries
//!   again on a later step. The adapter tells the two apart with the
//!   container's own test (see [`Session::begin`]).
//! - **End of input.** At the real end the decoder is finished, emitting the
//!   held packet, and begin returns 0 so it is drained; every begin after that
//!   returns [`NO_DATA`], which the container takes as the end. A partial
//!   packet left at the end is dropped. Input arriving after the end starts a
//!   fresh decoder.
//! - **Sequence break** (a seek or loop without a reset): the held packet is
//!   finished and a fresh decoder starts on the new one.
//! - **Malformed packet:** counted in [`Stats::errors`], and a fresh decoder
//!   waits for the next packet; the damaged packet is silent.
//! - **A packet start mid-packet** (the callback's return 3 with bytes still
//!   owed) drops the partial packet.
use crate::{Decoder, Format, Tables};
use snapshot::codec::{Reader, Writer};
use std::collections::VecDeque;
use std::sync::Arc;

/// `S_OK`.
pub const OK: u32 = 0;
/// Decode: the packet's frames are all out; begin the next packet.
pub const PACKET_DONE: u32 = 4;
/// Begin: no input available (`0x80040004`).
pub const NO_DATA: u32 = 0x8004_0004;
/// Create: format not supported (`0x80040000`; the container reports 7).
pub const UNSUPPORTED: u32 = 0x8004_0000;
/// Any entry: null or unknown handle (`E_INVALIDARG`).
pub const INVALID_ARG: u32 = 0x8007_0057;
/// The input callback's return for the first chunk of a codec packet
/// (`0x388832..0x388842`).
pub const PACKET_START: u32 = 3;

/// What create receives, in argument order after the handle pointer,
/// context and callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CreateParams {
    pub version: u32,
    /// WMAv2 `dwSamplesPerBlock` (extra bytes 0..4).
    pub samples_per_block: u32,
    pub sample_rate: u32,
    pub channels: u32,
    pub bytes_per_second: u32,
    pub block_align: u32,
    /// WMAv2 encode options (extra bytes 4..6).
    pub flags: u32,
}

impl CreateParams {
    /// The two retail profiles the native decoder implements (see
    /// [`Format::from_wave`]), or `None`.
    pub fn format(&self) -> Option<Format> {
        let retail = self.version == 2
            && self.samples_per_block == 0x8800
            && self.sample_rate == 44_100
            && self.channels == 2
            && self.flags == 15
            && matches!(
                (self.bytes_per_second, self.block_align),
                (12003, 4459) | (16002, 5945)
            );
        retail.then_some(Format {
            bytes_per_second: self.bytes_per_second,
            block_align: self.block_align as u16,
        })
    }
}

/// Counters for diagnostics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub packets: u64,
    pub pcm_frames: u64,
    /// Fresh decoders after a sequence break or input after the end.
    pub restarts: u64,
    /// Packets the decoder rejected.
    pub errors: u64,
    /// Partial packets dropped by an early packet start or a reset.
    pub dropped_partials: u64,
}

/// One guest codec instance.
pub struct Session {
    tables: Arc<Tables>,
    format: Format,
    decoder: Decoder<'static>,
    /// Bytes of the packet being assembled.
    partial: Vec<u8>,
    /// Interleaved stereo PCM not yet retrieved.
    pcm: VecDeque<i16>,
    /// The sequence number the next packet should carry.
    next_sequence: Option<u8>,
    /// The decoder has emitted its last packet.
    ended: bool,
    /// The last decoder error, for diagnostics.
    pub last_error: Option<crate::Error>,
    pub stats: Stats,
}

impl Session {
    /// Create, or [`UNSUPPORTED`] for anything but the retail profiles.
    pub fn new(tables: Arc<Tables>, params: &CreateParams) -> Result<Self, u32> {
        let format = params.format().ok_or(UNSUPPORTED)?;
        let decoder = Decoder::new_shared(Arc::clone(&tables), format).map_err(|_| UNSUPPORTED)?;
        Ok(Self {
            tables,
            format,
            decoder,
            partial: Vec::new(),
            pcm: VecDeque::new(),
            next_sequence: None,
            ended: false,
            last_error: None,
            stats: Stats::default(),
        })
    }

    pub fn format(&self) -> Format {
        self.format
    }

    /// Input bytes still owed for the current packet, 0 when one is complete.
    pub fn wants(&self) -> usize {
        usize::from(self.format.block_align).saturating_sub(self.partial.len())
    }

    /// Bytes from the game's input callback. `packet_start` is its return
    /// value [`PACKET_START`].
    pub fn feed(&mut self, bytes: &[u8], packet_start: bool) {
        if packet_start && !self.partial.is_empty() {
            self.partial.clear();
            self.stats.dropped_partials += 1;
        }
        self.partial.extend_from_slice(bytes);
    }

    /// Begin packet, after the adapter has fed what the callback had.
    /// Decodes a complete packet if there is one. Otherwise the callback ran
    /// dry, and `end_of_input` says whether that was the end of the file:
    /// in the game, the container's reader has passed its last data packet
    /// (`[container+4] > [container+24h]`, the test the container itself
    /// makes after [`NO_DATA`] at `0x388696`). Without it, begin returns
    /// [`NO_DATA`] and nothing is lost; at the end it finishes the stream
    /// once (returning 0 so the held packet is drained) and then returns
    /// [`NO_DATA`].
    pub fn begin(&mut self, end_of_input: bool) -> u32 {
        let align = usize::from(self.format.block_align);
        if self.partial.len() >= align {
            let packet: Vec<u8> = self.partial.drain(..align).collect();
            self.packet(&packet);
            return OK;
        }
        if !end_of_input || self.ended {
            return NO_DATA;
        }
        if !self.partial.is_empty() {
            self.partial.clear();
            self.stats.dropped_partials += 1;
        }
        self.end();
        OK
    }

    /// Decode: everything the begun packet produced is already queued.
    pub fn decode(&mut self) -> u32 {
        PACKET_DONE
    }

    /// Up to `max_frames` stereo frames of interleaved PCM.
    pub fn take_pcm(&mut self, max_frames: usize) -> Vec<i16> {
        let n = max_frames.saturating_mul(2).min(self.pcm.len());
        self.pcm.drain(..n).collect()
    }

    /// Stereo frames queued.
    pub fn queued_frames(&self) -> usize {
        self.pcm.len() / 2
    }

    /// Reset (a seek): forget input, queued PCM and decoder state.
    pub fn reset(&mut self) {
        if !self.partial.is_empty() {
            self.stats.dropped_partials += 1;
        }
        self.partial.clear();
        self.pcm.clear();
        self.restart();
        self.next_sequence = None;
        self.ended = false;
    }

    fn restart(&mut self) {
        self.decoder = Decoder::new_shared(Arc::clone(&self.tables), self.format)
            .expect("format accepted at create");
    }

    fn packet(&mut self, packet: &[u8]) {
        let sequence = packet[0] >> 4;
        if self.ended || self.next_sequence.is_some_and(|s| s != sequence) {
            // A loop or seek without a reset, or input after the end.
            if !self.ended {
                self.end();
            }
            self.restart();
            self.ended = false;
            self.stats.restarts += 1;
        }
        self.next_sequence = Some((sequence + 1) & 15);
        self.stats.packets += 1;
        match self.decoder.push(packet) {
            Ok(pcm) => self.queue(pcm),
            Err(e) => {
                self.stats.errors += 1;
                self.last_error = Some(e);
                self.restart();
                self.next_sequence = None;
            }
        }
    }

    /// Finishes the decoder, queueing its held packet.
    fn end(&mut self) {
        self.ended = true;
        match self.decoder.finish() {
            Ok(pcm) => self.queue(pcm),
            Err(e) => {
                self.stats.errors += 1;
                self.last_error = Some(e);
            }
        }
    }

    fn queue(&mut self, pcm: Vec<i16>) {
        self.stats.pcm_frames += pcm.len() as u64 / 2;
        self.pcm.extend(pcm);
    }

    /// Machine snapshots (docs/runtime/snapshots.md, phase 4): the whole
    /// instance, decoder included (overlap, reservoir packet, exponents,
    /// block sizes), so [`Session::load_state`] gives a session that
    /// produces exactly this one's PCM from the same input.
    pub fn save_state(&self, w: &mut Writer) {
        let Session {
            // Keep: the loading process's tables (the same verified XBE).
            tables: _,
            format,
            decoder,
            partial,
            pcm,
            next_sequence,
            ended,
            // Diagnostics: the error itself is not data.
            last_error: _,
            stats,
        } = self;
        let Format {
            bytes_per_second,
            block_align,
        } = format;
        w.u32(*bytes_per_second).u32(u32::from(*block_align));
        decoder.save(w);
        w.blob(partial);
        let queued: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
        w.blob(&queued);
        w.bool(next_sequence.is_some());
        w.u8(next_sequence.unwrap_or(0));
        w.bool(*ended);
        let Stats {
            packets,
            pcm_frames,
            restarts,
            errors,
            dropped_partials,
        } = stats;
        for v in [packets, pcm_frames, restarts, errors, dropped_partials] {
            w.u64(*v);
        }
    }

    /// A session saved with [`Session::save_state`], decoding with
    /// `tables`.
    pub fn load_state(tables: Arc<Tables>, r: &mut Reader<'_>) -> Result<Self, snapshot::Error> {
        let bad = |what: &str| snapshot::Error::Corrupt("wma", what.to_owned());
        let bytes_per_second = r.u32()?;
        let block_align = u16::try_from(r.u32()?).map_err(|_| bad("block align"))?;
        let format = Format {
            bytes_per_second,
            block_align,
        };
        let decoder = Decoder::load(Arc::clone(&tables), format, r)?;
        let partial = r.blob()?.to_vec();
        let queued = r.blob()?;
        if queued.len() % 4 != 0 {
            return Err(bad("PCM is not whole stereo frames"));
        }
        let pcm = queued
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_le_bytes(*b))
            .collect();
        let has_sequence = r.bool()?;
        let sequence = r.u8()?;
        if sequence > 15 {
            return Err(bad("packet sequence over 15"));
        }
        let ended = r.bool()?;
        let stats = Stats {
            packets: r.u64()?,
            pcm_frames: r.u64()?,
            restarts: r.u64()?,
            errors: r.u64()?,
            dropped_partials: r.u64()?,
        };
        Ok(Self {
            tables,
            format,
            decoder,
            partial,
            pcm,
            next_sequence: has_sequence.then_some(sequence),
            ended,
            last_error: None,
            stats,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RETAIL_96: CreateParams = CreateParams {
        version: 2,
        samples_per_block: 0x8800,
        sample_rate: 44_100,
        channels: 2,
        bytes_per_second: 12003,
        block_align: 4459,
        flags: 15,
    };

    /// A silent packet: `sequence`, one full-size uncoded stereo frame.
    fn silent(sequence: u8) -> Vec<u8> {
        let mut p = vec![0; 4459];
        p[0] = (sequence << 4) | 1;
        p
    }

    /// An MSB-first packet from `(value, width)` fields, zero padded.
    fn bits(fields: &[(u32, usize)]) -> Vec<u8> {
        let mut data = vec![0u8; 4459];
        let mut pos = 0;
        for &(value, width) in fields {
            for i in (0..width).rev() {
                data[pos / 8] |= (((value >> i) & 1) as u8) << (7 - pos % 8);
                pos += 1;
            }
        }
        data
    }

    fn session() -> Session {
        Session::new(Arc::new(Tables::synthetic()), &RETAIL_96).unwrap()
    }

    /// Feeds `packet` the way the game's callback does: 128-byte chunks,
    /// the first flagged as a packet start.
    fn feed(s: &mut Session, packet: &[u8]) {
        for (i, c) in packet.chunks(128).enumerate() {
            s.feed(c, i == 0);
        }
    }

    /// Everything a session gives the game from here on: per begin, its
    /// status, then all its PCM.
    fn play_out(s: &mut Session, packets: &[Vec<u8>]) -> Vec<(u32, Vec<i16>)> {
        let mut out = Vec::new();
        for p in packets {
            feed(s, p);
            let status = s.begin(false);
            out.push((status, s.take_pcm(usize::MAX)));
        }
        let status = s.begin(true);
        out.push((status, s.take_pcm(usize::MAX)));
        out
    }

    #[test]
    fn a_loaded_session_decodes_on_exactly_like_the_saved_one() {
        let mut a = session();
        for seq in 0..3 {
            feed(&mut a, &silent(seq));
            assert_eq!(a.begin(false), OK);
        }
        // Half a packet in hand, PCM not yet taken, and an overlap tail
        // the next block adds to its output.
        a.feed(&silent(3)[..2000], true);
        a.take_pcm(1000);
        a.decoder.fill_overlap(|i| (i as i32 % 300) - 150);
        let mut w = Writer::new();
        a.save_state(&mut w);
        let bytes = w.into_bytes();

        let rest = |s: &mut Session| {
            s.feed(&silent(3)[2000..], false);
            assert_eq!(s.begin(false), OK);
            let mut out = vec![(OK, s.take_pcm(usize::MAX))];
            out.extend(play_out(s, &[silent(4), silent(5), silent(9)]));
            (out, s.stats, s.wants(), s.queued_frames())
        };
        let want = rest(&mut a);
        assert!(
            want.0.iter().any(|(_, pcm)| pcm.iter().any(|v| *v != 0)),
            "the overlap tail is heard"
        );

        let mut r = Reader::new(&bytes, "wma");
        let mut b = Session::load_state(Arc::new(Tables::synthetic()), &mut r).unwrap();
        r.finish().unwrap();
        assert_eq!(rest(&mut b), want);
    }

    #[test]
    fn a_damaged_session_is_refused() {
        let mut w = Writer::new();
        session().save_state(&mut w);
        let mut bytes = w.into_bytes();
        // The first block size (after the format and an empty decoder's
        // pending flag and two empty exponent lists).
        let at = 8 + 1 + 2 * 8;
        bytes[at] = 3;
        let mut r = Reader::new(&bytes, "wma");
        assert!(Session::load_state(Arc::new(Tables::synthetic()), &mut r).is_err());
    }

    #[test]
    fn only_the_retail_profiles_are_created() {
        let tables = Arc::new(Tables::synthetic());
        assert!(Session::new(Arc::clone(&tables), &RETAIL_96).is_ok());
        let p128 = CreateParams {
            bytes_per_second: 16002,
            block_align: 5945,
            ..RETAIL_96
        };
        assert_eq!(
            Session::new(Arc::clone(&tables), &p128).unwrap().wants(),
            5945
        );
        for bad in [
            CreateParams {
                channels: 1,
                ..RETAIL_96
            },
            CreateParams {
                sample_rate: 22_050,
                ..RETAIL_96
            },
            CreateParams {
                version: 1,
                ..RETAIL_96
            },
            CreateParams {
                block_align: 5945,
                ..RETAIL_96
            },
            CreateParams {
                flags: 0,
                ..RETAIL_96
            },
            CreateParams {
                samples_per_block: 0x4400,
                ..RETAIL_96
            },
        ] {
            assert_eq!(
                Session::new(Arc::clone(&tables), &bad).err(),
                Some(UNSUPPORTED),
                "{bad:?}"
            );
        }
    }

    /// PCM lengths, in interleaved samples, the direct decoder produces for
    /// each push of `packets` and then for `finish`.
    fn direct_lengths(packets: &[Vec<u8>]) -> Vec<usize> {
        let tables = Tables::synthetic();
        let mut d = Decoder::new(&tables, RETAIL_96.format().unwrap()).unwrap();
        let mut v: Vec<usize> = packets.iter().map(|p| d.push(p).unwrap().len()).collect();
        v.push(d.finish().unwrap().len());
        v
    }

    #[test]
    fn packets_stream_through_and_the_end_drains_the_held_packet() {
        let packets: Vec<Vec<u8>> = (0..3).map(silent).collect();
        let want = direct_lengths(&packets);
        // A fresh decoder suppresses its first (silent) frame.
        assert_eq!(want.iter().sum::<usize>(), 2 * 4096);
        let mut s = session();
        // Partial input: begin waits for more.
        s.feed(&packets[0][..128], true);
        assert_eq!(s.begin(false), NO_DATA);
        s.feed(&packets[0][128..], false);
        assert_eq!(s.wants(), 0);
        assert_eq!(s.begin(false), OK);
        assert_eq!(s.decode(), PACKET_DONE);
        assert_eq!(2 * s.queued_frames(), want[0]);
        for (p, &n) in packets[1..].iter().zip(&want[1..]) {
            s.take_pcm(usize::MAX);
            feed(&mut s, p);
            assert_eq!(s.begin(false), OK);
            assert_eq!(2 * s.queued_frames(), n);
        }
        // Drained in pieces, as the container asks for at most 2048.
        let queued = s.queued_frames();
        assert_eq!(s.take_pcm(1).len(), 2.min(2 * queued));
        s.take_pcm(usize::MAX);
        // A dry callback before the end: nothing happens, nothing is lost.
        assert_eq!(s.begin(false), NO_DATA);
        assert_eq!(s.queued_frames(), 0);
        // End of input: the held packet comes out once, then no data.
        assert_eq!(s.begin(true), OK);
        assert_eq!(2 * s.queued_frames(), want[3]);
        assert_eq!(s.take_pcm(usize::MAX), vec![0; want[3]]);
        assert_eq!(s.begin(true), NO_DATA);
        assert_eq!(s.begin(false), NO_DATA);
        assert_eq!(s.stats.packets, 3);
        assert_eq!(s.stats.pcm_frames, 2 * 2048);
    }

    #[test]
    fn the_same_packets_decode_as_the_direct_decoder_does() {
        let tables = Arc::new(Tables::synthetic());
        let packets: Vec<Vec<u8>> = (0..5).map(silent).collect();
        let mut direct = Decoder::new(&tables, RETAIL_96.format().unwrap()).unwrap();
        let mut want = Vec::new();
        for p in &packets {
            want.extend(direct.push(p).unwrap());
        }
        want.extend(direct.finish().unwrap());
        let mut s = Session::new(Arc::clone(&tables), &RETAIL_96).unwrap();
        let mut got = Vec::new();
        for p in &packets {
            feed(&mut s, p);
            assert_eq!(s.begin(false), OK);
            got.extend(s.take_pcm(usize::MAX));
        }
        assert_eq!(s.begin(true), OK);
        got.extend(s.take_pcm(usize::MAX));
        assert_eq!(s.begin(true), NO_DATA);
        assert_eq!(got, want);
        assert_eq!(got.len(), 4 * 4096);
    }

    #[test]
    fn breaks_and_damage_restart_the_decoder_instead_of_failing() {
        let two = direct_lengths(&[silent(0), silent(1)]);
        assert!(two[2] > 0);
        let mut s = session();
        feed(&mut s, &silent(0));
        s.begin(false);
        feed(&mut s, &silent(1));
        s.begin(false);
        assert_eq!(2 * s.queued_frames(), two[1]);
        s.take_pcm(usize::MAX);
        // Sequence jump (a loop): the held packet is finished, a fresh
        // decoder takes the new one.
        feed(&mut s, &silent(7));
        assert_eq!(s.begin(false), OK);
        assert_eq!(2 * s.queued_frames(), two[2]);
        assert_eq!(s.stats.restarts, 1);
        // A malformed packet (total gain above 512, as in the decoder's own
        // test) fails when the next packet lets it decode: counted, and a
        // fresh decoder starts after it.
        let mut fields = vec![(8, 4), (1, 4), (0, 13), (0, 9), (0, 1), (3, 2)];
        fields.extend([(127, 7); 4]);
        fields.push((4, 7));
        feed(&mut s, &bits(&fields));
        assert_eq!(s.begin(false), OK);
        assert_eq!(s.stats.errors, 0);
        feed(&mut s, &silent(9));
        assert_eq!(s.begin(false), OK);
        assert_eq!(s.stats.errors, 1);
        assert!(s.last_error.is_some());
        // And the stream carries on with the packets after it.
        let before = s.queued_frames();
        // (A fresh decoder holds one packet and suppresses its first frame.)
        for p in 10..13 {
            feed(&mut s, &silent(p));
            assert_eq!(s.begin(false), OK);
        }
        assert!(s.queued_frames() > before);
        s.take_pcm(usize::MAX);
        // A packet start with bytes still owed drops the partial packet.
        s.feed(&[0; 100], true);
        feed(&mut s, &silent(13));
        assert_eq!(s.stats.dropped_partials, 1);
        assert_eq!(s.begin(false), OK);
        // Reset forgets everything; a new stream starts cleanly.
        s.feed(&[0; 10], true);
        s.reset();
        assert_eq!((s.queued_frames(), s.wants()), (0, 4459));
        for p in [silent(3), silent(4)] {
            feed(&mut s, &p);
            assert_eq!(s.begin(false), OK);
        }
        assert_eq!(s.begin(true), OK, "end of input");
        assert_eq!(2 * s.queued_frames(), two.iter().sum::<usize>());
    }

    #[test]
    fn a_dry_reader_between_packets_loses_nothing() {
        let packets: Vec<Vec<u8>> = (0..6).map(silent).collect();
        let tables = Arc::new(Tables::synthetic());
        let mut direct = Decoder::new(&tables, RETAIL_96.format().unwrap()).unwrap();
        let mut want = Vec::new();
        for p in &packets {
            want.extend(direct.push(p).unwrap());
        }
        want.extend(direct.finish().unwrap());
        assert_eq!(want.len(), 5 * 4096);
        let mut s = Session::new(tables, &RETAIL_96).unwrap();
        let mut got = Vec::new();
        for (i, p) in packets.iter().enumerate() {
            feed(&mut s, p);
            assert_eq!(s.begin(false), OK);
            got.extend(s.take_pcm(usize::MAX));
            if i == 2 {
                // The reader has nothing yet, twice; then it catches up.
                assert_eq!(s.begin(false), NO_DATA);
                assert_eq!(s.begin(false), NO_DATA);
            }
        }
        assert_eq!(s.begin(true), OK);
        got.extend(s.take_pcm(usize::MAX));
        assert_eq!(got, want);
        assert_eq!((s.stats.restarts, s.stats.pcm_frames), (0, 5 * 2048));
    }

    #[test]
    fn a_partial_packet_at_the_end_is_dropped() {
        let mut s = session();
        feed(&mut s, &silent(0));
        assert_eq!(s.begin(false), OK);
        s.feed(&silent(1)[..300], true);
        assert_eq!(s.begin(false), NO_DATA, "still arriving");
        assert_eq!(s.begin(true), OK);
        assert_eq!(s.stats.dropped_partials, 1);
        assert_eq!(s.begin(true), NO_DATA);
    }
}
