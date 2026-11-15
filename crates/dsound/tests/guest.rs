//! End-to-end tests over synthetic guest memory. The calls go through
//! `overrides::invoke` with the real Halo 2 addresses, so they exercise the
//! same path the recompiled game will: stdcall arguments in, EAX and guest
//! memory out. No game data is involved.

use std::sync::{Arc, Mutex};

use audio::Mixer;
use dsound::abi::*;
use dsound::overrides::{Return, invoke};
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
        assert_eq!(
            self.call(CREATE_STREAM, &[self.ds_ptr, desc, out, 0]).eax,
            S_OK
        );
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
    let (p1, c1, p2, c2) = (
        rig.scratch(4),
        rig.scratch(4),
        rig.scratch(4),
        rig.scratch(4),
    );

    // Lock the last 4 bytes plus a wrapped 4 bytes from the start.
    assert_eq!(rig.call(LOCK, &[buf, 12, 8, p1, c1, p2, c2, 0]).eax, S_OK);
    let (a, ca) = (rig.h.read_u32(p1).unwrap(), rig.h.read_u32(c1).unwrap());
    let (b, cb) = (rig.h.read_u32(p2).unwrap(), rig.h.read_u32(c2).unwrap());
    assert_eq!((ca, cb), (4, 4));
    assert_eq!(a - b, 12, "second region is the buffer start, first is +12");

    // Whole buffer via DSBLOCK_ENTIREBUFFER, then write 8 samples of 16384.
    assert_eq!(
        rig.call(LOCK, &[buf, 0, 0, p1, c1, 0, 0, DSBLOCK_ENTIREBUFFER])
            .eax,
        S_OK
    );
    let base = rig.h.read_u32(p1).unwrap();
    assert_eq!(base, b);
    rig.h.write(base, &pcm16(&[16384; 8])).unwrap();
    let r = rig.call(UNLOCK, &[buf, base, 16, 0, 0]);
    assert_eq!((r.eax, r.pop_bytes), (S_OK, 20));

    rig.call(PLAY, &[buf, 0, 0, DSBPLAY_LOOPING]);
    let out = rig.render(16);
    assert!(
        out.iter().all(|&v| v == 0.5),
        "looping buffer fills all 16 frames"
    );
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
    assert_eq!(
        rig.call(LOCK, &[buf, 0, 4, sink, sink, 0, 0, 0]).eax,
        DSERR_INVALIDCALL
    );
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
    // A timestamped StopEx is scheduled on the DSP clock: still a stop.
    assert!(matches!(
        rig.stop_of(0x0037b71b, &[buf, 1, 0, 0]),
        Stop::Unimplemented { .. }
    ));
    assert!(matches!(
        rig.stop_of(
            LOCK,
            &[buf, 0, 4, sink, sink, 0, 0, DSBLOCK_FROMWRITECURSOR]
        ),
        Stop::Unimplemented { .. }
    ));
    // A garbage handle is a stop, not a guest-visible error.
    assert!(matches!(
        rig.stop_of(PLAY, &[0xdead0000, 0, 0, 0]),
        Stop::BadHandle { .. }
    ));
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
    assert_eq!(rig.ds.ignored().get("mix-in buffer Play"), Some(&1));
    assert!(rig.render(4).iter().all(|&v| v == 0.0));
}

#[test]
fn stream_packets_complete_in_order_with_status_and_events() {
    let mut rig = Rig::new();
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let stream = rig.create_stream(wfx, 8, 0, 0);
    // The guest's virtual calls land on the override addresses.
    assert_eq!(
        rig.h.read_u32(stream).unwrap(),
        Layout::default().stream_vtable
    );

    let mut ready = rig.scratch(4);
    assert_eq!(rig.call(STREAM_STATUS, &[stream, ready]).eax, S_OK);
    assert_eq!(
        rig.h.read_u32(ready).unwrap(),
        XMO_STATUSF_ACCEPT_INPUT_DATA
    );
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
        assert_eq!(
            rig.h.read_u32(*status).unwrap(),
            XMEDIAPACKET_STATUS_PENDING
        );
        assert_eq!(rig.h.read_u32(*done).unwrap(), 0);
    }
    assert!(rig.h.signalled.is_empty());

    // Mix 5 frames: packet 0 (4 frames) is done, packet 1 is half played.
    let out = rig.render(5);
    assert_eq!(out[0], 0.25);
    rig.call(DO_WORK, &[]);
    assert_eq!(
        rig.h.read_u32(slots[0].0).unwrap(),
        XMEDIAPACKET_STATUS_SUCCESS
    );
    assert_eq!(rig.h.read_u32(slots[0].1).unwrap(), 8);
    assert_eq!(
        rig.h.read_u32(slots[1].0).unwrap(),
        XMEDIAPACKET_STATUS_PENDING
    );
    assert_eq!(rig.h.signalled, vec![0x101]);

    // Finish the rest: completion order is submission order.
    rig.render(16);
    rig.call(DO_WORK, &[]);
    assert_eq!(rig.h.signalled, vec![0x101, 0x102, 0x103]);
    for (i, (status, done)) in slots.iter().enumerate() {
        assert_eq!(
            rig.h.read_u32(*status).unwrap(),
            XMEDIAPACKET_STATUS_SUCCESS
        );
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
    assert!(matches!(
        rig.stop_of(FLUSH_EX, &[stream, 5, 0, 0]),
        Stop::Unimplemented { .. }
    ));
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
    assert_eq!(
        rig.call(0x0037d598, &[rig.ds_ptr, f(1.0), f(2.0), f(3.0), 0])
            .eax,
        S_OK
    );
    assert_eq!(rig.ds.listener().position, [1.0, 2.0, 3.0]);
    rig.call(0x0037d598, &[rig.ds_ptr, f(0.0), f(0.0), f(0.0), 0]);
    assert_eq!(rig.call(0x0037d506, &[rig.ds_ptr, f(3.048), 0]).eax, S_OK);
    assert_eq!(rig.ds.listener().distance_factor, 3.048);
    assert_eq!(
        rig.call(0x0037c668, &[buf, f(5.0), f(0.0), f(0.0), 0]).eax,
        S_OK
    );
    rig.call(0x0037c620, &[buf, f(1.0e9), 1]);
    rig.call(0x0037c644, &[buf, f(1.0), 1]);
    rig.call(PLAY, &[buf, 0, 0, DSBPLAY_LOOPING]);
    let out = rig.render(2);
    assert!(
        out[1] > out[0],
        "source on the right is louder on the right: {out:?}"
    );
    // Doppler is applied (a source with no velocity is unshifted); the cone
    // outside volume is applied but the default 360 degree cone never
    // reaches it.
    rig.call(0x0037c69d, &[buf, f(0.0), 1]);
    assert!(rig.ds.ignored().get("SetDopplerFactor").is_none());
    rig.call(0x0037c600, &[buf, (-10_000i32) as u32, 1]);
    assert_eq!(rig.render(2), out);
    // Rolloff: 5 units out with min distance 1 is 1 / (1 + 4); 0 turns the
    // attenuation off, as Halo 2 sets it.
    let near_right = out[1];
    rig.call(0x0037c668, &[buf, f(0.0), f(0.0), f(5.0), 0]);
    let ahead = rig.render(2);
    assert!((ahead[0] - 0.2 * 0.5).abs() < 1e-6, "{ahead:?}");
    rig.call(0x0037c6c1, &[buf, f(0.0), 1]);
    assert_eq!(rig.render(2), vec![0.5; 4]);
    assert!(near_right < 0.5);
}

