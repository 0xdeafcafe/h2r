//! The registered overrides through the real runtime Runner and Dispatch.
use audio::Mixer;
use dsound::abi::*;
use dsound::register::{ARENA, Events, Hle, RegisterError, register, unregister};
use dsound::{DirectSound, Layout, Stop};
use runtime::{
    CpuAbi, Dispatch, Event, ExecCtx, Guest, Runner, Runtime, TransferAbi, abi, exec::HARNESS_EXIT,
    guest::Permissions,
};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

/// Every override address and its stack dword count (`ret 4 * argc`),
/// written out independently of `dsound::overrides::ENTRIES`.
const PINNED: [(u32, u32); 66] = [
    (0x0037d797, 3), // DirectSoundCreate
    (0x0037d7de, 2), // DirectSoundCreateBuffer
    (0x0037d835, 2), // DirectSoundCreateStream
    (0x00379f2a, 1), // IDirectSound_Release
    (0x0037d4be, 4), // IDirectSound_CreateSoundBuffer
    (0x0037d4e2, 4), // IDirectSound_CreateSoundStream
    (0x0037b5ae, 2), // IDirectSound_GetCaps
    (0x0037b5ca, 2), // IDirectSound_GetSpeakerConfig
    (0x0037b5e6, 5), // IDirectSound_GetEffectData
    (0x0037b60d, 6), // IDirectSound_SetEffectData
    (0x0037b637, 3), // IDirectSound_SetMixBinHeadroom
    (0x0037b657, 1), // IDirectSound_SynchPlayback
    (0x0037d141, 1), // IDirectSound_CommitDeferredSettings
    (0x0037d506, 3), // IDirectSound_SetDistanceFactor
    (0x0037d52a, 3), // IDirectSound_SetDopplerFactor
    (0x0037d5cd, 3), // IDirectSound_SetRolloffFactor
    (0x0037d598, 5), // IDirectSound_SetPosition
    (0x0037d54e, 8), // IDirectSound_SetOrientation
    (0x00379f5b, 0), // DirectSoundUseLightHRTF4Channel
    (0x0037b844, 0), // DirectSoundDoWork
    (0x0037b86d, 4), // XAudioDownloadEffectsImage
    (0x0037ba6f, 3), // XAudioSetEffectData
    (0x00379f45, 1), // IDirectSoundBuffer_Release
    (0x00379f40, 5), // IDirectSoundBuffer_Unlock
    (0x0037cc4a, 3), // IDirectSoundBuffer_SetBufferData
    (0x0037b7b3, 8), // IDirectSoundBuffer_Lock
    (0x0037b6df, 4), // IDirectSoundBuffer_Play
    (0x0037b703, 1), // IDirectSoundBuffer_Stop
    (0x0037b71b, 4), // IDirectSoundBuffer_StopEx
    (0x0037b73f, 2), // IDirectSoundBuffer_Pause
    (0x0037b75b, 2), // IDirectSoundBuffer_GetStatus
    (0x0037b777, 3), // IDirectSoundBuffer_GetCurrentPosition
    (0x0037b797, 2), // IDirectSoundBuffer_SetCurrentPosition
    (0x0037b66f, 2), // IDirectSoundBuffer_SetVolume
    (0x0037c5c8, 2), // IDirectSoundBuffer_SetFrequency
    (0x0037b68b, 2), // IDirectSoundBuffer_SetFilter
    (0x0037b6a7, 2), // IDirectSoundBuffer_SetHeadroom
    (0x0037b6c3, 2), // IDirectSoundBuffer_SetMixBinVolumes
    (0x0037c5e4, 2), // IDirectSoundBuffer_SetMixBins
    (0x0037c668, 5), // IDirectSoundBuffer_SetPosition
    (0x0037c644, 3), // IDirectSoundBuffer_SetMinDistance
    (0x0037c620, 3), // IDirectSoundBuffer_SetMaxDistance
    (0x0037c600, 3), // IDirectSoundBuffer_SetConeOutsideVolume
    (0x0037c69d, 3), // IDirectSoundBuffer_SetDopplerFactor
    (0x0037c6c1, 3), // IDirectSoundBuffer_SetRolloffFactor
    (0x0037c6e5, 3), // IDirectSoundBuffer_SetI3DL2Source
    (0x0037b7e3, 2), // IDirectSoundBuffer_GetVoiceProperties
    (0x0037b7ff, 2), // IDirectSoundStream_SetVolume
    (0x0037b804, 2), // IDirectSoundStream_SetPitch
    (0x0037b809, 2), // IDirectSoundStream_SetLFO
    (0x0037b80e, 2), // IDirectSoundStream_SetEG
    (0x0037b813, 2), // IDirectSoundStream_SetFilter
    (0x0037b818, 2), // IDirectSoundStream_SetHeadroom
    (0x0037b81d, 2), // IDirectSoundStream_SetMixBinVolumes
    (0x0037c70a, 2), // IDirectSoundStream_SetMixBins
    (0x0037c705, 2), // IDirectSoundStream_SetOutputBuffer
    (0x0037b822, 2), // IDirectSoundStream_Pause
    (0x0037b827, 4), // IDirectSoundStream_FlushEx
    (0x0037b83f, 2), // IDirectSoundStream_GetVoiceProperties
    (0x0037ab40, 1), // CDirectSoundStream_AddRef
    (0x0037ab87, 1), // CDirectSoundStream_Release
    (0x0037abd5, 2), // CDirectSoundStream_GetInfo
    (0x0037acd4, 2), // CDirectSoundStream_GetStatus
    (0x0037ad25, 3), // CDirectSoundStream_Process
    (0x0037ac3c, 1), // CDirectSoundStream_Discontinuity
    (0x0037ac89, 1), // CDirectSoundStream_Flush
];

const STACK_TOP: u32 = 0x0061_0000;
const SCRATCH: u32 = 0x0070_0000;
const CALLBACK: u32 = 0x0012_3450;
/// A page tests can make read-only after setup.
const GUARDED: u32 = 0x0071_0000;

/// Signals like the kernel: relocks guest memory, here on another thread with
/// a timeout, so a signal delivered under a held guard fails instead of hanging.
struct KernelLike {
    runtime: RuntimeSlot,
    calls: Signalled,
}

impl Events for KernelLike {
    fn signal(&mut self, guest_handle: u32) -> Result<(), String> {
        let runtime = self.runtime.lock().unwrap().clone().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _guard = runtime.memory();
            let _ = tx.send(());
        });
        rx.recv_timeout(Duration::from_secs(30))
            .map_err(|_| "signalled under the memory guard".to_owned())?;
        self.calls.lock().unwrap().push(guest_handle);
        Ok(())
    }
}

type RuntimeSlot = Arc<Mutex<Option<Arc<Runtime>>>>;
type Signalled = Arc<Mutex<Vec<u32>>>;

fn hle() -> (Arc<Hle>, RuntimeSlot, Signalled) {
    let slot = Arc::new(Mutex::new(None));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let events = KernelLike {
        runtime: Arc::clone(&slot),
        calls: Arc::clone(&calls),
    };
    let ds = DirectSound::new(Arc::new(Mutex::new(Mixer::new(8000))), Layout::default());
    (Hle::new(ds, Box::new(events)), slot, calls)
}

struct World {
    runtime: Arc<Runtime>,
    ctx: ExecCtx,
    cpu: CpuAbi,
    hle: Arc<Hle>,
    signals: Arc<Mutex<Vec<u32>>>,
    callbacks: Arc<Mutex<Vec<[u32; 3]>>>,
    cursor: u32,
    seen: BTreeSet<u32>,
    /// The whole stack mapping just before the last `run`.
    before: Vec<u8>,
}

fn world() -> World {
    world_with(None)
}

/// A world whose `Events` is `custom` (built from the runtime slot) instead of
/// the default `KernelLike`.
type EventsFactory = Box<dyn FnOnce(RuntimeSlot) -> Box<dyn Events>>;

fn world_with(custom: Option<EventsFactory>) -> World {
    let mut g = Guest::new();
    // The DSOUND section; the runner checks Execute on override targets.
    g.map_zeroed(0x0037_9000, 0xf000, Permissions::RWX).unwrap();
    g.map_zeroed(0x0012_3000, 0x1000, Permissions::RWX).unwrap();
    g.map_zeroed(STACK_TOP - 0x1_0000, 0x1_0000, Permissions::RW)
        .unwrap();
    g.map_zeroed(SCRATCH, 0x1_0000, Permissions::RW).unwrap();
    g.map_zeroed(GUARDED, 0x1000, Permissions::RW).unwrap();
    let runtime = Runtime::new(g, Dispatch::default());
    let (hle, slot, signals) = match custom {
        None => hle(),
        Some(make) => {
            let slot: RuntimeSlot = Arc::new(Mutex::new(None));
            let ds = DirectSound::new(Arc::new(Mutex::new(Mixer::new(8000))), Layout::default());
            (
                Hle::new(ds, make(Arc::clone(&slot))),
                slot,
                Arc::new(Mutex::new(Vec::new())),
            )
        }
    };
    *slot.lock().unwrap() = Some(Arc::clone(&runtime));
    runtime.edit_dispatch(|d| register(d, &hle)).unwrap();
    // A guest stream callback: records its three stdcall args, `ret 12`.
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
            cpu.eax = 0x0bad_0bad; // clobbered; the override restores EAX
            m.ret(cpu, 12)
        })
    });
    World {
        ctx: ExecCtx::new(Arc::clone(&runtime), 1),
        runtime,
        cpu: CpuAbi::default(),
        hle,
        signals,
        callbacks,
        cursor: 0,
        seen: BTreeSet::new(),
        before: Vec::new(),
    }
}

const ESP: u32 = STACK_TOP - 0x100;

impl World {
    fn run(&mut self, target: u32, args: &[u32]) -> TransferAbi {
        {
            let mut m = self.runtime.memory();
            m.set_u32(ESP, HARNESS_EXIT).unwrap();
            for (i, a) in args.iter().enumerate() {
                m.set_u32(ESP + 4 + 4 * i as u32, *a).unwrap();
            }
        }
        self.cpu.esp = ESP;
        self.cpu.eax = 0xdead_beef;
        self.seen.insert(target);
        self.before = self.stack();
        Runner::default().run(&mut self.cpu, &mut self.ctx, target)
    }

    fn stack(&self) -> Vec<u8> {
        let mut v = vec![0; 0x1_0000];
        self.runtime
            .memory()
            .read_bytes(STACK_TOP - 0x1_0000, &mut v, 1)
            .unwrap();
        v
    }

