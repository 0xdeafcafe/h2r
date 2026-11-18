//! The `render-loop` driver on the runtime: `impl.render.views` places the
//! Rust `render_views`, and every callee is a Rust override that logs the
//! call and returns as the original callee would (`ret n`). The calls the
//! runtime makes must be the ones the port asks for when driven directly,
//! which `engine/tests/render_loop_differential.rs` proves equal to the
//! original's. No game code: the callees are stubs at their addresses.

use std::sync::{Arc, Mutex};

use engine::points;
use engine::render_loop::{Machine, Regs, Step, views};
use feature::{Registry, Switches};
use runtime::exec::HARNESS_EXIT;
use runtime::guest::Permissions;
use runtime::{CpuAbi, Dispatch, Event, ExecCtx, Guest, Runner, Runtime};

const VIEWS: u32 = 0x0060_0000;
const STACK: u32 = 0x0070_0f00;

type Log = Arc<Mutex<Vec<(u32, [u32; 7])>>>;

/// The stub's answer for call `n` to `target`: EAX, ECX, EDX.
fn answer(n: usize, target: u32) -> [u32; 3] {
    let k = (n as u32).wrapping_mul(0x9e37_79b9) ^ target;
    use engine::render_loop::scene::scene_passes as sp;
    let al = if target == views::VIEW_TEST {
        n as u32 & 1
    } else if target == sp::SKIP_A || target == sp::SKIP_B {
        // render_scene_passes' early-outs: never taken.
        0
    } else {
        k & 0xff
    };
    [(k & !0xff) | al, k.rotate_left(7), k.rotate_left(13)]
}

fn guest() -> Guest {
    let mut g = Guest::new();
    // Code pages for the entry and the callees (overrides; no code in them).
    g.map_zeroed(0x0001_0000, 0x0014_0000, Permissions::RX)
        .unwrap();
    g.map_zeroed(0x0045_0000, 0x0011_0000, Permissions::RW)
        .unwrap();
    g.map_zeroed(VIEWS, 0x1000, Permissions::RW).unwrap();
    g.map_zeroed(0x0070_0000, 0x1000, Permissions::RW).unwrap();
    // Views 0, 1 and 3 go through the test (the stub answers call n with
    // AL = n & 1); view 2 is an alternate view.
    for (i, first) in [0u32, 0, 1, 0].iter().enumerate() {
        let rec = VIEWS + i as u32 * views::VIEW_STRIDE;
        g.set_u32(rec, *first).unwrap();
        g.set_u32(rec + 4, 0x100 + i as u32).unwrap();
    }
    g.set_u32(0x0047_989c, 1).unwrap(); // the debug hook
    g.set_u8(0x0050_93fc, 1).unwrap(); // a passing view test is skipped
    g.set_u32(STACK, HARNESS_EXIT).unwrap();
    g.set_u32(STACK + 4, 0xaaaa).unwrap();
    g.set_u32(STACK + 8, 0xbbbb).unwrap();
    g.set_u32(STACK + 12, VIEWS).unwrap();
    g
}

fn entry() -> Regs {
    Regs {
        eax: 4,
        ecx: 0x1234,
        edx: 7,
        ebx: 0xb0b0,
        esp: STACK,
        ebp: 0xe0e0,
        esi: 0x5151,
        edi: 0xd1d1,
        xmm0: 0,
        xmm1: 0,
    }
}

fn snapshot(r: &Regs) -> [u32; 7] {
    [r.eax, r.ecx, r.edx, r.ebx, r.ebp, r.esi, r.edi]
}

/// The port driven directly, with the same stubs.
fn direct() -> (Vec<(u32, [u32; 7])>, Regs) {
    let mut g = guest();
    let mut r = entry();
    let mut log = Vec::new();
    let (mut m, mut step) = Machine::start(&mut r, &mut g).unwrap();
    loop {
        match step {
            Step::Return { to } => {
                assert_eq!(to, HARNESS_EXIT);
                return (log, r);
            }
            Step::Call {
                target, cleanup, ..
            } => {
                log.push((target, snapshot(&r)));
                [r.eax, r.ecx, r.edx] = answer(log.len() - 1, target);
                r.esp += cleanup;
                step = m.resume(&mut r, &mut g).unwrap();
            }
        }
    }
}

