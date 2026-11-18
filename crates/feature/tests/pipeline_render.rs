//! `pipeline-render` on a real runtime with synthetic guest code: a Rust
//! function registered as the generated `render_frame_and_present` (its
//! state lives in guest registers, stack and memory only, as generated
//! code's does, so a run stopped on the render thread can carry on on the
//! game thread through its resume points), a D3D entry, a kernel call, a
//! frame wait, and a sim between frames. Each test runs the same frames
//! with the feature off (serial) and on (pipelined) and compares.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use engine::points;
use feature::pipeline_render::{Addrs, PipelineRender};
use feature::{Registry, Remap, Switches};
use runtime::exec::HARNESS_EXIT;
use runtime::guest::Permissions;
use runtime::{CpuAbi, Dispatch, Event, ExecCtx, Guest, Runner, Runtime, TransferAbi, kernel};

const CODE: u32 = 0x1_0000;
const FRAME: u32 = 0x1_1000;
const D3D_DRAW: u32 = 0x1_2000;
const D3D_SIM: u32 = 0x1_2100;
const FRAME_WAIT: u32 = 0x1_3000;
/// A D3D entry that calls back into the guest (as a push-buffer callback
/// does), through a Rust continuation; the callback calls the kernel.
const D3D_CALLBACK: u32 = 0x1_2200;
const CALLBACK: u32 = 0x1_4000;
/// KeQuerySystemTime(PLARGE_INTEGER): stdcall, one argument.
const KERNEL_ORDINAL: u32 = 128;

const DATA: u32 = 0x2_0000;
const GLOBALS: u32 = 0x2_0000;
const GLOBALS_RECORD: u32 = 0x2_1000;
const COUNTDOWNS: (u32, u32) = (0x2_0020, 0x2_0024);
const PRESENTS: u32 = 0x2_0030;
const FRAMES: u32 = 0x2_0040;
const CLEAR: u32 = 0x2_0044;
const MARK: u32 = 0x2_0048;
const STAMPS: (u32, u32) = (0x2_0050, 0x2_0060);
const VBLANKS: (u32, u32) = (0x2_0060, 0x2_0068);
/// What the sim writes and the render reads.
const INPUT: u32 = 0x2_4000;
const FALLBACK: u32 = 0x2_4010;
const WAITS: u32 = 0x2_4020;
const KERNEL_OUT: u32 = 0x2_4038;
const KERNEL_CALLS: u32 = 0x2_4040;
/// Bytes of stack the render's entry probes below its `esp`, page by page,
/// as `_chkstk` does for a large frame (0 for none).
const DEEP: u32 = 0x2_4050;
/// Set: the sim frees objects (below) and the render's tail reads one.
const CHURN: u32 = 0x2_4054;
/// Object table: a body pointer per index (0: freed), and the bodies.
const OBJECTS: u32 = 0x2_4100;
/// The I/O queue head (a live range, as the game's pool is), the request
/// records (outside it), and what the worker read through one.
const QUEUE: u32 = 0x2_4060;
const REQUESTS: u32 = 0x2_4400;
const WORKER_READ: u32 = 0x2_4064;
/// A page mapped read-only; the render makes it writable (mode 6).
const PROTECTED: u32 = 0x3_0000;
/// NtYieldExecution: refused, not serviced (mode 5's fallback).
const YIELD_ORDINAL: u32 = 238;
/// MmSetAddressProtect(base, size, protect): stdcall, three arguments.
const PROTECT_ORDINAL: u32 = 182;
const BODIES: u32 = 0x2_4200;
/// Only the render writes this page.
const OUT: u32 = 0x2_5000;
/// Both write this line: the sim byte 0, the render byte 8.
const SHARED_LINE: u32 = 0x2_6000;
/// The D3D layer's own page (a push buffer).
const PUSH: u32 = 0x2_7000;
const DATA_LEN: u32 = 0x8000;

/// The game thread's stack: as large as Halo 2's (its main thread's).
const GAME_STACK: u32 = 0x4_0000;
const STACK_TOP: u32 = 0x10_0000;

static ADDRS: Addrs = Addrs {
    frame_wait: FRAME_WAIT,
    engine_globals: GLOBALS,
    ui_state: (0x2_0010, 0x2_0011, 0x2_0014),
    cache_countdowns: COUNTDOWNS,
    present_counter: PRESENTS,
    frame_counter: FRAMES,
    launch_clear: CLEAR,
    mark_counters: &[MARK],
    mark_bytes: &[STAMPS],
    host_owned: &[VBLANKS],
    // What the kernel call answers: the render reads it after the call.
    live_bytes: &[(KERNEL_OUT, KERNEL_OUT + 4), (QUEUE, QUEUE + 4)],
    io_queue: QUEUE,
    warmup: 0,
    after_fallback: 0,
};