    /// Call; require a return to the caller with the pinned cleanup.
    fn ret(&mut self, target: u32, args: &[u32]) -> u32 {
        let argc = PINNED.iter().find(|p| p.0 == target).unwrap().1;
        assert_eq!(args.len() as u32, argc, "{target:#x} test arguments");
        let t = self.run(target, args);
        assert_eq!(t.kind, abi::STOP, "{target:#x}: {t:?}");
        assert!(
            matches!(self.ctx.event(t.event_id), Some(Event::HarnessExit)),
            "{target:#x} did not return: {:?} {:?}",
            self.ctx.event(t.event_id),
            self.hle.take_stops()
        );
        assert_eq!(self.cpu.esp, ESP + 4 + 4 * argc, "{target:#x} cleanup");
        self.cpu.eax
    }

    /// Call; require an S_OK return.
    fn ok(&mut self, target: u32, args: &[u32]) {
        assert_eq!(self.ret(target, args), S_OK, "{target:#x}");
    }

    /// Call; require a DirectSound stop with the frame untouched.
    fn stop(&mut self, target: u32, args: &[u32]) -> Stop {
        let t = self.run(target, args);
        assert_eq!(t.kind, abi::STOP, "{target:#x}");
        let Some(Event::Unsupported {
            pc,
            mnemonic,
            detail,
            ..
        }) = self.ctx.event(t.event_id)
        else {
            panic!("{target:#x}: {:?}", self.ctx.event(t.event_id));
        };
        assert_eq!(*pc, target);
        assert!(mnemonic.starts_with("dsound::"));
        assert!(detail.starts_with("hle: "));
        assert_eq!(
            (self.cpu.esp, self.cpu.eip, self.cpu.eax),
            (ESP, target, 0xdead_beef)
        );
        assert!(self.stack() == self.before, "{target:#x} changed the stack");
        let mut stops = self.hle.take_stops();
        let last = stops.pop().unwrap();
        assert_eq!((last.addr, last.args.as_slice()), (target, args));
        last.stop
    }

