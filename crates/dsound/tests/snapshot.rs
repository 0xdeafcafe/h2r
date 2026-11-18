//! The `dsound`, `audio` and guest RAM sections together
//! (docs/runtime/snapshots.md, phase 4): a game's DirectSound objects play
//! through the registered overrides, are saved with a packet half played,
//! an event pinned and a stream callback queued, and play on. A second
//! world loads the save and plays on the same way: the same PCM, packet
//! statuses, event signals, callbacks and arena allocations.
use audio::Mixer;
use dsound::abi::*;
use dsound::register::{EventRef, Events, Hle, SnapshotError, register};
use dsound::{DirectSound, Layout};
use runtime::{
    CpuAbi, Dispatch, Event, ExecCtx, Guest, Runner, Runtime, abi, exec::HARNESS_EXIT,
    guest::Permissions,
};
use snapshot::{BuildKey, Header, Snapshot, SnapshotWriter};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

const STACK_TOP: u32 = 0x0061_0000;
const ESP: u32 = STACK_TOP - 0x100;
const SCRATCH: u32 = 0x0070_0000;
const CALLBACK: u32 = 0x0012_3450;
/// Where the fake kernel keeps the object behind event handle `h`.
const OBJECTS: u32 = 0x8000_0000;

const DS_CREATE: u32 = 0x0037_d797;
const CREATE_BUFFER: u32 = 0x0037_d4be;
const CREATE_STREAM: u32 = 0x0037_d4e2;
const SET_BUFFER_DATA: u32 = 0x0037_cc4a;
const BUFFER_SET_VOLUME: u32 = 0x0037_b66f;
const PLAY: u32 = 0x0037_b6df;
const PROCESS: u32 = 0x0037_ad25;
const DO_WORK: u32 = 0x0037_b844;

/// The kernel's side of event pins: a count per object, and every signal.
#[derive(Default)]
struct FakeKernel {
    counts: BTreeMap<u32, usize>,
    signals: Vec<u32>,
}

type Shared = Arc<Mutex<FakeKernel>>;

struct FakePin {
    kernel: Shared,
    address: u32,
    armed: bool,
    /// Whether the pin can name its object for a snapshot.
    named: bool,
}

impl EventRef for FakePin {
    fn signal(&mut self) -> Result<(), String> {
        self.kernel.lock().unwrap().signals.push(self.address);
        Ok(())
    }
    fn address(&self) -> Option<u32> {
        self.named.then_some(self.address)
    }
    fn forget(mut self: Box<Self>) {
        self.armed = false;
    }
}

impl Drop for FakePin {
    fn drop(&mut self) {
        if self.armed {
            let mut k = self.kernel.lock().unwrap();
            let n = k.counts.get_mut(&self.address).expect("a counted pin");
            *n -= 1;
            if *n == 0 {
                k.counts.remove(&self.address);
            }
        }
    }
}

struct FakeEvents {
    kernel: Shared,
    named: bool,
}

impl Events for FakeEvents {
    fn signal(&mut self, guest_handle: u32) -> Result<(), String> {
        self.kernel
            .lock()
            .unwrap()
            .signals
            .push(OBJECTS | guest_handle);
        Ok(())
    }
    fn reference(&mut self, guest_handle: u32) -> Option<Result<Box<dyn EventRef>, String>> {
        let address = OBJECTS | guest_handle;
        *self
            .kernel
            .lock()
            .unwrap()
            .counts
            .entry(address)
            .or_default() += 1;
        Some(Ok(Box::new(FakePin {
            kernel: Arc::clone(&self.kernel),
            address,
            armed: true,
            named: self.named,
        })))
    }
    fn adopt(&mut self, address: u32) -> Option<Result<Box<dyn EventRef>, String>> {
        if !self.named {
            return None;
        }
        if !self.kernel.lock().unwrap().counts.contains_key(&address) {
            return Some(Err(format!("{address:#x} is not pinned")));
        }
        Some(Ok(Box::new(FakePin {
            kernel: Arc::clone(&self.kernel),
            address,
            armed: true,
            named: true,
        })))
    }
}