/// What the D3D entries saw, in order: (entry, value read at INPUT, ran on
/// the render thread's runtime).
type Log = Arc<Mutex<Vec<(&'static str, u32, bool)>>>;

struct Rig {
    rt: Arc<Runtime>,
    ctx: ExecCtx,
    log: Log,
    feature: PipelineRender,
    plan: feature::Plan,
}

fn stdcall_ret(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    cleanup: u32,
) -> Result<TransferAbi, runtime::GuestFault> {
    ctx.memory().ret(cpu, cleanup)
}

/// The synthetic render front-end. Entry: read INPUT, write OUT, the
/// render's byte of SHARED_LINE, the frame counter, a visit mark; call
/// D3D_DRAW(&INPUT). Then, when FALLBACK is set, the kernel. Then the frame
/// wait. Then present (the counter) and return.
fn render(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    sel: u32,
) -> Result<TransferAbi, runtime::GuestFault> {
    let rt = Arc::clone(ctx.runtime());
    let mut m = rt.memory();
    match sel {
        FRAME => {
            probe(&m, cpu.esp)?;
            let v = m.u32(INPUT)?;
            // The object this frame draws, picked from the frame's state and
            // kept in a register (as a handle list built early in the
            // frame is used late).
            cpu.esi = v % 16;
            m.set_u32(OUT + 4 * (v % 64), v.wrapping_mul(3))?;
            m.set_u8(SHARED_LINE + 8, v as u8)?;
            let n = m.u32(FRAMES)? + 1;
            m.set_u32(FRAMES, n)?;
            let mark = m.u32(MARK)? + 1;
            m.set_u32(MARK, mark)?;
            m.set_u8(STAMPS.0 + (v % 16), mark as u8)?;
            // The stamp, as a handle list's mark is used later in a frame.
            cpu.edi = u32::from(mark as u8);
            cpu.ebp = 0;
            m.set_u8(VBLANKS.0, 0xee)?;
            m.set_u8(CLEAR, 0)?;
            cpu.esp -= 4;
            m.set_u32(cpu.esp, INPUT)?;
            m.call(cpu, D3D_DRAW, FRAME + 0x10)
        }
        s if s == FRAME + 0x10 => {
            if m.u32(FALLBACK)? == 2 {
                return m.call(cpu, D3D_CALLBACK, FRAME + 0x20);
            }
            if m.u32(FALLBACK)? == 6 {
                // The page made writable, then written at FRAME + 0x20.
                for v in [4, 0x1000, PROTECTED] {
                    cpu.esp -= 4;
                    m.set_u32(cpu.esp, v)?;
                }
                return m.call(cpu, kernel::function_token(PROTECT_ORDINAL), FRAME + 0x20);
            }
            if m.u32(FALLBACK)? == 4 {
                // An I/O request: a record outside the live ranges (a
                // buffer pointer and a completion byte the worker sets),
                // queued at QUEUE (a live range), then the kernel call
                // wakes the worker.
                let rec = REQUESTS + 0x10 * cpu.esi;
                m.set_u32(rec, OUT)?;
                m.set_u32(rec + 4, rec + 8)?;
                m.set_u8(rec + 8, 0)?;
                m.set_u32(QUEUE, rec)?;
            }
            if m.u32(FALLBACK)? == 3 {
                // The answer to a local on the stack (as the semaphore's
                // previous count is).
                cpu.esp -= 8;
                let local = cpu.esp;
                cpu.esp -= 4;
                m.set_u32(cpu.esp, local)?;
                return m.call(cpu, kernel::function_token(KERNEL_ORDINAL), FRAME + 0x20);
            }
            if m.u32(FALLBACK)? != 0 {
                cpu.esp -= 4;
                m.set_u32(cpu.esp, KERNEL_OUT)?;
                return m.call(cpu, kernel::function_token(KERNEL_ORDINAL), FRAME + 0x20);
            }
            drop(m);
            render(cpu, ctx, FRAME + 0x20)
        }
        s if s == FRAME + 0x20 => {
            let mode = m.u32(FALLBACK)?;
            if mode == 5 && cpu.ebp == 0 {
                // After the serviced call, a refused one: a fallback.
                cpu.ebp = 5;
                return m.call(cpu, kernel::function_token(YIELD_ORDINAL), FRAME + 0x20);
            }
            if cpu.ebp == 5 {
                // The rest of the render (live, on the game thread, where the
                // mode word is the next tick's already) sees the stamp its
                // frame made before the serviced call.
                cpu.ebp = 0;
                let stamp = u32::from(m.u8(STAMPS.0 + cpu.esi)?);
                m.set_u32(OUT + 0xb00 + 4 * cpu.esi, u32::from(stamp == cpu.edi))?;
            }
            if mode == 6 {
                m.set_u32(PROTECTED + 4 * cpu.esi, 0x600 + cpu.esi)?;
            }
            // After the kernel (on the game thread in a fallback, still on
            // the render stack): a deep frame here too.
            probe(&m, cpu.esp)?;
            // The object picked at the entry, through the table: freed by
            // a later tick, its body is gone (a null read).
            if m.u32(CHURN)? != 0 {
                let body = m.u32(OBJECTS + 4 * cpu.esi)?;
                let radius = m.u32(body + 0x3c)?;
                m.set_u32(OUT + 0x900 + 4 * cpu.esi, radius)?;
            }
            // What the kernel answered goes to OUT too.
            let k = if m.u32(FALLBACK)? == 3 {
                let k = m.u32(cpu.esp)?;
                cpu.esp += 8;
                k
            } else {
                m.u32(KERNEL_OUT)?
            };
            m.set_u32(OUT + 0x800, k)?;
            // Per frame too, so a later frame does not hide a wrong one.
            m.set_u32(OUT + 0xa00 + 4 * cpu.esi, k)?;
            m.call(cpu, FRAME_WAIT, FRAME + 0x30)
        }
        s if s == FRAME + 0x30 => {
            let n = m.u64(PRESENTS)? + 1;
            m.set_u64(PRESENTS, n)?;
            m.ret(cpu, 0)
        }
        _ => unreachable!("selector {sel:#x}"),
    }
}

/// `_chkstk`'s probes: a read of every page of a DEEP-byte frame below
/// `esp`, down to its last byte.
fn probe(m: &Guest, esp: u32) -> Result<(), runtime::GuestFault> {
    let deep = m.u32(DEEP)?;
    let low = esp - deep;
    let mut at = esp;
    while at > low {
        at -= 0x1000.min(at - low);
        m.u32(at)?;
    }
    Ok(())
}

/// The guest callback D3D_CALLBACK calls: the kernel, then return.
fn callback(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    sel: u32,
) -> Result<TransferAbi, runtime::GuestFault> {
    let rt = Arc::clone(ctx.runtime());
    let mut m = rt.memory();
    if sel == CALLBACK {
        cpu.esp -= 4;
        m.set_u32(cpu.esp, KERNEL_OUT)?;
        return m.call(cpu, kernel::function_token(KERNEL_ORDINAL), CALLBACK + 0x10);
    }
    m.ret(cpu, 0)
}

fn rig(on: bool) -> Rig {
    rig_with(on, &[])
}

/// `serviced`: the kernel calls the game thread makes for the render.
fn rig_with(on: bool, serviced: &[&'static str]) -> Rig {
    rig_cfg(on, serviced, ADDRS.clone())
}

/// [`rig_with`] with other addresses (the guards' settings).
fn rig_cfg(on: bool, serviced: &[&'static str], addrs: Addrs) -> Rig {
    rig_full(on, serviced, addrs, &[])
}

/// [`rig_cfg`] watching guest words (`H2_PIPELINE_WATCH`).
fn rig_full(on: bool, serviced: &[&'static str], addrs: Addrs, watch: &[u32]) -> Rig {
    let log: Log = Arc::default();
    let mut d = Dispatch::default();
    d.register_generated_rust(FRAME, render).unwrap();
    d.register_generated_rust(CALLBACK, callback).unwrap();
    d.register_resume(CALLBACK + 0x10, CALLBACK, CALLBACK + 0x10)
        .unwrap();
    d.register_override(D3D_CALLBACK, "D3D callback", |cpu, ctx, _| {
        let rt = Arc::clone(ctx.runtime());
        let mut m = rt.memory();
        ctx.call_guest(cpu, &mut m, CALLBACK, |cpu, ctx| ctx.memory().ret(cpu, 0))
    });
    for at in [0x10, 0x20, 0x30] {
        d.register_resume(FRAME + at, FRAME, FRAME + at).unwrap();
    }
    let main: Arc<Mutex<Option<Arc<Runtime>>>> = Arc::default();
    {
        let (log, main) = (Arc::clone(&log), Arc::clone(&main));
        d.register_override(D3D_DRAW, "D3D draw", move |cpu, ctx, _| {
            let on_render = !main
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|m| Arc::ptr_eq(m, ctx.runtime()));
            if on_render {
                // Long enough for the sim to write INPUT meanwhile.
                std::thread::sleep(Duration::from_millis(1));
            }
            let mut m = ctx.memory();
            let at = m.u32(cpu.esp + 4)?;
            let v = m.u32(at)?;
            let n = m.u32(PUSH)? + 1;
            m.set_u32(PUSH, n)?;
            m.set_u32(PUSH + 4 + 4 * (v % 32), v)?;
            drop(m);
            log.lock().unwrap().push(("draw", v, on_render));
            stdcall_ret(cpu, ctx, 4)
        });
    }
    {
        let log = Arc::clone(&log);
        d.register_override(D3D_SIM, "D3D sim call", move |cpu, ctx, _| {
            let v = ctx.memory().u32(INPUT)?;
            log.lock().unwrap().push(("sim", v, false));
            stdcall_ret(cpu, ctx, 0)
        });
    }
    d.register_override(FRAME_WAIT, "frame wait", |cpu, ctx, _| {
        let mut m = ctx.memory();
        let n = m.u32(WAITS)? + 1;
        m.set_u32(WAITS, n)?;
        m.ret(cpu, 0)
    });
    d.register_override(
        kernel::function_token(YIELD_ORDINAL),
        "NtYieldExecution",
        |cpu, ctx, _| {
            cpu.eax = 0;
            ctx.memory().ret(cpu, 0)
        },
    );
    d.register_override(
        kernel::function_token(PROTECT_ORDINAL),
        "MmSetAddressProtect",
        |cpu, ctx, _| {
            let mut m = ctx.memory();
            let (base, size) = (m.u32(cpu.esp + 4)?, m.u32(cpu.esp + 8)?);
            m.protect_range(base, size, Permissions::RW)
                .expect("a mapped page");
            m.ret(cpu, 12)
        },
    );
    d.register_override(
        kernel::function_token(KERNEL_ORDINAL),
        "KeQuerySystemTime",
        |cpu, ctx, _| {
            let mut m = ctx.memory();
            let out = m.u32(cpu.esp + 4)?;
            let n = m.u32(KERNEL_CALLS)? + 1;
            m.set_u32(KERNEL_CALLS, n)?;
            m.set_u32(out, 0x1000 + n)?;
            // The I/O worker this call wakes, as the game's runs while the
            // game thread is in the call: it takes the queued request,
            // reads its buffer through the record's pointer and sets the
            // record's completion byte.
            let q = m.u32(QUEUE)?;
            if q != 0 {
                let buf = m.u32(q)?;
                let first = m.u32(buf)?;
                let done = m.u32(q + 4)?;
                m.set_u8(done, 1)?;
                m.set_u32(WORKER_READ, first)?;
                m.set_u32(QUEUE, 0)?;
            }
            m.ret(cpu, 4)
        },
    );
    let mut g = Guest::new();
    g.map_zeroed(CODE, 0x1_0000, Permissions::RX).unwrap();
    g.map_zeroed(DATA, DATA_LEN, Permissions::RW).unwrap();
    g.map_zeroed(STACK_TOP - GAME_STACK, GAME_STACK, Permissions::RW)
        .unwrap();
    g.map_zeroed(PROTECTED, 0x1000, Permissions::READ).unwrap();
    // Request records in use before: completion bytes set.
    for slot in 0..16 {
        g.set_u8(REQUESTS + 0x10 * slot + 8, 1).unwrap();
    }
    // The world path: globals point at a record with +0x1120 set.
    g.set_u32(GLOBALS, GLOBALS_RECORD).unwrap();
    g.set_u8(GLOBALS_RECORD + 0x1120, 1).unwrap();
    g.prepare_fast_pages();
    let rt = Runtime::new(g, d);
    *main.lock().unwrap() = Some(Arc::clone(&rt));
    let feature = PipelineRender::with(on, addrs)
        .with_d3d(
            vec![D3D_DRAW, D3D_SIM, D3D_CALLBACK],
            |addr, _| [D3D_DRAW, D3D_SIM, D3D_CALLBACK].contains(&addr),
            &[],
        )
        .serviced(serviced)
        .watch(watch)
        .print(false);
    let mut r = Registry::new(Switches::defaults());
    assert!(r.add(&feature).unwrap());
    let plan = r
        .build(&Remap::new(&[(&points::RENDER_FRAME, FRAME)]))
        .unwrap();
    plan.install(&rt).unwrap();
    let ctx = ExecCtx::new(Arc::clone(&rt), 1);
    Rig {
        rt,
        ctx,
        log,
        feature,
        plan,
    }
}

impl Rig {
    /// Runs `at` to its end: the event it stopped with, and the CPU.
    fn run_to_end(&mut self, at: u32) -> (Option<Event>, CpuAbi) {
        let esp = STACK_TOP - 0x100;
        self.rt.memory().set_u32(esp, HARNESS_EXIT).unwrap();
        let mut cpu = CpuAbi {
            esp,
            ..Default::default()
        };
        let t = Runner::default().run(&mut cpu, &mut self.ctx, at);
        (self.ctx.event(t.event_id).cloned(), cpu)
    }

    fn run(&mut self, at: u32) {
        let esp = STACK_TOP - 0x100;
        self.rt.memory().set_u32(esp, HARNESS_EXIT).unwrap();
        let mut cpu = CpuAbi {
            esp,
            ..Default::default()
        };
        let t = Runner::default().run(&mut cpu, &mut self.ctx, at);
        assert!(
            matches!(self.ctx.event(t.event_id), Some(Event::HarnessExit)),
            "{:?}",
            self.ctx.event(t.event_id)
        );
        assert_eq!(cpu.esp, esp + 4, "returned as the original would");
    }

    /// Round `k`: the sim writes, maybe calls D3D, then the frame.
    fn round(&mut self, k: u32, fallback: bool) {
        {
            let mut m = self.rt.memory();
            m.set_u32(INPUT, k).unwrap();
            m.set_u8(SHARED_LINE, k as u8).unwrap();
            m.set_u32(FALLBACK, fallback as u32).unwrap();
            // The sim's own visit marks: its counter and stamps.
            let mark = m.u32(MARK).unwrap() + 1;
            m.set_u32(MARK, mark).unwrap();
            m.set_u8(STAMPS.0 + 8 + (k % 8), mark as u8).unwrap();
            m.set_u8(CLEAR, 1).unwrap();
            m.set_u8(VBLANKS.0 + 1, k as u8).unwrap();
            if m.u32(CHURN).unwrap() != 0 {
                // This tick spawns object k and frees the last one.
                let (now, last) = (k % 16, (k + 15) % 16);
                m.set_u32(BODIES + 0x40 * now + 0x3c, 100 + k).unwrap();
                m.set_u32(OBJECTS + 4 * now, BODIES + 0x40 * now).unwrap();
                m.set_u32(OBJECTS + 4 * last, 0).unwrap();
            }
        }
        if k.is_multiple_of(3) {
            self.run(D3D_SIM);
        }
        self.run(FRAME);
    }

    /// One more frame off the world path: serial, so it joins the last.
    fn finish(&mut self) {
        self.rt.memory().set_u32(GLOBALS, 0).unwrap();
        self.run(FRAME);
        assert!(!self.feature.in_flight());
    }

    fn data(&self) -> Vec<u8> {
        let mut v = vec![0; DATA_LEN as usize];
        self.rt.memory().read_bytes(DATA, &mut v, 1).unwrap();
        v
    }
}

fn rounds() -> u32 {
    std::env::var("H2_PIPELINE_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(120)
}

fn fallback_round(k: u32) -> bool {
    k % 7 == 3
}

/// Every byte of DATA but the visit marks (render-private by design: the
/// pipelined run's sim never sees the render's counter).
fn compare(serial: &[u8], piped: &[u8]) {
    let marks = |a: u32| (MARK..MARK + 4).contains(&a) || (STAMPS.0..STAMPS.1).contains(&a);
    let diffs: Vec<String> = serial
        .iter()
        .zip(piped)
        .enumerate()
        .filter(|(i, (s, p))| s != p && !marks(DATA + *i as u32))
        .map(|(i, (s, p))| format!("{:#x}: serial {s:#x} pipelined {p:#x}", DATA + i as u32))
        .take(20)
        .collect();
    assert!(diffs.is_empty(), "{diffs:#?}");
}

#[test]
fn pipelined_frames_end_in_the_serial_runs_memory_with_fallbacks_finished_serially() {
    let n = rounds();
    let mut serial = rig(false);
    let mut piped = rig(true);
    for k in 1..=n {
        serial.round(k, fallback_round(k));
        piped.round(k, fallback_round(k));
    }
    // Host-owned bytes: the render's write never came back (the last
    // frame, serial, writes it in both runs).
    assert_eq!(piped.rt.memory().u8(VBLANKS.0).unwrap(), 0);
    assert_eq!(serial.rt.memory().u8(VBLANKS.0).unwrap(), 0xee);
    serial.finish();
    piped.finish();
    compare(&serial.data(), &piped.data());

    let s = piped.feature.stats();
    let fallbacks = (1..=n).filter(|&k| fallback_round(k)).count() as u64;
    assert_eq!(s.pipelined, n as u64, "every world frame launched: {s:?}");
    assert_eq!(s.completed, n as u64 - fallbacks);
    assert_eq!(
        s.fallbacks.get("kernel:KeQuerySystemTime"),
        Some(&fallbacks),
        "{s:?}"
    );
    assert_eq!(s.serial.get("not_world"), Some(&1));
    assert_eq!(s.mispredicts, 0, "one present a frame, predicted");
    assert_eq!(serial.feature.stats().pipelined, 0, "off");

    // The kernel ran on the game thread once per fallback, as serially.
    let kernel = |r: &Rig| r.rt.memory().u32(KERNEL_CALLS).unwrap();
    assert_eq!(kernel(&piped), kernel(&serial));
    assert_eq!(kernel(&piped), fallbacks as u32);
    // One frame wait a frame, never twice in a fallback.
    assert_eq!(piped.rt.memory().u32(WAITS).unwrap(), n + 1);
    assert_eq!(piped.rt.memory().u64(PRESENTS).unwrap(), u64::from(n) + 1);
    assert_eq!(piped.rt.memory().u32(FRAMES).unwrap(), n + 1);
    // The sim's byte of the shared line and the render's both landed.
    let m = piped.rt.memory();
    assert_eq!(
        (m.u8(SHARED_LINE).unwrap(), m.u8(SHARED_LINE + 8).unwrap()),
        (n as u8, n as u8)
    );
    // The launch cleared the one-shot byte, and the render's clear never
    // came back over the sim's set of the next round: the last frame was
    // serial and cleared it.
    assert_eq!(m.u8(CLEAR).unwrap(), 0);
}

#[test]
fn d3d_reads_come_from_the_launch_snapshot_and_sim_d3d_calls_follow_the_render() {
    let n = rounds();
    let mut serial = rig(false);
    let mut piped = rig(true);
    for k in 1..=n {
        serial.round(k, false);
        piped.round(k, false);
    }
    serial.finish();
    piped.finish();
    let strip = |l: &Log| -> Vec<(&'static str, u32)> {
        l.lock().unwrap().iter().map(|&(e, v, _)| (e, v)).collect()
    };
    // The same values in the same order: each frame's draw read the INPUT of
    // its own frame although the sim had moved on, and each sim D3D call
    // came after the previous frame's draw.
    assert_eq!(strip(&piped.log), strip(&serial.log));
    let on_render = piped.log.lock().unwrap().iter().filter(|e| e.2).count();
    assert_eq!(
        on_render, n as usize,
        "every draw of a pipelined frame on the render thread"
    );
    assert!(serial.log.lock().unwrap().iter().all(|e| !e.2));
    let s = piped.feature.stats();
    assert!(
        s.d3d_joins > 0,
        "a sim D3D call met a render in flight: {s:?}"
    );
    // The D3D layer's own page came back.
    assert_eq!(
        piped.rt.memory().u32(PUSH).unwrap(),
        serial.rt.memory().u32(PUSH).unwrap()
    );
}

#[test]
fn a_snapshot_waits_for_the_next_frame_to_join() {
    let mut piped = rig(true);
    piped.round(1, false);
    assert!(piped.feature.in_flight());
    let blockers = piped.plan.snapshot_blockers();
    assert_eq!(blockers.len(), 1, "{blockers:?}");
    assert_eq!(blockers[0].0, "pipeline-render");
    // The next frame joins and runs serially, so nothing is in flight.
    piped.round(2, false);
    assert!(!piped.feature.in_flight());
    assert!(piped.plan.snapshot_blockers().is_empty());
    assert_eq!(piped.feature.stats().serial.get("quiesce"), Some(&1));
    piped.round(3, false);
    assert!(piped.feature.in_flight(), "and pipelines again after");
    piped.finish();
}

#[test]
fn a_refusal_under_a_rust_continuation_stops_with_its_name_serial_runs_it() {
    // Serially the callback's kernel call just runs.
    let mut serial = rig(false);
    serial.round(1, false);
    serial.rt.memory().set_u32(FALLBACK, 2).unwrap();
    serial.run(FRAME);
    assert_eq!(serial.rt.memory().u32(KERNEL_CALLS).unwrap(), 1);

    let mut piped = rig(true);
    piped.round(1, false);
    piped.rt.memory().set_u32(INPUT, 2).unwrap();
    piped.rt.memory().set_u32(FALLBACK, 2).unwrap();
    piped.run(FRAME);
    // The join finds the render stopped inside D3D_CALLBACK's continuation.
    let (event, _) = piped.run_to_end(FRAME);
    let Some(Event::Unsupported { detail, .. }) = event else {
        panic!("{event:?}")
    };
    assert!(
        detail.contains("unresumable:kernel:KeQuerySystemTime"),
        "{detail}"
    );
    let s = piped.feature.stats();
    assert_eq!(
        s.fallbacks.get("unresumable:kernel:KeQuerySystemTime"),
        Some(&1),
        "{s:?}"
    );
}

#[test]
fn a_render_as_deep_as_the_game_threads_stack_runs_pipelined_and_in_fallbacks() {
    // The game's first world frame probes a frame of over 128 KiB
    // (`_chkstk`); the render stack was 128 KiB and that read faulted just
    // under it. The render may use what the game thread could serially.
    let deep = GAME_STACK - 0x1000;
    let mut serial = rig(false);
    let mut piped = rig(true);
    for r in [&mut serial, &mut piped] {
        r.rt.memory().set_u32(DEEP, deep).unwrap();
    }
    for k in 1..=8 {
        serial.round(k, fallback_round(k));
        piped.round(k, fallback_round(k));
    }
    serial.finish();
    piped.finish();
    compare(&serial.data(), &piped.data());
    let s = piped.feature.stats();
    assert_eq!(s.pipelined, 8, "{s:?}");
    assert_eq!(s.completed, 7, "{s:?}");
    assert_eq!(s.fallbacks.get("kernel:KeQuerySystemTime"), Some(&1));
}

#[test]
fn switched_on_it_runs_menu_frames_serially_and_launches_nothing() {
    // Off the world path (menus, loading screens): the original, on the
    // game thread, with the same memory as the feature off.
    let mut serial = rig(false);
    let mut piped = rig(true);
    for r in [&mut serial, &mut piped] {
        let mut m = r.rt.memory();
        m.set_u32(GLOBALS, 0).unwrap();
        m.set_u32(DEEP, GAME_STACK - 0x1000).unwrap();
    }
    for k in 1..=20 {
        serial.round(k, false);
        piped.round(k, false);
        assert!(!piped.feature.in_flight());
    }
    assert_eq!(
        serial.data(),
        piped.data(),
        "no mark, counter or byte differs"
    );
    let s = piped.feature.stats();
    assert_eq!(s.pipelined, 0, "{s:?}");
    assert_eq!(s.serial.get("not_world"), Some(&20), "{s:?}");
    assert!(piped.log.lock().unwrap().iter().all(|e| !e.2));
}

/// Rounds 1..=n with objects spawned and freed each tick; `fallback_round`
/// frames make the kernel call in the middle of the render.
fn churn(r: &mut Rig, n: u32, call: u32) -> Option<Event> {
    // Mode 5 falls back on purpose, and a fallback's rest of the render
    // runs on live RAM a tick ahead: it would read the freed objects (the
    // hazard the first test shows), so its frames read none.
    r.rt.memory().set_u32(CHURN, u32::from(call != 5)).unwrap();
    for k in 1..=n {
        {
            let mut m = r.rt.memory();
            m.set_u32(INPUT, k).unwrap();
            m.set_u8(SHARED_LINE, k as u8).unwrap();
            let call = if fallback_round(k) { call } else { 0 };
            m.set_u32(FALLBACK, call).unwrap();
            m.set_u8(CLEAR, 1).unwrap();
            let (now, last) = (k % 16, (k + 15) % 16);
            m.set_u32(BODIES + 0x40 * now + 0x3c, 100 + k).unwrap();
            m.set_u32(OBJECTS + 4 * now, BODIES + 0x40 * now).unwrap();
            m.set_u32(OBJECTS + 4 * last, 0).unwrap();
        }
        let (event, _) = r.run_to_end(FRAME);
        if !matches!(event, Some(Event::HarnessExit)) {
            return event;
        }
    }
    None
}

#[test]
fn a_fallback_finishing_the_render_on_live_ram_reads_the_next_ticks_objects() {
    // The game's crash at present 2803: a render refused at the async I/O
    // pool's mutex finished on the game thread against live RAM, a tick
    // later; an object its frame had listed was freed by then, and the
    // object setup read the body's radius (+0x3c) through a null pointer.
    let mut serial = rig(false);
    assert_eq!(
        churn(&mut serial, 12, 1),
        None,
        "serially every frame is fine"
    );
    let mut piped = rig(true);
    let event = churn(&mut piped, 12, 1);
    let Some(Event::MemoryFault { fault, .. }) = event else {
        panic!("{event:?}")
    };
    assert_eq!(fault.addr, 0x3c, "{fault:?}");
    assert_eq!(
        piped
            .feature
            .stats()
            .fallbacks
            .get("kernel:KeQuerySystemTime"),
        Some(&1)
    );
}

#[test]
fn a_serviced_kernel_call_keeps_the_render_on_its_own_frame() {
    // The same frames with the call serviced: the game thread makes it, the
    // render carries on on its thread against its snapshot, and reads the
    // object its frame listed. The answer goes to a live byte range (1) or
    // to a local on the render's stack (3).
    for call in [1, 3] {
        serviced_rounds(call);
    }
}

fn serviced_rounds(call: u32) {
    let n = rounds();
    let mut serial = rig(false);
    let mut piped = rig_with(true, &["KeQuerySystemTime"]);
    assert_eq!(churn(&mut serial, n, call), None);
    assert_eq!(churn(&mut piped, n, call), None);
    serial.finish();
    piped.finish();
    compare(&serial.data(), &piped.data());
    let s = piped.feature.stats();
    let calls = (1..=n).filter(|&k| fallback_round(k)).count() as u64;
    assert!(s.fallbacks.is_empty(), "{s:?}");
    assert_eq!(s.completed, n as u64, "{s:?}");
    assert_eq!(s.serviced.get("kernel:KeQuerySystemTime"), Some(&calls));
    // The kernel ran on the game thread, once per call, as serially, and
    // its answer (a live byte range) reached the render.
    let kernel = |r: &Rig| r.rt.memory().u32(KERNEL_CALLS).unwrap();
    assert_eq!(kernel(&piped), kernel(&serial));
    assert_eq!(kernel(&piped), calls as u32);
    let out = |r: &Rig| r.rt.memory().u32(OUT + 0x800).unwrap();
    assert_eq!(out(&piped), out(&serial));
    assert_eq!(piped.rt.memory().u32(WAITS).unwrap(), n + 1);
}

#[test]
fn the_worker_a_serviced_call_wakes_reads_the_request_the_render_wrote() {
    // The game's crash at present 1744: the render queued I/O requests and
    // the serviced semaphore release woke the worker, which took a request
    // whose fields the render had written only in its view (the queue head
    // `0x004e28ec` was past the synced pool range, and a request's fields
    // point at the requester's memory). Here the queue head is synced but
    // the record is not in any live range: the serviced call flushes what
    // the render wrote, and the worker's completion byte is not undone by
    // the final merge (the record was in use before: its byte was 1).
    serviced_rounds(4);
    let mut serial = rig(false);
    let mut piped = rig_with(true, &["KeQuerySystemTime"]);
    churn(&mut serial, 20, 4);
    churn(&mut piped, 20, 4);
    serial.finish();
    piped.finish();
    let m = piped.rt.memory();
    for k in (1..=20).filter(|&k| fallback_round(k)) {
        assert_eq!(m.u8(REQUESTS + 0x10 * (k % 16) + 8).unwrap(), 1, "{k}");
    }
    assert_eq!(
        m.u32(WORKER_READ).unwrap(),
        serial.rt.memory().u32(WORKER_READ).unwrap()
    );
}

#[test]
fn a_serviced_call_under_a_rust_continuation_carries_on_too() {
    // The render thread keeps its own context, continuation and all.
    let mut serial = rig(false);
    serial.round(1, false);
    serial.rt.memory().set_u32(FALLBACK, 2).unwrap();
    serial.run(FRAME);
    let mut piped = rig_with(true, &["KeQuerySystemTime"]);
    piped.round(1, false);
    piped.rt.memory().set_u32(INPUT, 2).unwrap();
    piped.rt.memory().set_u32(FALLBACK, 2).unwrap();
    piped.run(FRAME);
    // The serial last frame without the callback's call.
    piped.rt.memory().set_u32(FALLBACK, 0).unwrap();
    piped.finish();
    let s = piped.feature.stats();
    assert!(s.fallbacks.is_empty(), "{s:?}");
    assert_eq!(s.serviced.get("kernel:KeQuerySystemTime"), Some(&1));
    assert_eq!(piped.rt.memory().u32(KERNEL_CALLS).unwrap(), 1);
}

#[test]
fn a_fallback_after_a_flush_hands_back_the_marks_the_flush_left_out() {
    // A serviced call flushes the render's writes but not its visit marks;
    // a later refused call falls back, and the rest of the render runs on
    // live RAM: it must see the stamps its frame made, though their page
    // was not stored to again after the flush.
    let n = 20;
    let mut serial = rig(false);
    let mut piped = rig_with(true, &["KeQuerySystemTime"]);
    assert_eq!(churn(&mut serial, n, 5), None);
    assert_eq!(churn(&mut piped, n, 5), None);
    serial.finish();
    piped.finish();
    compare(&serial.data(), &piped.data());
    let s = piped.feature.stats();
    let calls = (1..=n).filter(|&k| fallback_round(k)).count() as u64;
    assert_eq!(s.fallbacks.get("kernel:NtYieldExecution"), Some(&calls));
    let m = piped.rt.memory();
    for k in (1..=n).filter(|&k| fallback_round(k)) {
        assert_eq!(m.u32(OUT + 0xb00 + 4 * (k % 16)).unwrap(), 1, "{k}");
    }
}

#[test]
fn a_serviced_protection_change_reaches_the_renders_view() {
    // MmSetAddressProtect on the render thread: the game thread makes it,
    // and the view's page gets the same protection, so the render's write
    // to the page it just made writable lands as it would serially.
    let n = 12;
    let mut serial = rig(false);
    let mut piped = rig_with(true, &["MmSetAddressProtect"]);
    assert_eq!(churn(&mut serial, n, 6), None);
    assert_eq!(churn(&mut piped, n, 6), None);
    serial.finish();
    piped.finish();
    compare(&serial.data(), &piped.data());
    let s = piped.feature.stats();
    assert!(s.fallbacks.is_empty(), "{s:?}");
    assert!(
        s.serviced.contains_key("kernel:MmSetAddressProtect"),
        "{s:?}"
    );
    let (sm, pm) = (serial.rt.memory(), piped.rt.memory());
    for i in 0..16 {
        let at = PROTECTED + 4 * i;
        assert_eq!(pm.u32(at).unwrap(), sm.u32(at).unwrap(), "{at:#x}");
    }
}

#[test]
fn the_guards_run_warm_up_busy_io_and_after_fallback_frames_serially() {
    let addrs = Addrs {
        warmup: 3,
        after_fallback: 2,
        ..ADDRS.clone()
    };
    let mut piped = rig_cfg(true, &[], addrs);
    // Three world frames of warm-up, then pipelined.
    for k in 1..=4 {
        piped.round(k, false);
    }
    let s = piped.feature.stats();
    assert_eq!(s.serial.get("warmup"), Some(&3), "{s:?}");
    assert_eq!(s.pipelined, 1, "{s:?}");
    // Requests queued: serial while the queue has any.
    piped.rt.memory().set_u32(QUEUE, REQUESTS).unwrap();
    piped.round(5, false);
    piped.round(6, false);
    piped.rt.memory().set_u32(QUEUE, 0).unwrap();
    assert_eq!(piped.feature.stats().serial.get("io_busy"), Some(&2));
    // A fallback (the kernel call at round 7 is not serviced here): the
    // next two frames are serial.
    piped.round(7, true);
    for k in 8..=11 {
        piped.round(k, false);
    }
    let s = piped.feature.stats();
    assert_eq!(
        s.fallbacks.get("kernel:KeQuerySystemTime"),
        Some(&1),
        "{s:?}"
    );
    assert_eq!(s.serial.get("after_fallback"), Some(&2), "{s:?}");
    // A menu frame starts the warm-up again.
    piped.rt.memory().set_u32(GLOBALS, 0).unwrap();
    piped.round(12, false);
    piped.rt.memory().set_u32(GLOBALS, GLOBALS_RECORD).unwrap();
    piped.round(13, false);
    let s = piped.feature.stats();
    assert_eq!(s.serial.get("warmup"), Some(&4), "{s:?}");
    piped.finish();
}

/// Switched on with every frame serial (a warm-up that never ends): guest
/// memory after every frame is the switched-off run's, byte for byte (the
/// data, visit marks included, the game thread's stack, the protected
/// page), and the feature made no call of its own. A serial frame is the
/// generated original on the game thread and nothing more.
#[test]
fn switched_on_with_every_frame_serial_memory_is_the_switched_off_runs() {
    let n = rounds();
    let all_serial = Addrs {
        warmup: u32::MAX,
        ..ADDRS.clone()
    };
    let calls = ["KeQuerySystemTime", "MmSetAddressProtect"];
    let mut off = rig_cfg(false, &calls, ADDRS.clone());
    let mut on = rig_cfg(true, &calls, all_serial);
    let bytes = |r: &Rig, at: u32, len: u32| {
        let mut v = vec![0; len as usize];
        r.rt.memory().read_bytes(at, &mut v, 1).unwrap();
        v
    };
    for k in 1..=n {
        // Every kind of frame the pipelined tests use, each run serially.
        let mode = [0, 1, 2, 3, 4, 6][(k % 6) as usize];
        for r in [&mut off, &mut on] {
            r.rt.memory().set_u32(CHURN, 1).unwrap();
            r.round(k, false);
            r.rt.memory().set_u32(FALLBACK, mode).unwrap();
            r.run(FRAME);
        }
        assert_eq!(off.data(), on.data(), "round {k} (mode {mode})");
        assert_eq!(
            bytes(&off, STACK_TOP - GAME_STACK, GAME_STACK),
            bytes(&on, STACK_TOP - GAME_STACK, GAME_STACK),
            "round {k}: the game thread's stack"
        );
        assert_eq!(
            bytes(&off, PROTECTED, 0x1000),
            bytes(&on, PROTECTED, 0x1000)
        );
    }
    let s = on.feature.stats();
    assert_eq!(s.pipelined, 0, "{s:?}");
    assert_eq!(s.serial.get("warmup"), Some(&(2 * u64::from(n))), "{s:?}");
    assert!(s.fallbacks.is_empty() && s.serviced.is_empty(), "{s:?}");
    assert!(!on.feature.in_flight());
}

/// `H2_PIPELINE_WATCH`: a watched word the sim changes every round is
/// reported once a round; an unwatched run reports nothing, and watching
/// changes nothing.
#[test]
fn a_watched_word_reports_each_change_once() {
    let n = rounds();
    let mut watched = rig_full(true, &[], ADDRS.clone(), &[INPUT]);
    let mut plain = rig(true);
    for k in 1..=n {
        watched.round(k, false);
        plain.round(k, false);
    }
    watched.finish();
    plain.finish();
    assert_eq!(watched.feature.stats().pipelined, u64::from(n));
    // Seen first at round 1's frame; each later round changed it once.
    assert_eq!(watched.feature.watch_lines(), n - 1);
    assert_eq!(plain.feature.watch_lines(), 0);
    compare(&plain.data(), &watched.data());
}
