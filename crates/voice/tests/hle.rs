//! The media-object overrides through the real runtime Runner and Dispatch,
//! with stand-ins for the engine's DPC and its two completion callbacks
//! (docs/live/live-voice.md, 3.3).

use runtime::{
    CpuAbi, Dispatch, Event, ExecCtx, Guest, GuestFault, Runner, Runtime, TransferAbi, abi,
    exec::HARNESS_EXIT, guest::Permissions,
};
use std::sync::{Arc, Mutex};
use voice::{
    HOST_FRAME,
    hle::{self, RegisterError, Shared, Voice, addr},
    io::FakeVoiceIo,
    xmo::hresult,
};

const MIC_CALLBACK: u32 = 0x0033_51a9;
const PHONE_CALLBACK: u32 = 0x0033_51c0;
const STACK_TOP: u32 = 0x0061_0000;
const SCRATCH: u32 = 0x0070_0000;
const FORMAT: u32 = SCRATCH;
const OUT: u32 = SCRATCH + 0x40;
const PACKET: u32 = SCRATCH + 0x100;
const CELLS: u32 = SCRATCH + 0x200;
const BUFFER: u32 = SCRATCH + 0x1000;

type Log = Arc<Mutex<Vec<(&'static str, [u32; 3])>>>;

struct World {
    runtime: Arc<Runtime>,
    ctx: ExecCtx,
    cpu: CpuAbi,
    voice: Shared,
    io: FakeVoiceIo,
    log: Log,
}

fn ret(cpu: &mut CpuAbi, ctx: &mut ExecCtx, bytes: u32) -> Result<TransferAbi, GuestFault> {
    let pc = ctx.memory().u32(cpu.esp)?;
    cpu.esp += 4 + bytes;
    Ok(TransferAbi::ret(pc))
}

fn world() -> World {
    let log: Log = Arc::default();
    let mut d = Dispatch::default();
    {
        let log = Arc::clone(&log);
        d.register_generated_rust(addr::ENGINE_DPC, move |cpu, ctx, _| {
            let m = ctx.memory();
            let args = [
                m.u32(cpu.esp + 4)?,
                m.u32(cpu.esp + 8)?,
                m.u32(cpu.esp + 12)?,
            ];
            drop(m);
            log.lock().unwrap().push(("dpc", args));
            ret(cpu, ctx, 0x10)
        })
        .unwrap();
    }
    for (at, name) in [(MIC_CALLBACK, "mic"), (PHONE_CALLBACK, "phone")] {
        let log = Arc::clone(&log);
        d.register_generated_rust(at, move |cpu, ctx, _| {
            let m = ctx.memory();
            let args = [
                m.u32(cpu.esp + 4)?,
                m.u32(cpu.esp + 8)?,
                m.u32(cpu.esp + 12)?,
            ];
            drop(m);
            log.lock().unwrap().push((name, args));
            ret(cpu, ctx, 0xc)
        })
        .unwrap();
    }
    let mut g = Guest::new();
    g.map_zeroed(0x0033_2000, 0x4000, Permissions::RWX).unwrap();
    g.map_zeroed(0x0041_0000, 0x2000, Permissions::RWX).unwrap();
    g.map_zeroed(0x0048_4000, 0x1000, Permissions::RW).unwrap();
    g.map_zeroed(STACK_TOP - 0x1_0000, 0x1_0000, Permissions::RW)
        .unwrap();
    g.map_zeroed(SCRATCH, 0x1_0000, Permissions::RW).unwrap();
    // The engine's format, as 0x332f67 writes it at 0x484290.
    g.write_bytes(
        FORMAT,
        &[
            1, 0, 1, 0, 0x80, 0x3e, 0, 0, 0x00, 0x7d, 0, 0, 2, 0, 16, 0, 0, 0,
        ],
        1,
    )
    .unwrap();
    let runtime = Runtime::new(g, d);
    let io = FakeVoiceIo::new();
    let voice: Shared = Arc::new(Mutex::new(Voice::new(Box::new(io.clone()))));
    hle::register(&runtime, &voice).unwrap();
    let ctx = ExecCtx::new(Arc::clone(&runtime), 1);
    World {
        runtime,
        ctx,
        cpu: CpuAbi::default(),
        voice,
        io,
        log,
    }
}

impl World {
    /// stdcall `target(args...)`, which must return to the harness with
    /// its arguments popped; gives EAX.
    fn call(&mut self, target: u32, args: &[u32]) -> u32 {
        let esp = STACK_TOP - 0x100;
        {
            let mut m = self.runtime.memory();
            m.set_u32(esp, HARNESS_EXIT).unwrap();
            for (i, a) in args.iter().enumerate() {
                m.set_u32(esp + 4 + 4 * i as u32, *a).unwrap();
            }
        }
        self.cpu.esp = esp;
        self.cpu.eax = 0xdead_beef;
        let t = Runner::default().run(&mut self.cpu, &mut self.ctx, target);
        assert_eq!(t.kind, abi::STOP, "{target:#x}: {t:?}");
        assert!(
            matches!(self.ctx.event(t.event_id), Some(Event::HarnessExit)),
            "{target:#x} did not return: {:?}",
            self.ctx.event(t.event_id)
        );
        assert_eq!(
            self.cpu.esp,
            esp + 4 + 4 * args.len() as u32,
            "{target:#x} cleanup"
        );
        self.cpu.eax
    }

    fn u32(&self, a: u32) -> u32 {
        self.runtime.memory().u32(a).unwrap()
    }

    fn samples(&self, a: u32, n: usize) -> Vec<i16> {
        let mut b = vec![0; n * 2];
        self.runtime.memory().read_bytes(a, &mut b, 1).unwrap();
        b.as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes(*c))
            .collect()
    }

    fn present(&self, ports: u32) {
        self.voice.lock().unwrap().objects.set_present(ports);
    }

    fn create(&mut self, ty: u32, port: u32, callback: u32, context: u32) -> u32 {
        self.call(addr::CREATE, &[ty, port, 2, FORMAT, callback, context, OUT])
    }

    /// The engine's packet (0x334c43): 640 bytes, the index as context,
    /// here with the size and status cells set.
    fn packet(&self, buffer: u32, index: u32) {
        let words = [buffer, 0x280, CELLS, CELLS + 4, index, 0];
        let mut m = self.runtime.memory();
        for (i, w) in words.iter().enumerate() {
            m.set_u32(PACKET + 4 * i as u32, *w).unwrap();
        }
    }

    fn dpc(&mut self) {
        self.call(addr::ENGINE_DPC, &[0x0048_4270, 0x11, 0x22, 0x33]);
    }

    fn take_log(&self) -> Vec<(&'static str, [u32; 3])> {
        std::mem::take(&mut *self.log.lock().unwrap())
    }
}

