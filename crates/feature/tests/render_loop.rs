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
    let al = if target == views::VIEW_TEST {
        n as u32 & 1
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
