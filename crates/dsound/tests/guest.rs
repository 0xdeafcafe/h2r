//! End-to-end tests over synthetic guest memory. The calls go through
//! `overrides::invoke` with the real Halo 2 addresses, so they exercise the
//! same path the recompiled game will: stdcall arguments in, EAX and guest
//! memory out. No game data is involved.

use std::sync::{Arc, Mutex};

use audio::Mixer;
use dsound::abi::*;
use dsound::overrides::{invoke, Return};
use dsound::testing::TestHost;
use dsound::{DirectSound, GuestMemory, Layout, Stop};

const RATE: u32 = 8000;

// Halo 2 5849 addresses, from docs/dsound.md.
const CREATE: u32 = 0x0037d797;
const CREATE_BUFFER: u32 = 0x0037d4be;
const CREATE_STREAM: u32 = 0x0037d4e2;
const SET_BUFFER_DATA: u32 = 0x0037cc4a;
const LOCK: u32 = 0x0037b7b3;
const UNLOCK: u32 = 0x00379f40;
const PLAY: u32 = 0x0037b6df;
const STOP: u32 = 0x0037b703;
const PAUSE: u32 = 0x0037b73f;
const GET_STATUS: u32 = 0x0037b75b;
const GET_POS: u32 = 0x0037b777;
const SET_POS: u32 = 0x0037b797;
const SET_VOLUME: u32 = 0x0037b66f;
const BUF_RELEASE: u32 = 0x00379f45;
const DS_RELEASE: u32 = 0x00379f2a;
const PROCESS: u32 = 0x0037ad25;
const STREAM_STATUS: u32 = 0x0037acd4;
const FLUSH: u32 = 0x0037ac89;
const FLUSH_EX: u32 = 0x0037b827;
const DO_WORK: u32 = 0x0037b844;
const STREAM_RELEASE: u32 = 0x0037ab87;
const STREAM_PAUSE: u32 = 0x0037b822;

struct Rig {
    h: TestHost,
    ds: DirectSound,
    mixer: Arc<Mutex<Mixer>>,
    ds_ptr: u32,
    /// Next free scratch offset.
    cursor: u32,
}

impl Rig {
    fn new() -> Self {
        let mixer = Arc::new(Mutex::new(Mixer::new(RATE)));
        let mut rig = Rig {
            h: TestHost::new(),
            ds: DirectSound::new(Arc::clone(&mixer), Layout::default()),
            mixer,
            ds_ptr: 0,
            cursor: 0,
        };
        let out = rig.scratch(4);
        assert_eq!(rig.call(CREATE, &[0, out, 0]).eax, S_OK);
        rig.ds_ptr = rig.h.read_u32(out).unwrap();
        assert_ne!(rig.ds_ptr, 0);
        rig
    }

    /// Reserves `n` bytes of guest scratch and returns the address.
    fn scratch(&mut self, n: u32) -> u32 {
        let p = self.h.scratch() + self.cursor;
        self.cursor += (n + 15) & !15;
        p
    }

    fn put(&mut self, bytes: &[u8]) -> u32 {
        let p = self.scratch(bytes.len() as u32);
        self.h.write(p, bytes).unwrap();
        p
    }

    fn call(&mut self, va: u32, args: &[u32]) -> Return {
        invoke(&mut self.ds, &mut self.h, va, args).expect("no stop")
    }

    fn stop_of(&mut self, va: u32, args: &[u32]) -> Stop {
        invoke(&mut self.ds, &mut self.h, va, args).expect_err("expected a stop")
    }

    fn wfx(&mut self, tag: u16, channels: u16, rate: u32, bits: u16, align: u16) -> u32 {
        let mut b = Vec::new();
        b.extend(tag.to_le_bytes());
        b.extend(channels.to_le_bytes());
        b.extend(rate.to_le_bytes());
        b.extend((rate * align as u32).to_le_bytes());
        b.extend(align.to_le_bytes());
        b.extend(bits.to_le_bytes());
        b.extend(0u16.to_le_bytes());
        self.put(&b)
    }

    fn buffer_desc(&mut self, flags: u32, bytes: u32, wfx: u32) -> u32 {
        let mut b = Vec::new();
        for v in [BufferDesc::SIZE, flags, bytes, wfx, 0, 0] {
            b.extend(v.to_le_bytes());
        }
        self.put(&b)
    }

    fn create_buffer(&mut self, flags: u32, bytes: u32, wfx: u32) -> u32 {
        let desc = self.buffer_desc(flags, bytes, wfx);
        let out = self.scratch(4);
        let r = self.call(CREATE_BUFFER, &[self.ds_ptr, desc, out, 0]);
        assert_eq!(r.eax, S_OK);
        assert_eq!(r.pop_bytes, 16);
        self.h.read_u32(out).unwrap()
    }

    fn create_stream(&mut self, wfx: u32, max_packets: u32, callback: u32, ctx: u32) -> u32 {
        let mut b = Vec::new();
        for v in [0u32, max_packets, wfx, callback, ctx, 0] {
            b.extend(v.to_le_bytes());
        }
        let desc = self.put(&b);
        let out = self.scratch(4);
        assert_eq!(self.call(CREATE_STREAM, &[self.ds_ptr, desc, out, 0]).eax, S_OK);
        self.h.read_u32(out).unwrap()
    }

    /// Halo 2's call at 0x1913a4: ("DSPImage", {9, 10}, flags 1, &desc).
    /// Returns the descriptor pointer.
    fn download_dry_image(&mut self) -> u32 {
        let name = self.put(b"DSPImage\0");
        let loc = self.put(&[9, 0, 0, 0, 10, 0, 0, 0]);
        let pp = self.scratch(4);
        assert_eq!(self.call(0x0037b86d, &[name, loc, 1, pp]).eax, S_OK);
        self.h.read_u32(pp).unwrap()
    }

