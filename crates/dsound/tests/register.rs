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
        rx.recv_timeout(Duration::from_secs(2))
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
    let mut g = Guest::new();
    // The DSOUND section; the runner checks Execute on override targets.
    g.map_zeroed(0x0037_9000, 0xf000, Permissions::RWX).unwrap();
    g.map_zeroed(0x0012_3000, 0x1000, Permissions::RWX).unwrap();
    g.map_zeroed(STACK_TOP - 0x1_0000, 0x1_0000, Permissions::RW)
        .unwrap();
    g.map_zeroed(SCRATCH, 0x1_0000, Permissions::RW).unwrap();
    g.map_zeroed(GUARDED, 0x1000, Permissions::RW).unwrap();
    let runtime = Runtime::new(g, Dispatch::default());
    let (hle, slot, signals) = hle();
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
    // The next override call on a guest thread runs it.
    w.ok(0x0037_b844, &[]);
    assert_eq!(
        *w.callbacks.lock().unwrap(),
        [[0xc0c0, 1, XMEDIAPACKET_STATUS_FLUSHED]]
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
    w.stop(0x0037_ba6f, &[1, 2, 3]);

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
    w.stop(0x0037_b7e3, &[buf, out]);
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
    w.stop(0x0037_b83f, &[stream, out]);
    assert_eq!(w.ret(0x0037_ab40, &[stream]), 2, "AddRef");
    assert_eq!(w.ret(0x0037_ab87, &[stream]), 1, "Release");
    w.stop(0x0037_abd5, &[stream, out]);
    w.ok(0x0037_acd4, &[stream, out]);
    assert_eq!(w.u32(out) & XMO_STATUSF_ACCEPT_INPUT_DATA, 1);
    let (pkt, status, done) = w.packet(0x40, 0);
    w.ok(0x0037_ad25, &[stream, pkt, 0]);
    assert_eq!(
        (w.u32(status), w.u32(done)),
        (XMEDIAPACKET_STATUS_PENDING, 0)
    );
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