#[test]
fn create_uses_the_original_slots() {
    let mut w = world();
    w.present(0b0101);
    assert_eq!(w.create(addr::MICROPHONE_TYPE, 2, MIC_CALLBACK, 0x5000), 0);
    let slot = addr::MICROPHONE_SLOTS + 2 * 0x1c;
    assert_eq!(w.u32(OUT), slot);
    assert_eq!(w.u32(slot), addr::VTABLE);
    assert_eq!(w.u32(slot + 0xc), 1, "refcount");
    assert_eq!(w.u32(slot + 0x10), MIC_CALLBACK);
    assert_eq!(w.u32(slot + 0x14), 0x5000);

    assert_eq!(w.create(addr::HEADPHONE_TYPE, 0, PHONE_CALLBACK, 0x5100), 0);
    assert_eq!(w.u32(OUT), addr::HEADPHONE_SLOTS);
}

#[test]
fn create_reports_the_originals_errors() {
    let mut w = world();
    w.present(0b0001);
    assert_eq!(
        w.create(addr::MICROPHONE_TYPE, 1, MIC_CALLBACK, 0),
        hresult::DEVICE_NOT_CONNECTED
    );
    assert_eq!(
        w.create(0x0040_88c8, 0, MIC_CALLBACK, 0),
        hresult::DEVICE_NOT_CONNECTED,
        "the third XPP type"
    );
    assert_eq!(w.create(addr::MICROPHONE_TYPE, 0, MIC_CALLBACK, 0), 0);
    assert_eq!(
        w.create(addr::MICROPHONE_TYPE, 0, MIC_CALLBACK, 0),
        hresult::SHARING_VIOLATION
    );
    assert_eq!(
        w.call(
            addr::CREATE,
            &[addr::MICROPHONE_TYPE, 0, 2, FORMAT, 0, 0, 0]
        ),
        hresult::E_INVALIDARG,
        "no out pointer"
    );
}