#[test]
fn the_runtime_makes_the_calls_the_port_asks_for() {
    let log: Log = Arc::default();
    let mut d = Dispatch::default();
    for target in views::CALLEES {
        let log = Arc::clone(&log);
        d.register_override(target, format!("stub {target:#x}"), move |cpu, ctx, _| {
            let mut l = log.lock().unwrap();
            l.push((
                target,
                [
                    cpu.eax, cpu.ecx, cpu.edx, cpu.ebx, cpu.ebp, cpu.esi, cpu.edi,
                ],
            ));
            [cpu.eax, cpu.ecx, cpu.edx] = answer(l.len() - 1, target);
            ctx.memory().ret(cpu, views::cleanup(target))
        });
    }
    let rt = Runtime::new(guest(), d);
    let mut r = Registry::new(Switches::parse("impl.render.views=on").unwrap());
    assert!(r.add(&feature::render_loop::RenderLoop).unwrap());
    let plan = r.build(&feature::Base).unwrap();
    assert!(
        plan.text()
            .contains("RENDER_VIEWS: implement render-loop:render_views"),
        "{}",
        plan.text()
    );
    plan.install(&rt).unwrap();

    let e = entry();
    let mut cpu = CpuAbi {
        eax: e.eax,
        ecx: e.ecx,
        edx: e.edx,
        ebx: e.ebx,
        esp: e.esp,
        ebp: e.ebp,
        esi: e.esi,
        edi: e.edi,
        ..Default::default()
    };
    let mut ctx = ExecCtx::new(Arc::clone(&rt), 1);
    let t = Runner::default().run(&mut cpu, &mut ctx, points::RENDER_VIEWS.base);
    assert!(
        matches!(ctx.event(t.event_id), Some(Event::HarnessExit)),
        "{:?}",
        ctx.event(t.event_id)
    );
    let (want, after) = direct();
    let got = log.lock().unwrap().clone();
    let targets: Vec<u32> = got.iter().map(|c| c.0).collect();
    assert_eq!(
        targets,
        [
            views::DEBUG_BEGIN,
            views::FRAME_BEGIN,
            views::VIEW_TEST,
            views::VIEW_PREPARE,
            views::GAME_VIEW,
            views::VIEW_TEST,
            views::ALT_VIEW,
            views::VIEW_TEST,
            views::END_A,
            views::END_B,
            views::DEBUG_END,
        ],
        "view 0 fails the test and is drawn, views 1 and 3 pass it and the skip flag hides them, view 2 is alternate"
    );
    assert_eq!(got, want, "the runtime's calls are the port's");
    assert_eq!(
        [
            cpu.eax, cpu.ecx, cpu.edx, cpu.ebx, cpu.esp, cpu.ebp, cpu.esi, cpu.edi
        ],
        [
            after.eax, after.ecx, after.edx, after.ebx, after.esp, after.ebp, after.esi, after.edi
        ]
    );
    assert_eq!(cpu.esp, STACK + 4 + 12, "ret 0xc");
    assert_eq!(
        [cpu.ebx, cpu.ebp, cpu.esi, cpu.edi],
        [0xb0b0, 0xe0e0, 0x5151, 0xd1d1]
    );
}

#[test]
fn render_loop_is_off_unless_switched() {
    let mut r = Registry::new(Switches::defaults());
    r.add(&feature::render_loop::RenderLoop).unwrap();
    let text = r.build(&feature::Base).unwrap().text();
    assert_eq!(text, "feature render-loop: on\n");
}