struct World {
    runtime: Arc<Runtime>,
    ctx: ExecCtx,
    cpu: CpuAbi,
    hle: Arc<Hle>,
    kernel: Shared,
    callbacks: Arc<Mutex<Vec<[u32; 3]>>>,
    cursor: u32,
}

fn world(named: bool) -> World {
    let mut g = Guest::new();
    g.map_zeroed(0x0037_9000, 0xf000, Permissions::RWX).unwrap();
    g.map_zeroed(0x0012_3000, 0x1000, Permissions::RWX).unwrap();
    g.map_zeroed(STACK_TOP - 0x1_0000, 0x1_0000, Permissions::RW)
        .unwrap();
    g.map_zeroed(SCRATCH, 0x1_0000, Permissions::RW).unwrap();
    let runtime = Runtime::new(g, Dispatch::default());
    let kernel = Shared::default();
    let events = FakeEvents {
        kernel: Arc::clone(&kernel),
        named,
    };
    let ds = DirectSound::new(Arc::new(Mutex::new(Mixer::new(8000))), Layout::default());
    let hle = Hle::new(ds, Box::new(events));
    runtime.edit_dispatch(|d| register(d, &hle)).unwrap();
    let callbacks = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&callbacks);
    runtime.edit_dispatch(|d| {
        d.register_override(CALLBACK, "test::callback", move |cpu, ctx, _| {
            let m = ctx.memory();
            let args = [
                m.u32(cpu.esp + 4)?,
                m.u32(cpu.esp + 8)?,
                m.u32(cpu.esp + 12)?,
            ];
            seen.lock().unwrap().push(args);
            m.ret(cpu, 12)
        })
    });
    World {
        ctx: ExecCtx::new(Arc::clone(&runtime), 1),
        runtime,
        cpu: CpuAbi::default(),
        hle,
        kernel,
        callbacks,
        cursor: 0,
    }
}

impl World {
    /// Calls an override; requires it to return S_OK to the caller.
    fn ok(&mut self, target: u32, args: &[u32]) {
        {
            let mut m = self.runtime.memory();
            m.set_u32(ESP, HARNESS_EXIT).unwrap();
            for (i, a) in args.iter().enumerate() {
                m.set_u32(ESP + 4 + 4 * i as u32, *a).unwrap();
            }
        }
        self.cpu.esp = ESP;
        let t = Runner::default().run(&mut self.cpu, &mut self.ctx, target);
        assert_eq!(t.kind, abi::STOP, "{target:#x}: {t:?}");
        assert!(
            matches!(self.ctx.event(t.event_id), Some(Event::HarnessExit)),
            "{target:#x}: {:?} {:?}",
            self.ctx.event(t.event_id),
            self.hle.take_stops()
        );
        assert_eq!(self.cpu.eax, S_OK, "{target:#x}");
    }

    /// Scratch memory, the same addresses in every world.
    fn put(&mut self, bytes: &[u8]) -> u32 {
        let p = SCRATCH + self.cursor;
        self.cursor += (bytes.len() as u32).max(1).next_multiple_of(16);
        self.runtime.memory().write_bytes(p, bytes, 1).unwrap();
        p
    }

    fn words(&mut self, words: &[u32]) -> u32 {
        let b: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        self.put(&b)
    }

    fn u32(&self, a: u32) -> u32 {
        self.runtime.memory().u32(a).unwrap()
    }

    /// `WAVEFORMATEX`: 16-bit mono PCM at 8 kHz.
    fn format(&mut self) -> u32 {
        self.words(&[0x0001_0001, 8000, 16000, 0x0010_0002, 0])
    }

    /// An `XMEDIAPACKET` of `frames` frames; returns it and its status word.
    fn packet(&mut self, frames: u32, event_or_context: u32, seed: u32) -> (u32, u32) {
        let pcm: Vec<u8> = (0..frames)
            .map(|i| (((i * 37 + seed * 101) % 2000) as i32 * 8 - 8000) as i16)
            .flat_map(i16::to_le_bytes)
            .collect();
        let data = self.put(&pcm);
        let status = self.words(&[0x5a5a_5a5a]);
        let done = self.words(&[0]);
        let pkt = self.words(&[data, frames * 2, done, status, event_or_context, 0]);
        (pkt, status)
    }