    fn status(&mut self, buf: u32) -> u32 {
        let out = self.scratch(4);
        assert_eq!(self.call(GET_STATUS, &[buf, out]).eax, S_OK);
        self.h.read_u32(out).unwrap()
    }

    fn render(&self, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; frames * 2];
        self.mixer.lock().unwrap().render(&mut out);
        out
    }

    /// Queues one packet, returning (status ptr, completed ptr).
    fn process(&mut self, stream: u32, data: &[u8], event_or_ctx: u32) -> (u32, u32) {
        let buf = self.put(data);
        let status = self.scratch(4);
        let done = self.scratch(4);
        let mut p = Vec::new();
        for v in [buf, data.len() as u32, done, status, event_or_ctx, 0] {
            p.extend(v.to_le_bytes());
        }
        let pkt = self.put(&p);
        assert_eq!(self.call(PROCESS, &[stream, pkt, 0]).eax, S_OK);
        (status, done)
    }
}

fn pcm16(samples: &[i16]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}

/// One mono Xbox ADPCM block (36 bytes): predictor 1000, step index 0, and
/// data nibbles 7 then 0xF first (decoded by hand in crates/audio to 1011
/// then 981), the rest zero.
fn adpcm_block() -> Vec<u8> {
    let mut b = vec![0u8; 36];
    b[..2].copy_from_slice(&1000i16.to_le_bytes());
    b[4] = 0xF7;
    b
}

#[test]
fn create_is_a_singleton_with_refcount() {
    let mut rig = Rig::new();
    let out = rig.scratch(4);
    rig.call(CREATE, &[0, out, 0]);
    assert_eq!(rig.h.read_u32(out).unwrap(), rig.ds_ptr);
    assert_eq!(rig.call(DS_RELEASE, &[rig.ds_ptr]).eax, 1);
    assert_eq!(rig.call(DS_RELEASE, &[rig.ds_ptr]).eax, 0);
    assert_eq!(rig.ds.live_objects(), 0);
    assert!(matches!(
        invoke(&mut rig.ds, &mut rig.h, DS_RELEASE, &[rig.ds_ptr]),
        Err(Stop::BadHandle { .. })
    ));
}

#[test]
fn adpcm_blob_in_guest_memory_plays_through_the_mixer() {
    let mut rig = Rig::new();
    // Xbox ADPCM, mono, 8000 Hz: tag 0x69, 4 bits, block align 36.
    let wfx = rig.wfx(0x69, 1, RATE, 4, 36);
    // dwBufferBytes = 0: the sample data comes later through SetBufferData,
    // as Halo 2 does for its sound cache.
    let buf = rig.create_buffer(0x80, 0, wfx);

    let mut blob = Vec::new();
    for _ in 0..2 {
        blob.extend(adpcm_block());
    }
    let pv = rig.put(&blob);
    let r = rig.call(SET_BUFFER_DATA, &[buf, pv, blob.len() as u32]);
    assert_eq!((r.eax, r.pop_bytes), (S_OK, 12));
    assert_eq!(rig.call(SET_VOLUME, &[buf, 0]).eax, S_OK);
    assert_eq!(rig.status(buf), 0);

    assert_eq!(rig.call(PLAY, &[buf, 0, 0, 0]).eax, S_OK);
    assert_eq!(rig.status(buf), DSBSTATUS_PLAYING);

    let out = rig.render(128);
    // Frame 0 is the header predictor, then the hand-decoded 1011 and 981.
    let s = |v: f32| v / 32768.0;
    assert_eq!(out[0], s(1000.0));
    assert_eq!(out[2], s(1011.0));
    assert_eq!(out[4], s(981.0));
    // Mono is duplicated to both channels.
    assert_eq!(out[0], out[1]);
    // 2 blocks = 128 frames at the host rate: the buffer ended and stopped.
    assert_eq!(rig.status(buf), 0);
}

#[test]
fn lock_returns_guest_pointers_and_unlock_publishes_writes() {
    let mut rig = Rig::new();
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let buf = rig.create_buffer(0x80, 16, wfx); // 8 samples, owned guest memory
    let (p1, c1, p2, c2) = (rig.scratch(4), rig.scratch(4), rig.scratch(4), rig.scratch(4));

    // Lock the last 4 bytes plus a wrapped 4 bytes from the start.
    assert_eq!(rig.call(LOCK, &[buf, 12, 8, p1, c1, p2, c2, 0]).eax, S_OK);
    let (a, ca) = (rig.h.read_u32(p1).unwrap(), rig.h.read_u32(c1).unwrap());
    let (b, cb) = (rig.h.read_u32(p2).unwrap(), rig.h.read_u32(c2).unwrap());
    assert_eq!((ca, cb), (4, 4));
    assert_eq!(a - b, 12, "second region is the buffer start, first is +12");

    // Whole buffer via DSBLOCK_ENTIREBUFFER, then write 8 samples of 16384.
    assert_eq!(rig.call(LOCK, &[buf, 0, 0, p1, c1, 0, 0, DSBLOCK_ENTIREBUFFER]).eax, S_OK);
    let base = rig.h.read_u32(p1).unwrap();
    assert_eq!(base, b);
    rig.h.write(base, &pcm16(&[16384; 8])).unwrap();
    let r = rig.call(UNLOCK, &[buf, base, 16, 0, 0]);
    assert_eq!((r.eax, r.pop_bytes), (S_OK, 20));

    rig.call(PLAY, &[buf, 0, 0, DSBPLAY_LOOPING]);
    let out = rig.render(16);
    assert!(out.iter().all(|&v| v == 0.5), "looping buffer fills all 16 frames");
    assert_eq!(rig.status(buf), DSBSTATUS_PLAYING | DSBSTATUS_LOOPING);

    // Position, stop, restart from a position.
    let (play, write) = (rig.scratch(4), rig.scratch(4));
    assert_eq!(rig.call(GET_POS, &[buf, play, write]).eax, S_OK);
    assert_eq!(rig.h.read_u32(play).unwrap() % 2, 0);
    rig.call(STOP, &[buf]);
    assert_eq!(rig.status(buf), 0);
    rig.call(SET_POS, &[buf, 4]);
    rig.call(GET_POS, &[buf, play, 0]);
    assert_eq!(rig.h.read_u32(play).unwrap(), 4);

    // Pause keeps the cursor and resumes; Stop clears pause.
    rig.call(PLAY, &[buf, 0, 0, DSBPLAY_LOOPING]);
    rig.call(PAUSE, &[buf, 1]);
    assert_eq!(rig.status(buf), DSBSTATUS_PAUSED | DSBSTATUS_LOOPING);
    assert!(rig.render(4).iter().all(|&v| v == 0.0));
    rig.call(PAUSE, &[buf, 0]);
    assert_eq!(rig.status(buf), DSBSTATUS_PLAYING | DSBSTATUS_LOOPING);
}