/// `impl.render.layer_entries` on the runtime: the build and two entry
/// handlers are Rust overrides that log and `ret n`; the runtime's calls
/// are the ones the port asks for when driven directly, and every entry
/// whose target is not skipped reaches its handler with its arguments.
#[test]
fn the_runtime_runs_the_rust_layer_entry_loop() {
    use engine::render_loop::layer::{self, Entries};
    const LIST: u32 = 0x0060_0000;
    const TABLE: u32 = 0x0060_0100;
    const TARGET: u32 = 0x0060_0400;
    const H0: u32 = 0x0001_2000;
    const H1: u32 = 0x0001_2100;
    const ESP: u32 = 0x0071_ff00;
    // Entry kinds 0, 1, 1, 0; the third one's target is skipped.
    let kinds = [0u16, 1, 1, 0];
    let guest = || {
        let mut g = Guest::new();
        g.map_zeroed(0x0001_0000, 0x0004_0000, Permissions::RX)
            .unwrap();
        g.map_zeroed(LIST, 0x1000, Permissions::RW).unwrap();
        g.map_zeroed(0x0070_0000, 0x0002_0000, Permissions::RW)
            .unwrap();
        g.set_u32(LIST + 0x14, TABLE).unwrap();
        for (k, h) in [H0, H1].into_iter().enumerate() {
            let rec = TABLE + k as u32 * layer::RECORD_SIZE;
            g.set_u32(rec + 4, h).unwrap();
            g.set_u32(rec + 8, 0x800 + k as u32).unwrap();
            g.set_u32(rec + 0xc, 0x900 + k as u32).unwrap();
            g.set_u8(rec + 0x14, 0x14).unwrap();
            g.set_u8(rec + 0x15, 0x15).unwrap();
        }
        g.set_u32(TARGET + 0x100, 0).unwrap();
        g.set_u32(TARGET + 0x200 + 0x100, u32::MAX).unwrap();
        g.set_u32(ESP, HARNESS_EXIT).unwrap();
        g.set_u32(ESP + 4, 0xa59).unwrap();
        g
    };
    // The build's effect: four entries into the buffer, count in EAX.
    let build = move |g: &mut dyn engine::memory::GuestMemory, buffer: u32| -> u32 {
        for (i, k) in kinds.iter().enumerate() {
            let e = buffer + i as u32 * layer::ENTRY_SIZE;
            g.write(e, &k.to_le_bytes()).unwrap();
            let t = if i == 2 { TARGET + 0x200 } else { TARGET };
            g.set_u32(e + 0x10, t).unwrap();
        }
        kinds.len() as u32
    };
    let entry = Regs {
        eax: 1,
        ecx: 2,
        edx: 3,
        ebx: 12,
        esp: ESP,
        ebp: 0xe0e0,
        esi: 0x5151,
        edi: LIST,
        xmm0: 0,
        xmm1: 0,
    };

    // Directly.
    let mut g = guest();
    let mut r = entry;
    let mut want = Vec::new();
    let (mut m, mut step) = Entries::start(&mut r, &mut g).unwrap();
    while let Step::Call {
        target, cleanup, ..
    } = step
    {
        want.push((target, snapshot(&r)));
        if target == layer::LAYER_BUILD {
            // No return address is pushed here: the buffer is the 4th dword.
            let buffer = engine::memory::GuestMemory::u32(&g, r.esp + 12).unwrap();
            r.eax = build(&mut g, buffer);
        }
        r.esp += cleanup;
        step = m.resume(&mut r, &mut g).unwrap();
    }
    assert_eq!(r.esp, ESP + 8, "ret 4");

    // On the runtime.
    let log: Log = Arc::default();
    let mut d = Dispatch::default();
    for target in [layer::LAYER_BUILD, H0, H1] {
        let log = Arc::clone(&log);
        d.register_override(target, format!("stub {target:#x}"), move |cpu, ctx, _| {
            log.lock().unwrap().push((
                target,
                [
                    cpu.eax, cpu.ecx, cpu.edx, cpu.ebx, cpu.ebp, cpu.esi, cpu.edi,
                ],
            ));
            let mut mem = ctx.memory();
            let cleanup = if target == layer::LAYER_BUILD {
                let buffer = mem.u32(cpu.esp + 16).unwrap();
                cpu.eax = build(&mut *mem, buffer);
                16
            } else {
                layer::HANDLER_CLEANUP
            };
            mem.ret(cpu, cleanup)
        });
    }
    let rt = Runtime::new(guest(), d);
    let mut reg = Registry::new(Switches::parse("impl.render.layer_entries=on").unwrap());
    reg.add(&feature::render_loop::RenderLoop).unwrap();
    reg.build(&feature::Base).unwrap().install(&rt).unwrap();
    let mut cpu = CpuAbi {
        eax: entry.eax,
        ecx: entry.ecx,
        edx: entry.edx,
        ebx: entry.ebx,
        esp: entry.esp,
        ebp: entry.ebp,
        esi: entry.esi,
        edi: entry.edi,
        ..Default::default()
    };
    let mut ctx = ExecCtx::new(Arc::clone(&rt), 1);
    let t = Runner::default().run(&mut cpu, &mut ctx, points::LAYER_ENTRIES.base);
    assert!(
        matches!(ctx.event(t.event_id), Some(Event::HarnessExit)),
        "{:?}",
        ctx.event(t.event_id)
    );
    let got = log.lock().unwrap().clone();
    let targets: Vec<u32> = got.iter().map(|c| c.0).collect();
    assert_eq!(
        targets,
        [layer::LAYER_BUILD, H0, H1, H0],
        "entry 2 is skipped"
    );
    assert_eq!(got, want, "the runtime's calls are the port's");
    assert_eq!(cpu.esp, ESP + 8);
    assert_eq!([cpu.ebp, cpu.esi, cpu.edi], [0xe0e0, 0x5151, LIST]);
}