/// Halo 2's positioned voice (sound core `0x220b90`, `0x220ea1`): a mix-in
/// buffer with `DSBCAPS_CTRL3D` reading FX send bin `0x17`, the 3D routing
/// `{6, 8, 7, 9, 10}`, max/min distance `FLT_MAX`, rolloff, Doppler and cone
/// volume 0; the voice's samples come from a stream sending to bin `0x17`.
#[test]
fn stream_through_a_positioned_mix_in_is_panned_and_follows_the_buffer() {
    let mut rig = Rig::new();
    let f = |v: f32| v.to_bits();
    let pairs = rig.put(
        &[6u32, 0, 8, 0, 7, 0, 9, 0, 10, 0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>(),
    );
    let bins_3d = rig.put(&[5u32.to_le_bytes(), pairs.to_le_bytes()].concat());
    let mut d = Vec::new();
    for v in [
        BufferDesc::SIZE,
        0x0008_0000 | DSBCAPS_CTRL3D,
        0,
        0,
        bins_3d,
        0x17,
    ] {
        d.extend(v.to_le_bytes());
    }
    let desc = rig.put(&d);
    let out = rig.scratch(4);
    assert_eq!(
        rig.call(CREATE_BUFFER, &[rig.ds_ptr, desc, out, 0]).eax,
        S_OK
    );
    let mix = rig.h.read_u32(out).unwrap();
    for (va, v) in [
        (0x0037c620, f(f32::MAX)),
        (0x0037c644, f(f32::MAX)),
        (0x0037c6c1, 0),
        (0x0037c69d, 0),
        (0x0037c600, 0),
    ] {
        assert_eq!(rig.call(va, &[mix, v, 1]).eax, S_OK, "{va:#x}");
    }
    assert_eq!(
        rig.call(0x0037c668, &[mix, f(10.0), f(0.0), f(0.0), 1]).eax,
        S_OK
    );

    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let stream = rig.create_stream(wfx, 4, 0, 0);
    let send = rig.put(&[0x17u32.to_le_bytes(), 0u32.to_le_bytes()].concat());
    let send = rig.put(&[1u32.to_le_bytes(), send.to_le_bytes()].concat());
    assert_eq!(rig.call(0x0037c70a, &[stream, send]).eax, S_OK);
    let (status, _) = rig.process(stream, &pcm16(&[16384; 64]), 0);

    // Not played yet: the mix-in passes nothing, the packet still plays.
    assert_eq!(rig.render(4), vec![0.0; 8]);
    assert_eq!(rig.call(PLAY, &[mix, 0, 0, 0]).eax, S_OK);
    let right = rig.render(4);
    assert_eq!(right[1], 0.5, "{right:?}");
    assert!(right[0] > 0.05 && right[0] < 0.15, "{right:?}");
    // Every frame of the packet has played: 4 + 4 + 56 = 64.
    rig.call(0x0037c668, &[mix, f(-10.0), f(0.0), f(0.0), 1]);
    let left = rig.render(4);
    assert_eq!((left[0], left[1]), (right[1], right[0]));
    // Halo's own distance attenuation goes through the mix-in volume.
    assert_eq!(rig.call(SET_VOLUME, &[mix, (-600i32) as u32]).eax, S_OK);
    let quieter = rig.render(4);
    assert!((quieter[0] - 0.5 * 10f32.powf(-0.3)).abs() < 1e-6);
    // Paused and stopped mix-ins are silent.
    assert_eq!(rig.call(PAUSE, &[mix, 1]).eax, S_OK);
    assert_eq!(rig.render(4), vec![0.0; 8]);
    assert_eq!(rig.call(PAUSE, &[mix, 0]).eax, S_OK);
    assert!(rig.render(4)[0] > 0.0);
    assert_eq!(rig.call(STOP, &[mix]).eax, S_OK);
    assert_eq!(rig.render(48), vec![0.0; 96]);
    assert_eq!(rig.call(DO_WORK, &[]).eax, S_OK);
    assert_eq!(rig.h.read_u32(status).unwrap(), 0, "packet completed");
    // Releasing the mix-in removes its submix.
    assert_eq!(rig.mixer.lock().unwrap().mix_in_count(), 1);
    assert_eq!(rig.call(BUF_RELEASE, &[mix]).eax, 0);
    assert_eq!(rig.mixer.lock().unwrap().mix_in_count(), 0);
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
    assert_eq!(
        rig.call(0x0037b5e6, &[rig.ds_ptr, 4, 0x20, back, 8]).eax,
        S_OK
    );
    assert_eq!(
        rig.h.read_vec(back, 8).unwrap(),
        vec![1, 2, 3, 4, 5, 6, 7, 8]
    );
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
    // Hostile pointers never panic: the type is the first read and faults.
    assert!(matches!(
        rig.stop_of(SET_EFFECT_DATA_X, &[3, 0x1000, 0x2000]),
        Stop::Memory(_)
    ));
    // Verified order (0x37ba6f): no CDirectSound means DSERR_INVALIDCALL
    // before the descriptor is touched.
    let ds = rig.ds_ptr;
    assert_eq!(rig.call(DS_RELEASE, &[ds]).eax, 0);
    assert_eq!(
        rig.call(SET_EFFECT_DATA_X, &[1, 0x1000, 0]).eax,
        DSERR_INVALIDCALL
    );
}

#[test]
fn dry_effects_image_satisfies_halos_init_reads() {
    let mut rig = Rig::new();
    let sink = rig.scratch(8);
    // Before the download there is no image to read from.
    assert_eq!(
        rig.call(0x0037b5e6, &[rig.ds_ptr, 4, 0x20, sink, 8]).eax,
        DSERR_INVALIDCALL
    );
    let desc = rig.download_dry_image();
    assert_ne!(
        desc, 0,
        "Halo's 0x55787 treats a null descriptor as 'no effects'"
    );
    // 0x19142f: the effect-state path is only taken when desc[0] == 0xF.
    assert_eq!(rig.h.read_u32(desc).unwrap(), 15);
    // The snapshot reads at 0x1913d0..0x191411: effects 4..7, offset 0x20, 8 bytes.
    rig.h.write(sink, &[0xAA; 8]).unwrap();
    for fx in 4..=7 {
        assert_eq!(
            rig.call(0x0037b5e6, &[rig.ds_ptr, fx, 0x20, sink, 8]).eax,
            S_OK
        );
        assert_eq!(
            rig.h.read_vec(sink, 8).unwrap(),
            vec![0; 8],
            "dry default is zeros"
        );
    }
    // 0x191451: 0x3c bytes at offset 0 of effects 1 and 0xb..0xe; word 1 is a
    // DSP address Halo range-checks, and zero maps to "no mix bin".
    let big = rig.scratch(0x40);
    for fx in [1, 0xb, 0xc, 0xd, 0xe] {
        assert_eq!(
            rig.call(0x0037b5e6, &[rig.ds_ptr, fx, 0, big, 0x3c]).eax,
            S_OK
        );
        assert_eq!(rig.h.read_u32(big + 4).unwrap(), 0);
    }
    // The write-back at 0x19128f round-trips; partial reads see it.
    let data = rig.put(&[1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(
        rig.call(0x0037b60d, &[rig.ds_ptr, 4, 0x20, data, 8, 0]).eax,
        S_OK
    );
    rig.call(0x0037b5e6, &[rig.ds_ptr, 4, 0x24, sink, 4]);
    assert_eq!(rig.h.read_vec(sink, 4).unwrap(), vec![5, 6, 7, 8]);
    rig.call(0x0037b5e6, &[rig.ds_ptr, 5, 0x20, sink, 8]);
    assert_eq!(
        rig.h.read_vec(sink, 8).unwrap(),
        vec![0; 8],
        "effects are independent"
    );
    // An effect past the image is DSERR_INVALIDCALL (0x37e39e); a runaway
    // offset is this model's E_INVALIDARG. Neither panics.
    assert_eq!(
        rig.call(0x0037b5e6, &[rig.ds_ptr, 15, 0, sink, 4]).eax,
        DSERR_INVALIDCALL
    );
    assert_eq!(
        rig.call(0x0037b60d, &[rig.ds_ptr, 15, 0, sink, 4, 0]).eax,
        DSERR_INVALIDCALL
    );
    assert_eq!(
        rig.call(0x0037b5e6, &[rig.ds_ptr, 4, 0xffff_fffc, sink, 8])
            .eax,
        E_INVALIDARG
    );
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
    assert!(matches!(
        rig.stop_of(0x0037b86d, &[name, FAR, 1, pp]),
        Stop::Memory(_)
    ));
    // An unmapped name faults and installs nothing.
    assert!(matches!(
        rig.stop_of(0x0037b86d, &[0x2000, loc, 1, pp]),
        Stop::Memory(_)
    ));
    let sink = rig.scratch(8);
    assert_eq!(
        rig.call(0x0037b5e6, &[rig.ds_ptr, 4, 0, sink, 4]).eax,
        DSERR_INVALIDCALL,
        "no image installed"
    );
    // Any other name is a stop, not a silent success.
    let other = rig.put(b"Other\0\0\0\0");
    assert!(matches!(
        rig.stop_of(0x0037b86d, &[other, loc, 1, pp]),
        Stop::Unimplemented { .. }
    ));
    assert_eq!(
        rig.call(0x0037b5e6, &[rig.ds_ptr, 4, 0, sink, 4]).eax,
        DSERR_INVALIDCALL
    );
}

#[test]
fn r02_struct_readers_do_not_wrap_or_panic() {
    let mut rig = Rig::new();
    let out = rig.scratch(4);
    // The first dword at FAR is readable, so the readers get as far as the
    // `addr + 4` step that used to overflow.
    assert!(rig.h.read_u32(FAR).is_ok());
    assert!(matches!(
        rig.stop_of(CREATE_BUFFER, &[rig.ds_ptr, FAR, out, 0]),
        Stop::Memory(_)
    ));
    assert!(matches!(
        rig.stop_of(CREATE_STREAM, &[rig.ds_ptr, FAR, out, 0]),
        Stop::Memory(_)
    ));
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let stream = rig.create_stream(wfx, 0, 0, 0);
    assert!(matches!(
        rig.stop_of(PROCESS, &[stream, FAR, 0]),
        Stop::Memory(_)
    ));
    // Mix-bin pairs that run off the end of the address space.
    let bins = rig.put(&[1, 0, 0, 0, 0xfc, 0xff, 0xff, 0xff]);
    assert!(matches!(
        rig.stop_of(0x0037c70a, &[stream, bins]),
        Stop::Memory(_)
    ));
    assert!(matches!(
        rig.stop_of(0x0037b5ae, &[rig.ds_ptr, FAR]),
        Stop::Memory(_)
    ));
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
    assert!(matches!(
        rig.stop_of(PROCESS, &[stream, pkt, 0]),
        Stop::Memory(_)
    ));
    assert_eq!(
        rig.h.read_u32(status).unwrap(),
        0,
        "nothing was queued or written"
    );
    assert!(matches!(
        rig.stop_of(SET_BUFFER_DATA, &[b, buf, 0xffff_fffe]),
        Stop::Memory(_)
    ));
    let desc = rig.buffer_desc(0x80, 0xffff_fffe, wfx);
    let out = rig.scratch(4);
    assert_eq!(
        rig.call(CREATE_BUFFER, &[rig.ds_ptr, desc, out, 0]).eax,
        E_INVALIDARG
    );
    assert_eq!(
        rig.h.alloc_calls, allocs,
        "nothing was allocated before validation"
    );
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
    assert!(
        !rig.h.freed.contains(&base),
        "the owned allocation must not be freed"
    );
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
    assert_eq!(
        rig.h.read_u32(b.0).unwrap(),
        XMEDIAPACKET_STATUS_PENDING,
        "B waits behind A"
    );
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
    assert_eq!(
        rig.h.read_u32(out).unwrap(),
        XMO_STATUSF_ACCEPT_INPUT_DATA | 0x20000
    );
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
    assert!(
        rig.render(4).iter().all(|&v| v == 0.0),
        "everything is held"
    );

    assert_eq!(rig.call(SYNCH_PLAYBACK, &[rig.ds_ptr]).eax, S_OK);
    let out = rig.render(4);
    // 0.5 + 0.125 + 0.25 from the three voices.
    assert!(out.iter().all(|&v| (v - 0.875).abs() < 1e-6), "{out:?}");
    assert_eq!(rig.status(buf), DSBSTATUS_PLAYING | DSBSTATUS_LOOPING);
    assert_eq!(rig.status(buf2), DSBSTATUS_PLAYING | DSBSTATUS_LOOPING);

    // Pause(3) on a voice that was never active is a stop, not a no-op.
    let idle = rig.create_buffer(0x80, 16, wfx);
    assert!(matches!(
        rig.stop_of(PAUSE, &[idle, 3]),
        Stop::Unimplemented { .. }
    ));
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
    let (started, stop) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    );
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
        assert!(
            *v == 0.0 || (v - full).abs() < 1e-2,
            "half-released group: {v} of {full}"
        );
    }
    assert!(
        seen.iter().any(|v| (v - full).abs() < 1e-2),
        "voices were released"
    );
}

#[test]
fn r09_flush_ex_with_flags_flushes_like_flush() {
    // Halo's call sites pass flags 1 (ASYNC) and 3 (ASYNC | ENVELOPE) with a
    // zero timestamp (0x2ae4e2, 0x333ef6, 0x335744, 0x33598d, 0x335d13). The
    // shim has no envelope and no deferral, so every flag value flushes at
    // once, as the plain Flush does.
    for flags in [1u32, 3, 2, 4, 7] {
        let mut rig = Rig::new();
        let wfx = rig.wfx(1, 1, RATE, 16, 2);
        let stream = rig.create_stream(wfx, 0, 0, 0);
        let a = rig.process(stream, &pcm16(&[8192; 8]), 0x9);
        let b = rig.process(stream, &pcm16(&[8192; 4]), 0xA);
        assert_eq!(
            rig.call(FLUSH_EX, &[stream, 0, 0, flags]).eax,
            S_OK,
            "flags {flags}"
        );
        assert_eq!(rig.h.read_u32(a.0).unwrap(), XMEDIAPACKET_STATUS_FLUSHED);
        assert_eq!(
            rig.h.read_u32(a.1).unwrap(),
            16,
            "the original size, flushed or not"
        );
        assert_eq!(rig.h.read_u32(b.0).unwrap(), XMEDIAPACKET_STATUS_FLUSHED);
        assert_eq!(rig.h.signalled, vec![0x9, 0xA]);
        assert_eq!(rig.ds.ignored().get("FlushEx(flags)"), Some(&1));
        // The mixer is empty afterwards and the stream takes new packets.
        assert!(rig.render(4).iter().all(|&v| v == 0.0));
        let c = rig.process(stream, &pcm16(&[8192; 2]), 0xB);
        assert_eq!(rig.h.read_u32(c.0).unwrap(), XMEDIAPACKET_STATUS_PENDING);
    }
}

#[test]
fn flush_ex_with_a_timestamp_is_still_a_stop() {
    let mut rig = Rig::new();
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let stream = rig.create_stream(wfx, 0, 0, 0);
    let a = rig.process(stream, &pcm16(&[8192; 8]), 0x9);
    // Timestamp low word, then high word (arg 2), then flags.
    assert!(matches!(
        rig.stop_of(FLUSH_EX, &[stream, 5, 0, 1]),
        Stop::Unimplemented { .. }
    ));
    assert!(matches!(
        rig.stop_of(FLUSH_EX, &[stream, 0, 1, 0]),
        Stop::Unimplemented { .. }
    ));
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
    assert_eq!(
        rig.call(CREATE_BUFFER, &[rig.ds_ptr, desc, out, 0]).eax,
        E_OUTOFMEMORY
    );
    rig.h.allocs_left = None;
    assert_eq!(rig.h.freed.len(), 1, "the sample memory was returned");
    assert_eq!(rig.ds.live_objects(), 1);
    // A fault writing the result pointer undoes both allocations too.
    rig.h.freed.clear();
    assert!(matches!(
        rig.stop_of(CREATE_BUFFER, &[rig.ds_ptr, desc, 0x2000, 0]),
        Stop::Memory(_)
    ));
    assert_eq!(rig.h.freed.len(), 2, "object and samples both freed");
    assert_eq!(rig.ds.live_objects(), 1);
    // Same for streams.
    let mut b = Vec::new();
    for v in [0u32, 0, wfx, 0, 0, 0] {
        b.extend(v.to_le_bytes());
    }
    let sd = rig.put(&b);
    assert!(matches!(
        rig.stop_of(CREATE_STREAM, &[rig.ds_ptr, sd, 0x2000, 0]),
        Stop::Memory(_)
    ));
    assert_eq!(rig.ds.live_objects(), 1);
    assert_eq!(rig.mixer.lock().unwrap().active_voices(), 0);
    // active_voices() misses a stopped voice that leaked; the counts do not.
    let m = rig.mixer.lock().unwrap();
    assert_eq!(
        (m.buffer_count(), m.stream_count()),
        (0, 0),
        "mixer objects leaked"
    );
}

// ---- XAudioSetEffectData ----------------------------------------------

/// `DSI3DL2LISTENER`-shaped parameters: the type word 12 then 12 dwords.
fn i3dl2_desc(rig: &mut Rig) -> u32 {
    let mut words = vec![12u32];
    words.extend((1..=12).map(|i| 0x4000_0000 + i));
    let b: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    rig.put(&b)
}

fn get_effect(rig: &mut Rig, index: u32, offset: u32, len: u32) -> Vec<u8> {
    let out = rig.scratch(len);
    assert_eq!(
        rig.call(0x0037b5e6, &[rig.ds_ptr, index, offset, out, len])
            .eax,
        S_OK
    );
    rig.h.read_vec(out, len).unwrap()
}

#[test]
fn xaudio_set_effect_data_i3dl2_updates_flags_and_returns_the_written_block() {
    let mut rig = Rig::new();
    let desc = i3dl2_desc(&mut rig);
    // No image yet: type 12's failed state read jumps to the epilogue with
    // the return local still 0 (0x37baef): S_OK, no pOut write.
    let guard = rig.put(&[0xEE; 8]);
    assert_eq!(rig.call(SET_EFFECT_DATA_X, &[9, desc, guard]).eax, S_OK);
    assert_eq!(rig.h.read_vec(guard, 8).unwrap(), vec![0xEE; 8]);
    rig.download_dry_image();
    // Seed the dword the call ORs 4 into (state +0x10).
    let seed = rig.put(&0x30u32.to_le_bytes());
    assert_eq!(
        rig.call(0x0037b60d, &[rig.ds_ptr, 9, 0x10, seed, 4, 0]).eax,
        S_OK
    );

    let out = rig.scratch(4 + 0x108 + 8);
    rig.h.write(out, &[0xEE; 4 + 0x108 + 8]).unwrap();
    assert_eq!(rig.call(SET_EFFECT_DATA_X, &[9, desc, out]).eax, S_OK);
    assert_eq!(
        get_effect(&mut rig, 9, 0x10, 4),
        0x34u32.to_le_bytes(),
        "4 ORed into the flags"
    );
    // pOut is the type then exactly the 0x108 bytes written at +0x118; the
    // bytes after it are untouched.
    assert_eq!(rig.h.read_u32(out).unwrap(), 12);
    assert_eq!(
        rig.h.read_vec(out + 4, 0x108).unwrap(),
        get_effect(&mut rig, 9, 0x118, 0x108)
    );
    assert_eq!(rig.h.read_vec(out + 4 + 0x108, 8).unwrap(), vec![0xEE; 8]);
    // The descriptor is kept for inspection.
    let kept = rig.ds.effect_desc(9).unwrap();
    assert_eq!(kept.ty, 12);
    assert_eq!(kept.words.len(), 12);
    assert_eq!((kept.words[0], kept.words[11]), (0x4000_0001, 0x4000_000C));
    // A null pOut is what Halo passes (0x21ee6f, 0x21fa07).
    assert_eq!(rig.call(SET_EFFECT_DATA_X, &[9, desc, 0]).eax, S_OK);
    assert_eq!(
        get_effect(&mut rig, 9, 0x10, 4),
        0x34u32.to_le_bytes(),
        "OR is idempotent"
    );
    // Other effects are independent.
    assert_eq!(get_effect(&mut rig, 8, 0x10, 4), vec![0; 4]);
}

#[test]
fn xaudio_set_effect_data_types_4_and_10_and_unknown_types() {
    let mut rig = Rig::new();
    rig.download_dry_image();
    let words = |rig: &mut Rig, w: &[u32]| {
        let b: Vec<u8> = w.iter().flat_map(|x| x.to_le_bytes()).collect();
        rig.put(&b)
    };
    // Type 4 reads 7 parameter dwords and returns 11 converted dwords.
    let d4 = words(&mut rig, &[4, 1, 2, 3, 4, 5, 6, 7]);
    let out = rig.scratch(4 + 0x2c);
    assert_eq!(rig.call(SET_EFFECT_DATA_X, &[3, d4, out]).eax, S_OK);
    assert_eq!(rig.h.read_u32(out).unwrap(), 4);
    assert_eq!(
        rig.h.read_vec(out + 4, 0x2c).unwrap(),
        get_effect(&mut rig, 3, 0x1c, 0x2c)
    );
    assert_eq!(
        rig.ds.effect_desc(3).unwrap().words,
        vec![1, 2, 3, 4, 5, 6, 7]
    );
    // Type 10 reads 3 and returns 5 dwords.
    let d10 = words(&mut rig, &[10, 0x3f80_0000, 0x4000_0000, 0x4040_0000]);
    let out = rig.scratch(4 + 0x14);
    assert_eq!(rig.call(SET_EFFECT_DATA_X, &[2, d10, out]).eax, S_OK);
    assert_eq!(rig.h.read_u32(out).unwrap(), 10);
    assert_eq!(rig.ds.effect_desc(2).unwrap().words.len(), 3);
    // Any other type is DSERR_INVALIDCALL and records nothing.
    for ty in [0u32, 1, 5, 11, 13, 0xffff_ffff] {
        let d = words(&mut rig, &[ty, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(
            rig.call(SET_EFFECT_DATA_X, &[1, d, 0]).eax,
            DSERR_INVALIDCALL,
            "type {ty}"
        );
    }
    assert!(rig.ds.effect_desc(1).is_none());
}

#[test]
fn xaudio_set_effect_data_checks_the_index_before_or_after_the_read_as_the_binary_does() {
    let mut rig = Rig::new();
    rig.download_dry_image();
    // The type word is readable and the parameters are not: the last dword
    // of the address space, then nothing.
    let edge = FAR;
    rig.h.write(edge, &12u32.to_le_bytes()).unwrap();
    // Type 12 checks the index first (0x37a34f before the copy at 0x37baf5);
    // an invalid index is S_OK, not an error (0x37ba78, 0x37baef).
    assert_eq!(rig.call(SET_EFFECT_DATA_X, &[15, edge, 0]).eax, S_OK);
    assert!(matches!(
        rig.stop_of(SET_EFFECT_DATA_X, &[9, edge, 0]),
        Stop::Memory(_)
    ));
    // Types 4 and 10 read the floats first.
    for ty in [4u32, 10] {
        rig.h.write(edge, &ty.to_le_bytes()).unwrap();
        assert!(
            matches!(
                rig.stop_of(SET_EFFECT_DATA_X, &[15, edge, 0]),
                Stop::Memory(_)
            ),
            "type {ty}"
        );
    }
    // Types 4 and 10 report the failed state write but still copy their
    // buffer to a non-null pRaw: type word, then zeros.
    for (ty, len) in [(4u32, 0x2c_usize), (10, 0x14)] {
        let mut w = vec![ty];
        w.extend([0u32; 7]);
        let b: Vec<u8> = w.iter().flat_map(|x| x.to_le_bytes()).collect();
        let d = rig.put(&b);
        let out = rig.scratch(8 + len as u32);
        rig.h.write(out, &vec![0xEE; 8 + len]).unwrap();
        assert_eq!(
            rig.call(SET_EFFECT_DATA_X, &[15, d, out]).eax,
            DSERR_INVALIDCALL,
            "type {ty}"
        );
        assert_eq!(rig.h.read_u32(out).unwrap(), ty);
        assert_eq!(rig.h.read_vec(out + 4, len as u32).unwrap(), vec![0; len]);
        assert_eq!(
            rig.h.read_vec(out + 4 + len as u32, 4).unwrap(),
            vec![0xEE; 4]
        );
    }
    // Type 12 with a bad index and a pOut leaves the pOut alone.
    let d12 = i3dl2_desc(&mut rig);
    let out = rig.put(&[0xEE; 8]);
    assert_eq!(rig.call(SET_EFFECT_DATA_X, &[15, d12, out]).eax, S_OK);
    assert_eq!(rig.h.read_vec(out, 8).unwrap(), vec![0xEE; 8]);
    // A bad pOut faults after the state changed, as in the binary.
    let desc = i3dl2_desc(&mut rig);
    assert!(matches!(
        rig.stop_of(SET_EFFECT_DATA_X, &[9, desc, 0x2000]),
        Stop::Memory(_)
    ));
    assert_eq!(get_effect(&mut rig, 9, 0x10, 4), 4u32.to_le_bytes());
}

// ---- GetVoiceProperties -------------------------------------------------

const GET_VOICE_PROPS_BUF: u32 = 0x0037b7e3;
const GET_VOICE_PROPS_STREAM: u32 = 0x0037b83f;
const GET_INFO: u32 = 0x0037abd5;
const SET_MIX_BINS_BUF: u32 = 0x0037c5e4;
const SET_MIX_BIN_VOLUMES_BUF: u32 = 0x0037b6c3;
const STREAM_SET_MIX_BINS: u32 = 0x0037c70a;

fn mix_bins(rig: &mut Rig, pairs: &[(u32, i32)]) -> u32 {
    let b: Vec<u8> = pairs
        .iter()
        .flat_map(|&(bin, v)| [bin.to_le_bytes(), v.to_le_bytes()])
        .flatten()
        .collect();
    let p = rig.put(&b);
    let d: Vec<u8> = [pairs.len() as u32, p]
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect();
    rig.put(&d)
}

/// The 0x5c-byte output as dwords: count, 8 pairs, then the rest.
fn voice_props(rig: &Rig, out: u32) -> Vec<u32> {
    rig.h
        .read_vec(out, 0x5c)
        .unwrap()
        .chunks(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

const UNUSED: [u32; 2] = [0xffff_ffff, 0xffff_d8f0];

#[test]
fn buffer_get_voice_properties_reports_mix_bin_routing() {
    let mut rig = Rig::new();
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let buf = rig.create_buffer(0x80, 16, wfx);
    let out = rig.scratch(0x5c);
    rig.h.write(out, &[0xCC; 0x5c]).unwrap();
    // Inactive voice (state bit 0 clear): DSERR_INVALIDCALL, nothing written.
    assert_eq!(
        rig.call(GET_VOICE_PROPS_BUF, &[buf, out]).eax,
        DSERR_INVALIDCALL
    );
    assert_eq!(rig.h.read_vec(out, 0x5c).unwrap(), vec![0xCC; 0x5c]);
    fill(&mut rig, buf, 1000);
    rig.call(PLAY, &[buf, 0, 0, DSBPLAY_LOOPING]);
    let r = rig.call(GET_VOICE_PROPS_BUF, &[buf, out]);
    assert_eq!((r.eax, r.pop_bytes), (S_OK, 8));
    let w = voice_props(&rig, out);
    // No DSMIXBINS at creation: the stereo default routes to bins 0 and 1
    // (the default table at 0x38595c), volumes 0.
    assert_eq!(&w[..5], &[2, 0, 0, 1, 0]);
    for slot in 2..8 {
        assert_eq!(&w[1 + 2 * slot..3 + 2 * slot], &UNUSED, "slot {slot}");
    }
    assert_eq!(
        &w[17..],
        &[0; 6],
        "level word and 3D/LFO state are not modelled"
    );
    // Explicit routing replaces it; SetMixBinVolumes edits a pair.
    let bins = mix_bins(&mut rig, &[(5, -100), (6, -200), (7, 0)]);
    assert_eq!(rig.call(SET_MIX_BINS_BUF, &[buf, bins]).eax, S_OK);
    let upd = mix_bins(&mut rig, &[(6, -300)]);
    assert_eq!(rig.call(SET_MIX_BIN_VOLUMES_BUF, &[buf, upd]).eax, S_OK);
    rig.call(GET_VOICE_PROPS_BUF, &[buf, out]);
    let w = voice_props(&rig, out);
    assert_eq!(
        &w[..7],
        &[3, 5, (-100i32) as u32, 6, (-300i32) as u32, 7, 0]
    );
    assert_eq!(&w[7..9], &UNUSED);
    // SetMixBins(NULL) restores the default.
    assert_eq!(rig.call(SET_MIX_BINS_BUF, &[buf, 0]).eax, S_OK);
    rig.call(GET_VOICE_PROPS_BUF, &[buf, out]);
    assert_eq!(&voice_props(&rig, out)[..5], &[2, 0, 0, 1, 0]);
    // A paused voice is still active; a stopped one is not.
    rig.call(PAUSE, &[buf, 1]);
    assert_eq!(rig.call(GET_VOICE_PROPS_BUF, &[buf, out]).eax, S_OK);
    rig.call(PAUSE, &[buf, 0]);
    rig.call(STOP, &[buf]);
    assert_eq!(
        rig.call(GET_VOICE_PROPS_BUF, &[buf, out]).eax,
        DSERR_INVALIDCALL
    );
}

#[test]
fn buffer_voice_properties_use_the_documented_default_tables() {
    let mut rig = Rig::new();
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    // A CTRL3D buffer with no DSMIXBINS takes the 3D table (bins 6 8 7 9 10).
    let b3d = rig.create_buffer(0x90, 16, wfx);
    fill(&mut rig, b3d, 1);
    rig.call(PLAY, &[b3d, 0, 0, DSBPLAY_LOOPING]);
    let out = rig.scratch(0x5c);
    assert_eq!(rig.call(GET_VOICE_PROPS_BUF, &[b3d, out]).eax, S_OK);
    let w = voice_props(&rig, out);
    assert_eq!(w[0], 5);
    assert_eq!(
        (1..=5).map(|i| w[2 * i - 1]).collect::<Vec<_>>(),
        vec![6, 8, 7, 9, 10]
    );
    // Out pointer faults are guest faults, not stops of another kind.
    assert!(matches!(
        rig.stop_of(GET_VOICE_PROPS_BUF, &[b3d, 0x2000]),
        Stop::Memory(_)
    ));
    // A garbage handle is a stop.
    assert!(matches!(
        rig.stop_of(GET_VOICE_PROPS_BUF, &[0xdead_0000, out]),
        Stop::BadHandle { .. }
    ));
}

#[test]
fn stream_get_voice_properties_follows_the_voice_state() {
    let mut rig = Rig::new();
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let mut b = Vec::new();
    let bins = mix_bins(&mut rig, &[(2, -50), (3, -60)]);
    for v in [0u32, 0, wfx, 0, 0, bins] {
        b.extend(v.to_le_bytes());
    }
    let desc = rig.put(&b);
    let pp = rig.scratch(4);
    assert_eq!(
        rig.call(CREATE_STREAM, &[rig.ds_ptr, desc, pp, 0]).eax,
        S_OK
    );
    let stream = rig.h.read_u32(pp).unwrap();
    let out = rig.scratch(0x5c);
    // Not started yet.
    assert_eq!(
        rig.call(GET_VOICE_PROPS_STREAM, &[stream, out]).eax,
        DSERR_INVALIDCALL
    );
    rig.process(stream, &pcm16(&[100; 64]), 0);
    assert_eq!(rig.call(GET_VOICE_PROPS_STREAM, &[stream, out]).eax, S_OK);
    let w = voice_props(&rig, out);
    assert_eq!(&w[..5], &[2, 2, (-50i32) as u32, 3, (-60i32) as u32]);
    assert_eq!(&w[5..7], &UNUSED);
    // Pause keeps the voice active; the stream-side SetMixBins is seen.
    rig.call(STREAM_PAUSE, &[stream, 1]);
    assert_eq!(rig.call(GET_VOICE_PROPS_STREAM, &[stream, out]).eax, S_OK);
    let nb = mix_bins(&mut rig, &[(9, -1)]);
    assert_eq!(rig.call(STREAM_SET_MIX_BINS, &[stream, nb]).eax, S_OK);
    rig.call(GET_VOICE_PROPS_STREAM, &[stream, out]);
    assert_eq!(&voice_props(&rig, out)[..3], &[1, 9, (-1i32) as u32]);
    // A flush stops the voice; the next Process starts it again.
    rig.call(FLUSH_EX, &[stream, 0, 0, 3]);
    assert_eq!(
        rig.call(GET_VOICE_PROPS_STREAM, &[stream, out]).eax,
        DSERR_INVALIDCALL
    );
    rig.process(stream, &pcm16(&[100; 4]), 0);
    assert_eq!(rig.call(GET_VOICE_PROPS_STREAM, &[stream, out]).eax, S_OK);
    // NULL SetMixBins restores the default routing for the format.
    rig.call(STREAM_SET_MIX_BINS, &[stream, 0]);
    rig.call(GET_VOICE_PROPS_STREAM, &[stream, out]);
    assert_eq!(&voice_props(&rig, out)[..5], &[2, 0, 0, 1, 0]);
}

// ---- GetInfo ------------------------------------------------------------

#[test]
fn stream_get_info_reports_the_packet_unit_and_lookahead() {
    let mut rig = Rig::new();
    let out = rig.scratch(16);
    // 16-bit mono PCM: unit 2 bytes; lookahead 2 * max(2, (16*1/8)*32 = 64).
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let s = rig.create_stream(wfx, 0, 0, 0);
    let r = rig.call(GET_INFO, &[s, out]);
    assert_eq!((r.eax, r.pop_bytes), (S_OK, 8));
    let info: Vec<u32> = (0..4)
        .map(|i| rig.h.read_u32(out + 4 * i).unwrap())
        .collect();
    assert_eq!(info, vec![5, 2, 0, 128]);
    // 16-bit stereo PCM: unit 4; lookahead 2 * (16*2/8)*32 = 256.
    let wfx = rig.wfx(1, 2, RATE, 16, 4);
    let s = rig.create_stream(wfx, 0, 0, 0);
    rig.call(GET_INFO, &[s, out]);
    let info: Vec<u32> = (0..4)
        .map(|i| rig.h.read_u32(out + 4 * i).unwrap())
        .collect();
    assert_eq!(info, vec![5, 4, 0, 256]);
    // Xbox ADPCM is 4 bits whatever the header says: stereo unit 72 beats
    // 32 frames (4*2/8)*32 = 32, so the lookahead is 2 * 72.
    let wfx = rig.wfx(0x69, 2, RATE, 4, 72);
    let s = rig.create_stream(wfx, 0, 0, 0);
    rig.call(GET_INFO, &[s, out]);
    let info: Vec<u32> = (0..4)
        .map(|i| rig.h.read_u32(out + 4 * i).unwrap())
        .collect();
    assert_eq!(info, vec![5, 72, 0, 144]);
    // Mono ADPCM: (4*1/8) = 0 frames bytes, so only the unit counts.
    let wfx = rig.wfx(0x69, 1, RATE, 4, 36);
    let s = rig.create_stream(wfx, 0, 0, 0);
    rig.call(GET_INFO, &[s, out]);
    assert_eq!(rig.h.read_u32(out + 12).unwrap(), 72);
    assert!(matches!(
        rig.stop_of(GET_INFO, &[0xdead_0000, out]),
        Stop::BadHandle { .. }
    ));
    assert!(matches!(
        rig.stop_of(GET_INFO, &[s, 0x2000]),
        Stop::Memory(_)
    ));
}

// ---- playback progress ---------------------------------------------------

#[test]
fn polling_status_or_process_completes_packets_without_do_work() {
    let mut rig = Rig::new();
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let stream = rig.create_stream(wfx, 1, 0, 0);
    let a = rig.process(stream, &pcm16(&[8192; 4]), 0x31);
    // Full: no input accepted.
    let st = rig.scratch(4);
    rig.call(STREAM_STATUS, &[stream, st]);
    assert_eq!(
        rig.h.read_u32(st).unwrap() & XMO_STATUSF_ACCEPT_INPUT_DATA,
        0
    );
    // The mixer consumes it; the real library completes from the interrupt,
    // so a game polling GetStatus (no DoWork) sees the slot free again.
    rig.render(8);
    rig.call(STREAM_STATUS, &[stream, st]);
    assert_eq!(rig.h.read_u32(st).unwrap() & XMEDIAPACKET_STATUS_SUCCESS, 0);
    assert_eq!(
        rig.h.read_u32(st).unwrap() & XMO_STATUSF_ACCEPT_INPUT_DATA,
        1
    );
    assert_eq!(rig.h.read_u32(a.0).unwrap(), XMEDIAPACKET_STATUS_SUCCESS);
    assert_eq!(rig.h.signalled, vec![0x31]);
    // Process on a full stream also collects first.
    let b = rig.process(stream, &pcm16(&[8192; 4]), 0x32);
    rig.render(8);
    let c_buf = rig.put(&pcm16(&[1; 2]));
    let pkt = rig.put(
        &[c_buf, 4, 0, 0, 0x33, 0]
            .iter()
            .flat_map(|w: &u32| w.to_le_bytes())
            .collect::<Vec<u8>>(),
    );
    assert_eq!(
        rig.call(PROCESS, &[stream, pkt, 0]).eax,
        S_OK,
        "capacity was freed by the poll inside Process"
    );
    assert_eq!(rig.h.read_u32(b.0).unwrap(), XMEDIAPACKET_STATUS_SUCCESS);
}

#[test]
fn advance_audio_is_a_controlled_clock_without_any_sink() {
    let mut rig = Rig::new();
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    // A non-looping buffer of 16 frames and a stream with two packets.
    let buf = rig.create_buffer(0x80, 32, wfx);
    fill(&mut rig, buf, 1000);
    rig.call(PLAY, &[buf, 0, 0, 0]);
    let stream = rig.create_stream(wfx, 0, 0, 0);
    let a = rig.process(stream, &pcm16(&[8192; 100]), 0x41);
    let b = rig.process(stream, &pcm16(&[8192; 100]), 0x42);
    assert_eq!(rig.status(buf) & DSBSTATUS_PLAYING, DSBSTATUS_PLAYING);

    // Nothing consumes the mixer, nothing progresses.
    rig.call(DO_WORK, &[]);
    assert!(rig.h.signalled.is_empty());
    // 50 frames: the buffer (16) is done, packet a (100) is half way.
    assert_eq!(rig.ds.advance_audio(&mut rig.h, 50).unwrap(), 50);
    assert_eq!(rig.status(buf) & DSBSTATUS_PLAYING, 0);
    assert_eq!(rig.h.read_u32(a.0).unwrap(), XMEDIAPACKET_STATUS_PENDING);
    assert_eq!(rig.ds.frames_rendered(), 50);
    // 50 more finish a and signal it after its status and size are written.
    rig.ds.advance_audio(&mut rig.h, 50).unwrap();
    assert_eq!(rig.h.read_u32(a.0).unwrap(), XMEDIAPACKET_STATUS_SUCCESS);
    assert_eq!(rig.h.read_u32(a.1).unwrap(), 200);
    assert_eq!(rig.h.signalled, vec![0x41]);
    // A long advance renders in bounded chunks and finishes b.
    rig.ds.advance_audio(&mut rig.h, 10_000).unwrap();
    assert_eq!(rig.h.read_u32(b.0).unwrap(), XMEDIAPACKET_STATUS_SUCCESS);
    assert_eq!(rig.h.signalled, vec![0x41, 0x42]);
}

#[test]
fn frames_per_do_work_ties_the_clock_to_the_games_own_calls() {
    let mut rig = Rig::new();
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let stream = rig.create_stream(wfx, 0, 0, 0);
    let a = rig.process(stream, &pcm16(&[8192; 100]), 0x51);
    rig.ds.set_frames_per_do_work(Some(40)).unwrap();
    // The game's DirectSoundDoWork (0x37b844) is the only caller: 40 frames each.
    rig.call(DO_WORK, &[]);
    rig.call(DO_WORK, &[]);
    assert_eq!(rig.h.read_u32(a.0).unwrap(), XMEDIAPACKET_STATUS_PENDING);
    rig.call(DO_WORK, &[]);
    assert_eq!(rig.h.read_u32(a.0).unwrap(), XMEDIAPACKET_STATUS_SUCCESS);
    assert_eq!(rig.h.signalled, vec![0x51]);
    assert_eq!(rig.ds.frames_rendered(), 120);
    rig.ds.set_frames_per_do_work(None).unwrap();
    rig.call(DO_WORK, &[]);
    assert_eq!(rig.ds.frames_rendered(), 120);
}

#[test]
fn only_one_consumer_may_render_the_mixer() {
    use dsound::{Consumer, ConsumerConflict};
    let mut rig = Rig::new();
    assert_eq!(rig.ds.consumer(), None);
    rig.ds.set_frames_per_do_work(Some(40)).unwrap();
    assert_eq!(rig.ds.consumer(), Some(Consumer::DoWork));
    let refused = ConsumerConflict {
        held: Consumer::DoWork,
        wanted: Consumer::Manual,
    };
    assert_eq!(
        rig.ds.advance_audio(&mut rig.h, 10),
        Err(Stop::Consumer(refused))
    );
    assert_eq!(rig.ds.frames_rendered(), 0, "nothing rendered twice");
    // The same consumer again is fine; another is refused until released.
    rig.ds.set_frames_per_do_work(Some(80)).unwrap();
    assert!(rig.ds.claim_consumer(Consumer::Output).is_err());
    rig.ds.set_frames_per_do_work(None).unwrap();
    rig.ds.claim_consumer(Consumer::External).unwrap();
    assert!(rig.ds.set_frames_per_do_work(Some(1)).is_err());
    rig.ds.release_consumer(Consumer::Manual); // not the holder: no effect
    assert_eq!(rig.ds.consumer(), Some(Consumer::External));
    rig.ds.release_consumer(Consumer::External);
    rig.ds.advance_audio(&mut rig.h, 10).unwrap();
    assert_eq!(rig.ds.consumer(), Some(Consumer::Manual));
}

#[test]
fn audio_clock_converts_host_time_to_whole_frames() {
    use std::time::Duration;
    let mut c = dsound::AudioClock::new(48_000, Duration::from_millis(250));
    assert_eq!(c.advance(Duration::from_millis(10)), 480);
    let mut c = dsound::AudioClock::new(1000, Duration::from_secs(1));
    let sum: u64 = (0..10)
        .map(|_| c.advance(Duration::from_micros(2500)))
        .sum();
    assert_eq!(sum, 25, "no frame is lost to rounding");
    // A capped stall discards its fraction instead of paying it later.
    let mut c = dsound::AudioClock::new(1000, Duration::from_millis(10));
    assert_eq!(c.advance(Duration::from_micros(1600)), 1); // 0.6 frame carried
    assert_eq!(c.advance(Duration::from_secs(5)), 10);
    assert_eq!(
        c.advance(Duration::from_micros(450)),
        0,
        "the 0.6 carried before the stall is gone"
    );
    assert_eq!(
        c.advance(Duration::from_micros(550)),
        1,
        "only post-stall fractions add"
    );
    let mut c = dsound::AudioClock::new(1000, Duration::from_millis(10));
    assert_eq!(c.advance(Duration::from_micros(11_900)), 10);
    assert_eq!(
        c.advance(Duration::from_micros(100)),
        0,
        "overshoot is dropped"
    );
    // A stall is capped, and the debt is not paid later.
    let mut c = dsound::AudioClock::new(48_000, Duration::from_millis(250));
    assert_eq!(c.advance(Duration::from_secs(30)), 12_000);
    assert_eq!(c.advance(Duration::ZERO), 0);
}

// ---- notification contract ----------------------------------------------

use dsound::testing::EventLog;

#[test]
fn event_references_follow_each_packet_and_status_precedes_the_signal() {
    let mut rig = Rig::new();
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let stream = rig.create_stream(wfx, 0, 0, 0);
    let a = rig.process(stream, &pcm16(&[8192; 4]), 0x61);
    let z = rig.process(stream, &[], 0x62);
    let cb_stream = rig.create_stream(wfx, 0, 0x0040_1000, 7);
    rig.process(cb_stream, &pcm16(&[8192; 4]), 0x63);
    // One reference per packet with an event. A callback stream's packet
    // carries a context, not an event, so it takes none. The empty packet
    // is queued behind a, so its reference comes after a's and is not yet signalled.
    assert_eq!(
        rig.h.event_log,
        vec![EventLog::Retain(0x61), EventLog::Retain(0x62)]
    );
    assert_eq!(rig.h.read_u32(z.0).unwrap(), XMEDIAPACKET_STATUS_PENDING);
    rig.render(8);
    rig.call(DO_WORK, &[]);
    // Order is submission order: signals come after the references, and by
    // then both statuses and sizes are in guest memory.
    assert_eq!(
        rig.h.event_log,
        vec![
            EventLog::Retain(0x61),
            EventLog::Retain(0x62),
            EventLog::Signal(0x61),
            EventLog::Signal(0x62)
        ]
    );
    for p in [a, z] {
        assert_eq!(rig.h.read_u32(p.0).unwrap(), XMEDIAPACKET_STATUS_SUCCESS);
    }
    assert_eq!(rig.h.read_u32(a.1).unwrap(), 8);
}

#[test]
fn packets_that_never_complete_return_their_event_reference() {
    let mut rig = Rig::new();
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let stream = rig.create_stream(wfx, 0, 0, 0);
    rig.process(stream, &pcm16(&[8192; 64]), 0x71);
    rig.process(stream, &pcm16(&[8192; 64]), 0x72);
    assert_eq!(rig.call(STREAM_RELEASE, &[stream]).eax, 0);
    // No completion for the dropped packets (their status stays pending), so
    // the references are given back instead of leaking.
    assert_eq!(
        rig.h.event_log,
        vec![
            EventLog::Retain(0x71),
            EventLog::Retain(0x72),
            EventLog::Release(0x71),
            EventLog::Release(0x72)
        ]
    );
    assert!(rig.h.signalled.is_empty());
}

/// Wraps [`TestHost`] and records guest memory at the moment of each signal.
struct Watch {
    inner: TestHost,
    watch: Vec<(u32, u32)>,
    seen: Vec<(u32, Vec<u32>)>,
}

impl GuestMemory for Watch {
    fn read(&self, a: u32, o: &mut [u8]) -> Result<(), dsound::MemFault> {
        self.inner.read(a, o)
    }
    fn write(&mut self, a: u32, d: &[u8]) -> Result<(), dsound::MemFault> {
        self.inner.write(a, d)
    }
    fn alloc(&mut self, s: u32, a: u32) -> Option<u32> {
        self.inner.alloc(s, a)
    }
    fn free(&mut self, a: u32) {
        self.inner.free(a)
    }
}

impl dsound::EventSink for Watch {
    fn signal_event(&mut self, handle: u32, token: u64) {
        let snap = self
            .watch
            .iter()
            .map(|&(p, _)| self.inner.read_u32(p).unwrap())
            .collect();
        self.seen.push((handle, snap));
        self.inner.signal_event(handle, token);
    }
    fn stream_callback(&mut self, cb: dsound::StreamCallback) {
        let snap = self
            .watch
            .iter()
            .map(|&(p, _)| self.inner.read_u32(p).unwrap())
            .collect();
        self.seen.push((cb.packet_context, snap));
        self.inner.stream_callback(cb);
    }
}

#[test]
fn packet_status_and_size_are_written_before_the_notification() {
    let mut rig = Rig::new();
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let events = rig.create_stream(wfx, 0, 0, 0);
    let calls = rig.create_stream(wfx, 0, 0x0040_1000, 1);
    let e = rig.process(events, &pcm16(&[8192; 4]), 0x81);
    let c = rig.process(calls, &pcm16(&[8192; 4]), 0x82);
    rig.render(16);
    // Move the rig's host into the watcher (statuses are in its memory).
    let mut w = Watch {
        inner: std::mem::take(&mut rig.h),
        watch: vec![e, c]
            .into_iter()
            .flat_map(|(s, d)| [(s, 0), (d, 0)])
            .collect(),
        seen: Vec::new(),
    };
    invoke(&mut rig.ds, &mut w, DO_WORK, &[]).unwrap();
    assert_eq!(w.seen.len(), 2);
    // Snapshot layout: [event status, event size, callback status, callback size].
    // Each notification goes out only after its own packet's status and size.
    for (id, snap) in &w.seen {
        let (status, size) = match *id {
            0x81 => (snap[0], snap[1]),
            0x82 => (snap[2], snap[3]),
            other => panic!("unexpected notification {other:#x}"),
        };
        assert_eq!(
            (status, size),
            (XMEDIAPACKET_STATUS_SUCCESS, 8),
            "notification {id:#x}"
        );
    }
    assert_eq!(
        w.seen.iter().map(|s| s.0).collect::<Vec<_>>(),
        vec![0x81, 0x82]
    );
}

// ---- effects image descriptor layout ---------------------------------------

#[test]
fn effects_image_descriptor_walks_like_the_xhv_engine_and_the_library_do() {
    let mut rig = Rig::new();
    let desc = rig.download_dry_image();
    let count = rig.h.read_u32(desc).unwrap();
    assert_eq!(count, 15);
    // XHVEngineCreate's callee (0x332706 then 0x33337a): the map for effect
    // `start + i` is at desc + 8 + (start + i) * 32 with start = 0xb; its
    // lpvStateSegment (map + 8) is dereferenced and the dword at +0x14 read,
    // then `(v - 0x1400) >> 5` taken (0x333380..0x33338a).
    let mut segments = Vec::new();
    for index in 0..count {
        let map = desc + 8 + index * 32;
        let seg = rig.h.read_u32(map + 8).unwrap();
        assert!(
            seg != 0 && seg.is_multiple_of(4),
            "effect {index} segment {seg:#x}"
        );
        let v = rig.h.read_u32(seg + 0x14).unwrap();
        assert_eq!(v.wrapping_sub(0x1400) >> 5, 0, "effect {index}");
        // The same bytes through GetEffectData (0x37e5f2: [map + 8] + offset).
        assert_eq!(
            get_effect(&mut rig, index, 0x14, 4),
            v.to_le_bytes(),
            "effect {index}"
        );
        segments.push(seg);
    }
    // 0xb..=0xe are the four the engine reads.
    assert!((0xb..=0xe).all(|i| i < count));
    // Segments do not overlap and the state is as large as the model allows.
    segments.sort_unstable();
    assert!(segments.windows(2).all(|w| w[1] - w[0] >= 0x1000));
    // SetEffectData lands in the memory the segment pointer reaches.
    let seg = rig.h.read_u32(desc + 8 + 0xb * 32 + 8).unwrap();
    let data = rig.put(&[1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(
        rig.call(0x0037b60d, &[rig.ds_ptr, 0xb, 0x30, data, 8, 0])
            .eax,
        S_OK
    );
    assert_eq!(
        rig.h.read_vec(seg + 0x30, 8).unwrap(),
        [1, 2, 3, 4, 5, 6, 7, 8]
    );
    // ...and a direct write through the pointer is what GetEffectData sees.
    rig.h.write(seg + 0x40, &[9, 9, 9, 9]).unwrap();
    assert_eq!(get_effect(&mut rig, 0xb, 0x40, 4), [9, 9, 9, 9]);
    // A second download returns the same descriptor, unchanged.
    assert_eq!(rig.download_dry_image(), desc);
    assert_eq!(rig.h.read_u32(desc + 8 + 0xb * 32 + 8).unwrap(), seg);
}

// ---- Voice effects -------------------------------------------------------
//
// Descriptors are built the way Halo's conversions build them (docs/dsound.md,
// "Voice effects"); the signal is a synthetic sine at the mixer's 8 kHz.

const STREAM_SET_FILTER: u32 = 0x0037b813;
const STREAM_SET_LFO: u32 = 0x0037b809;
const STREAM_SET_EG: u32 = 0x0037b80e;
const STREAM_SET_HEADROOM: u32 = 0x0037b818;
const STREAM_SET_OUTPUT_BUFFER: u32 = 0x0037c705;
const BUF_SET_FILTER: u32 = 0x0037b68b;
const BUF_SET_HEADROOM: u32 = 0x0037b6a7;
const BUF_SET_I3DL2: u32 = 0x0037c6e5;
const BUF_SET_DOPPLER: u32 = 0x0037c69d;

fn dwords(rig: &mut Rig, w: &[u32]) -> u32 {
    let b: Vec<u8> = w.iter().flat_map(|x| x.to_le_bytes()).collect();
    rig.put(&b)
}

fn sine16(hz: f64, frames: usize) -> Vec<u8> {
    let s: Vec<i16> = (0..frames)
        .map(|i| {
            ((2.0 * std::f64::consts::PI * hz * i as f64 / f64::from(RATE)).sin() * 16384.0) as i16
        })
        .collect();
    pcm16(&s)
}

/// RMS over both channels of `out[from..]` (frames), against the half-scale
/// input: 1.0 means the tone came through at the level it went in.
fn tone_level(out: &[f32], from: usize) -> f64 {
    let t = &out[from * 2..];
    let ms = t.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / t.len() as f64;
    // Per-channel RMS of a half-scale sine is 0.5 / sqrt(2); a mono voice
    // with the default routing (or on both speaker bins) fills both channels.
    ms.sqrt() / (0.5 / std::f64::consts::SQRT_2)
}

fn mono_stream(rig: &mut Rig) -> u32 {
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    rig.create_stream(wfx, 0, 0, 0)
}

fn db(g: f64) -> f64 {
    20.0 * g.log10()
}

/// A level through a stream with `setup` applied first.
fn stream_tone(hz: f64, setup: impl FnOnce(&mut Rig, u32)) -> f64 {
    let mut rig = Rig::new();
    let s = mono_stream(&mut rig);
    setup(&mut rig, s);
    rig.process(s, &sine16(hz, RATE as usize), 0);
    let out = rig.render(RATE as usize);
    tone_level(&out, RATE as usize / 2)
}

#[test]
fn stream_set_filter_low_pass_from_a_halo_descriptor() {
    // DLS2 (mode 1), Q coefficient 0, cutoff word, damping word for Q 0.7071,
    // and the second pair Halo also fills.
    let lp = |rig: &mut Rig, s: u32| {
        let d = dwords(
            rig,
            &[
                1,
                0,
                audio::fx::hz_to_coeff(500.0),
                46341,
                audio::fx::hz_to_coeff(500.0),
                46341,
            ],
        );
        assert_eq!(rig.call(STREAM_SET_FILTER, &[s, d]).eax, S_OK);
    };
    let dry = stream_tone(2000.0, |_, _| {});
    let wet = stream_tone(2000.0, lp);
    assert!((dry - 1.0).abs() < 0.05, "unfiltered tone: {dry}");
    // 2 kHz is two octaves above the cutoff: about -24 dB at 12 dB/oct (a bit
    // more at the 8 kHz test rate, where the bilinear warp bends it).
    assert!((db(wet) + 26.0).abs() < 4.0, "{} dB", db(wet));
    // A tone in the pass band is untouched.
    assert!((db(stream_tone(100.0, lp))).abs() < 0.5);
}

#[test]
fn stream_set_filter_multi_high_pass_and_bypass() {
    let hp = |rig: &mut Rig, s: u32| {
        // MULTI, selector 4 (high pass), 1 kHz.
        let d = dwords(rig, &[3, 4, audio::fx::hz_to_coeff(1000.0), 46341, 0, 0]);
        assert_eq!(rig.call(STREAM_SET_FILTER, &[s, d]).eax, S_OK);
    };
    assert!(db(stream_tone(100.0, hp)) < -30.0, "lows removed");
    assert!(db(stream_tone(3000.0, hp)).abs() < 0.5, "highs kept");
    // Mode 0 and a null descriptor both bypass.
    let off = |rig: &mut Rig, s: u32| {
        let d = dwords(rig, &[1, 0, audio::fx::hz_to_coeff(200.0), 46341, 0, 0]);
        rig.call(STREAM_SET_FILTER, &[s, d]);
        let bypass = dwords(rig, &[0, 0, 0, 0, 0, 0]);
        assert_eq!(rig.call(STREAM_SET_FILTER, &[s, bypass]).eax, S_OK);
    };
    assert!((stream_tone(1500.0, off) - 1.0).abs() < 0.05);
    let null = |rig: &mut Rig, s: u32| {
        let d = dwords(rig, &[1, 0, audio::fx::hz_to_coeff(200.0), 46341, 0, 0]);
        rig.call(STREAM_SET_FILTER, &[s, d]);
        assert_eq!(rig.call(STREAM_SET_FILTER, &[s, 0]).eax, S_OK);
    };
    assert!((stream_tone(1500.0, null) - 1.0).abs() < 0.05);
    // An unreadable descriptor faults rather than being ignored.
    let mut rig = Rig::new();
    let s = mono_stream(&mut rig);
    assert!(matches!(
        invoke(
            &mut rig.ds,
            &mut rig.h,
            STREAM_SET_FILTER,
            &[s, 0xDEAD_0000]
        ),
        Err(Stop::Memory(_))
    ));
}

#[test]
fn buffer_set_filter_applies_to_a_playing_buffer() {
    let mut rig = Rig::new();
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let buf = rig.create_buffer(0, 16_000, wfx);
    let data = rig.put(&sine16(2000.0, 8000));
    rig.call(SET_BUFFER_DATA, &[buf, data, 16_000]);
    let d = dwords(
        &mut rig,
        &[1, 0, audio::fx::hz_to_coeff(500.0), 46341, 0, 0],
    );
    assert_eq!(rig.call(BUF_SET_FILTER, &[buf, d]).eax, S_OK);
    rig.call(PLAY, &[buf, 0, 0, DSBPLAY_LOOPING]);
    let out = rig.render(8000);
    assert!(db(tone_level(&out, 4000)) < -18.0);
    assert!(
        rig.ds.ignored().get("SetFilter").is_none(),
        "no longer dropped"
    );
}

#[test]
fn stream_set_eg_fade_in_from_halos_descriptor() {
    // The shape Halo builds at 0x2ae7a0: generator 1, mode 2 (attack), no
    // delay, attack 94 units (~1 s), release 50, sustain 0xff.
    let mut rig = Rig::new();
    let s = mono_stream(&mut rig);
    let d = dwords(&mut rig, &[1, 2, 0, 94, 0, 0, 50, 0xFF, 0, 0]);
    assert_eq!(rig.call(STREAM_SET_EG, &[s, d]).eax, S_OK);
    rig.process(s, &sine16(500.0, 16_000), 0);
    let out = rig.render(16_000);
    let peak = |from: usize, to: usize| {
        out[from * 2..to * 2]
            .iter()
            .fold(0.0f32, |m, v| m.max(v.abs()))
    };
    assert!(peak(0, 400) < 0.07, "starts silent: {}", peak(0, 400));
    let mid = peak(3900, 4300);
    assert!((mid - 0.25).abs() < 0.04, "half way up at ~0.5 s: {mid}");
    assert!(
        (peak(9000, 9500) - 0.5).abs() < 0.01,
        "full level after the attack"
    );
    // The envelope is bypassed with mode 0, and generator 0 is counted.
    let mut rig = Rig::new();
    let s = mono_stream(&mut rig);
    let on = dwords(&mut rig, &[1, 2, 0, 94, 0, 0, 0, 0xFF, 0, 0]);
    rig.call(STREAM_SET_EG, &[s, on]);
    let off = dwords(&mut rig, &[1, 0, 0, 94, 0, 0, 0, 0xFF, 0, 0]);
    rig.call(STREAM_SET_EG, &[s, off]);
    rig.process(s, &sine16(500.0, 800), 0);
    let out = rig.render(800);
    assert!(out.iter().fold(0.0f32, |m, v| m.max(v.abs())) > 0.45);
    let zero = dwords(&mut rig, &[0, 2, 0, 94, 0, 0, 0, 0xFF, 0, 0]);
    assert_eq!(rig.call(STREAM_SET_EG, &[s, zero]).eax, S_OK);
    assert_eq!(
        rig.ds
            .ignored()
            .get("SetEG (generator 0: pitch and filter envelope)"),
        Some(&1)
    );
    assert_eq!(rig.call(STREAM_SET_EG, &[s, 0]).eax, E_INVALIDARG);
}

#[test]
fn stream_set_lfo_tremolo_from_a_halo_descriptor() {
    // dwLFO 0 (filter/amplitude), delay 0, delta 175 (~4 Hz), amplitude 128
    // (8 dB).
    let mut rig = Rig::new();
    let s = mono_stream(&mut rig);
    let d = dwords(&mut rig, &[0, 0, 175, 0, 0, 128]);
    assert_eq!(rig.call(STREAM_SET_LFO, &[s, d]).eax, S_OK);
    rig.process(s, &sine16(500.0, 8000), 0);
    let out = rig.render(8000);
    let peak = |from: usize| {
        out[from * 2..from * 2 + 40]
            .iter()
            .fold(0.0f32, |m, v| m.max(v.abs()))
    };
    // 4.005 Hz: the triangle peaks a quarter cycle in (~499 frames), bottoms
    // out at three quarters.
    let loud = peak(499);
    let quiet = peak(1498);
    assert!(
        (db(f64::from(loud / quiet)) - 16.0).abs() < 1.5,
        "{loud} vs {quiet}"
    );
    // Pitch LFO is accepted (dwLFO 1) and null is E_INVALIDARG.
    let p = dwords(&mut rig, &[1, 0, 100, 64, 0, 0]);
    assert_eq!(rig.call(STREAM_SET_LFO, &[s, p]).eax, S_OK);
    assert_eq!(rig.call(STREAM_SET_LFO, &[s, 0]).eax, E_INVALIDARG);
}

/// Plays one packet of `frames` frames one frame at a time, servicing the
/// stream (`DirectSoundDoWork`) after each, and returns the frame count at
/// which its status stopped being pending, with its completed size.
fn completion_of(frames: usize, setup: impl FnOnce(&mut Rig, u32)) -> (usize, u32) {
    let mut rig = Rig::new();
    let s = mono_stream(&mut rig);
    setup(&mut rig, s);
    let (status, done) = rig.process(s, &sine16(300.0, frames), 0);
    for step in 1..=frames + 200 {
        rig.render(1);
        rig.call(DO_WORK, &[]);
        let st = rig.h.read_u32(status).unwrap();
        if st != 0x8000_000A {
            assert_eq!(st, 0, "completed successfully");
            return (step, rig.h.read_u32(done).unwrap());
        }
    }
    panic!("never completed");
}

#[test]
fn effects_leave_packet_completion_alone() {
    let plain = completion_of(800, |_, _| {});
    assert_eq!(plain, (800, 1600));
    let all = completion_of(800, |rig, s| {
        let f = dwords(rig, &[1, 0, audio::fx::hz_to_coeff(500.0), 46341, 0, 0]);
        rig.call(STREAM_SET_FILTER, &[s, f]);
        let l = dwords(rig, &[0, 20, 175, 0, 32, 64]);
        rig.call(STREAM_SET_LFO, &[s, l]);
        let e = dwords(rig, &[1, 2, 0, 20, 0, 0, 50, 0xFF, 0, 0]);
        rig.call(STREAM_SET_EG, &[s, e]);
        rig.call(STREAM_SET_HEADROOM, &[s, 600]);
    });
    assert_eq!(all, plain);
}

#[test]
fn pitch_lfo_leaves_packet_completion_alone() {
    // dwLFO 1 (pitch), delta 175 (~4 Hz), depth 127: a deep vibrato. The
    // packet must still complete after exactly its own length.
    let plain = completion_of(800, |_, _| {});
    let vibrato = completion_of(800, |rig, s| {
        let l = dwords(rig, &[1, 0, 175, 127, 0, 0]);
        assert_eq!(rig.call(STREAM_SET_LFO, &[s, l]).eax, S_OK);
    });
    assert_eq!(vibrato, plain);
}

#[test]
fn set_headroom_attenuates_buffers_and_streams() {
    let gain = |mb: u32| {
        let mut rig = Rig::new();
        let s = mono_stream(&mut rig);
        assert_eq!(rig.call(STREAM_SET_HEADROOM, &[s, mb]).eax, S_OK);
        rig.process(s, &sine16(300.0, 4000), 0);
        let out = rig.render(4000);
        tone_level(&out, 2000)
    };
    assert!((gain(0) - 1.0).abs() < 0.05, "Halo's value changes nothing");
    assert!((db(gain(600)) + 6.0).abs() < 0.5);
    let mut rig = Rig::new();
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let buf = rig.create_buffer(0, 16_000, wfx);
    let data = rig.put(&sine16(300.0, 8000));
    rig.call(SET_BUFFER_DATA, &[buf, data, 16_000]);
    assert_eq!(rig.call(BUF_SET_HEADROOM, &[buf, 600]).eax, S_OK);
    rig.call(PLAY, &[buf, 0, 0, DSBPLAY_LOOPING]);
    let out = rig.render(4000);
    assert!((db(tone_level(&out, 2000)) + 6.0).abs() < 0.5);
    assert!(rig.ds.ignored().get("SetHeadroom").is_none());
}

/// Creates a CTRL3D mix-in on FX send `bin` (Halo's `0x220b90` shape), routed
/// to the stereo speaker bins, and plays it.
fn mix_in(rig: &mut Rig, bin: u32) -> u32 {
    let b: Vec<u8> = [BufferDesc::SIZE, 0x80010, 0, 0, 0, bin]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let desc = rig.put(&b);
    let out = rig.scratch(4);
    assert_eq!(
        rig.call(CREATE_BUFFER, &[rig.ds_ptr, desc, out, 0]).eax,
        S_OK
    );
    let mix = rig.h.read_u32(out).unwrap();
    let bins = mix_bins(rig, &[(0, 0), (1, 0)]);
    assert_eq!(rig.call(SET_MIX_BINS_BUF, &[mix, bins]).eax, S_OK);
    rig.call(PLAY, &[mix, 0, 0, 0]);
    mix
}

#[test]
fn set_output_buffer_routes_a_stream_into_a_mix_in() {
    let mut rig = Rig::new();
    let mix = mix_in(&mut rig, 0x17);
    let s = mono_stream(&mut rig);
    // A stream with no send to the mix-in's bin is silent...
    let dead = mix_bins(&mut rig, &[(0x19, 0)]);
    rig.call(STREAM_SET_MIX_BINS, &[s, dead]);
    rig.process(s, &sine16(300.0, 8000), 0);
    assert!(rig.render(400).iter().all(|&v| v == 0.0));
    // ...until it is given the mix-in as its output buffer.
    assert_eq!(rig.call(STREAM_SET_OUTPUT_BUFFER, &[s, mix]).eax, S_OK);
    assert!(rig.render(400).iter().any(|&v| v.abs() > 0.1));
    // Null restores the mix bins.
    assert_eq!(rig.call(STREAM_SET_OUTPUT_BUFFER, &[s, 0]).eax, S_OK);
    assert!(rig.render(400).iter().all(|&v| v == 0.0));
    // A buffer that is not a mix-in is counted and changes nothing.
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let plain = rig.create_buffer(0, 16_000, wfx);
    assert_eq!(rig.call(STREAM_SET_OUTPUT_BUFFER, &[s, plain]).eax, S_OK);
    assert_eq!(
        rig.ds
            .ignored()
            .get("SetOutputBuffer (not a mix-in buffer)"),
        Some(&1)
    );
    assert!(rig.render(400).iter().all(|&v| v == 0.0));
}

#[test]
fn set_i3dl2_source_occlusion_on_a_mix_in() {
    let level = |occlusion_hf: i32, direct: i32| {
        let mut rig = Rig::new();
        let mix = mix_in(&mut rig, 0x17);
        let s = mono_stream(&mut rig);
        let bins = mix_bins(&mut rig, &[(0x17, 0)]);
        rig.call(STREAM_SET_MIX_BINS, &[s, bins]);
        // DSI3DL2BUFFER: direct, directHF, room, roomHF, roomRolloff,
        // obstruction (HF, LF ratio), occlusion (HF, LF ratio).
        let d = dwords(
            &mut rig,
            &[
                direct as u32,
                0,
                0,
                0,
                0,
                0,
                0,
                occlusion_hf as u32,
                0.25f32.to_bits(),
            ],
        );
        assert_eq!(rig.call(BUF_SET_I3DL2, &[mix, d, 1]).eax, S_OK);
        rig.process(s, &sine16(50.0, 4000), 0);
        let out = rig.render(4000);
        tone_level(&out, 2000)
    };
    // The mix-in's default 3D routing is not used here; the speaker bins give
    // a centred mono signal at the level it went in (both channels).
    let open = level(0, 0);
    assert!(open > 0.9, "{open}");
    // Occlusion -2000 mB with LF ratio 0.25 lowers low frequencies by 5 dB.
    assert!((db(level(-2000, 0) / open) + 5.0).abs() < 0.5);
    // The direct level is a plain attenuation.
    assert!((db(level(0, -600) / open) + 6.0).abs() < 0.3);
    // Null is rejected; an unreadable pointer faults.
    let mut rig = Rig::new();
    let mix = mix_in(&mut rig, 0x17);
    assert_eq!(rig.call(BUF_SET_I3DL2, &[mix, 0, 1]).eax, E_INVALIDARG);
    assert!(matches!(
        invoke(
            &mut rig.ds,
            &mut rig.h,
            BUF_SET_I3DL2,
            &[mix, 0xDEAD_0000, 1]
        ),
        Err(Stop::Memory(_))
    ));
}

#[test]
fn i3dl2_reverb_environment_reaches_the_mixer_send() {
    let mut rig = Rig::new();
    // Before an image and an environment, the send is dropped.
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let buf = rig.create_buffer(0, 2000, wfx);
    let data = rig.put(&pcm16(&[16384; 1000]));
    rig.call(SET_BUFFER_DATA, &[buf, data, 2000]);
    let wet = mix_bins(&mut rig, &[(10, 0)]);
    rig.call(SET_MIX_BINS_BUF, &[buf, wet]);
    rig.call(PLAY, &[buf, 0, 0, 0]);
    assert!(rig.render(4000).iter().all(|&v| v == 0.0));
    // Halo's DSI3DL2LISTENER for effect 9 (type word 12 first).
    rig.download_dry_image();
    let f = |x: f32| x.to_bits();
    let env = dwords(
        &mut rig,
        &[
            12,
            0,
            0,
            f(0.0),
            f(1.5),
            f(0.8),
            (-1000i32) as u32,
            f(0.01),
            (-500i32) as u32,
            f(0.02),
            f(100.0),
            f(100.0),
            f(5000.0),
        ],
    );
    assert_eq!(rig.call(SET_EFFECT_DATA_X, &[9, env, 0]).eax, S_OK);
    assert_eq!(
        rig.ds
            .ignored()
            .get("XAudioSetEffectData (I3DL2 reverb applied)"),
        Some(&1)
    );
    assert!(rig.mixer.lock().unwrap().reverb().is_some());
    rig.call(PLAY, &[buf, 0, 0, 0]);
    let out = rig.render(4000);
    assert!(
        out.iter().any(|&v| v != 0.0),
        "reverb tail from the I3DL2 bin"
    );
    // Another effect index with the same type does not touch the mixer's reverb.
    let mut other = Rig::new();
    other.download_dry_image();
    let env2 = dwords(
        &mut other,
        &[
            12,
            0,
            0,
            f(0.0),
            f(1.5),
            f(0.8),
            0,
            f(0.01),
            0,
            f(0.02),
            f(100.0),
            f(100.0),
            f(5000.0),
        ],
    );
    assert_eq!(other.call(SET_EFFECT_DATA_X, &[3, env2, 0]).eax, S_OK);
    assert!(other.mixer.lock().unwrap().reverb().is_none());
}

#[test]
fn buffer_doppler_factor_is_applied_not_counted() {
    let mut rig = Rig::new();
    let wfx = rig.wfx(1, 1, RATE, 16, 2);
    let buf = rig.create_buffer(0x10, 2000, wfx);
    assert_eq!(
        rig.call(BUF_SET_DOPPLER, &[buf, 0.5f32.to_bits(), 1]).eax,
        S_OK
    );
    assert!(rig.ds.ignored().get("SetDopplerFactor").is_none());
}

#[test]
fn stream_set_output_buffer_retains_its_target() {
    let mut rig = Rig::new();
    let mix = mix_in(&mut rig, 0x17);
    let s = mono_stream(&mut rig);
    let dead = mix_bins(&mut rig, &[(0x19, 0)]);
    rig.call(STREAM_SET_MIX_BINS, &[s, dead]);
    rig.process(s, &sine16(300.0, 8000), 0);
    assert_eq!(rig.call(STREAM_SET_OUTPUT_BUFFER, &[s, mix]).eax, S_OK);
    // The caller drops its reference: the stream's keeps the mix-in alive.
    assert_eq!(rig.call(BUF_RELEASE, &[mix]).eax, 1);
    assert!(
        rig.render(400).iter().any(|&v| v.abs() > 0.1),
        "still routed"
    );
    // Setting it again is balanced (add before release).
    assert_eq!(rig.call(STREAM_SET_OUTPUT_BUFFER, &[s, mix]).eax, S_OK);
    assert!(rig.render(400).iter().any(|&v| v.abs() > 0.1));
    let live = rig.ds.live_objects();
    // Clearing gives the last reference back and frees the mix-in.
    assert_eq!(rig.call(STREAM_SET_OUTPUT_BUFFER, &[s, 0]).eax, S_OK);
    assert_eq!(rig.ds.live_objects(), live - 1);
    assert!(rig.render(400).iter().all(|&v| v == 0.0));
    // Replacement releases the old target; destroying the stream releases the new one.
    let a = mix_in(&mut rig, 0x17);
    let b = mix_in(&mut rig, 0x18);
    rig.call(STREAM_SET_OUTPUT_BUFFER, &[s, a]);
    rig.call(BUF_RELEASE, &[a]);
    let live = rig.ds.live_objects();
    rig.call(STREAM_SET_OUTPUT_BUFFER, &[s, b]);
    assert_eq!(rig.ds.live_objects(), live - 1, "a freed when replaced");
    rig.call(BUF_RELEASE, &[b]);
    let live = rig.ds.live_objects();
    rig.call(STREAM_RELEASE, &[s]);
    assert_eq!(rig.ds.live_objects(), live - 2, "stream and b freed");
    // A stale pointer is rejected and changes nothing.
    let s2 = mono_stream(&mut rig);
    assert!(matches!(
        invoke(&mut rig.ds, &mut rig.h, STREAM_SET_OUTPUT_BUFFER, &[s2, a]),
        Err(Stop::BadHandle { .. })
    ));
}

#[test]
fn hostile_i3dl2_source_values_stay_finite() {
    let nan = f32::NAN.to_bits();
    for words in [
        [
            i32::MAX as u32,
            i32::MIN as u32,
            i32::MAX as u32,
            5,
            f32::INFINITY.to_bits(),
            i32::MIN as u32,
            nan,
            77,
            nan,
        ],
        [0, 0, 0, 0, 0, 0, nan, 0, nan],
    ] {
        let mut rig = Rig::new();
        let mix = mix_in(&mut rig, 0x17);
        let s = mono_stream(&mut rig);
        let bins = mix_bins(&mut rig, &[(0x17, 0)]);
        rig.call(STREAM_SET_MIX_BINS, &[s, bins]);
        let d = dwords(&mut rig, &words);
        assert_eq!(rig.call(BUF_SET_I3DL2, &[mix, d, 1]).eax, S_OK);
        rig.process(s, &sine16(300.0, 2000), 0);
        let out = rig.render(2000);
        assert!(out.iter().all(|v| v.is_finite()));
    }
}