#[test]
fn bad_calls_return_hresults_not_stops() {
    let mut rig = Rig::new();
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let buf = rig.create_buffer(0x80, 0, wfx);
    // No data yet: Play and Lock are invalid calls.
    assert_eq!(rig.call(PLAY, &[buf, 0, 0, 0]).eax, DSERR_INVALIDCALL);
    let sink = rig.scratch(4);
    assert_eq!(rig.call(LOCK, &[buf, 0, 4, sink, sink, 0, 0, 0]).eax, DSERR_INVALIDCALL);
    // Odd length and reserved flags.
    let pv = rig.put(&[0; 8]);
    assert_eq!(rig.call(SET_BUFFER_DATA, &[buf, pv, 3]).eax, E_INVALIDARG);
    rig.call(SET_BUFFER_DATA, &[buf, pv, 8]);
    assert_eq!(rig.call(PLAY, &[buf, 0, 0, 0x80]).eax, E_INVALIDARG);
    assert_eq!(rig.call(SET_VOLUME, &[buf, 100]).eax, E_INVALIDARG);
}

#[test]
fn unimplemented_is_a_stop_never_success() {
    let mut rig = Rig::new();
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let buf = rig.create_buffer(0x80, 8, wfx);
    let sink = rig.scratch(4);
    // The file-based form of the effects image is not modelled.
    let name = rig.put(b"dsstdfx.bin\0");
    let loc = rig.put(&[9, 0, 0, 0, 10, 0, 0, 0]);
    let pp = rig.scratch(4);
    assert!(matches!(
        rig.stop_of(0x0037b86d, &[name, loc, 0, pp]),
        Stop::Unimplemented { .. }
    ));
    assert!(matches!(rig.stop_of(0x0037b7e3, &[buf, 0]), Stop::Unimplemented { .. }));
    assert!(matches!(
        rig.stop_of(LOCK, &[buf, 0, 4, sink, sink, 0, 0, DSBLOCK_FROMWRITECURSOR]),
        Stop::Unimplemented { .. }
    ));
    // A garbage handle is a stop, not a guest-visible error.
    assert!(matches!(rig.stop_of(PLAY, &[0xdead0000, 0, 0, 0]), Stop::BadHandle { .. }));
    // A format-less data buffer needs SetFormat, which is not implemented.
    let desc = rig.buffer_desc(0x80, 0, 0);
    let out = rig.scratch(4);
    assert!(matches!(
        rig.stop_of(CREATE_BUFFER, &[rig.ds_ptr, desc, out, 0]),
        Stop::Unimplemented { .. }
    ));
}

#[test]
fn mix_in_buffers_are_accepted_but_reported() {
    let mut rig = Rig::new();
    // The Halo 2 init pattern: flags 0x100000, no data, no format.
    let buf = rig.create_buffer(0x0010_0000, 0, 0);
    assert_eq!(rig.call(PLAY, &[buf, 0, 0, 0]).eax, S_OK);
    assert_eq!(rig.status(buf), DSBSTATUS_PLAYING | DSBSTATUS_LOOPING);
    assert_eq!(rig.ds.ignored().get("mix-in buffer Play (no DSP, silent)"), Some(&1));
    assert!(rig.render(4).iter().all(|&v| v == 0.0));
}

#[test]
fn stream_packets_complete_in_order_with_status_and_events() {
    let mut rig = Rig::new();
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let stream = rig.create_stream(wfx, 8, 0, 0);
    // The guest's virtual calls land on the override addresses.
    assert_eq!(rig.h.read_u32(stream).unwrap(), Layout::default().stream_vtable);

    let mut ready = rig.scratch(4);
    assert_eq!(rig.call(STREAM_STATUS, &[stream, ready]).eax, S_OK);
    assert_eq!(rig.h.read_u32(ready).unwrap(), XMO_STATUSF_ACCEPT_INPUT_DATA);
    ready = 0;
    let _ = ready;

    // Three packets of 4, 2 and 6 frames; events 0x101..0x103.
    let sizes = [4usize, 2, 6];
    let mut slots = Vec::new();
    for (i, n) in sizes.iter().enumerate() {
        let data = pcm16(&vec![8192; *n]);
        slots.push(rig.process(stream, &data, 0x101 + i as u32));
    }
    for (status, done) in &slots {
        assert_eq!(rig.h.read_u32(*status).unwrap(), XMEDIAPACKET_STATUS_PENDING);
        assert_eq!(rig.h.read_u32(*done).unwrap(), 0);
    }
    assert!(rig.h.signalled.is_empty());

    // Mix 5 frames: packet 0 (4 frames) is done, packet 1 is half played.
    let out = rig.render(5);
    assert_eq!(out[0], 0.25);
    rig.call(DO_WORK, &[]);
    assert_eq!(rig.h.read_u32(slots[0].0).unwrap(), XMEDIAPACKET_STATUS_SUCCESS);
    assert_eq!(rig.h.read_u32(slots[0].1).unwrap(), 8);
    assert_eq!(rig.h.read_u32(slots[1].0).unwrap(), XMEDIAPACKET_STATUS_PENDING);
    assert_eq!(rig.h.signalled, vec![0x101]);

    // Finish the rest: completion order is submission order.
    rig.render(16);
    rig.call(DO_WORK, &[]);
    assert_eq!(rig.h.signalled, vec![0x101, 0x102, 0x103]);
    for (i, (status, done)) in slots.iter().enumerate() {
        assert_eq!(rig.h.read_u32(*status).unwrap(), XMEDIAPACKET_STATUS_SUCCESS);
        assert_eq!(rig.h.read_u32(*done).unwrap(), 2 * sizes[i] as u32);
    }
    // A second DoWork does not re-signal.
    rig.call(DO_WORK, &[]);
    assert_eq!(rig.h.signalled.len(), 3);
}