type Fault = engine::memory::MemFault;
type Calls = Vec<(u32, [u32; 7])>;
type StartFn<P> = fn(&mut Regs, &mut dyn engine::memory::GuestMemory) -> Result<(P, Step), Fault>;
type ResumeFn<P> =
    fn(&mut P, &mut Regs, &mut dyn engine::memory::GuestMemory) -> Result<Step, Fault>;

/// A port, its point and its callees, for [`both`].
struct Under<P> {
    switch: &'static str,
    point: &'static engine::points::Point,
    callees: &'static [u32],
    cleanup: fn(u32) -> u32,
    /// Callees that return with ECX as they found it (the port relies on it).
    keep_ecx: &'static [u32],
    start: StartFn<P>,
    resume: ResumeFn<P>,
}

/// The port driven directly over `guest()`, every callee answered by
/// `answer` (EAX, ECX, EDX) and popping its arguments; then the same port
/// placed by the feature on the runtime, every callee a Rust override with
/// the same answers. Both call logs and both register sets at return.
fn both<P>(
    u: &Under<P>,
    guest: impl Fn() -> Guest,
    entry: Regs,
) -> (Calls, Calls, [u32; 8], [u32; 8]) {
    let mut g = guest();
    let mut r = entry;
    let mut want = Vec::new();
    let (mut m, mut step) = (u.start)(&mut r, &mut g).unwrap();
    loop {
        match step {
            Step::Return { to } => {
                assert_eq!(to, HARNESS_EXIT);
                break;
            }
            Step::Call {
                target, cleanup, ..
            } => {
                want.push((target, snapshot(&r)));
                let ecx = r.ecx;
                [r.eax, r.ecx, r.edx] = answer(want.len() - 1, target);
                if u.keep_ecx.contains(&target) {
                    r.ecx = ecx;
                }
                r.esp += cleanup;
                step = (u.resume)(&mut m, &mut r, &mut g).unwrap();
            }
        }
    }
    let direct = [r.eax, r.ecx, r.edx, r.ebx, r.esp, r.ebp, r.esi, r.edi];

    let log: Log = Arc::default();
    let mut d = Dispatch::default();
    for &target in u.callees {
        let log = Arc::clone(&log);
        let cleanup = u.cleanup;
        let keep_ecx = u.keep_ecx.contains(&target);
        d.register_override(target, format!("stub {target:#x}"), move |cpu, ctx, _| {
            let mut l = log.lock().unwrap();
            l.push((
                target,
                [
                    cpu.eax, cpu.ecx, cpu.edx, cpu.ebx, cpu.ebp, cpu.esi, cpu.edi,
                ],
            ));
            let ecx = cpu.ecx;
            [cpu.eax, cpu.ecx, cpu.edx] = answer(l.len() - 1, target);
            if keep_ecx {
                cpu.ecx = ecx;
            }
            ctx.memory().ret(cpu, cleanup(target))
        });
    }
    let rt = Runtime::new(guest(), d);
    let mut reg = Registry::new(Switches::parse(u.switch).unwrap());
    reg.add(&feature::render_loop::RenderLoop).unwrap();
    reg.build(&feature::Base).unwrap().install(&rt).unwrap();
    let mut cpu = CpuAbi {
        eax: entry.eax,
        ecx: entry.ecx,
        edx: entry.edx,
        ebx: entry.ebx,
        esp: entry.esp,
        ebp: entry.ebp,
        esi: entry.esi,
        edi: entry.edi,
        ..Default::default()
    };
    let mut ctx = ExecCtx::new(Arc::clone(&rt), 1);
    let t = Runner::default().run(&mut cpu, &mut ctx, u.point.base);
    assert!(
        matches!(ctx.event(t.event_id), Some(Event::HarnessExit)),
        "{:?}",
        ctx.event(t.event_id)
    );
    let got = log.lock().unwrap().clone();
    let runtime = [
        cpu.eax, cpu.ecx, cpu.edx, cpu.ebx, cpu.esp, cpu.ebp, cpu.esi, cpu.edi,
    ];
    (want, got, direct, runtime)
}

