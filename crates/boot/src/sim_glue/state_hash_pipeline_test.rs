//! A frame with two views and a two-slot animation cache. One render
//! consumes the same resource twice, evicts it, and needs it again. The
//! following tick evicts both slots while the worker still has its pose.
use super::{Snapshot, gameplay};
use engine::points;
use feature::pipeline_render::{Addrs, DrawBoundary, PipelineRender, RANDOM_A, RANDOM_C};
use feature::{Registry, Remap, Switches};
use runtime::exec::HARNESS_EXIT;
use runtime::guest::Permissions;
use runtime::{CpuAbi, Dispatch, Event, ExecCtx, Guest, GuestFault, Runner, Runtime};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

const FRAME: u32 = 0x10000;
const DRAW: u32 = FRAME + 0x100;
const WAIT: u32 = FRAME + 0x200;
const DEVICE_SYNC: u32 = FRAME + 0x300;
const AFTER_DRAW: u32 = FRAME + 4;
const AFTER_WAIT: u32 = FRAME + 8;
const DATA: u32 = 0x20000;
const CONTEXT: u32 = DATA + 0x1000;
const FLAGS: u32 = DATA + 0x100;
const LOCATION: u32 = FLAGS + 4;
const RNG: u32 = FLAGS + 8;
const EFFECT_RNG: u32 = FLAGS + 12;
const EFFECT_LOCATION: u32 = FLAGS + 16;
const CACHE: u32 = DATA + 0x200;
const CLOCK: u32 = DATA + 0x300;
const IO: u32 = CLOCK + 4;
const IO_COUNT: u32 = CLOCK + 8;
const POSE: u32 = CLOCK + 12;
const IMAGE: u32 = CLOCK + 16;
const STACK: u32 = 0x40000;

fn sample(m: &mut Guest, seed: u32) -> Result<u32, GuestFault> {
    let x = m.u32(seed)?.wrapping_mul(RANDOM_A).wrapping_add(RANDOM_C);
    m.set_u32(seed, x)?;
    Ok(x)
}

fn animation(m: &mut Guest, id: u32) -> Result<u32, GuestFault> {
    let slots = [CACHE, CACHE + 12];
    let used = m.u32(CLOCK)? + 1;
    m.set_u32(CLOCK, used)?;
    let slot = if m.u32(slots[0])? == id {
        slots[0]
    } else if m.u32(slots[1])? == id {
        slots[1]
    } else {
        let slot = if m.u32(slots[0] + 4)? <= m.u32(slots[1] + 4)? {
            slots[0]
        } else {
            slots[1]
        };
        let old = m.u32(slot)?;
        // Record the ordered (victim, request) pairs, including re-loads.
        let io = m.u32(IO)?.wrapping_mul(97).wrapping_add(old * 8 + id);
        let count = m.u32(IO_COUNT)? + 1;
        m.set_u32(IO, io)?;
        m.set_u32(IO_COUNT, count)?;
        m.set_u32(slot, id)?;
        m.set_u32(slot + 8, id * 100 + count)?;
        slot
    };
    m.set_u32(slot + 4, used)?;
    Ok(slot + 8)
}