#[test]
fn stream_callback_replaces_event_and_gets_contexts() {
    let mut rig = Rig::new();
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let stream = rig.create_stream(wfx, 0, 0x0040_1000, 0xC0FFEE);
    let (status, _) = rig.process(stream, &pcm16(&[1000; 4]), 0x55);
    rig.render(8);
    rig.call(DO_WORK, &[]);
    assert!(rig.h.signalled.is_empty());
    assert_eq!(rig.h.callbacks.len(), 1);
    let cb = rig.h.callbacks[0];
    assert_eq!(
        (cb.function, cb.stream_context, cb.packet_context, cb.status),
        (0x0040_1000, 0xC0FFEE, 0x55, XMEDIAPACKET_STATUS_SUCCESS)
    );
    assert_eq!(rig.h.read_u32(status).unwrap(), XMEDIAPACKET_STATUS_SUCCESS);
}

#[test]
fn stream_flush_pause_and_capacity() {
    let mut rig = Rig::new();
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let stream = rig.create_stream(wfx, 2, 0, 0);
    let a = rig.process(stream, &pcm16(&[8192; 8]), 0x1);
    let b = rig.process(stream, &pcm16(&[8192; 8]), 0x2);

    // The stream is full: Process fails with DSERR_INVALIDCALL, status clear.
    let status = rig.scratch(4);
    assert_eq!(rig.call(STREAM_STATUS, &[stream, status]).eax, S_OK);
    assert_eq!(rig.h.read_u32(status).unwrap(), 0);
    let pkt = rig.put(&[0u8; 24]);
    assert_eq!(rig.call(PROCESS, &[stream, pkt, 0]).eax, DSERR_INVALIDCALL);

    // Paused streams consume nothing.
    rig.call(STREAM_PAUSE, &[stream, 1]);
    assert!(rig.render(4).iter().all(|&v| v == 0.0));
    rig.call(STREAM_PAUSE, &[stream, 0]);
    rig.render(2);

    // Flush completes both with the flushed status and signals both events.
    assert_eq!(rig.call(FLUSH, &[stream]).eax, S_OK);
    for (st, done) in [a, b] {
        assert_eq!(rig.h.read_u32(st).unwrap(), XMEDIAPACKET_STATUS_FLUSHED);
        // Verified in the binary (0x37f850 -> 0x37f3a8): a flushed packet
        // reports its full dwMaxSize as completed.
        assert_eq!(rig.h.read_u32(done).unwrap(), 16);
    }
    assert_eq!(rig.h.signalled, vec![0x1, 0x2]);

    // FlushEx(timestamp = 0) is the same; a future timestamp is a stop.
    assert_eq!(rig.call(FLUSH_EX, &[stream, 0, 0, 0]).eax, S_OK);
    assert!(matches!(rig.stop_of(FLUSH_EX, &[stream, 5, 0, 0]), Stop::Unimplemented { .. }));
}

#[test]
fn release_frees_buffers_and_streams() {
    let mut rig = Rig::new();
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let buf = rig.create_buffer(0x80, 8, wfx);
    let stream = rig.create_stream(wfx, 0, 0, 0);
    assert_eq!(rig.ds.live_objects(), 3);
    assert_eq!(rig.call(BUF_RELEASE, &[buf]).eax, 0);
    assert_eq!(rig.call(STREAM_RELEASE, &[stream]).eax, 0);
    assert_eq!(rig.ds.live_objects(), 1);
    assert!(rig.h.freed.contains(&buf) && rig.h.freed.contains(&stream));
    // The mixer lost both voices.
    assert_eq!(rig.mixer.lock().unwrap().active_voices(), 0);
}

#[test]
fn three_d_state_is_stored_and_pans_ctrl3d_buffers() {
    let mut rig = Rig::new();
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let buf = rig.create_buffer(0x80 | DSBCAPS_CTRL3D, 8, wfx);
    let (p1, c1) = (rig.scratch(4), rig.scratch(4));
    rig.call(LOCK, &[buf, 0, 0, p1, c1, 0, 0, DSBLOCK_ENTIREBUFFER]);
    let base = rig.h.read_u32(p1).unwrap();
    rig.h.write(base, &pcm16(&[16384; 4])).unwrap();
    rig.call(UNLOCK, &[buf, base, 8, 0, 0]);

    let f = |v: f32| v.to_bits();
    // Listener at the origin; source to the right.
    assert_eq!(rig.call(0x0037d598, &[rig.ds_ptr, f(1.0), f(2.0), f(3.0), 0]).eax, S_OK);
    assert_eq!(rig.ds.listener().position, [1.0, 2.0, 3.0]);
    rig.call(0x0037d598, &[rig.ds_ptr, f(0.0), f(0.0), f(0.0), 0]);
    assert_eq!(rig.call(0x0037d506, &[rig.ds_ptr, f(3.048), 0]).eax, S_OK);
    assert_eq!(rig.ds.listener().distance_factor, 3.048);
    assert_eq!(rig.call(0x0037c668, &[buf, f(5.0), f(0.0), f(0.0), 0]).eax, S_OK);
    rig.call(0x0037c620, &[buf, f(1.0e9), 1]);
    rig.call(0x0037c644, &[buf, f(1.0), 1]);
    rig.call(PLAY, &[buf, 0, 0, DSBPLAY_LOOPING]);
    let out = rig.render(2);
    assert!(out[1] > out[0], "source on the right is louder on the right: {out:?}");
    // Cone, doppler etc. are accepted but counted.
    rig.call(0x0037c600, &[buf, 0, 1]);
    assert_eq!(rig.ds.ignored().get("SetConeOutsideVolume"), Some(&1));
}