/// Globals and stack for the scene ports: the data pages mapped, no object
/// chain (`0x004b9ed8` = -1), every layer bit on.
fn scene_guest() -> Guest {
    let mut g = Guest::new();
    g.map_zeroed(0x0001_0000, 0x0016_0000, Permissions::RX)
        .unwrap();
    g.map_zeroed(0x0045_0000, 0x0011_0000, Permissions::RW)
        .unwrap();
    g.map_zeroed(0x0060_0000, 0x1000, Permissions::RW).unwrap();
    g.map_zeroed(0x0070_0000, 0x1000, Permissions::RW).unwrap();
    g.set_u32(0x004c_1a50, u32::MAX).unwrap();
    g.set_u32(0x004b_9ed8, u32::MAX).unwrap();
    g.set_u32(0x0048_5898, 5).unwrap();
    g.set_u8(0x004b_a022, 1).unwrap();
    g.set_u32(STACK, HARNESS_EXIT).unwrap();
    // View kind 1 (the base layers with targets), transparents on, two
    // flags, the view, mode 0.
    for (i, v) in [1u32, 1, 1, 1, 0x0060_0000, 0].into_iter().enumerate() {
        g.set_u32(STACK + 4 + 4 * i as u32, v).unwrap();
    }
    g
}

/// `impl.render.scene_passes` on the runtime: the calls the runtime makes
/// are the port's, through the targeted base layers, the light pass and
/// the transparents.
#[test]
fn the_runtime_runs_the_rust_scene_passes() {
    use engine::render_loop::scene::{ScenePasses, scene_passes as sp};
    let u = Under {
        switch: "impl.render.scene_passes=on",
        point: &points::RENDER_SCENE_PASSES,
        callees: &sp::CALLEES,
        cleanup: sp::cleanup,
        keep_ecx: &[],
        start: ScenePasses::start,
        resume: ScenePasses::resume,
    };
    let entry = Regs {
        esp: STACK,
        ..entry()
    };
    let (want, got, direct, runtime) = both(&u, scene_guest, entry);
    let targets: Vec<u32> = got.iter().map(|c| c.0).collect();
    for t in [
        sp::TARGET,
        sp::LAYER_DRAW,
        sp::LIGHTS,
        sp::TRANSPARENTS,
        sp::EXIT,
    ] {
        assert!(targets.contains(&t), "{t:#x} not reached: {targets:#x?}");
    }
    assert_eq!(got, want, "the runtime's calls are the port's");
    assert_eq!(runtime, direct);
    assert_eq!(runtime[4], STACK + 4 + sp::RET_CLEANUP, "ret 0x18");
}