    fn scratch(&mut self, n: u32) -> u32 {
        let p = SCRATCH + self.cursor;
        self.cursor += (n + 15) & !15;
        p
    }
    fn put(&mut self, bytes: &[u8]) -> u32 {
        let p = self.scratch(bytes.len() as u32);
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
    fn pcm16_format(&mut self) -> u32 {
        let mut b = Vec::new();
        b.extend(1u16.to_le_bytes());
        b.extend(1u16.to_le_bytes());
        b.extend(8000u32.to_le_bytes());
        b.extend(16000u32.to_le_bytes());
        b.extend(2u16.to_le_bytes());
        b.extend(16u16.to_le_bytes());
        b.extend(0u16.to_le_bytes());
        self.put(&b)
    }
    fn packet(&mut self, bytes: u32, event_or_context: u32) -> (u32, u32, u32) {
        let data = self.scratch(bytes.max(1));
        let status = self.words(&[0x5a5a_5a5a]);
        let done = self.words(&[0x5a5a_5a5a]);
        let pkt = self.words(&[data, bytes, done, status, event_or_context, 0]);
        (pkt, status, done)
    }
}

fn bits(v: f32) -> u32 {
    v.to_bits()
}

#[test]
fn registration_installs_exactly_the_pinned_addresses() {
    let mut d = Dispatch::default();
    let (hle, _, _) = hle();
    let r = register(&mut d, &hle).unwrap();
    let got: BTreeSet<_> = r.installed.iter().map(|e| (e.va, e.argc)).collect();
    let want: BTreeSet<_> = PINNED.iter().copied().collect();
    assert_eq!(want.len(), 66);
    assert_eq!(got, want);
    for (a, _) in PINNED {
        assert!(
            d.override_name(a)
                .is_some_and(|n| n.starts_with("dsound::")),
            "{a:#x}"
        );
    }
}

#[test]
fn registering_twice_is_reported_and_changes_nothing() {
    let mut d = Dispatch::default();
    let (hle, _, _) = hle();
    register(&mut d, &hle).unwrap();
    let Err(RegisterError::Occupied(conflicts)) = register(&mut d, &hle) else {
        panic!("second registration was accepted");
    };
    assert_eq!(conflicts.len(), 66);
    assert!(conflicts.iter().all(|c| c.existing == c.ours));
}

#[test]
fn registering_over_a_foreign_override_installs_nothing() {
    let mut d = Dispatch::default();
    d.register_override(0x0037_ad25, "bink-hle::something", |_, _, _| {
        Ok(TransferAbi::ret(0))
    });
    let (hle, _, _) = hle();
    let Err(RegisterError::Occupied(conflicts)) = register(&mut d, &hle) else {
        panic!("registration over a foreign override was accepted");
    };
    assert_eq!(conflicts.len(), 1);
    assert_eq!(
        (conflicts[0].addr, conflicts[0].existing.as_str()),
        (0x0037_ad25, "bink-hle::something")
    );
    for (a, _) in PINNED.iter().filter(|p| p.0 != 0x0037_ad25) {
        assert_eq!(d.override_name(*a), None, "{a:#x} installed anyway");
    }
}

#[test]
fn unregister_disables_and_a_later_register_is_reported() {
    let mut d = Dispatch::default();
    let (hle, _, _) = hle();
    let r = register(&mut d, &hle).unwrap();
    unregister(&mut d, &r);
    assert!(register(&mut d, &hle).is_err());
}

#[test]
fn unregistered_addresses_fall_back_to_the_original_dispatch() {
    let mut w = world();
    let r = dsound::Registration {
        installed: dsound::overrides::ENTRIES.to_vec(),
    };
    w.runtime.edit_dispatch(|d| unregister(d, &r));
    for (addr, argc) in PINNED {
        let t = w.run(addr, &vec![0; argc as usize]);
        // No generated original exists here, so the fallback is the runner's
        // UnsupportedTarget fault, never the disabled override.
        assert_eq!(t.kind, abi::FAULT, "{addr:#x} still dispatched: {t:?}");
        assert!(
            matches!(
                w.ctx.event(t.event_id),
                Some(Event::UnsupportedTarget { target, .. }) if *target == addr
            ),
            "{addr:#x}: {:?}",
            w.ctx.event(t.event_id)
        );
        assert!(w.stack() == w.before, "{addr:#x} touched the stack");
    }
    assert_eq!(w.hle.direct_sound().live_objects(), 0);
    assert!(w.hle.take_stops().is_empty());
}

#[test]
fn callbacks_survive_a_fault_on_a_later_packet() {
    let mut w = world();
    let out = w.scratch(4);
    w.ok(0x0037_d797, &[0, out, 0]);
    let ds = w.u32(out);
    let fmt = w.pcm16_format();
    let sdesc = w.words(&[0, 4, fmt, CALLBACK, 0xc0c0, 0]);
    w.ok(0x0037_d4e2, &[ds, sdesc, out, 0]);
    let stream = w.u32(out);
    // A plays; B (empty) waits behind it with its status on GUARDED.
    let (a, _, _) = w.packet(0x40, 1);
    let done = w.words(&[0]);
    let b = w.words(&[SCRATCH, 0, done, GUARDED, 2, 0]);
    w.ok(0x0037_ad25, &[stream, a, 0]);
    w.ok(0x0037_ad25, &[stream, b, 0]);
    w.runtime
        .memory()
        .protect(GUARDED, Permissions::READ)
        .unwrap();
    // Flush completes A (callback owed), then B's status write faults.
    let t = w.run(0x0037_ac89, &[stream]);
    assert_eq!(t.kind, abi::FAULT);
    let Some(Event::MemoryFault { fault, .. }) = w.ctx.event(t.event_id) else {
        panic!("{:?}", w.ctx.event(t.event_id));
    };
    assert_eq!(fault.addr, GUARDED);
    assert!(w.callbacks.lock().unwrap().is_empty());
    assert_eq!(w.hle.pending_callbacks(), 1, "A's callback was dropped");
    // B stays queued after its faulting write (its record is kept, not
    // dropped), so DoWork would retry it and fault again.
    let t = w.run(0x0037_b844, &[]);
    assert_eq!(t.kind, abi::FAULT);
    // Once the page is writable the retry completes B, and the next override
    // call on a guest thread runs A's owed callback first, then B's.
    w.runtime
        .memory()
        .protect(GUARDED, Permissions::RW)
        .unwrap();
    w.ok(0x0037_b844, &[]);
    let cbs = w.callbacks.lock().unwrap().clone();
    assert_eq!(cbs.len(), 2, "{cbs:x?}");
    assert_eq!(cbs[0], [0xc0c0, 1, XMEDIAPACKET_STATUS_FLUSHED]);
    assert_eq!(cbs[1][1], 2);
    assert_eq!(
        w.u32(GUARDED),
        cbs[1][2],
        "status written before the callback"
    );
}

#[test]
fn every_override_decodes_its_frame_and_pops_its_arguments() {
    let mut w = world();

    // --- IDirectSound and objects -------------------------------------
    let out = w.scratch(4);
    w.ok(0x0037_d797, &[0, out, 0]);
    let ds = w.u32(out);
    assert!(ARENA.contains(&ds), "object {ds:#x} not from the arena");

    let fmt = w.pcm16_format();
    let desc = w.words(&[BufferDesc::SIZE, 0, 0x100, fmt, 0, 0]);
    w.ok(0x0037_d4be, &[ds, desc, out, 0]);
    let buf = w.u32(out);
    let desc0 = w.words(&[BufferDesc::SIZE, 0, 0, fmt, 0, 0]);
    w.ok(0x0037_d7de, &[desc0, out]);
    let buf2 = w.u32(out);
    let sdesc = w.words(&[0, 4, fmt, 0, 0, 0]);
    w.ok(0x0037_d4e2, &[ds, sdesc, out, 0]);
    let stream = w.u32(out);
    assert_eq!(w.u32(stream), 0x0041_7170, "stream vtable word");
    w.ok(0x0037_d835, &[sdesc, out]);
    let stream2 = w.u32(out);
    assert_ne!(stream2, stream);

    let caps = w.scratch(16);
    w.ok(0x0037_b5ae, &[ds, caps]);
    assert_eq!(w.u32(caps), 64);
    assert_eq!(w.u32(caps + 8), 0x7fff);
    w.ok(0x0037_b5ca, &[ds, out]);
    assert_eq!(w.u32(out), DSSPEAKER_STEREO);

    let name = w.put(b"DSPImage\0");
    let loc = w.words(&[9, 10]);
    w.ok(0x0037_b86d, &[name, loc, 1, out]);
    assert_eq!(w.u32(w.u32(out)), 15, "effect count");
    let data = w.words(&[0x1111_2222, 0x3333_4444]);
    w.ok(0x0037_b60d, &[ds, 4, 0x20, data, 8, 0]);
    let back = w.scratch(8);
    w.ok(0x0037_b5e6, &[ds, 4, 0x20, back, 8]);
    assert_eq!((w.u32(back), w.u32(back + 4)), (0x1111_2222, 0x3333_4444));
    // XAudioSetEffectData: Halo's I3DL2 call (type 12, index 9, no pOut),
    // then an unknown type, which is a guest-visible error, not a stop.
    let i3dl2 = w.words(&[12, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
    w.ok(0x0037_ba6f, &[9, i3dl2, 0]);
    w.ok(0x0037_b5e6, &[ds, 9, 0x10, back, 4]);
    assert_eq!(w.u32(back), 4, "flags ORed with 4");
    let junk = w.words(&[99, 0, 0]);
    assert_eq!(w.ret(0x0037_ba6f, &[9, junk, 0]), DSERR_INVALIDCALL);

    w.ok(0x0037_b637, &[ds, 1, 3]);
    assert_eq!(w.hle.direct_sound().mix_bin_headroom(1), Some(3));
    w.ok(0x0037_b657, &[ds]);
    w.ok(0x0037_d141, &[ds]);
    w.ok(0x0037_d506, &[ds, bits(3.048), 0]);
    w.ok(0x0037_d52a, &[ds, bits(1.0), 0]);
    w.ok(0x0037_d5cd, &[ds, bits(1.0), 0]);
    w.ok(0x0037_d598, &[ds, bits(1.0), bits(2.0), bits(3.0), 0]);
    w.ok(
        0x0037_d54e,
        &[
            ds,
            bits(0.0),
            bits(0.0),
            bits(1.0),
            bits(0.0),
            bits(1.0),
            bits(0.0),
            0,
        ],
    );
    let listener = format!("{:?}", w.hle.direct_sound().listener());
    assert!(listener.contains("3.048"), "{listener}");
    assert!(
        listener.contains("2.0") && listener.contains("3.0"),
        "{listener}"
    );
    w.ok(0x0037_9f5b, &[]);
    assert!(w.hle.direct_sound().light_hrtf_4_channel());
    w.ok(0x0037_b844, &[]);

    // --- IDirectSoundBuffer ---------------------------------------------
    let (p1, n1, p2, n2) = (w.scratch(4), w.scratch(4), w.scratch(4), w.scratch(4));
    w.ok(0x0037_b7b3, &[buf, 0x80, 0x100, p1, n1, p2, n2, 0]);
    let base = w.u32(p2);
    assert!(ARENA.contains(&base), "buffer memory {base:#x}");
    assert_eq!(
        (w.u32(p1), w.u32(n1), w.u32(n2)),
        (base + 0x80, 0x80, 0x80),
        "Lock wraps"
    );
    w.ok(0x0037_9f40, &[buf, w.u32(p1), 0x80, base, 0x80]);
    let samples = w.put(&[0x10; 0x40]);
    w.ok(0x0037_cc4a, &[buf2, samples, 0x40]);
    w.ok(0x0037_b6df, &[buf, 0, 0, DSBPLAY_LOOPING]);
    w.ok(0x0037_b75b, &[buf, out]);
    assert_eq!(w.u32(out), DSBSTATUS_PLAYING | DSBSTATUS_LOOPING);
    w.ok(0x0037_b73f, &[buf, 1]);
    w.ok(0x0037_b73f, &[buf, 0]);
    w.ok(0x0037_b797, &[buf, 0x10]);
    let (play, write) = (w.scratch(4), w.scratch(4));
    w.ok(0x0037_b777, &[buf, play, write]);
    assert_eq!(w.u32(play), 0x10, "SetCurrentPosition/GetCurrentPosition");
    w.ok(0x0037_b703, &[buf]);
    w.ok(0x0037_b71b, &[buf, 0, 0, 0]);
    w.ok(0x0037_b66f, &[buf, -600i32 as u32]);
    w.ok(0x0037_c5c8, &[buf, 22050]);
    w.ok(0x0037_b68b, &[buf, 0]);
    w.ok(0x0037_b6a7, &[buf, 0]);
    let pairs = w.words(&[0, 0, 1, -600i32 as u32]);
    let bins = w.words(&[2, pairs]);
    w.ok(0x0037_b6c3, &[buf, bins]);
    w.ok(0x0037_c5e4, &[buf, bins]);
    w.ok(0x0037_c668, &[buf, bits(1.0), bits(0.0), bits(0.0), 0]);
    w.ok(0x0037_c644, &[buf, bits(1.0), 0]);
    w.ok(0x0037_c620, &[buf, bits(100.0), 0]);
    w.ok(0x0037_c600, &[buf, 0, 0]);
    w.ok(0x0037_c69d, &[buf, bits(1.0), 0]);
    w.ok(0x0037_c6c1, &[buf, bits(1.0), 0]);
    w.ok(0x0037_c6e5, &[buf, SCRATCH, 0]);
    // The buffer was stopped above: its voice is inactive.
    assert_eq!(w.ret(0x0037_b7e3, &[buf, out]), DSERR_INVALIDCALL);
    w.ok(0x0037_b6df, &[buf, 0, 0, DSBPLAY_LOOPING]);
    let props = w.scratch(0x5c);
    w.ok(0x0037_b7e3, &[buf, props]);
    assert_eq!(
        (w.u32(props), w.u32(props + 4), w.u32(props + 8)),
        (2, 0, 0),
        "default bins 0 and 1"
    );
    assert_eq!(w.u32(props + 12), 1);
    assert_eq!(
        (w.u32(props + 0x14), w.u32(props + 0x18)),
        (0xffff_ffff, 0xffff_d8f0),
        "unused slot"
    );
    w.ok(0x0037_b703, &[buf]);
    // StopEx with a timestamp is a stop; the high timestamp word is arg 3.
    w.stop(0x0037_b71b, &[buf, 0, 1, 0]);

    // --- IDirectSoundStream ---------------------------------------------
    w.ok(0x0037_b7ff, &[stream, -100i32 as u32]);
    w.ok(0x0037_b804, &[stream, 0]);
    w.ok(0x0037_b809, &[stream, SCRATCH]);
    w.ok(0x0037_b80e, &[stream, SCRATCH]);
    w.ok(0x0037_b813, &[stream, SCRATCH]);
    w.ok(0x0037_b818, &[stream, 0]);
    w.ok(0x0037_b81d, &[stream, bins]);
    w.ok(0x0037_c70a, &[stream, bins]);
    w.ok(0x0037_c705, &[stream, 0]);
    w.ok(0x0037_b822, &[stream, 0]);
    w.ok(0x0037_b827, &[stream, 0, 0, 0]);
    // Halo's flags: ASYNC (1) and ASYNC | ENVELOPE (3), timestamp 0.
    w.ok(0x0037_b827, &[stream, 0, 0, 1]);
    w.ok(0x0037_b827, &[stream, 0, 0, 3]);
    // A timestamped FlushEx is a stop.
    w.stop(0x0037_b827, &[stream, 5, 0, 1]);
    assert_eq!(
        w.ret(0x0037_b83f, &[stream, out]),
        DSERR_INVALIDCALL,
        "not started"
    );
    assert_eq!(w.ret(0x0037_ab40, &[stream]), 2, "AddRef");
    assert_eq!(w.ret(0x0037_ab87, &[stream]), 1, "Release");
    let info = w.scratch(16);
    w.ok(0x0037_abd5, &[stream, info]);
    assert_eq!(
        (
            w.u32(info),
            w.u32(info + 4),
            w.u32(info + 8),
            w.u32(info + 12)
        ),
        (5, 2, 0, 128)
    );
    w.ok(0x0037_acd4, &[stream, out]);
    assert_eq!(w.u32(out) & XMO_STATUSF_ACCEPT_INPUT_DATA, 1);
    let (pkt, status, done) = w.packet(0x40, 0);
    w.ok(0x0037_ad25, &[stream, pkt, 0]);
    assert_eq!(
        (w.u32(status), w.u32(done)),
        (XMEDIAPACKET_STATUS_PENDING, 0)
    );
    w.ok(0x0037_b83f, &[stream, props]);
    assert_eq!(w.u32(props), 2, "stream voice properties once it has data");
    w.ok(0x0037_ac3c, &[stream]);
    assert_eq!(
        w.hle.direct_sound().stream_discontinuity_count(stream),
        Ok(1)
    );
    w.ok(0x0037_ac89, &[stream]);
    assert_eq!(
        (w.u32(status), w.u32(done)),
        (XMEDIAPACKET_STATUS_FLUSHED, 0x40)
    );

    // --- teardown ---------------------------------------------------------
    assert_eq!(w.ret(0x0037_9f45, &[buf2]), 0);
    assert_eq!(w.ret(0x0037_9f2a, &[ds]), 0, "last reference");

    let missing: Vec<_> = PINNED
        .iter()
        .filter(|p| !w.seen.contains(&p.0))
        .map(|p| format!("{:#x}", p.0))
        .collect();
    assert!(missing.is_empty(), "not exercised: {missing:?}");
    assert!(w.hle.take_signal_failures().is_empty());
}

#[test]
fn completion_events_are_signalled_after_the_guards_are_dropped() {
    let mut w = world();
    let out = w.scratch(4);
    w.ok(0x0037_d797, &[0, out, 0]);
    let ds = w.u32(out);
    let fmt = w.pcm16_format();
    let sdesc = w.words(&[0, 4, fmt, 0, 0, 0]);
    w.ok(0x0037_d4e2, &[ds, sdesc, out, 0]);
    let stream = w.u32(out);
    // A zero-length packet completes inside Process.
    let (pkt, status, _) = w.packet(0, 0x77);
    w.ok(0x0037_ad25, &[stream, pkt, 0]);
    assert_eq!(w.u32(status), XMEDIAPACKET_STATUS_SUCCESS);
    assert_eq!(*w.signals.lock().unwrap(), [0x77]);
    assert!(w.hle.take_signal_failures().is_empty());
}

#[test]
fn stream_callbacks_run_as_guest_calls_before_returning() {
    let mut w = world();
    let out = w.scratch(4);
    w.ok(0x0037_d797, &[0, out, 0]);
    let ds = w.u32(out);
    let fmt = w.pcm16_format();
    let sdesc = w.words(&[0, 4, fmt, CALLBACK, 0xc0c0, 0]);
    w.ok(0x0037_d4e2, &[ds, sdesc, out, 0]);
    let stream = w.u32(out);
    let (pkt, _, _) = w.packet(0, 0xabcd);
    // Returns S_OK with the frame popped although the callback clobbered EAX.
    w.ok(0x0037_ad25, &[stream, pkt, 0]);
    assert_eq!(
        *w.callbacks.lock().unwrap(),
        [[0xc0c0, 0xabcd, XMEDIAPACKET_STATUS_SUCCESS]]
    );
    assert!(w.signals.lock().unwrap().is_empty());

    // Two queued packets: two callbacks in order, then one return.
    let (a, _, _) = w.packet(0x40, 1);
    let (b, _, _) = w.packet(0, 2);
    w.ok(0x0037_ad25, &[stream, a, 0]);
    w.ok(0x0037_ad25, &[stream, b, 0]);
    w.callbacks.lock().unwrap().clear();
    w.ok(0x0037_ac89, &[stream]);
    assert_eq!(
        *w.callbacks.lock().unwrap(),
        [
            [0xc0c0, 1, XMEDIAPACKET_STATUS_FLUSHED],
            [0xc0c0, 2, XMEDIAPACKET_STATUS_FLUSHED]
        ]
    );
}

#[test]
fn guest_memory_faults_are_runtime_faults_not_stops() {
    let mut w = world();
    let out = w.scratch(4);
    w.ok(0x0037_d797, &[0, out, 0]);
    let ds = w.u32(out);
    let t = w.run(0x0037_b5ca, &[ds, 0x0900_0000]);
    assert_eq!(t.kind, abi::FAULT);
    let Some(Event::MemoryFault { fault, .. }) = w.ctx.event(t.event_id) else {
        panic!("{:?}", w.ctx.event(t.event_id));
    };
    assert_eq!(fault.addr, 0x0900_0000);
    assert_eq!(fault.reason, runtime::guest::FaultReason::Unmapped);
    assert!(w.hle.take_stops().is_empty());
}

#[test]
fn freed_objects_are_reused_zeroed() {
    let mut w = world();
    let out = w.scratch(4);
    w.ok(0x0037_d797, &[0, out, 0]);
    let ds = w.u32(out);
    let fmt = w.pcm16_format();
    let desc = w.words(&[BufferDesc::SIZE, 0, 0x100, fmt, 0, 0]);
    w.ok(0x0037_d4be, &[ds, desc, out, 0]);
    let buf = w.u32(out);
    let live = w.hle.arena().live();
    assert_eq!(w.ret(0x0037_9f45, &[buf]), 0);
    assert_eq!(w.hle.arena().live(), live - 2, "object and sample memory");
    w.ok(0x0037_d4be, &[ds, desc, out, 0]);
    assert_eq!(w.hle.arena().live(), live);
    assert_eq!(w.hle.arena().mapped().len(), 1);
}

// ---- event references -----------------------------------------------------

/// Pins events like `Kernel::reference_dispatcher`: a pinned event signals
/// even after the guest closed its handle, a plain handle signal does not.
struct PinEvents {
    runtime: RuntimeSlot,
    log: Arc<Mutex<Vec<String>>>,
    closed: Arc<Mutex<BTreeSet<u32>>>,
    /// Handles whose `reference` fails (no pin is made for them).
    failing: BTreeSet<u32>,
}

struct Pin {
    handle: u32,
    runtime: RuntimeSlot,
    log: Arc<Mutex<Vec<String>>>,
}

fn relock(runtime: &RuntimeSlot) -> Result<(), String> {
    let runtime = runtime.lock().unwrap().clone().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _guard = runtime.memory();
        let _ = tx.send(());
    });
    rx.recv_timeout(Duration::from_secs(30))
        .map_err(|_| "called under the memory guard".to_owned())
}

impl dsound::EventRef for Pin {
    fn signal(&mut self) -> Result<(), String> {
        relock(&self.runtime)?;
        self.log
            .lock()
            .unwrap()
            .push(format!("pin-signal {:#x}", self.handle));
        Ok(())
    }
}

impl Drop for Pin {
    fn drop(&mut self) {
        self.log
            .lock()
            .unwrap()
            .push(format!("unpin {:#x}", self.handle));
    }
}

impl Events for PinEvents {
    fn signal(&mut self, handle: u32) -> Result<(), String> {
        if self.closed.lock().unwrap().contains(&handle) {
            return Err(format!("handle {handle:#x} is closed"));
        }
        self.log
            .lock()
            .unwrap()
            .push(format!("handle-signal {handle:#x}"));
        Ok(())
    }
    fn reference(&mut self, handle: u32) -> Option<Result<Box<dyn dsound::EventRef>, String>> {
        relock(&self.runtime).ok()?;
        if self.failing.contains(&handle) {
            return Some(Err(format!("reference {handle:#x} failed")));
        }
        self.log.lock().unwrap().push(format!("ref {handle:#x}"));
        Some(Ok(Box::new(Pin {
            handle,
            runtime: Arc::clone(&self.runtime),
            log: Arc::clone(&self.log),
        })))
    }
}

type PinLog = Arc<Mutex<Vec<String>>>;

fn pin_world() -> (World, PinLog, Arc<Mutex<BTreeSet<u32>>>) {
    pin_world_failing(&[])
}

fn pin_world_failing(failing: &[u32]) -> (World, PinLog, Arc<Mutex<BTreeSet<u32>>>) {
    let failing: BTreeSet<u32> = failing.iter().copied().collect();
    let log: PinLog = Arc::new(Mutex::new(Vec::new()));
    let closed = Arc::new(Mutex::new(BTreeSet::new()));
    let (l, c) = (Arc::clone(&log), Arc::clone(&closed));
    let w = world_with(Some(Box::new(move |runtime| {
        Box::new(PinEvents {
            runtime,
            log: l,
            closed: c,
            failing,
        })
    })));
    (w, log, closed)
}

fn stream_of(w: &mut World, max_packets: u32) -> u32 {
    let out = w.scratch(4);
    w.ok(0x0037_d797, &[0, out, 0]);
    let ds = w.u32(out);
    let fmt = w.pcm16_format();
    let sdesc = w.words(&[0, max_packets, fmt, 0, 0, 0]);
    w.ok(0x0037_d4e2, &[ds, sdesc, out, 0]);
    w.u32(out)
}

#[test]
fn a_queued_packet_pins_its_event_and_the_pin_survives_closing_the_handle() {
    let (mut w, log, closed) = pin_world();
    let stream = stream_of(&mut w, 4);
    let (pkt, status, done) = w.packet(0x40, 0x91);
    w.ok(0x0037_ad25, &[stream, pkt, 0]);
    assert_eq!(*log.lock().unwrap(), ["ref 0x91"]);
    assert_eq!(w.hle.pinned_events(), 1);
    // The guest closes its handle before the packet completes.
    closed.lock().unwrap().insert(0x91);
    w.ok(0x0037_ac89, &[stream]);
    assert_eq!(
        (w.u32(status), w.u32(done)),
        (XMEDIAPACKET_STATUS_FLUSHED, 0x40)
    );
    // The signal went through the pin, then the pin was dropped.
    assert_eq!(
        *log.lock().unwrap(),
        ["ref 0x91", "pin-signal 0x91", "unpin 0x91"]
    );
    assert_eq!(w.hle.pinned_events(), 0);
    assert!(w.hle.take_signal_failures().is_empty());
}

#[test]
fn pins_are_per_packet_in_order_and_a_zero_length_packet_signals_inside_process() {
    let (mut w, log, _) = pin_world();
    let stream = stream_of(&mut w, 4);
    let (a, _, _) = w.packet(0x40, 0x92);
    let (b, _, _) = w.packet(0x40, 0x92);
    w.ok(0x0037_ad25, &[stream, a, 0]);
    w.ok(0x0037_ad25, &[stream, b, 0]);
    assert_eq!(w.hle.pinned_events(), 2);
    w.ok(0x0037_ac89, &[stream]);
    assert_eq!(
        *log.lock().unwrap(),
        [
            "ref 0x92",
            "ref 0x92",
            "pin-signal 0x92",
            "unpin 0x92",
            "pin-signal 0x92",
            "unpin 0x92"
        ]
    );
    log.lock().unwrap().clear();
    // Retain and signal come from one call and are delivered in that order.
    let (z, status, _) = w.packet(0, 0x93);
    w.ok(0x0037_ad25, &[stream, z, 0]);
    assert_eq!(w.u32(status), XMEDIAPACKET_STATUS_SUCCESS);
    assert_eq!(
        *log.lock().unwrap(),
        ["ref 0x93", "pin-signal 0x93", "unpin 0x93"]
    );
    assert_eq!(w.hle.pinned_events(), 0);
}

#[test]
fn releasing_a_stream_drops_the_pins_of_packets_that_never_complete() {
    let (mut w, log, _) = pin_world();
    let stream = stream_of(&mut w, 4);
    let (a, status, _) = w.packet(0x40, 0x94);
    w.ok(0x0037_ad25, &[stream, a, 0]);
    assert_eq!(w.hle.pinned_events(), 1);
    assert_eq!(w.ret(0x0037_ab87, &[stream]), 0);
    assert_eq!(w.hle.pinned_events(), 0);
    assert_eq!(*log.lock().unwrap(), ["ref 0x94", "unpin 0x94"]);
    assert_eq!(
        w.u32(status),
        XMEDIAPACKET_STATUS_PENDING,
        "dropped packets stay pending, as in the library"
    );
}

#[test]
fn a_failed_reference_never_steals_another_packets_pin() {
    // A pins; its handle is closed; B's reference fails. B's completion must
    // fall back to its handle and must not consume (or drop) A's pin.
    let (mut w, log, closed) = pin_world_failing(&[0x97]);
    let stream = stream_of(&mut w, 4);
    let (a, _, _) = w.packet(0x40, 0x96);
    let (b, _, _) = w.packet(0x40, 0x97);
    w.ok(0x0037_ad25, &[stream, a, 0]);
    w.ok(0x0037_ad25, &[stream, b, 0]);
    assert_eq!(w.hle.pinned_events(), 1);
    assert_eq!(
        w.hle.take_signal_failures().len(),
        1,
        "B's reference failure"
    );
    closed.lock().unwrap().insert(0x96);
    // Releasing the stream with only B completing is not what happens here:
    // flush completes both in order.
    w.ok(0x0037_ac89, &[stream]);
    assert_eq!(
        *log.lock().unwrap(),
        [
            "ref 0x96",
            "pin-signal 0x96",
            "unpin 0x96",
            "handle-signal 0x97"
        ]
    );
    assert_eq!(w.hle.pinned_events(), 0);
    assert!(w.hle.take_signal_failures().is_empty());
}

#[test]
fn releasing_a_stream_after_a_failed_reference_keeps_the_other_packets_pin_accounting() {
    let (mut w, log, _) = pin_world_failing(&[0x9a]);
    let s1 = stream_of(&mut w, 4);
    let (a, _, _) = w.packet(0x40, 0x99);
    w.ok(0x0037_ad25, &[s1, a, 0]);
    let s2 = stream_of(&mut w, 4);
    let (b, _, _) = w.packet(0x40, 0x9a);
    w.ok(0x0037_ad25, &[s2, b, 0]);
    assert_eq!(w.hle.pinned_events(), 1);
    // B has no pin: releasing its stream must not drop A's.
    assert_eq!(w.ret(0x0037_ab87, &[s2]), 0);
    assert_eq!(w.hle.pinned_events(), 1, "A's pin survives B's release");
    assert_eq!(*log.lock().unwrap(), ["ref 0x99"]);
}

#[test]
fn a_faulting_completion_write_keeps_the_packet_and_its_pin() {
    let (mut w, log, _) = pin_world();
    let stream = stream_of(&mut w, 4);
    let (a, _, _) = w.packet(0x40, 0x9b);
    let done = w.words(&[0]);
    let b = w.words(&[SCRATCH, 0, done, GUARDED, 0x9c, 0]);
    w.ok(0x0037_ad25, &[stream, a, 0]);
    w.ok(0x0037_ad25, &[stream, b, 0]);
    w.runtime
        .memory()
        .protect(GUARDED, Permissions::READ)
        .unwrap();
    let t = w.run(0x0037_ac89, &[stream]);
    assert_eq!(t.kind, abi::FAULT);
    // B's pin is still held and was not released on the failure.
    assert_eq!(w.hle.pinned_events(), 1);
    assert!(!log.lock().unwrap().iter().any(|l| l == "unpin 0x9c"));
    // Once the page is writable the packet completes and signals through it.
    w.runtime
        .memory()
        .protect(GUARDED, Permissions::RW)
        .unwrap();
    w.ok(0x0037_b844, &[]);
    assert_eq!(w.u32(GUARDED), XMEDIAPACKET_STATUS_FLUSHED);
    assert_eq!(w.hle.pinned_events(), 0);
    let log = log.lock().unwrap();
    let sig = log.iter().position(|l| l == "pin-signal 0x9c").unwrap();
    assert!(log[sig + 1] == "unpin 0x9c");
}

#[test]
fn concurrent_pump_and_process_deliver_every_pin_in_order() {
    let (mut w, log, _) = pin_world();
    let stream = stream_of(&mut w, 64);
    let pump = w
        .hle
        .spawn_pump(Arc::clone(&w.runtime), Duration::from_millis(1), true)
        .unwrap();
    for i in 0..48u32 {
        let (pkt, _, _) = w.packet(16, 0x200 + i);
        w.ok(0x0037_ad25, &[stream, pkt, 0]);
    }
    assert!(wait_for(Duration::from_secs(30), || {
        w.hle.pinned_events() == 0
            && log
                .lock()
                .unwrap()
                .iter()
                .filter(|l| l.starts_with("unpin"))
                .count()
                == 48
    }));
    drop(pump);
    let log = log.lock().unwrap();
    assert!(
        !log.iter().any(|l| l.starts_with("handle-signal")),
        "a completion raced ahead of its retain and fell back: {log:?}"
    );
    for i in 0..48u32 {
        let pos = |what: &str| {
            log.iter()
                .position(|l| *l == format!("{what} {:#x}", 0x200 + i))
                .unwrap()
        };
        assert!(pos("ref") < pos("pin-signal"));
        assert!(pos("pin-signal") < pos("unpin"));
    }
    assert!(w.hle.take_signal_failures().is_empty());
}

#[test]
fn unpinned_sinks_fall_back_to_signalling_the_handle() {
    // `KernelLike` does not override `reference`: the handle path is used.
    let mut w = world();
    let stream = stream_of(&mut w, 4);
    let (pkt, _, _) = w.packet(0x40, 0x95);
    w.ok(0x0037_ad25, &[stream, pkt, 0]);
    assert_eq!(w.hle.pinned_events(), 0);
    w.ok(0x0037_ac89, &[stream]);
    assert_eq!(*w.signals.lock().unwrap(), [0x95]);
}

// ---- audio clock and pump --------------------------------------------------

#[test]
fn host_driven_clock_completes_packets_and_defers_callbacks() {
    let mut w = world();
    let out = w.scratch(4);
    w.ok(0x0037_d797, &[0, out, 0]);
    let ds = w.u32(out);
    let fmt = w.pcm16_format();
    let ev = w.words(&[0, 4, fmt, 0, 0, 0]);
    w.ok(0x0037_d4e2, &[ds, ev, out, 0]);
    let s_event = w.u32(out);
    let cb = w.words(&[0, 4, fmt, CALLBACK, 0xc0c0, 0]);
    w.ok(0x0037_d4e2, &[ds, cb, out, 0]);
    let s_cb = w.u32(out);
    // 100 frames at 8 kHz each (200 bytes of 16-bit mono).
    let (a, a_status, a_done) = w.packet(200, 0xa1);
    let (b, b_status, _) = w.packet(200, 0xb2);
    w.ok(0x0037_ad25, &[s_event, a, 0]);
    w.ok(0x0037_ad25, &[s_cb, b, 0]);
    // Nothing consumes the mixer, so nothing finishes.
    w.hle.do_work(&w.runtime).unwrap();
    assert_eq!(w.u32(a_status), XMEDIAPACKET_STATUS_PENDING);
    // 12.5 ms at 8 kHz is exactly 100 frames.
    let mut clock = dsound::AudioClock::new(8000, Duration::from_millis(250));
    let frames = w
        .hle
        .tick(&w.runtime, &mut clock, Duration::from_micros(12_500))
        .unwrap();
    assert_eq!(frames, 100);
    assert_eq!(
        (w.u32(a_status), w.u32(a_done)),
        (XMEDIAPACKET_STATUS_SUCCESS, 200)
    );
    assert_eq!(w.u32(b_status), XMEDIAPACKET_STATUS_SUCCESS);
    // The event is delivered by the host call; the callback must run on a
    // guest thread, so it waits for the next override call.
    assert_eq!(*w.signals.lock().unwrap(), [0xa1]);
    assert_eq!(w.hle.pending_callbacks(), 1);
    assert!(w.callbacks.lock().unwrap().is_empty());
    w.ok(0x0037_b844, &[]);
    assert_eq!(
        *w.callbacks.lock().unwrap(),
        [[0xc0c0, 0xb2, XMEDIAPACKET_STATUS_SUCCESS]]
    );
    assert!(w.hle.take_signal_failures().is_empty());
}

fn wait_for(limit: Duration, mut f: impl FnMut() -> bool) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < limit {
        if f() {
            return true;
        }
        thread::sleep(Duration::from_millis(2));
    }
    f()
}