#[test]
fn the_engine_dpc_completes_then_runs_the_original() {
    let mut w = world();
    w.present(1);
    w.voice.lock().unwrap().objects.set_talker(Some(0));
    assert_eq!(w.create(addr::MICROPHONE_TYPE, 0, MIC_CALLBACK, 0x5000), 0);
    let mic = w.u32(OUT);
    assert_eq!(w.create(addr::HEADPHONE_TYPE, 0, PHONE_CALLBACK, 0x5100), 0);
    let phone = w.u32(OUT);

    w.packet(BUFFER, 1);
    assert_eq!(w.call(addr::PROCESS, &[mic, 0, PACKET]), 0);
    assert_eq!((w.u32(CELLS), w.u32(CELLS + 4)), (0, hresult::E_PENDING));
    w.packet(BUFFER + 0x1000, 0);
    {
        let mut m = w.runtime.memory();
        let pcm: Vec<u8> = [700i16; 320].iter().flat_map(|s| s.to_le_bytes()).collect();
        m.write_bytes(BUFFER + 0x1000, &pcm, 1).unwrap();
    }
    assert_eq!(w.call(addr::PROCESS, &[phone, PACKET, 0]), 0);
    assert_eq!(
        w.call(addr::PROCESS, &[0x0048_4000, 0, PACKET]),
        hresult::E_INVALIDARG,
        "not an object"
    );

    w.io.speak([1234; HOST_FRAME]);
    w.dpc();
    assert_eq!(
        w.take_log(),
        [
            ("mic", [0x5000, 1, 0]),
            ("phone", [0x5100, 0, 0]),
            ("dpc", [0x0048_4270, 0x11, 0x22]),
        ]
    );
    assert!(w.samples(BUFFER, 320).iter().all(|&s| s == 1234));
    assert_eq!((w.u32(CELLS), w.u32(CELLS + 4)), (0x280, 0));
    let played = w.io.take_played();
    assert_eq!(played.len(), 1);
    assert_eq!(played[0][HOST_FRAME - 1], 700);

    // Nothing queued: only the original runs.
    w.dpc();
    assert_eq!(w.take_log(), [("dpc", [0x0048_4270, 0x11, 0x22])]);
}

#[test]
fn release_at_zero_frees_the_port() {
    let mut w = world();
    w.present(1);
    assert_eq!(w.create(addr::MICROPHONE_TYPE, 0, MIC_CALLBACK, 0), 0);
    let mic = w.u32(OUT);
    w.packet(BUFFER, 0);
    w.call(addr::PROCESS, &[mic, 0, PACKET]);
    // AddRef stays generated; it only increments +0xc.
    w.runtime.memory().set_u32(mic + 0xc, 2).unwrap();
    assert_eq!(w.call(addr::RELEASE, &[mic]), 1);
    assert_eq!(
        w.create(addr::MICROPHONE_TYPE, 0, MIC_CALLBACK, 0),
        hresult::SHARING_VIOLATION
    );
    assert_eq!(w.call(addr::RELEASE, &[mic]), 0);
    w.dpc();
    assert_eq!(w.take_log().len(), 1, "the flushed packet is not reported");
    assert_eq!(w.create(addr::MICROPHONE_TYPE, 0, MIC_CALLBACK, 0), 0);
}

#[test]
fn registering_twice_installs_nothing() {
    let w = world();
    assert_eq!(
        hle::register(&w.runtime, &w.voice),
        Err(RegisterError::Occupied {
            addr: addr::CREATE,
            existing: "voice::XVoiceCreateMediaObjectEx".into()
        })
    );
}

#[test]
fn register_needs_the_generated_dpc() {
    let runtime = Runtime::new(Guest::new(), Dispatch::default());
    let voice: Shared = Arc::new(Mutex::new(Voice::new(Box::new(FakeVoiceIo::new()))));
    assert_eq!(
        hle::register(&runtime, &voice),
        Err(RegisterError::NoOriginalDpc)
    );
}