/// `impl.render.game_view` on the runtime, through to `render_view_scene`.
#[test]
fn the_runtime_runs_the_rust_game_view() {
    use engine::render_loop::scene::{GameView, game_view as gv};
    const VIEW: u32 = 0x0060_0000;
    let guest = || {
        let mut g = scene_guest();
        g.set_u32(0x0046_86d4, VIEW + 0x200).unwrap();
        g.set_u32(0x0046_86cc, VIEW + 0x300).unwrap();
        g.set_u32(VIEW + 4, 1).unwrap();
        g.set_u32(VIEW + 8, u32::MAX).unwrap();
        g.set_u32(VIEW + 0x34, 1000.0f32.to_bits()).unwrap();
        g.set_u32(STACK + 4, VIEW).unwrap();
        g
    };
    let u = Under {
        switch: "impl.render.game_view=on",
        point: &points::RENDER_GAME_VIEW,
        callees: &gv::CALLEES,
        cleanup: gv::cleanup,
        keep_ecx: &[],
        start: GameView::start,
        resume: GameView::resume,
    };
    let entry = Regs {
        esp: STACK,
        ..entry()
    };
    let (want, got, direct, runtime) = both(&u, guest, entry);
    let targets: Vec<u32> = got.iter().map(|c| c.0).collect();
    assert_eq!(
        targets,
        [
            gv::PREPARE,
            gv::OBSERVER,
            gv::CAMERA,
            gv::PROJECTION,
            gv::VIEW_SCENE
        ],
        "no object for the view: no cluster call"
    );
    assert_eq!(got, want, "the runtime's calls are the port's");
    assert_eq!(runtime, direct);
    assert_eq!(runtime[4], STACK + 4 + gv::RET_CLEANUP, "ret 8");
}

/// The render-state list's guest: a context at `0x00600000` whose list
/// holds every kind of entry (each `SetRenderState_Simple` type, a deferred
/// state, state 0x88, setters with both value copies, colours), one entry
/// repeated and one the D3D shadow already holds.
fn states_guest() -> Guest {
    use engine::render_loop::states as st;
    const CTX: u32 = 0x0060_0000;
    let mut g = scene_guest();
    g.map_zeroed(0x003f_0000, 0x0001_0000, Permissions::RX)
        .unwrap();
    g.map_zeroed(0x0040_0000, 0x0005_0000, Permissions::RW)
        .unwrap();
    g.set_u32(CTX + 0x20, CTX + 0x100).unwrap();
    g.set_u32(CTX + 0x13c, CTX + 0x200).unwrap();
    // Definitions: (state, type).
    let defs = [
        (0x10, 2),
        (0x11, 1),
        (0x12, 0),
        (0x60, 2),
        (0x88, 1),
        (0x89, 2),
        (0x8a, 1),
        (0x13, 1),
        (0x8b, 2),
        (0x14, 2),
    ];
    for (i, (state, ty)) in defs.into_iter().enumerate() {
        g.set_u32(st::DEFS + 4 * i as u32, state | ty << 24)
            .unwrap();
    }
    // Entries: (definition, source, n); source 1 a float, 0 a colour.
    let entries = [
        (0, 1, 0),
        (0, 1, 0),
        (1, 1, 1),
        (2, 1, 2),
        (3, 1, 3),
        (4, 1, 4),
        (5, 1, 5),
        (6, 1, 6),
        (7, 1, 7),
        (8, 0, 0),
        (9, 0, 1),
    ];
    for (i, (def, source, n)) in entries.into_iter().enumerate() {
        g.set_u32(CTX + 0x200 + 4 * i as u32, def | source << 8 | n << 24)
            .unwrap();
    }
    let floats = [2.7f32, 0.5, 0.25, 9.5, 1.0, 3.2, 7.0, 4.0];
    for (n, f) in floats.into_iter().enumerate() {
        g.set_u32(CTX + 0x5c + 4 * n as u32, f.to_bits()).unwrap();
    }
    g.set_u32(st::SHADOW + 4 * 0x13, 4.0f32.to_bits()).unwrap();
    g.set_u32(st::SHADOW + 4 * 0x12, 0x00ab_cdef).unwrap();
    for s in 0..0x5c {
        g.set_u32(st::HEADERS + 4 * s, 0x0004_0000 | s << 2)
            .unwrap();
    }
    g.set_u32(st::ALPHA_SCALE, 255.0f32.to_bits()).unwrap();
    g.set_u32(st::DEVICE, CTX + 0x800).unwrap();
    g.set_u32(CTX + 0x300, entries.len() as u32 * 0x200)
        .unwrap();
    g.set_u32(STACK + 4, CTX + 0x300).unwrap();
    g
}