#[test]
fn the_pump_renders_a_sinkless_mixer_and_completes_packets_on_its_own() {
    let mut w = world();
    let stream = stream_of(&mut w, 4);
    // 1600 frames = 200 ms of audio.
    let (pkt, status, _) = w.packet(3200, 0xc1);
    w.ok(0x0037_ad25, &[stream, pkt, 0]);
    let pump = w
        .hle
        .spawn_pump(Arc::clone(&w.runtime), Duration::from_millis(2), true)
        .unwrap();
    let runtime = Arc::clone(&w.runtime);
    // The status is written first, the event is signalled after the guards drop.
    let signals = Arc::clone(&w.signals);
    assert!(
        wait_for(Duration::from_secs(30), || !signals
            .lock()
            .unwrap()
            .is_empty()),
        "the packet completed with no game DoWork and no external sink"
    );
    assert_eq!(
        runtime.memory().u32(status).unwrap(),
        XMEDIAPACKET_STATUS_SUCCESS
    );
    assert_eq!(*w.signals.lock().unwrap(), [0xc1]);
    assert!(!pump.is_finished());
    drop(pump);
    assert!(w.hle.take_stops().is_empty());
}

#[test]
fn a_service_only_pump_completes_what_an_external_sink_consumed() {
    let mut w = world();
    let stream = stream_of(&mut w, 4);
    let (pkt, status, _) = w.packet(200, 0xd1);
    w.ok(0x0037_ad25, &[stream, pkt, 0]);
    // The app's renderer (or a device callback) consumes the mixer...
    let mixer = Arc::clone(w.hle.direct_sound().mixer());
    mixer.lock().unwrap().render(&mut vec![0.0; 2 * 128]);
    assert_eq!(
        w.u32(status),
        XMEDIAPACKET_STATUS_PENDING,
        "consumed, not yet reported"
    );
    // ...and the pump (render: false) reports it without waiting for the game.
    let pump = w
        .hle
        .spawn_pump(Arc::clone(&w.runtime), Duration::from_millis(2), false)
        .unwrap();
    let runtime = Arc::clone(&w.runtime);
    let signals = Arc::clone(&w.signals);
    assert!(wait_for(Duration::from_secs(30), || !signals
        .lock()
        .unwrap()
        .is_empty()));
    assert_eq!(
        runtime.memory().u32(status).unwrap(),
        XMEDIAPACKET_STATUS_SUCCESS
    );
    assert_eq!(*w.signals.lock().unwrap(), [0xd1]);
    assert_eq!(
        w.hle.direct_sound().frames_rendered(),
        0,
        "the service-only pump never renders"
    );
    drop(pump);
}