#[test]
fn stored_state_setters() {
    let mut rig = Rig::new();
    // SetMixBinHeadroom(ds, bin, headroom); bin out of range is E_INVALIDARG.
    assert_eq!(rig.call(0x0037b637, &[rig.ds_ptr, 5, 3]).eax, S_OK);
    assert_eq!(rig.ds.mix_bin_headroom(5), Some(3));
    assert_eq!(rig.call(0x0037b637, &[rig.ds_ptr, 32, 3]).eax, E_INVALIDARG);
    // Speaker config is plain stereo, as Halo 2's init checks.
    let out = rig.scratch(4);
    rig.call(0x0037b5ca, &[rig.ds_ptr, out]);
    assert_eq!(rig.h.read_u32(out).unwrap() & 0xFFFF, DSSPEAKER_STEREO);
    // SetEffectData is stored and read back (needs the effects image).
    rig.download_dry_image();
    let data = rig.put(&[1, 2, 3, 4, 5, 6, 7, 8]);
    rig.call(0x0037b60d, &[rig.ds_ptr, 4, 0x20, data, 8, 0]);
    let back = rig.scratch(8);
    assert_eq!(rig.call(0x0037b5e6, &[rig.ds_ptr, 4, 0x20, back, 8]).eax, S_OK);
    assert_eq!(rig.h.read_vec(back, 8).unwrap(), vec![1, 2, 3, 4, 5, 6, 7, 8]);
}

#[test]
fn pitch_maps_to_frequency_about_48khz() {
    assert_eq!(hz_to_pitch(48_000), 0);
    assert_eq!(hz_to_pitch(96_000), 4096);
    assert_eq!(pitch_to_hz(0), 48_000);
    assert_eq!(pitch_to_hz(-4096), 24_000);
    // Pitch has 1/4096-octave resolution: round trips land within a Hz.
    assert!(pitch_to_hz(hz_to_pitch(22_050)).abs_diff(22_050) <= 1);
}

// ---- review round 2 regressions ------------------------------------------

const SYNCH_PLAYBACK: u32 = 0x0037b657;
const SET_EFFECT_DATA_X: u32 = 0x0037ba6f;
const FAR: u32 = 0xffff_fffc;

/// Fills `buf` (16 bytes, 8 mono samples) with a constant via Lock/Unlock.
fn fill(rig: &mut Rig, buf: u32, sample: i16) {
    let (p1, c1) = (rig.scratch(4), rig.scratch(4));
    rig.call(LOCK, &[buf, 0, 0, p1, c1, 0, 0, DSBLOCK_ENTIREBUFFER]);
    let base = rig.h.read_u32(p1).unwrap();
    rig.h.write(base, &pcm16(&[sample; 8])).unwrap();
    rig.call(UNLOCK, &[buf, base, 16, 0, 0]);
}

#[test]
fn r01_xaudio_set_effect_data_has_three_args_and_does_not_panic() {
    assert_eq!(dsound::overrides::entry(SET_EFFECT_DATA_X).unwrap().argc, 3);
    let mut rig = Rig::new();
    // The binary reads the descriptor, updates state and may write through
    // arg 3 (an output pointer). None of that is modelled, so it must stop,
    // with or without an image, and whatever the (even unreadable) pointers.
    for args in [[1, 2, 0], [3, 0x1000, 0x2000], [15, 2, 0]] {
        assert!(matches!(rig.stop_of(SET_EFFECT_DATA_X, &args), Stop::Unimplemented { .. }));
    }
    rig.download_dry_image();
    assert!(matches!(rig.stop_of(SET_EFFECT_DATA_X, &[3, 2, 0]), Stop::Unimplemented { .. }));
}