    /// A new object pointer from the arena.
    fn create_stream(&mut self, ds: u32, fmt: u32) -> u32 {
        let out = self.words(&[0]);
        let desc = self.words(&[0, 4, fmt, 0, 0, 0]);
        self.ok(CREATE_STREAM, &[ds, desc, out, 0]);
        self.u32(out)
    }
}

/// What the scene leaves for later: the root, the format, and the status
/// words of the packets still playing at the save.
struct Scene {
    ds: u32,
    fmt: u32,
    statuses: Vec<u32>,
}

/// A looping buffer playing guest data, a stream of event packets and a
/// stream of callback packets, played 150 frames: one event packet done
/// and signalled through its pin, the next half played and pinned, one
/// callback packet done with its callback queued, the next half played.
fn scene(w: &mut World) -> Scene {
    let out = w.words(&[0]);
    w.ok(DS_CREATE, &[0, out, 0]);
    let ds = w.u32(out);
    let fmt = w.format();
    let desc = w.words(&[BufferDesc::SIZE, 0, 0, fmt, 0, 0]);
    w.ok(CREATE_BUFFER, &[ds, desc, out, 0]);
    let buf = w.u32(out);
    let samples: Vec<u8> = (0..400i32)
        .flat_map(|i| ((i * 211 % 4000 - 2000) as i16 * 4).to_le_bytes())
        .collect();
    let samples = w.put(&samples);
    w.ok(SET_BUFFER_DATA, &[buf, samples, 800]);
    w.ok(BUFFER_SET_VOLUME, &[buf, -300i32 as u32]);
    w.ok(PLAY, &[buf, 0, 0, DSBPLAY_LOOPING]);

    let events = w.create_stream(ds, fmt);
    let cdesc = w.words(&[0, 4, fmt, CALLBACK, 0xc0c0, 0]);
    w.ok(CREATE_STREAM, &[ds, cdesc, out, 0]);
    let calls = w.u32(out);
    let (e1, _) = w.packet(100, 0x91, 1);
    let (e2, e2_status) = w.packet(200, 0x92, 2);
    let (c1, _) = w.packet(50, 0xc1, 3);
    let (c2, c2_status) = w.packet(150, 0xc2, 4);
    for (stream, pkt) in [(events, e1), (events, e2), (calls, c1), (calls, c2)] {
        w.ok(PROCESS, &[stream, pkt, 0]);
    }
    w.hle.advance_audio(&w.runtime, 150).unwrap();
    Scene {
        ds,
        fmt,
        statuses: vec![e2_status, c2_status],
    }
}

/// Everything the continuation produces, for comparison.
#[derive(Debug, PartialEq)]
struct After {
    pcm: Vec<u32>,
    statuses: Vec<u32>,
    signals: Vec<u32>,
    callbacks: Vec<[u32; 3]>,
    counts: BTreeMap<u32, usize>,
    pinned: usize,
    next_object: u32,
}

/// Renders 200 frames by hand, completes, runs the callbacks through the
/// game's `DirectSoundDoWork` and makes one more object.
fn carry_on(w: &mut World, s: &Scene) -> After {
    w.kernel.lock().unwrap().signals.clear();
    w.callbacks.lock().unwrap().clear();
    let mixer = Arc::clone(w.hle.direct_sound().mixer());
    let mut pcm = vec![0.0f32; 400];
    mixer.lock().unwrap().render(&mut pcm);
    w.hle.do_work(&w.runtime).unwrap();
    w.ok(DO_WORK, &[]);
    let next_object = w.create_stream(s.ds, s.fmt);
    let k = w.kernel.lock().unwrap();
    After {
        pcm: pcm.iter().map(|v| v.to_bits()).collect(),
        statuses: s.statuses.iter().map(|&a| w.u32(a)).collect(),
        signals: k.signals.clone(),
        callbacks: w.callbacks.lock().unwrap().clone(),
        counts: k.counts.clone(),
        pinned: w.hle.pinned_events(),
        next_object,
    }
}

