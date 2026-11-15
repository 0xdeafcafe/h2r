//! Controller-speaker routing through the guest-facing calls: a stream or
//! buffer whose data starts like an indexed sound is mixed to the side
//! output. Synthetic bytes only; see `docs/playstation-audio.md`.

use std::sync::{Arc, Mutex};

use audio::Mixer;
use dsound::abi::*;
use dsound::overrides::invoke;
use dsound::padroute::{PadRouter, PadSpeakerConfig, RouteTable, SoundIndex};
use dsound::testing::TestHost;
use dsound::{DirectSound, GuestMemory, Layout};

const RATE: u32 = 8000;
// Halo 2 5849 addresses, from docs/dsound.md.
const CREATE: u32 = 0x0037d797;
const CREATE_BUFFER: u32 = 0x0037d4be;
const CREATE_STREAM: u32 = 0x0037d4e2;
const SET_BUFFER_DATA: u32 = 0x0037cc4a;
const PLAY: u32 = 0x0037b6df;
const PROCESS: u32 = 0x0037ad25;
const FLUSH: u32 = 0x0037ac89;

struct Rig {
    h: TestHost,
    ds: DirectSound,
    mixer: Arc<Mutex<Mixer>>,
    ds_ptr: u32,
    cursor: u32,
}

impl Rig {
    fn new(config: PadSpeakerConfig, index: &SoundIndex) -> Self {
        let mixer = Arc::new(Mutex::new(Mixer::new(RATE)));
        let mut ds = DirectSound::new(Arc::clone(&mixer), Layout::default());
        ds.set_pad_router(Some(PadRouter::new(index, &RouteTable::new(&config))));
        let mut rig = Rig {
            h: TestHost::new(),
            ds,
            mixer,
            ds_ptr: 0,
            cursor: 0,
        };
        let out = rig.scratch(4);
        assert_eq!(rig.call(CREATE, &[0, out, 0]), S_OK);
        rig.ds_ptr = rig.h.read_u32(out).unwrap();
        rig
    }
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
    fn call(&mut self, va: u32, args: &[u32]) -> u32 {
        invoke(&mut self.ds, &mut self.h, va, args)
            .expect("no stop")
            .eax
    }
    /// PCM16 mono at `RATE`.
    fn wfx(&mut self) -> u32 {
        let mut b = Vec::new();
        b.extend(1u16.to_le_bytes());
        b.extend(1u16.to_le_bytes());
        b.extend(RATE.to_le_bytes());
        b.extend((RATE * 2).to_le_bytes());
        b.extend(2u16.to_le_bytes());
        b.extend(16u16.to_le_bytes());
        b.extend(0u16.to_le_bytes());
        self.put(&b)
    }
    fn stream(&mut self) -> u32 {
        let wfx = self.wfx();
        let mut b = Vec::new();
        for v in [0u32, 0, wfx, 0, 0, 0] {
            b.extend(v.to_le_bytes());
        }
        let desc = self.put(&b);
        let out = self.scratch(4);
        assert_eq!(self.call(CREATE_STREAM, &[self.ds_ptr, desc, out, 0]), S_OK);
        self.h.read_u32(out).unwrap()
    }
    fn buffer(&mut self) -> u32 {
        let wfx = self.wfx();
        let mut b = Vec::new();
        for v in [BufferDesc::SIZE, 0, 0, wfx, 0, 0] {
            b.extend(v.to_le_bytes());
        }
        let desc = self.put(&b);
        let out = self.scratch(4);
        assert_eq!(self.call(CREATE_BUFFER, &[self.ds_ptr, desc, out, 0]), S_OK);
        self.h.read_u32(out).unwrap()
    }
    fn process(&mut self, stream: u32, data: &[u8]) {
        let buf = self.put(data);
        let (status, done) = (self.scratch(4), self.scratch(4));
        let mut p = Vec::new();
        for v in [buf, data.len() as u32, done, status, 0, 0] {
            p.extend(v.to_le_bytes());
        }
        let pkt = self.put(&p);
        assert_eq!(self.call(PROCESS, &[stream, pkt, 0]), S_OK);
    }
    /// Peak level on (main, side) over `frames`.
    fn render(&self, frames: usize) -> (f32, f32) {
        let (mut out, mut side) = (vec![0.0; frames * 2], vec![0.0; frames * 2]);
        self.mixer
            .lock()
            .unwrap()
            .render_split(&mut out, Some(&mut side));
        let peak = |v: &[f32]| v.iter().fold(0.0f32, |a, &s| a.max(s.abs()));
        (peak(&out), peak(&side))
    }
}