fn run(on: bool) -> (Vec<u32>, u64, u32) {
    let mut g = Guest::new();
    g.map_zeroed(FRAME, 0x1000, Permissions::RX).unwrap();
    g.map_zeroed(DATA, 0x3000, Permissions::RW).unwrap();
    g.map_zeroed(STACK, 0x1000, Permissions::RW).unwrap();
    g.set_u32(DATA, CONTEXT).unwrap();
    g.set_u8(CONTEXT + 0x1120, 1).unwrap();
    g.set_u32(FLAGS, 0x24).unwrap();
    g.set_u32(RNG, 123).unwrap();
    g.set_u32(EFFECT_RNG, 456).unwrap();
    g.prepare_fast_pages();
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    let mut d = Dispatch::default();
    d.register_generated_rust(FRAME, |cpu, ctx, pc| {
        let mut m = ctx.memory();
        match pc {
            FRAME => {
                // Repeated views and hits must each retain their place.
                for (view, ids) in [(1, [1, 2, 1]), (2, [3, 2, 1])] {
                    let particle = sample(&mut m, EFFECT_RNG)?;
                    let flags = (m.u32(FLAGS)? & !4) | (particle & 4);
                    m.set_u32(FLAGS, flags)?;
                    m.set_u32(LOCATION, view)?;
                    // Stamp the effect location with the variant picked
                    // during this visit, so swapping the RNG draws is visible.
                    m.set_u32(EFFECT_LOCATION, (particle & !3) | view)?;
                    for id in ids {
                        let payload = animation(&mut m, id)?;
                        m.set_u32(POSE, payload)?;
                    }
                    sample(&mut m, RNG)?;
                }
                m.call(cpu, DRAW, AFTER_DRAW)
            }
            AFTER_DRAW => m.call(cpu, WAIT, AFTER_WAIT),
            AFTER_WAIT => {
                let present = m.u64(DATA + 32)?;
                m.set_u64(DATA + 32, present + 1)?;
                m.ret(cpu, 0)
            }
            _ => unreachable!(),
        }
    })
    .unwrap();
    for pc in [AFTER_DRAW, AFTER_WAIT] {
        d.register_resume(pc, FRAME, pc).unwrap();
    }
    d.register_override(DEVICE_SYNC, "sync device", |cpu, ctx, _| {
        ctx.memory().ret(cpu, 0)
    });
    d.register_override(WAIT, "frame wait", |cpu, ctx, _| ctx.memory().ret(cpu, 0));
    d.register_override(DRAW, "draw pose", move |cpu, ctx, _| {
        if ctx.thread_id == runtime::executor::EXECUTOR_THREAD_ID {
            started_tx.send(()).unwrap();
            release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10))
                .unwrap();
        }
        let mut m = ctx.memory();
        let pixel = m.u32(m.u32(POSE)?)?;
        m.set_u32(IMAGE, pixel)?;
        m.ret(cpu, 0)
    });
    let rt = Runtime::new(g, d);
    let addrs = Addrs {
        frame_wait: WAIT,
        engine_globals: DATA,
        ui_state: (DATA + 4, DATA + 5, DATA + 8),
        cache_countdowns: (DATA + 12, DATA + 16),
        present_counter: DATA + 32,
        frame_counter: DATA + 40,
        launch_clear: DATA + 44,
        mark_counters: &[],
        mark_bytes: &[],
        host_owned: &[],
        live_bytes: &[],
        render_owned: &[],
        io_queue: 0,
        io_free: None,
        io_handles: &[],
        warmup: 0,
        after_fallback: 0,
        random: None,
        shared_caches: &[],
        draw_boundary: Some(DrawBoundary {
            entry: DRAW,
            return_pc: AFTER_DRAW,
            depth: 8,
            view_context: None,
            debug_flag_from_esp: None,
        }),
    };
    let pipeline = PipelineRender::with(on, addrs)
        .with_d3d(vec![DRAW, DEVICE_SYNC], |at, _| at == DRAW, &[])
        .print(false);
    let mut registry = Registry::new(Switches::default());
    registry.add(&pipeline).unwrap();
    let plan = registry
        .build(&Remap::new(&[(&points::RENDER_FRAME, FRAME)]))
        .unwrap();
    plan.install(&rt).unwrap();
    let mut ctx = ExecCtx::new(Arc::clone(&rt), 1);
    let mut call = |entry| {
        let mut cpu = CpuAbi {
            esp: STACK + 0x800,
            ..Default::default()
        };
        rt.memory().set_u32(cpu.esp, HARNESS_EXIT).unwrap();
        let out = Runner::default().run(&mut cpu, &mut ctx, entry);
        assert!(matches!(ctx.event(out.event_id), Some(Event::HarnessExit)));
    };
    call(FRAME);
    if on {
        started_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    }
    let (bytes, hash) = {
        let mut m = rt.memory();
        let particle =
            m.u32(FLAGS).unwrap() ^ m.u32(LOCATION).unwrap() ^ m.u32(EFFECT_LOCATION).unwrap();
        let shared = sample(&mut m, RNG).unwrap();
        let effect = sample(&mut m, EFFECT_RNG).unwrap();
        let pose = m.u32(m.u32(POSE).unwrap()).unwrap();
        for id in [4, 5] {
            animation(&mut m, id).unwrap();
        }
        let bytes = [
            FLAGS,
            LOCATION,
            RNG,
            EFFECT_RNG,
            CLOCK,
            IO,
            IO_COUNT,
            EFFECT_LOCATION,
        ]
        .into_iter()
        .chain((0..6).map(|n| CACHE + n * 4))
        .map(|at| m.u32(at).unwrap())
        .collect();
        let observation = Snapshot {
            tick: 1,
            bsp: Some(particle ^ effect ^ pose ^ m.u32(IO).unwrap()),
            ..Default::default()
        };
        (bytes, gameplay(&observation, Some(shared)))
    };
    if on {
        assert!(pipeline.in_flight());
        release_tx.send(()).unwrap();
    }
    call(DEVICE_SYNC);
    assert_eq!(pipeline.stats().completed, u64::from(on));
    let image = rt.memory().u32(IMAGE).unwrap();
    (bytes, hash, image)
}

#[test]
fn ordered_frame_owners_match_the_serial_tick_hash_while_drawing() {
    let serial = run(false);
    let pipelined = run(true);
    assert_eq!(serial, pipelined);
    assert_eq!(serial.0[6], 7, "five render loads and two tick loads");
    assert_eq!(serial.2, 105, "the draw uses the fifth render load");
}