// ---- the output thread ---------------------------------------------------

/// A device stand-in for the output thread: it drains instantly (nothing
/// stays queued), keeps everything written, and takes at most `room` more
/// frames in total.
struct CaptureSink {
    rate: u32,
    room: usize,
    played: Arc<Mutex<Vec<f32>>>,
}

impl audio::AudioSink for CaptureSink {
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn queued_frames(&self) -> usize {
        0
    }
    fn free_frames(&self) -> usize {
        self.room
    }
    fn write(&mut self, samples: &[f32]) -> usize {
        let n = (samples.len() / 2).min(self.room);
        self.room -= n;
        self.played
            .lock()
            .unwrap()
            .extend_from_slice(&samples[..2 * n]);
        n
    }
    fn stats(&self) -> audio::SinkStats {
        audio::SinkStats::default()
    }
}

/// Events that record, at the moment of each signal, the status word the
/// packet carrying that handle has in guest memory.
struct StatusAtSignal {
    runtime: RuntimeSlot,
    status_of: Arc<Mutex<Vec<(u32, u32)>>>,
    seen: Arc<Mutex<Vec<(u32, u32)>>>,
}

impl Events for StatusAtSignal {
    fn signal(&mut self, handle: u32) -> Result<(), String> {
        let runtime = self.runtime.lock().unwrap().clone().unwrap();
        let ptr = self
            .status_of
            .lock()
            .unwrap()
            .iter()
            .find(|p| p.0 == handle)
            .map(|p| p.1)
            .ok_or("unknown handle")?;
        let status = runtime.memory().u32(ptr).map_err(|e| e.to_string())?;
        self.seen.lock().unwrap().push((handle, status));
        Ok(())
    }
}