fn key() -> BuildKey {
    BuildKey {
        stamp: "dsound-test".into(),
        switches: String::new(),
    }
}

fn save(w: &World) -> Result<Vec<u8>, SnapshotError> {
    let mut out = SnapshotWriter::new(Header {
        build: key(),
        name: "dsound".into(),
        tick: 0,
        presents: 0,
        notes: Vec::new(),
    });
    w.hle.save_snapshot(&mut out)?;
    snapshot::guest::save_guest(&mut out, &w.runtime.memory(), None).unwrap();
    Ok(out.finish().unwrap())
}

#[test]
fn a_loaded_world_plays_on_like_the_saved_one() {
    let mut a = world(true);
    let s = scene(&mut a);
    assert_eq!(a.hle.pinned_events(), 1, "the half-played event packet");
    assert_eq!(a.hle.pending_callbacks(), 1, "the finished callback packet");
    assert_eq!(
        a.kernel.lock().unwrap().signals,
        [OBJECTS | 0x91],
        "the first event packet was signalled"
    );
    let file = save(&a).unwrap();
    let counts = a.kernel.lock().unwrap().counts.clone();
    let cursor = a.cursor;
    let want = carry_on(&mut a, &s);
    assert_eq!(want.callbacks.len(), 2, "{:?}", want.callbacks);
    assert_eq!(want.signals, [OBJECTS | 0x92]);
    assert!(want.counts.is_empty());

    // Another world, with other objects and a pin of its own.
    let mut b = world(true);
    let out = b.words(&[0]);
    b.ok(DS_CREATE, &[0, out, 0]);
    let ds = b.u32(out);
    let fmt = b.format();
    let other = b.create_stream(ds, fmt);
    let (p, _) = b.packet(300, 0x77, 9);
    b.ok(PROCESS, &[other, p, 0]);
    assert_eq!(b.hle.pinned_events(), 1);

    let snap = Snapshot::open(&file, &key()).unwrap();
    snapshot::guest::load_guest(&snap, &mut b.runtime.memory(), None).unwrap();
    // What loading `kcore` does to the counts.
    b.kernel.lock().unwrap().counts = counts;
    b.hle.load_snapshot(&snap).unwrap();
    // A load makes fresh contexts.
    b.ctx = ExecCtx::new(Arc::clone(&b.runtime), 1);
    b.cursor = cursor;
    assert_eq!(b.hle.pinned_events(), 1);
    assert_eq!(b.hle.pending_callbacks(), 1);
    assert_eq!(b.hle.direct_sound().live_objects(), 4);
    let got = carry_on(&mut b, &s);
    assert!(got.pcm == want.pcm, "mixed PCM differs after the load");
    assert_eq!(got, want);
}

#[test]
fn a_pin_that_cannot_name_its_object_refuses_the_save() {
    let mut a = world(false);
    scene(&mut a);
    let err = save(&a).unwrap_err();
    assert!(matches!(err, SnapshotError::Busy(_)), "{err}");
    assert_eq!(a.hle.snapshot_blockers().len(), 1);
}

#[test]
fn a_sink_that_cannot_adopt_pins_refuses_the_load_unchanged() {
    let mut a = world(true);
    scene(&mut a);
    let file = save(&a).unwrap();
    let b = world(false);
    let snap = Snapshot::open(&file, &key()).unwrap();
    let err = b.hle.load_snapshot(&snap).unwrap_err();
    assert!(matches!(err, SnapshotError::Mismatch(_)), "{err}");
    assert_eq!(b.hle.direct_sound().live_objects(), 0);
    assert_eq!(b.hle.pending_callbacks(), 0);
    let mixer = Arc::clone(b.hle.direct_sound().mixer());
    assert_eq!(mixer.lock().unwrap().buffer_count(), 0);
}