/// `impl.render.states` and `impl.render.states_skip` on the runtime: the
/// calls the runtime makes are the port's (Simple, the colour, setters
/// popping their argument), with the runtime's x87 control word; the
/// skipping port makes two Simple calls fewer (the repeat and the state
/// the shadow holds).
#[test]
fn the_runtime_runs_the_rust_render_states() {
    use engine::memory::GuestMemory;
    use engine::render_loop::states::{self as st, Mode, RenderStates};
    let callees: &'static [u32] =
        Vec::leak([&[st::SIMPLE, st::COLOUR][..], &st::SETTERS[..]].concat());
    let entry = Regs {
        ebx: 0x0060_0000,
        esp: STACK,
        ..entry()
    };
    let exact: StartFn<RenderStates> =
        |r: &mut Regs, m: &mut dyn GuestMemory| RenderStates::start(r, m, 0x037f, Mode::Exact);
    let skip: StartFn<RenderStates> = |r: &mut Regs, m: &mut dyn GuestMemory| {
        RenderStates::start(r, m, 0x037f, Mode::SkipRedundant)
    };
    let mut simple = Vec::new();
    for (switch, start) in [
        ("impl.render.states=on", exact),
        ("impl.render.states_skip=on", skip),
    ] {
        let u = Under {
            switch,
            point: &points::RENDER_STATES,
            callees,
            cleanup: st::cleanup,
            // The port reads the definition through ECX after the colour.
            keep_ecx: &[st::COLOUR],
            start,
            resume: RenderStates::resume,
        };
        let (want, got, direct, runtime) = both(&u, states_guest, entry);
        let targets: Vec<u32> = got.iter().map(|c| c.0).collect();
        for t in [
            st::SIMPLE,
            st::COLOUR,
            0x003f_5c70,
            0x003f_5370,
            0x003f_5ba0,
        ] {
            assert!(targets.contains(&t), "{t:#x} not reached: {targets:#x?}");
        }
        assert_eq!(got, want, "{switch}: the runtime's calls are the port's");
        assert_eq!(runtime, direct, "{switch}");
        assert_eq!(runtime[4], STACK + 4 + st::RET_CLEANUP, "ret 4");
        // 2.7 rounds to nearest under the default control word.
        let first = got.iter().find(|c| c.0 == st::SIMPLE).unwrap();
        assert_eq!(first.1[2], 3, "{switch}: fistp(2.7)");
        simple.push(targets.iter().filter(|&&t| t == st::SIMPLE).count());
    }
    assert_eq!(simple[0], simple[1] + 2, "{simple:?}");
}