type Seen = Arc<Mutex<Vec<(u32, u32)>>>;

fn status_world() -> (World, Seen, Seen) {
    let status_of: Seen = Arc::new(Mutex::new(Vec::new()));
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let (s, v) = (Arc::clone(&status_of), Arc::clone(&seen));
    let w = world_with(Some(Box::new(move |runtime| {
        Box::new(StatusAtSignal {
            runtime,
            status_of: s,
            seen: v,
        })
    })));
    (w, status_of, seen)
}

/// Queues a mono PCM16 packet of `frames` samples of `value` with `event`.
fn tone_packet(w: &mut World, stream: u32, frames: usize, value: i16, event: u32) -> u32 {
    let pcm: Vec<u8> = std::iter::repeat_n(value, frames)
        .flat_map(|s| s.to_le_bytes())
        .collect();
    let data = w.put(&pcm);
    let status = w.words(&[0x5a5a_5a5a]);
    let done = w.words(&[0x5a5a_5a5a]);
    let pkt = w.words(&[data, pcm.len() as u32, done, status, event, 0]);
    w.ok(0x0037_ad25, &[stream, pkt, 0]);
    status
}

/// Logical-pacing config at origin 0.
fn logical(max_queued: Duration) -> dsound::OutputConfig {
    dsound::OutputConfig {
        period: Duration::from_millis(1),
        pacing: dsound::Pacing::Logical {
            origin: 0,
            max_queued,
        },
        halt_when: None,
        side: None,
    }
}

fn capture(
    played: &Arc<Mutex<Vec<f32>>>,
    room: usize,
) -> impl FnOnce() -> CaptureSink + Send + use<> {
    let played = Arc::clone(played);
    move || CaptureSink {
        rate: 8000,
        room,
        played,
    }
}

#[test]
fn the_output_thread_plays_streams_and_completes_them_on_the_logical_clock() {
    let (mut w, status_of, seen) = status_world();
    let stream = stream_of(&mut w, 8);
    // 8000 Hz mono: 256, 512 and 256 frames of three levels (whole
    // quanta, so each completes at a quantum boundary).
    let plan = [
        (256, 8192i16, 0x71u32),
        (512, 16384, 0x72),
        (256, -8192, 0x73),
    ];
    for (frames, value, event) in plan {
        let status = tone_packet(&mut w, stream, frames, value, event);
        status_of.lock().unwrap().push((event, status));
    }
    let played = Arc::new(Mutex::new(Vec::new()));
    let output = w
        .hle
        .spawn_output(
            Arc::clone(&w.runtime),
            capture(&played, usize::MAX),
            logical(Duration::from_secs(1)),
        )
        .unwrap();
    // No logical time: nothing renders, nothing completes, however long the
    // host waits. The game is "loading": it makes no DirectSound call.
    thread::sleep(Duration::from_millis(30));
    assert_eq!(output.stats().rendered_frames, 0);
    assert!(seen.lock().unwrap().is_empty());
    // 31.9 ms is not yet a whole quantum: nothing.
    assert_eq!(output.advance_to(319_000).unwrap(), 0);
    assert!(seen.lock().unwrap().is_empty());
    // 32 ms = 256 frames: exactly the first packet, reported before
    // advance_to returns.
    assert_eq!(output.advance_to(320_000).unwrap(), 256);
    assert_eq!(seen.lock().unwrap().len(), 1);
    // Then the rest: 768 more frames.
    assert_eq!(output.advance_to(1_280_000).unwrap(), 768);
    assert_eq!(output.advance_to(1_000_000).unwrap(), 0, "time going back");
    // Submission order, each status already final when its event fired.
    assert_eq!(
        *seen.lock().unwrap(),
        [
            (0x71, XMEDIAPACKET_STATUS_SUCCESS),
            (0x72, XMEDIAPACKET_STATUS_SUCCESS),
            (0x73, XMEDIAPACKET_STATUS_SUCCESS)
        ]
    );
    // The thread plays every packet's samples, in order (mono to both).
    assert!(wait_for(Duration::from_secs(30), || played
        .lock()
        .unwrap()
        .len()
        == 2 * 1024));
    let played = played.lock().unwrap();
    let left: Vec<f32> = played.iter().step_by(2).copied().collect();
    assert!(left[..255].iter().all(|&v| v == 0.25));
    assert!(left[257..767].iter().all(|&v| v == 0.5));
    assert!(left[769..1023].iter().all(|&v| v == -0.25));
    assert_eq!(output.stats().dropped_frames, 0);
    assert_eq!(output.stats().rendered_frames, 1024);
    drop(output);
    assert!(w.hle.take_stops().is_empty());
    assert!(w.hle.take_signal_failures().is_empty());
}

#[test]
fn logical_pacing_bounds_its_queue_when_the_sink_is_full() {
    let (mut w, status_of, seen) = status_world();
    let stream = stream_of(&mut w, 8);
    let status = tone_packet(&mut w, stream, 512, 1000, 0x81);
    status_of.lock().unwrap().push((0x81, status));
    let played = Arc::new(Mutex::new(Vec::new()));
    // The sink opens only when the test says, so the thread drains nothing
    // while the runner renders; then it has room for 64 frames.
    let (open_tx, open_rx) = mpsc::channel::<()>();
    let sink = capture(&played, 64);
    let output = w
        .hle
        .spawn_output(
            Arc::clone(&w.runtime),
            move || {
                open_rx.recv().unwrap();
                sink()
            },
            logical(Duration::from_millis(32)),
        )
        .unwrap();
    // 64 ms renders and completes the packet regardless of the sink.
    assert_eq!(output.advance_to(640_000).unwrap(), 512);
    assert_eq!(*seen.lock().unwrap(), [(0x81, XMEDIAPACKET_STATUS_SUCCESS)]);
    // At most 32 ms (256 frames) waits; the older 256 are dropped.
    assert_eq!(output.stats().dropped_frames, 256);
    open_tx.send(()).unwrap();
    assert!(wait_for(Duration::from_secs(30), || played
        .lock()
        .unwrap()
        .len()
        == 2 * 64));
    let stats = output.stats();
    assert_eq!((stats.rendered_frames, stats.dropped_frames), (512, 256));
}

#[test]
fn device_pacing_follows_the_sink_and_its_rate() {
    let (mut w, status_of, seen) = status_world();
    let stream = stream_of(&mut w, 8);
    let status = tone_packet(&mut w, stream, 800, 1000, 0x91);
    status_of.lock().unwrap().push((0x91, status));
    let played = Arc::new(Mutex::new(Vec::new()));
    let sink_played = Arc::clone(&played);
    let output = w
        .hle
        .spawn_output(
            Arc::clone(&w.runtime),
            move || CaptureSink {
                rate: 16_000,
                room: usize::MAX,
                played: sink_played,
            },
            dsound::OutputConfig {
                period: Duration::from_millis(1),
                pacing: dsound::Pacing::Device {
                    target_latency_ms: 20,
                },
                halt_when: None,
                side: None,
            },
        )
        .unwrap();
    // The instantly draining "device" takes 20 ms (320 frames at 16 kHz) a
    // wake; the 8 kHz packet is 100 ms, 1600 output frames.
    assert!(wait_for(Duration::from_secs(30), || !seen
        .lock()
        .unwrap()
        .is_empty()));
    assert_eq!(output.stats().sample_rate, 16_000);
    assert_eq!(
        w.hle.direct_sound().mixer().lock().unwrap().sample_rate(),
        16_000
    );
    assert!(output.stats().rendered_frames >= 1600);
    assert_eq!(*seen.lock().unwrap(), [(0x91, XMEDIAPACKET_STATUS_SUCCESS)]);
    assert!(played.lock().unwrap().iter().any(|&v| v != 0.0));
}

#[test]
fn the_output_thread_is_the_only_consumer() {
    let mut w = world();
    let stream = stream_of(&mut w, 4);
    let _ = tone_packet(&mut w, stream, 16, 1, 0);
    w.hle
        .direct_sound()
        .set_frames_per_do_work(Some(800))
        .unwrap();
    let open = || CaptureSink {
        rate: 8000,
        room: usize::MAX,
        played: Arc::default(),
    };
    let refused = w
        .hle
        .spawn_output(
            Arc::clone(&w.runtime),
            open,
            dsound::OutputConfig::default(),
        )
        .err()
        .unwrap();
    assert_eq!(
        (refused.held, refused.wanted),
        (dsound::Consumer::DoWork, dsound::Consumer::Output)
    );
    w.hle.direct_sound().set_frames_per_do_work(None).unwrap();
    let output = w
        .hle
        .spawn_output(
            Arc::clone(&w.runtime),
            open,
            dsound::OutputConfig::default(),
        )
        .unwrap();
    assert!(
        w.hle
            .spawn_pump(Arc::clone(&w.runtime), Duration::from_millis(1), true)
            .is_err()
    );
    assert!(
        w.hle
            .direct_sound()
            .set_frames_per_do_work(Some(1))
            .is_err()
    );
    assert!(w.hle.advance_audio(&w.runtime, 1).is_err());
    // A service-only pump does not render, so it may run alongside.
    let pump = w
        .hle
        .spawn_pump(Arc::clone(&w.runtime), Duration::from_millis(1), false)
        .unwrap();
    drop(pump);
    drop(output);
    assert_eq!(w.hle.direct_sound().consumer(), None);
    w.hle.advance_audio(&w.runtime, 1).unwrap();
}

#[test]
fn callbacks_completed_by_the_output_thread_run_at_the_next_override_call() {
    let mut w = world();
    let out = w.scratch(4);
    w.ok(0x0037_d797, &[0, out, 0]);
    let ds = w.u32(out);
    let fmt = w.pcm16_format();
    let sdesc = w.words(&[0, 8, fmt, CALLBACK, 0x5000, 0]);
    w.ok(0x0037_d4e2, &[ds, sdesc, out, 0]);
    let stream = w.u32(out);
    for ctx in [0x61, 0x62] {
        let _ = tone_packet(&mut w, stream, 40, 100, ctx);
    }
    let output = w
        .hle
        .spawn_output(
            Arc::clone(&w.runtime),
            || CaptureSink {
                rate: 8000,
                room: usize::MAX,
                played: Arc::default(),
            },
            dsound::OutputConfig {
                period: Duration::from_millis(1),
                pacing: dsound::Pacing::Device {
                    target_latency_ms: 40,
                },
                halt_when: None,
                side: None,
            },
        )
        .unwrap();
    let hle = Arc::clone(&w.hle);
    assert!(wait_for(Duration::from_secs(30), || hle.pending_callbacks() == 2));
    drop(output);
    assert!(
        w.callbacks.lock().unwrap().is_empty(),
        "guest code waits for a guest thread"
    );
    // The game's next DirectSound call (here DoWork) runs them, in order.
    w.ok(0x0037_b844, &[]);
    assert_eq!(
        *w.callbacks.lock().unwrap(),
        [
            [0x5000, 0x61, XMEDIAPACKET_STATUS_SUCCESS],
            [0x5000, 0x62, XMEDIAPACKET_STATUS_SUCCESS]
        ]
    );
}