#[test]
fn dry_effects_image_satisfies_halos_init_reads() {
    let mut rig = Rig::new();
    let sink = rig.scratch(8);
    // Before the download there is no image to read from.
    assert_eq!(rig.call(0x0037b5e6, &[rig.ds_ptr, 4, 0x20, sink, 8]).eax, DSERR_INVALIDCALL);
    let desc = rig.download_dry_image();
    assert_ne!(desc, 0, "Halo's 0x55787 treats a null descriptor as 'no effects'");
    // 0x19142f: the effect-state path is only taken when desc[0] == 0xF.
    assert_eq!(rig.h.read_u32(desc).unwrap(), 15);
    // The snapshot reads at 0x1913d0..0x191411: effects 4..7, offset 0x20, 8 bytes.
    rig.h.write(sink, &[0xAA; 8]).unwrap();
    for fx in 4..=7 {
        assert_eq!(rig.call(0x0037b5e6, &[rig.ds_ptr, fx, 0x20, sink, 8]).eax, S_OK);
        assert_eq!(rig.h.read_vec(sink, 8).unwrap(), vec![0; 8], "dry default is zeros");
    }
    // 0x191451: 0x3c bytes at offset 0 of effects 1 and 0xb..0xe; word 1 is a
    // DSP address Halo range-checks, and zero maps to "no mix bin".
    let big = rig.scratch(0x40);
    for fx in [1, 0xb, 0xc, 0xd, 0xe] {
        assert_eq!(rig.call(0x0037b5e6, &[rig.ds_ptr, fx, 0, big, 0x3c]).eax, S_OK);
        assert_eq!(rig.h.read_u32(big + 4).unwrap(), 0);
    }
    // The write-back at 0x19128f round-trips; partial reads see it.
    let data = rig.put(&[1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(rig.call(0x0037b60d, &[rig.ds_ptr, 4, 0x20, data, 8, 0]).eax, S_OK);
    rig.call(0x0037b5e6, &[rig.ds_ptr, 4, 0x24, sink, 4]);
    assert_eq!(rig.h.read_vec(sink, 4).unwrap(), vec![5, 6, 7, 8]);
    rig.call(0x0037b5e6, &[rig.ds_ptr, 5, 0x20, sink, 8]);
    assert_eq!(rig.h.read_vec(sink, 8).unwrap(), vec![0; 8], "effects are independent");
    // Out-of-range effect or offset is E_INVALIDARG, not a panic.
    assert_eq!(rig.call(0x0037b5e6, &[rig.ds_ptr, 15, 0, sink, 4]).eax, E_INVALIDARG);
    assert_eq!(rig.call(0x0037b5e6, &[rig.ds_ptr, 4, 0xffff_fffc, sink, 8]).eax, E_INVALIDARG);
    // A second download returns the same descriptor.
    assert_eq!(rig.download_dry_image(), desc);
}

#[test]
fn effects_image_argument_checks() {
    let mut rig = Rig::new();
    let name = rig.put(b"DSPImage\0");
    let pp = rig.scratch(4);
    // Reverb index outside the image's 15 effects.
    let bad = rig.put(&[15, 0, 0, 0, 10, 0, 0, 0]);
    assert_eq!(rig.call(0x0037b86d, &[name, bad, 1, pp]).eax, E_INVALIDARG);
    // Null out-pointer.
    let loc = rig.put(&[9, 0, 0, 0, 10, 0, 0, 0]);
    assert_eq!(rig.call(0x0037b86d, &[name, loc, 1, 0]).eax, E_INVALIDARG);
    // Descriptor location at the top of the address space faults.
    assert!(matches!(rig.stop_of(0x0037b86d, &[name, FAR, 1, pp]), Stop::Memory(_)));
    // An unmapped name faults and installs nothing.
    assert!(matches!(rig.stop_of(0x0037b86d, &[0x2000, loc, 1, pp]), Stop::Memory(_)));
    let sink = rig.scratch(8);
    assert_eq!(rig.call(0x0037b5e6, &[rig.ds_ptr, 4, 0, sink, 4]).eax, DSERR_INVALIDCALL, "no image installed");
    // Any other name is a stop, not a silent success.
    let other = rig.put(b"Other\0\0\0\0");
    assert!(matches!(rig.stop_of(0x0037b86d, &[other, loc, 1, pp]), Stop::Unimplemented { .. }));
    assert_eq!(rig.call(0x0037b5e6, &[rig.ds_ptr, 4, 0, sink, 4]).eax, DSERR_INVALIDCALL);
}

#[test]
fn r02_struct_readers_do_not_wrap_or_panic() {
    let mut rig = Rig::new();
    let out = rig.scratch(4);
    // The first dword at FAR is readable, so the readers get as far as the
    // `addr + 4` step that used to overflow.
    assert!(rig.h.read_u32(FAR).is_ok());
    assert!(matches!(rig.stop_of(CREATE_BUFFER, &[rig.ds_ptr, FAR, out, 0]), Stop::Memory(_)));
    assert!(matches!(rig.stop_of(CREATE_STREAM, &[rig.ds_ptr, FAR, out, 0]), Stop::Memory(_)));
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let stream = rig.create_stream(wfx, 0, 0, 0);
    assert!(matches!(rig.stop_of(PROCESS, &[stream, FAR, 0]), Stop::Memory(_)));
    // Mix-bin pairs that run off the end of the address space.
    let bins = rig.put(&[1, 0, 0, 0, 0xfc, 0xff, 0xff, 0xff]);
    assert!(matches!(rig.stop_of(0x0037c70a, &[stream, bins]), Stop::Memory(_)));
    assert!(matches!(rig.stop_of(0x0037b5ae, &[rig.ds_ptr, FAR]), Stop::Memory(_)));
}

#[test]
fn r03_hostile_lengths_fault_before_allocating() {
    let mut rig = Rig::new();
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let stream = rig.create_stream(wfx, 0, 0, 0);
    let buf = rig.put(&[0; 4]);
    let b = rig.create_buffer(0x80, 0, wfx);
    let allocs = rig.h.alloc_calls;
    let status = rig.scratch(4);
    let mut p = Vec::new();
    for v in [buf, 0xffff_fffeu32, 0, status, 0, 0] {
        p.extend(v.to_le_bytes());
    }
    let pkt = rig.put(&p);
    assert!(matches!(rig.stop_of(PROCESS, &[stream, pkt, 0]), Stop::Memory(_)));
    assert_eq!(rig.h.read_u32(status).unwrap(), 0, "nothing was queued or written");
    assert!(matches!(rig.stop_of(SET_BUFFER_DATA, &[b, buf, 0xffff_fffe]), Stop::Memory(_)));
    let desc = rig.buffer_desc(0x80, 0xffff_fffe, wfx);
    let out = rig.scratch(4);
    assert_eq!(rig.call(CREATE_BUFFER, &[rig.ds_ptr, desc, out, 0]).eax, E_INVALIDARG);
    assert_eq!(rig.h.alloc_calls, allocs, "nothing was allocated before validation");
}

#[test]
fn r04_set_buffer_data_with_same_pointer_is_a_noop() {
    let mut rig = Rig::new();
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let buf = rig.create_buffer(0x80, 16, wfx);
    let (p1, c1) = (rig.scratch(4), rig.scratch(4));
    rig.call(LOCK, &[buf, 0, 0, p1, c1, 0, 0, DSBLOCK_ENTIREBUFFER]);
    let base = rig.h.read_u32(p1).unwrap();
    rig.h.write(base, &pcm16(&[16384; 8])).unwrap();
    assert_eq!(rig.call(SET_BUFFER_DATA, &[buf, base, 16]).eax, S_OK);
    assert!(!rig.h.freed.contains(&base), "the owned allocation must not be freed");
    // A sub-range of the owned allocation keeps it alive too.
    assert_eq!(rig.call(SET_BUFFER_DATA, &[buf, base, 8]).eax, S_OK);
    assert!(!rig.h.freed.contains(&base));
    rig.call(PLAY, &[buf, 0, 0, DSBPLAY_LOOPING]);
    assert!(rig.render(4).iter().all(|&v| v == 0.5));
    rig.call(BUF_RELEASE, &[buf]);
    assert_eq!(rig.h.freed.iter().filter(|&&a| a == base).count(), 1);
}

#[test]
fn r05_empty_packet_completes_after_the_packet_before_it() {
    let mut rig = Rig::new();
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let stream = rig.create_stream(wfx, 0, 0, 0);
    let a = rig.process(stream, &pcm16(&[8192; 8]), 0x1);
    let b = rig.process(stream, &[], 0x2);
    assert_eq!(rig.h.read_u32(b.0).unwrap(), XMEDIAPACKET_STATUS_PENDING, "B waits behind A");
    assert!(rig.h.signalled.is_empty());
    rig.render(8);
    rig.call(DO_WORK, &[]);
    assert_eq!(rig.h.signalled, vec![0x1, 0x2]);
    assert_eq!(rig.h.read_u32(a.0).unwrap(), XMEDIAPACKET_STATUS_SUCCESS);
    assert_eq!(rig.h.read_u32(b.0).unwrap(), XMEDIAPACKET_STATUS_SUCCESS);
    // An empty packet on an idle stream completes at once.
    let c = rig.process(stream, &[], 0x3);
    assert_eq!(rig.h.read_u32(c.0).unwrap(), XMEDIAPACKET_STATUS_SUCCESS);
    // A flush aborts an empty packet that sits behind a pending one.
    let d = rig.process(stream, &pcm16(&[8192; 8]), 0x4);
    let e = rig.process(stream, &[], 0x5);
    rig.call(FLUSH, &[stream]);
    assert_eq!(rig.h.read_u32(d.0).unwrap(), XMEDIAPACKET_STATUS_FLUSHED);
    assert_eq!(rig.h.read_u32(e.0).unwrap(), XMEDIAPACKET_STATUS_FLUSHED);
}

#[test]
fn r06_stream_status_reports_paused_bit() {
    let mut rig = Rig::new();
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let stream = rig.create_stream(wfx, 0, 0, 0);
    let out = rig.scratch(4);
    // Paused before the voice is active: bit not set.
    rig.call(STREAM_PAUSE, &[stream, 1]);
    rig.call(STREAM_STATUS, &[stream, out]);
    assert_eq!(rig.h.read_u32(out).unwrap() & XMO_STATUSF_PAUSED, 0);
    rig.call(STREAM_PAUSE, &[stream, 0]);
    rig.process(stream, &pcm16(&[8192; 8]), 0);
    rig.call(STREAM_STATUS, &[stream, out]);
    assert_eq!(rig.h.read_u32(out).unwrap(), XMO_STATUSF_ACCEPT_INPUT_DATA);
    rig.call(STREAM_PAUSE, &[stream, 1]);
    rig.call(STREAM_STATUS, &[stream, out]);
    assert_eq!(rig.h.read_u32(out).unwrap(), XMO_STATUSF_ACCEPT_INPUT_DATA | 0x20000);
    rig.call(STREAM_PAUSE, &[stream, 0]);
    rig.call(STREAM_STATUS, &[stream, out]);
    assert_eq!(rig.h.read_u32(out).unwrap() & 0x20000, 0);
}

#[test]
fn r07_flush_reports_the_original_size_for_untouched_packets() {
    let mut rig = Rig::new();
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let stream = rig.create_stream(wfx, 0, 0, 0);
    let a = rig.process(stream, &pcm16(&[1; 10]), 0);
    let b = rig.process(stream, &pcm16(&[1; 6]), 0);
    rig.call(FLUSH, &[stream]);
    assert_eq!(rig.h.read_u32(a.1).unwrap(), 20);
    assert_eq!(rig.h.read_u32(b.1).unwrap(), 12);
    assert_eq!(rig.h.read_u32(a.0).unwrap(), XMEDIAPACKET_STATUS_FLUSHED);
}

#[test]
fn r08_synch_playback_releases_held_voices() {
    let mut rig = Rig::new();
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    // Play(SYNCHPLAYBACK) on an idle buffer does not start it.
    let buf = rig.create_buffer(0x80, 16, wfx);
    fill(&mut rig, buf, 16384);
    rig.call(PLAY, &[buf, 0, 0, DSBPLAY_LOOPING | DSBPLAY_SYNCHPLAYBACK]);
    assert!(rig.render(4).iter().all(|&v| v == 0.0));
    assert_eq!(rig.status(buf) & DSBSTATUS_PLAYING, 0);
    // A playing buffer paused with mode 2 is held.
    let buf2 = rig.create_buffer(0x80, 16, wfx);
    fill(&mut rig, buf2, 4096);
    rig.call(PLAY, &[buf2, 0, 0, DSBPLAY_LOOPING]);
    rig.call(PAUSE, &[buf2, 2]);
    // A stream paused with mode 2 is held.
    let stream = rig.create_stream(wfx, 0, 0, 0);
    rig.process(stream, &pcm16(&[8192; 16]), 0);
    rig.call(STREAM_PAUSE, &[stream, 2]);
    assert!(rig.render(4).iter().all(|&v| v == 0.0), "everything is held");

    assert_eq!(rig.call(SYNCH_PLAYBACK, &[rig.ds_ptr]).eax, S_OK);
    let out = rig.render(4);
    // 0.5 + 0.125 + 0.25 from the three voices.
    assert!(out.iter().all(|&v| (v - 0.875).abs() < 1e-6), "{out:?}");
    assert_eq!(rig.status(buf), DSBSTATUS_PLAYING | DSBSTATUS_LOOPING);
    assert_eq!(rig.status(buf2), DSBSTATUS_PLAYING | DSBSTATUS_LOOPING);

    // Pause(3) on a voice that was never active is a stop, not a no-op.
    let idle = rig.create_buffer(0x80, 16, wfx);
    assert!(matches!(rig.stop_of(PAUSE, &[idle, 3]), Stop::Unimplemented { .. }));
}

#[test]
fn r08_synch_playback_releases_the_group_atomically() {
    use std::sync::atomic::{AtomicBool, Ordering};
    const VOICES: usize = 600;
    let mut rig = Rig::new();
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    for _ in 0..VOICES {
        let buf = rig.create_buffer(0x80, 16, wfx);
        fill(&mut rig, buf, 16384);
        rig.call(PLAY, &[buf, 0, 0, DSBPLAY_LOOPING | DSBPLAY_SYNCHPLAYBACK]);
    }
    let full = VOICES as f32 * 0.5;
    let (started, stop) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    let mixer = Arc::clone(&rig.mixer);
    let (s2, p2) = (Arc::clone(&started), Arc::clone(&stop));
    // Stands in for the audio callback: renders one frame at a time and
    // records every level it hears.
    let t = std::thread::spawn(move || {
        let mut seen = Vec::new();
        let mut out = [0.0f32; 2];
        while !p2.load(Ordering::Relaxed) {
            mixer.lock().unwrap().render(&mut out);
            seen.push(out[0]);
            s2.store(true, Ordering::Relaxed);
        }
        seen
    });
    while !started.load(Ordering::Relaxed) {
        std::thread::yield_now();
    }
    assert_eq!(rig.call(SYNCH_PLAYBACK, &[rig.ds_ptr]).eax, S_OK);
    // Let the callback hear the released state before stopping it.
    std::thread::sleep(std::time::Duration::from_millis(20));
    stop.store(true, Ordering::Relaxed);
    let seen = t.join().unwrap();
    for v in &seen {
        assert!(*v == 0.0 || (v - full).abs() < 1e-2, "half-released group: {v} of {full}");
    }
    assert!(seen.iter().any(|v| (v - full).abs() < 1e-2), "voices were released");
}

#[test]
fn r09_flush_ex_with_flags_stops_and_keeps_packets() {
    let mut rig = Rig::new();
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let stream = rig.create_stream(wfx, 0, 0, 0);
    let a = rig.process(stream, &pcm16(&[8192; 8]), 0x9);
    // Halo's call at 0x2ae4e2: FlushEx(0, 0, 3).
    assert!(matches!(rig.stop_of(FLUSH_EX, &[stream, 0, 0, 3]), Stop::Unimplemented { .. }));
    assert_eq!(rig.h.read_u32(a.0).unwrap(), XMEDIAPACKET_STATUS_PENDING);
    assert!(rig.h.signalled.is_empty());
}

#[test]
fn r10_repeated_pause_keeps_the_resume_intent() {
    let mut rig = Rig::new();
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let buf = rig.create_buffer(0x80, 16, wfx);
    fill(&mut rig, buf, 16384);
    rig.call(PLAY, &[buf, 0, 0, DSBPLAY_LOOPING]);
    rig.call(PAUSE, &[buf, 1]);
    rig.call(PAUSE, &[buf, 1]);
    rig.call(PAUSE, &[buf, 0]);
    assert_eq!(rig.status(buf), DSBSTATUS_PLAYING | DSBSTATUS_LOOPING);
    assert!(rig.render(4).iter().all(|&v| v == 0.5));
}

#[test]
fn r11_failed_creation_rolls_back_samples_and_voice() {
    let mut rig = Rig::new();
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let desc = rig.buffer_desc(0x80, 16, wfx);
    let out = rig.scratch(4);
    // The sample allocation succeeds, the object allocation fails.
    rig.h.allocs_left = Some(1);
    assert_eq!(rig.call(CREATE_BUFFER, &[rig.ds_ptr, desc, out, 0]).eax, E_OUTOFMEMORY);
    rig.h.allocs_left = None;
    assert_eq!(rig.h.freed.len(), 1, "the sample memory was returned");
    assert_eq!(rig.ds.live_objects(), 1);
    // A fault writing the result pointer undoes both allocations too.
    rig.h.freed.clear();
    assert!(matches!(rig.stop_of(CREATE_BUFFER, &[rig.ds_ptr, desc, 0x2000, 0]), Stop::Memory(_)));
    assert_eq!(rig.h.freed.len(), 2, "object and samples both freed");
    assert_eq!(rig.ds.live_objects(), 1);
    // Same for streams.
    let mut b = Vec::new();
    for v in [0u32, 0, wfx, 0, 0, 0] {
        b.extend(v.to_le_bytes());
    }
    let sd = rig.put(&b);
    assert!(matches!(rig.stop_of(CREATE_STREAM, &[rig.ds_ptr, sd, 0x2000, 0]), Stop::Memory(_)));
    assert_eq!(rig.ds.live_objects(), 1);
    assert_eq!(rig.mixer.lock().unwrap().active_voices(), 0);
    // active_voices() misses a stopped voice that leaked; the counts do not.
    let m = rig.mixer.lock().unwrap();
    assert_eq!((m.buffer_count(), m.stream_count()), (0, 0), "mixer objects leaked");
}
