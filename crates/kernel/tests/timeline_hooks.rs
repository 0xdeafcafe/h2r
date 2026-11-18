//! The scheduler's timeline hooks (docs/perf/profiling.md, `sched`): a `schedule`
//! span per `Machine::run` iteration with the slice or DPC inside it, the
//! logical clock as the `logical us` counter, and the ticks each idle
//! advance skipped. Alone in its binary: the timeline is process-wide.
mod common;
use common::*;
use kernel::periodic::{Rate, Source};
use kernel::{Machine, RunOutcome};
use runtime::{CallConvention, GuestInterrupt};

const CALLBACK: u32 = 0x800;
const DELAY: u32 = DATA + 0x120;

#[test]
fn a_sleep_through_vblanks_records_schedule_spans_and_logical_time() {
    let h = Harness::new();
    // 100 ms asleep, then exit 0.
    h.runtime
        .memory()
        .set_u64(DELAY, (1_000_000i64).wrapping_neg() as u64)
        .unwrap();
    h.code(0, |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(99), // KeDelayExecutionThread
            &[0, 0, DELAY],
            CODE + 0x100,
        )
    });
    h.code(0x100, |cpu, ctx, _| {
        cpu.eax = 0;
        ctx.memory().ret(cpu, 0)
    });
    h.code(CALLBACK, |cpu, ctx, _| ctx.memory().ret(cpu, 0));
    h.kernel
        .add_periodic(Source {
            name: "vblank",
            rate: Rate::HZ_60,
            max_catch_up: 4,
            handler: Box::new(move |_, scratch, _| {
                Ok(Some(GuestInterrupt {
                    target: CODE + CALLBACK,
                    args: vec![scratch],
                    convention: CallConvention::Cdecl,
                    thread: None,
                    coalesce: false,
                    name: "vblank",
                }))
            }),
        })
        .unwrap();
    let mut m = Machine::new(
        h.runtime.clone(),
        h.kernel.clone(),
        h.cpu,
        STACK_BOTTOM,
        STACK_TOP,
    )
    .unwrap();
    timeline::start(timeline::Config::all());
    let outcome = m.run(100).unwrap();
    let t = timeline::stop();
    assert!(
        matches!(outcome, RunOutcome::Complete { .. }),
        "{outcome:?}"
    );
    // Two slices (before and after the sleep) and six vblank DPCs, each
    // in an iteration of its own; idle advances in iterations of their own.
    assert_eq!(t.count("slice"), 2);
    assert_eq!(t.count("vblank"), 6);
    assert!(t.count("schedule") >= 8, "{}", t.count("schedule"));
    assert_eq!(
        t.count("idle: clock to deadline"),
        6,
        "five vblanks, then the sleep's deadline"
    );
    let mut json = Vec::new();
    t.write_chrome(&mut json).unwrap();
    let json = String::from_utf8(json).unwrap();
    // The clock reaches the sleep's end: 100 ms = 100000 us.
    assert!(
        json.contains(&format!(
            "\"{}\":100000}}",
            kernel::threads::LOGICAL_COUNTER
        )),
        "{json}"
    );
    // The first idle advance skips to the first vblank: ceil(1e7 / 60).
    assert!(json.contains("\"ticks\":166667"), "{json}");
    assert!(json.contains("\"accesses\":"), "slices carry their charge");
}