/// A synthetic PCM16 "sound": `n` samples of a seeded ramp.
fn sound(seed: i16, n: usize) -> Vec<u8> {
    (0..n)
        .flat_map(|i| (2000 + seed * 500 + (i % 50) as i16 * 40).to_le_bytes())
        .collect()
}

fn index() -> SoundIndex {
    let mut idx = SoundIndex::new();
    idx.insert(r"fx\player\shield_down", &sound(1, 400));
    idx.insert(r"music\theme", &sound(2, 400));
    idx
}

#[test]
fn routed_stream_plays_on_the_side_until_the_sound_ends() {
    let mut rig = Rig::new(
        PadSpeakerConfig {
            enabled: true,
            ..Default::default()
        },
        &index(),
    );
    let s = rig.stream();
    // The opening packet of the shield sound, then its continuation.
    let shield = sound(1, 400);
    rig.process(s, &shield[..600]);
    rig.process(s, &shield[600..]);
    let (main, side) = rig.render(150);
    assert_eq!(main, 0.0, "exclusive: nothing on the main mix");
    assert!(side > 0.0);
    // The continuation packet kept the label.
    let (main, side) = rig.render(150);
    assert_eq!(main, 0.0);
    assert!(side > 0.0);
    // A flush ends the sound; an unknown sound on the idle stream is main.
    assert_eq!(rig.call(FLUSH, &[s]), S_OK);
    rig.process(s, &sound(7, 100));
    let (main, side) = rig.render(50);
    assert!(main > 0.0);
    assert_eq!(side, 0.0);
    // A known but unrouted sound stays on the main mix too.
    assert_eq!(rig.call(FLUSH, &[s]), S_OK);
    rig.process(s, &sound(2, 400));
    let (main, side) = rig.render(50);
    assert!(main > 0.0);
    assert_eq!(side, 0.0);
}

#[test]
fn non_exclusive_routing_plays_on_both() {
    let cfg = PadSpeakerConfig {
        enabled: true,
        exclusive: false,
        ..Default::default()
    };
    let mut rig = Rig::new(cfg, &index());
    let s = rig.stream();
    rig.process(s, &sound(1, 400));
    let (main, side) = rig.render(100);
    assert!(main > 0.0 && side > 0.0);
}

#[test]
fn buffers_are_labelled_at_set_buffer_data() {
    let mut rig = Rig::new(
        PadSpeakerConfig {
            enabled: true,
            ..Default::default()
        },
        &index(),
    );
    let b = rig.buffer();
    let data = rig.put(&sound(1, 400));
    assert_eq!(rig.call(SET_BUFFER_DATA, &[b, data, 800]), S_OK);
    assert_eq!(rig.call(PLAY, &[b, 0, 0, 0]), S_OK);
    let (main, side) = rig.render(100);
    assert_eq!(main, 0.0);
    assert!(side > 0.0);
    // New data that is not an indexed sound goes back to the main mix.
    let other = rig.put(&sound(9, 400));
    rig.call(0x0037b703, &[b]); // Stop
    assert_eq!(rig.call(SET_BUFFER_DATA, &[b, other, 800]), S_OK);
    assert_eq!(rig.call(PLAY, &[b, 0, 0, 0]), S_OK);
    let (main, side) = rig.render(100);
    assert!(main > 0.0);
    assert_eq!(side, 0.0);
}