/// Runs one guest scenario under logical pacing, the runner advancing
/// logical time in `steps`; returns what the sink played, the completions,
/// and the frames rendered.
fn logical_trace(steps: &[u64]) -> (Vec<f32>, Vec<(u32, u32)>, u64) {
    let (mut w, status_of, seen) = status_world();
    let stream = stream_of(&mut w, 8);
    for (frames, value, event) in [(1200usize, 4096i16, 0xa1u32), (600, -4096, 0xa2)] {
        let status = tone_packet(&mut w, stream, frames, value, event);
        status_of.lock().unwrap().push((event, status));
    }
    let played = Arc::new(Mutex::new(Vec::new()));
    let output = w
        .hle
        .spawn_output(
            Arc::clone(&w.runtime),
            capture(&played, usize::MAX),
            logical(Duration::from_secs(1)),
        )
        .unwrap();
    for &t in steps {
        output.advance_to(t).unwrap();
    }
    let rendered = output.stats().rendered_frames;
    assert!(wait_for(
        Duration::from_secs(5),
        || played.lock().unwrap().len() as u64 == 2 * rendered
    ));
    drop(output);
    let seen = seen.lock().unwrap().clone();
    let played = played.lock().unwrap().clone();
    (played, seen, rendered)
}

#[test]
fn logical_pacing_does_not_depend_on_how_time_is_partitioned() {
    // 200 ms at 8 kHz is 1600 frames (six whole quanta, 1536): one step,
    // two, or four render the same audio and complete the same packets.
    let one = logical_trace(&[2_000_000]);
    let two = logical_trace(&[1_000_000, 2_000_000]);
    let many = logical_trace(&[320_000, 330_000, 1_500_000, 2_000_000]);
    assert_eq!(one.2, 1536);
    assert_eq!(one.1, [(0xa1, XMEDIAPACKET_STATUS_SUCCESS)]);
    assert_eq!(one, two);
    assert_eq!(one, many);
}

/// One guest history at 32 ms, observed: a 256-frame packet is submitted at
/// logical time 32 ms, and the runner's scheduler point for 32 ms comes
/// before (`tick_first`) or after the submission; then the runner moves on
/// to 64 ms. The output thread either runs freely throughout or cannot run
/// at all (its sink is held closed). Returns (status and events right
/// after the 32 ms step, the same after 64 ms, the PCM played).
#[allow(clippy::type_complexity)]
fn history_at_32ms(
    tick_first: bool,
    thread_runs: bool,
) -> ((u32, Vec<(u32, u32)>), (u32, Vec<(u32, u32)>), Vec<f32>) {
    let (mut w, status_of, seen) = status_world();
    let stream = stream_of(&mut w, 8);
    let played = Arc::new(Mutex::new(Vec::new()));
    let (open_tx, open_rx) = mpsc::channel::<()>();
    if thread_runs {
        open_tx.send(()).unwrap();
    }
    let sink = capture(&played, usize::MAX);
    let output = w
        .hle
        .spawn_output(
            Arc::clone(&w.runtime),
            move || {
                open_rx.recv().unwrap();
                sink()
            },
            logical(Duration::from_secs(1)),
        )
        .unwrap();
    if thread_runs {
        // Give the host thread every chance to run ahead of the guest.
        thread::sleep(Duration::from_millis(30));
    }
    let submit = |w: &mut World| {
        let status = tone_packet(w, stream, 256, 8192, 0x71);
        status_of.lock().unwrap().push((0x71, status));
        status
    };
    let status = if tick_first {
        output.advance_to(320_000).unwrap();
        submit(&mut w)
    } else {
        let s = submit(&mut w);
        output.advance_to(320_000).unwrap();
        s
    };
    if thread_runs {
        thread::sleep(Duration::from_millis(30));
    }
    let at_32 = (w.u32(status), seen.lock().unwrap().clone());
    output.advance_to(640_000).unwrap();
    let at_64 = (w.u32(status), seen.lock().unwrap().clone());
    if !thread_runs {
        open_tx.send(()).unwrap();
    }
    assert!(wait_for(Duration::from_secs(30), || played
        .lock()
        .unwrap()
        .len()
        == 2 * 512));
    drop(output);
    let pcm = played.lock().unwrap().clone();
    (at_32, at_64, pcm)
}

#[test]
fn guest_results_do_not_depend_on_the_output_thread() {
    // The reviewer's case: the same guest history gives the same statuses,
    // events and audio whether or not the host thread ran in between.
    for tick_first in [true, false] {
        let free = history_at_32ms(tick_first, true);
        let held = history_at_32ms(tick_first, false);
        assert_eq!(free, held, "tick_first={tick_first}");
        let ((status_32, events_32), (status_64, events_64), pcm) = free;
        if tick_first {
            // The 32 ms block was rendered before the packet existed.
            assert_eq!(status_32, XMEDIAPACKET_STATUS_PENDING);
            assert!(events_32.is_empty());
            assert!(pcm[..512].iter().all(|&v| v == 0.0));
            assert!(pcm[512..].iter().any(|&v| v != 0.0));
        } else {
            assert_eq!(status_32, XMEDIAPACKET_STATUS_SUCCESS);
            assert_eq!(events_32, [(0x71, XMEDIAPACKET_STATUS_SUCCESS)]);
            assert!(pcm[..512].iter().any(|&v| v != 0.0));
        }
        assert_eq!(status_64, XMEDIAPACKET_STATUS_SUCCESS);
        assert_eq!(events_64, [(0x71, XMEDIAPACKET_STATUS_SUCCESS)]);
    }
}

#[test]
fn each_thread_leases_the_mixer_alone() {
    let w = world();
    let open = || CaptureSink {
        rate: 8000,
        room: usize::MAX,
        played: Arc::default(),
    };
    let a = w
        .hle
        .spawn_output(
            Arc::clone(&w.runtime),
            open,
            dsound::OutputConfig::default(),
        )
        .unwrap();
    // A second output thread, or a render pump, is refused.
    let b = w
        .hle
        .spawn_output(
            Arc::clone(&w.runtime),
            open,
            dsound::OutputConfig::default(),
        )
        .err()
        .unwrap();
    assert_eq!(
        (b.held, b.wanted),
        (dsound::Consumer::Output, dsound::Consumer::Output)
    );
    assert!(
        w.hle
            .spawn_pump(Arc::clone(&w.runtime), Duration::from_millis(1), true)
            .is_err()
    );
    drop(a);
    assert_eq!(w.hle.direct_sound().consumer(), None);
    // Likewise for render pumps.
    let p = w
        .hle
        .spawn_pump(Arc::clone(&w.runtime), Duration::from_millis(1), true)
        .unwrap();
    let q = w
        .hle
        .spawn_pump(Arc::clone(&w.runtime), Duration::from_millis(1), true)
        .err()
        .unwrap();
    assert_eq!(
        (q.held, q.wanted),
        (dsound::Consumer::Pump, dsound::Consumer::Pump)
    );
    drop(p);
    assert_eq!(w.hle.direct_sound().consumer(), None);
    // A stale lease releases nothing it no longer holds.
    let mut ds = w.hle.direct_sound();
    let old = ds.lease_consumer(dsound::Consumer::Output).unwrap();
    ds.release_lease(&old);
    let new = ds.lease_consumer(dsound::Consumer::Pump).unwrap();
    ds.release_lease(&old);
    assert_eq!(ds.consumer(), Some(dsound::Consumer::Pump));
    assert!(ds.holds(&new) && !ds.holds(&old));
    assert!(ds.claim_consumer(dsound::Consumer::Pump).is_err());
    ds.release_consumer(dsound::Consumer::Pump);
    assert_eq!(
        ds.consumer(),
        Some(dsound::Consumer::Pump),
        "only the lease"
    );
    ds.release_lease(&new);
    assert_eq!(ds.consumer(), None);
}

#[test]
fn a_halted_output_leaves_the_guest_alone() {
    for by_flag in [true, false] {
        let (mut w, status_of, seen) = status_world();
        let stream = stream_of(&mut w, 8);
        let status = tone_packet(&mut w, stream, 256, 1000, 0xb1);
        status_of.lock().unwrap().push((0xb1, status));
        let before = w.u32(status);
        let faulted = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&faulted);
        let mut config = logical(Duration::from_secs(1));
        if by_flag {
            config.halt_when = Some(Box::new(move || flag.load(Ordering::Acquire)));
        }
        let played = Arc::new(Mutex::new(Vec::new()));
        let output = w
            .hle
            .spawn_output(Arc::clone(&w.runtime), capture(&played, usize::MAX), config)
            .unwrap();
        if by_flag {
            faulted.store(true, Ordering::Release);
        } else {
            output.halt();
        }
        // The runner keeps calling: nothing renders, nothing is reported.
        assert_eq!(output.advance_to(10_000_000).unwrap(), 0);
        assert!(wait_for(Duration::from_secs(30), || output.is_finished()));
        assert!(output.stats().halted);
        assert!(seen.lock().unwrap().is_empty());
        assert_eq!(w.u32(status), before, "no status written");
        // The mixer stays claimed until the Output drops.
        assert_eq!(
            w.hle.direct_sound().consumer(),
            Some(dsound::Consumer::Output)
        );
        drop(output);
        assert_eq!(w.hle.direct_sound().consumer(), None);
    }
}

#[test]
fn an_unbounded_sink_at_another_rate_is_resampled() {
    // A capture sink with no limit (usize::MAX free frames) at twice the
    // mixer's 8 kHz: the free-space conversion must not overflow.
    let (mut w, status_of, seen) = status_world();
    let stream = stream_of(&mut w, 8);
    let status = tone_packet(&mut w, stream, 256, 8192, 0xc1);
    status_of.lock().unwrap().push((0xc1, status));
    let played = Arc::new(Mutex::new(Vec::new()));
    let sink_played = Arc::clone(&played);
    let output = w
        .hle
        .spawn_output(
            Arc::clone(&w.runtime),
            move || CaptureSink {
                rate: 16_000,
                room: usize::MAX,
                played: sink_played,
            },
            logical(Duration::from_secs(1)),
        )
        .unwrap();
    assert_eq!(output.advance_to(320_000).unwrap(), 256);
    assert_eq!(*seen.lock().unwrap(), [(0xc1, XMEDIAPACKET_STATUS_SUCCESS)]);
    // About twice the frames come out (the resampler holds the last one).
    assert!(wait_for(Duration::from_secs(30), || played
        .lock()
        .unwrap()
        .len()
        >= 2 * 510));
    assert!(!output.is_finished(), "the output thread is still running");
    assert_eq!(output.stats().sample_rate, 16_000);
    let played = played.lock().unwrap();
    assert!(played.len() <= 2 * 512);
    assert!(played[20..1000].iter().all(|&v| v == 0.25));
}

// The controller-speaker side output (docs/playstation-audio.md): a second
// fake sink for the voices the pad router labels. Synthetic PCM only.

/// A side sink whose device can be unplugged by the test.
struct SideSink {
    inner: CaptureSink,
    connected: Arc<AtomicBool>,
}

impl audio::AudioSink for SideSink {
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
    fn queued_frames(&self) -> usize {
        0
    }
    fn free_frames(&self) -> usize {
        self.inner.free_frames()
    }
    fn write(&mut self, samples: &[f32]) -> usize {
        self.inner.write(samples)
    }
    fn stats(&self) -> audio::SinkStats {
        audio::SinkStats::default()
    }
    fn connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }
}

/// The bytes `tone_packet` queues.
fn tone_bytes(frames: usize, value: i16) -> Vec<u8> {
    std::iter::repeat_n(value, frames)
        .flat_map(|s| s.to_le_bytes())
        .collect()
}

/// Installs a router that sends the tone of `value` (a 256-frame packet) to
/// the controller, exclusively or not.
fn route_tone(w: &World, value: i16, exclusive: bool) {
    use dsound::padroute::{PadRouter, PadSpeakerConfig, RouteTable, SoundIndex};
    let mut index = SoundIndex::new();
    index.insert(r"fx\player\shield_down", &tone_bytes(256, value));
    index.insert(r"music\theme", &tone_bytes(256, value / 2));
    let config = PadSpeakerConfig {
        enabled: true,
        exclusive,
        ..PadSpeakerConfig::default()
    };
    w.hle
        .direct_sound()
        .set_pad_router(Some(PadRouter::new(&index, &RouteTable::new(&config))));
}

struct SideRig {
    main: Arc<Mutex<Vec<f32>>>,
    side: Arc<Mutex<Vec<f32>>>,
    connected: Arc<AtomicBool>,
    live: Arc<Mutex<Vec<bool>>>,
}

impl SideRig {
    fn new() -> Self {
        SideRig {
            main: Arc::default(),
            side: Arc::default(),
            connected: Arc::new(AtomicBool::new(true)),
            live: Arc::default(),
        }
    }
    /// A side output whose sink is there (`present`) or absent.
    fn side(&self, present: bool) -> dsound::SideOutput {
        let (side, connected, live) = (
            Arc::clone(&self.side),
            Arc::clone(&self.connected),
            Arc::clone(&self.live),
        );
        dsound::SideOutput {
            open: Box::new(move || {
                present.then(|| {
                    Box::new(SideSink {
                        inner: CaptureSink {
                            rate: 8000,
                            room: usize::MAX,
                            played: side,
                        },
                        connected,
                    }) as Box<dyn audio::AudioSink>
                })
            }),
            on_live: Some(Box::new(move |l| live.lock().unwrap().push(l))),
        }
    }
    fn config(&self, present: bool) -> dsound::OutputConfig {
        dsound::OutputConfig {
            side: Some(self.side(present)),
            ..logical(Duration::from_secs(1))
        }
    }
    fn live_was(&self, want: &[bool]) -> bool {
        wait_for(Duration::from_secs(10), || *self.live.lock().unwrap() == want)
    }
}

fn left(v: &[f32]) -> Vec<f32> {
    v.iter().step_by(2).copied().collect()
}

#[test]
fn routed_sounds_play_only_on_the_controller_when_exclusive() {
    let (mut w, _, _) = status_world();
    route_tone(&w, 8192, true);
    let stream = stream_of(&mut w, 8);
    let rig = SideRig::new();
    let output = w
        .hle
        .spawn_output(Arc::clone(&w.runtime), capture(&rig.main, usize::MAX), rig.config(true))
        .unwrap();
    assert!(rig.live_was(&[true]), "the side output went live at open");
    // A routed packet, then (once it has played: the label is the
    // stream's) an unrouted one, indexed for the main mix.
    let _ = tone_packet(&mut w, stream, 256, 8192, 0x71);
    assert_eq!(output.advance_to(320_000).unwrap(), 256);
    let _ = tone_packet(&mut w, stream, 256, 4096, 0x72);
    assert_eq!(output.advance_to(640_000).unwrap(), 256);
    assert!(wait_for(Duration::from_secs(10), || {
        rig.main.lock().unwrap().len() == 2 * 512 && rig.side.lock().unwrap().len() == 2 * 512
    }));
    let (main, side) = (left(&rig.main.lock().unwrap()), left(&rig.side.lock().unwrap()));
    // The routed tone: on the controller, silent on the main mix.
    assert!(main[..255].iter().all(|&v| v == 0.0), "{:?}", &main[..4]);
    assert!(side[..255].iter().all(|&v| v > 0.2), "{:?}", &side[..4]);
    // The unrouted one: the other way round.
    assert!(main[257..511].iter().all(|&v| v == 0.125));
    assert!(side[257..511].iter().all(|&v| v == 0.0));
    let stats = output.stats().side.unwrap();
    assert!(stats.live);
    assert_eq!((stats.rendered_frames, stats.dropped_frames, stats.sample_rate), (512, 0, 8000));
    drop(output);
    assert_eq!(*rig.live.lock().unwrap(), [true, false], "back off on drop");
}

#[test]
fn routed_sounds_play_on_both_when_not_exclusive() {
    let (mut w, _, _) = status_world();
    route_tone(&w, 8192, false);
    let stream = stream_of(&mut w, 8);
    let rig = SideRig::new();
    let output = w
        .hle
        .spawn_output(Arc::clone(&w.runtime), capture(&rig.main, usize::MAX), rig.config(true))
        .unwrap();
    assert!(rig.live_was(&[true]));
    let _ = tone_packet(&mut w, stream, 256, 8192, 0x71);
    assert_eq!(output.advance_to(320_000).unwrap(), 256);
    assert!(wait_for(Duration::from_secs(10), || {
        rig.main.lock().unwrap().len() == 2 * 256 && rig.side.lock().unwrap().len() == 2 * 256
    }));
    assert!(left(&rig.main.lock().unwrap())[..255].iter().all(|&v| v == 0.25));
    assert!(left(&rig.side.lock().unwrap())[..255].iter().all(|&v| v > 0.2));
}

#[test]
fn without_the_controller_device_routed_sounds_stay_on_the_main_mix() {
    let (mut w, _, _) = status_world();
    route_tone(&w, 8192, true);
    let stream = stream_of(&mut w, 8);
    let rig = SideRig::new();
    let output = w
        .hle
        .spawn_output(Arc::clone(&w.runtime), capture(&rig.main, usize::MAX), rig.config(false))
        .unwrap();
    let _ = tone_packet(&mut w, stream, 256, 8192, 0x71);
    assert_eq!(output.advance_to(320_000).unwrap(), 256);
    assert!(wait_for(Duration::from_secs(10), || rig.main.lock().unwrap().len() == 2 * 256));
    assert!(left(&rig.main.lock().unwrap())[..255].iter().all(|&v| v == 0.25));
    let stats = output.stats().side.unwrap();
    assert!(!stats.live);
    assert_eq!(stats.rendered_frames, 0);
    drop(output);
    assert!(rig.live.lock().unwrap().is_empty(), "never live, never switched");
}

#[test]
fn an_unplugged_controller_hands_its_sounds_back_to_the_main_mix() {
    let (mut w, _, _) = status_world();
    route_tone(&w, 8192, true);
    let stream = stream_of(&mut w, 8);
    let rig = SideRig::new();
    let output = w
        .hle
        .spawn_output(Arc::clone(&w.runtime), capture(&rig.main, usize::MAX), rig.config(true))
        .unwrap();
    assert!(rig.live_was(&[true]));
    rig.connected.store(false, Ordering::Release);
    assert!(rig.live_was(&[true, false]));
    assert!(!output.stats().side.unwrap().live);
    let _ = tone_packet(&mut w, stream, 256, 8192, 0x71);
    assert_eq!(output.advance_to(320_000).unwrap(), 256);
    assert!(wait_for(Duration::from_secs(10), || rig.main.lock().unwrap().len() == 2 * 256));
    assert!(left(&rig.main.lock().unwrap())[..255].iter().all(|&v| v == 0.25));
    assert!(rig.side.lock().unwrap().is_empty());
    // Plugged back in: live again, and the next routed sound goes there.
    rig.connected.store(true, Ordering::Release);
    assert!(rig.live_was(&[true, false, true]));
    let _ = tone_packet(&mut w, stream, 256, 8192, 0x72);
    assert_eq!(output.advance_to(640_000).unwrap(), 256);
    assert!(wait_for(Duration::from_secs(10), || rig.side.lock().unwrap().len() == 2 * 256));
    assert!(left(&rig.side.lock().unwrap())[..255].iter().all(|&v| v > 0.2));
}

#[test]
fn device_pacing_feeds_the_side_output_too() {
    let (mut w, _, _) = status_world();
    route_tone(&w, 8192, true);
    let stream = stream_of(&mut w, 8);
    let _ = tone_packet(&mut w, stream, 256, 8192, 0x71);
    let rig = SideRig::new();
    let output = w
        .hle
        .spawn_output(
            Arc::clone(&w.runtime),
            capture(&rig.main, 256),
            dsound::OutputConfig {
                period: Duration::from_millis(1),
                pacing: dsound::Pacing::Device {
                    target_latency_ms: 40,
                },
                halt_when: None,
                side: Some(rig.side(true)),
            },
        )
        .unwrap();
    assert!(wait_for(Duration::from_secs(10), || rig.side.lock().unwrap().len() >= 2 * 256));
    drop(output);
    let (main, side) = (left(&rig.main.lock().unwrap()), left(&rig.side.lock().unwrap()));
    assert!(main.iter().all(|&v| v == 0.0), "the routed tone stays off the main mix");
    assert!(side[..255].iter().all(|&v| v > 0.2));
}
