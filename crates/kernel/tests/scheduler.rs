//! Scheduler fairness: NtYieldExecution (238), idle time advance, DPC
//! delivery and KTHREAD priorities. Synthetic guest handlers only.
mod common;
use common::*;
use kernel::{Machine, RunOutcome, status, threads::DPC_THREAD_ID};
use runtime::{CpuAbi, ExecCtx, GuestFault, TransferAbi, kernel::function_token as token};

const TIMER: u32 = DATA + 0x100;
const DPC: u32 = DATA + 0x140;
const DPC_COUNT: u32 = DATA + 0x180;
const HANDLES: u32 = DATA + 0x200;
const OBJECT: u32 = DATA + 0x240;
/// Count, then entries.
const LOG: u32 = DATA + 0x800;
/// Per-thread loop counters, indexed by thread ID.
const COUNTERS: u32 = DATA + 0x400;

fn machine(h: &Harness) -> Machine {
    Machine::new(
        h.runtime.clone(),
        h.kernel.clone(),
        h.cpu,
        STACK_BOTTOM,
        STACK_TOP,
    )
    .unwrap()
}
fn log(ctx: &mut ExecCtx, value: u32) -> Result<(), GuestFault> {
    let mut m = ctx.memory();
    let n = m.u32(LOG)?;
    m.set_u32(LOG + 4 + n * 4, value)?;
    m.set_u32(LOG, n + 1)
}
fn logged(h: &Harness) -> Vec<u32> {
    (0..h.get(LOG)).map(|i| h.get(LOG + 4 + i * 4)).collect()
}
fn relative(ms: i64) -> (u32, u32) {
    let v = (-ms * 10_000) as u64;
    (v as u32, (v >> 32) as u32)
}
/// PsCreateSystemThreadEx with no SystemRoutine: `start` runs directly.
fn create(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    slot: u32,
    start: u32,
    continuation: u32,
) -> Result<TransferAbi, GuestFault> {
    guest_call(
        cpu,
        ctx,
        token(255),
        &[HANDLES + slot * 4, 0, 0x4000, 0, 0, start, 0, 0, 0, 0],
        continuation,
    )
}
/// A thread body at `offset`: log its ID, yield, `rounds` times, then log
/// its ID once more and return. Each yield must report `expect`.
fn yielder(h: &Harness, offset: u32, rounds: u32, expect: u32) {
    h.code(offset, move |cpu, ctx, _| {
        log(ctx, ctx.thread_id)?;
        let counter = COUNTERS + ctx.thread_id * 4;
        let n = ctx.memory().u32(counter)?;
        if n == rounds {
            cpu.eax = 0;
            return ctx.memory().ret(cpu, 0);
        }
        ctx.memory().set_u32(counter, n + 1)?;
        guest_call(cpu, ctx, token(238), &[], CODE + offset + 0x10)
    });
    h.code(offset + 0x10, move |cpu, ctx, _| {
        assert_eq!(cpu.eax, expect, "thread {} yield status", ctx.thread_id);
        Ok(TransferAbi::jump(CODE + offset))
    });
}

/// The boot hang: a lone thread spinning on NtYieldExecution while XNet's
/// 20 ms periodic timer DPC is armed. Each idle advance delivers the DPC
/// and then must give the yielding thread its slice.
#[test]
fn a_lone_yielder_runs_between_periodic_timer_dpcs() {
    let mut h = Harness::new();
    let mut machine = machine(&h);
    h.code(0x300, |cpu, ctx, _| {
        let mut m = ctx.memory();
        let count = m.u32(DPC_COUNT)?;
        m.set_u32(DPC_COUNT, count + 1)?;
        m.ret(cpu, 16)
    });
    h.success(113, &[TIMER, 1]);
    h.success(107, &[DPC, CODE + 0x300, 0]);
    let (lo, hi) = relative(20);
    assert_eq!(h.success(150, &[TIMER, lo, hi, 20, DPC]), 0);
    yielder(&h, 0, 5, status::NO_YIELD_PERFORMED);
    let outcome = machine.run(100).unwrap();
    assert_eq!(
        outcome,
        RunOutcome::Complete {
            exits: [(1, 0)].into()
        }
    );
    assert_eq!(logged(&h), [1; 6]);
    // One idle advance and one DPC per yield: 5 x 20 ms.
    assert_eq!(h.get(DPC_COUNT), 5);
    assert_eq!(h.kernel.clock().counter(), 5 * 200_000);
    let d = DPC_THREAD_ID;
    assert_eq!(machine.schedule(), &[1, d, 1, d, 1, d, 1, d, 1, d, 1]);
}

/// Several polling threads share one idle advance per round, and every one
/// of them runs before the clock moves again.
#[test]
fn equal_priority_yielders_round_robin_and_share_idle_advances() {
    let mut h = Harness::new();
    let mut machine = machine(&h);
    h.code(0x300, |cpu, ctx, _| {
        let mut m = ctx.memory();
        let count = m.u32(DPC_COUNT)?;
        m.set_u32(DPC_COUNT, count + 1)?;
        m.ret(cpu, 16)
    });
    h.success(113, &[TIMER, 1]);
    h.success(107, &[DPC, CODE + 0x300, 0]);
    let (lo, hi) = relative(20);
    assert_eq!(h.success(150, &[TIMER, lo, hi, 20, DPC]), 0);
    h.code(0, |cpu, ctx, _| {
        create(cpu, ctx, 0, CODE + 0x1000, CODE + 0x10)
    });
    h.code(0x10, |cpu, ctx, _| {
        create(cpu, ctx, 1, CODE + 0x1000, CODE + 0x1000)
    });
    yielder(&h, 0x1000, 3, status::SUCCESS);
    let outcome = machine.run(100).unwrap();
    assert!(
        matches!(outcome, RunOutcome::Complete { .. }),
        "{outcome:?}"
    );
    assert_eq!(logged(&h), [1, 2, 3, 1, 2, 3, 1, 2, 3, 1, 2, 3]);
    // Each round of three yields is one idle advance and one DPC.
    assert_eq!(h.get(DPC_COUNT), 3);
    assert_eq!(h.kernel.clock().counter(), 3 * 200_000);
}

/// A yield with only a peer and no deadline hands over (SUCCESS); once alone
/// with nothing pending it returns NO_YIELD_PERFORMED at once.
#[test]
fn a_yield_hands_over_to_a_peer_and_returns_when_alone() {
    let h = Harness::new();
    let mut machine = machine(&h);
    h.code(0, |cpu, ctx, _| {
        create(cpu, ctx, 0, CODE + 0x200, CODE + 0x10)
    });
    h.code(0x10, |cpu, ctx, _| {
        log(ctx, 1)?;
        guest_call(cpu, ctx, token(238), &[], CODE + 0x20)
    });
    h.code(0x20, |cpu, ctx, _| {
        assert_eq!(cpu.eax, status::SUCCESS);
        log(ctx, 1)?;
        guest_call(cpu, ctx, token(238), &[], CODE + 0x30)
    });
    h.code(0x30, |cpu, ctx, _| {
        assert_eq!(cpu.eax, status::NO_YIELD_PERFORMED);
        log(ctx, 1)?;
        cpu.eax = 0;
        ctx.memory().ret(cpu, 0)
    });
    h.code(0x200, |cpu, ctx, _| {
        log(ctx, 2)?;
        cpu.eax = 0;
        ctx.memory().ret(cpu, 0)
    });
    let outcome = machine.run(20).unwrap();
    assert!(
        matches!(outcome, RunOutcome::Complete { .. }),
        "{outcome:?}"
    );
    assert_eq!(logged(&h), [1, 2, 1, 1]);
    assert_eq!(machine.schedule(), &[1, 2, 1]);
    assert_eq!(h.kernel.clock().counter(), 0, "no idle advance");
}

/// Reference a created thread's KTHREAD (null object type) and change its
/// base priority by `increment`.
fn set_priority(h: &Harness, offset: u32, slot: u32, increment: i32, next: u32) {
    h.code(offset, move |cpu, ctx, _| {
        let handle = ctx.memory().u32(HANDLES + slot * 4)?;
        guest_call(
            cpu,
            ctx,
            token(246),
            &[handle, 0, OBJECT],
            CODE + offset + 0x8,
        )
    });
    h.code(offset + 0x8, move |cpu, ctx, _| {
        assert_eq!(cpu.eax, 0);
        let thread = ctx.memory().u32(OBJECT)?;
        guest_call(cpu, ctx, token(143), &[thread, increment as u32], next)
    });
}

/// The highest-priority ready thread runs first. A yield passes the slice
/// to the best other ready thread, even one of lower priority than the
/// yielder (KiSelectReadyThread(1)), so a high-priority pair that keeps
/// yielding to each other runs ahead of a lower-priority thread.
#[test]
fn ready_threads_run_by_priority_and_a_yield_passes_to_the_best_other() {
    let h = Harness::new();
    let mut machine = machine(&h);
    // Thread 2 is low (8 - 2 = 6), thread 3 high (8 + 2 = 10).
    h.code(0, |cpu, ctx, _| {
        create(cpu, ctx, 0, CODE + 0x2000, CODE + 0x10)
    });
    h.code(0x10, |cpu, ctx, _| {
        create(cpu, ctx, 1, CODE + 0x3000, CODE + 0x20)
    });
    set_priority(&h, 0x20, 0, -2, CODE + 0x50);
    set_priority(&h, 0x50, 1, 2, CODE + 0x1000);
    yielder(&h, 0x1000, 3, status::SUCCESS);
    yielder(&h, 0x2000, 0, status::SUCCESS);
    yielder(&h, 0x3000, 2, status::SUCCESS);
    let outcome = machine.run(50).unwrap();
    assert!(
        matches!(outcome, RunOutcome::Complete { .. }),
        "{outcome:?}"
    );
    // 1 and 3 alternate; 2 (lowest) only runs once both have exited.
    assert_eq!(logged(&h), [1, 3, 1, 3, 1, 3, 1, 2]);
    assert_eq!(machine.schedule(), &[1, 3, 1, 3, 1, 3, 1, 2]);
}

/// A yield to a strictly lower-priority thread still hands over.
#[test]
fn a_high_priority_yield_hands_over_to_a_lower_priority_thread() {
    let h = Harness::new();
    let mut machine = machine(&h);
    h.code(0, |cpu, ctx, _| {
        create(cpu, ctx, 0, CODE + 0x200, CODE + 0x10)
    });
    set_priority(&h, 0x10, 0, -4, CODE + 0x30);
    h.code(0x30, |cpu, ctx, _| {
        guest_call(cpu, ctx, token(238), &[], CODE + 0x40)
    });
    h.code(0x40, |cpu, ctx, _| {
        assert_eq!(cpu.eax, status::SUCCESS);
        log(ctx, 1)?;
        cpu.eax = 0;
        ctx.memory().ret(cpu, 0)
    });
    h.code(0x200, |cpu, ctx, _| {
        log(ctx, 2)?;
        cpu.eax = 0;
        ctx.memory().ret(cpu, 0)
    });
    let outcome = machine.run(20).unwrap();
    assert!(
        matches!(outcome, RunOutcome::Complete { .. }),
        "{outcome:?}"
    );
    assert_eq!(logged(&h), [2, 1]);
}

/// A DPC that re-arms its own timer due now queues itself again every time
/// it runs. DPC delivery still alternates with the ready thread instead of
/// starving it: each of the thread's kernel calls sees one more DPC.
#[test]
fn a_self_requeueing_dpc_cannot_starve_a_ready_thread() {
    let h = Harness::new();
    let mut machine = machine(&h);
    h.code(0x300, |cpu, ctx, _| {
        {
            let mut m = ctx.memory();
            let count = m.u32(DPC_COUNT)?;
            m.set_u32(DPC_COUNT, count + 1)?;
        }
        // Absolute zero: already due, so it expires inside KeSetTimer.
        guest_call(cpu, ctx, token(149), &[TIMER, 0, 0, DPC], CODE + 0x340)
    });
    h.code(0x340, |cpu, ctx, _| ctx.memory().ret(cpu, 16));
    h.code(0, |cpu, ctx, _| {
        guest_call(cpu, ctx, token(113), &[TIMER, 0], CODE + 0x10)
    });
    h.code(0x10, |cpu, ctx, _| {
        guest_call(cpu, ctx, token(107), &[DPC, CODE + 0x300, 0], CODE + 0x20)
    });
    h.code(0x20, |cpu, ctx, _| {
        guest_call(cpu, ctx, token(149), &[TIMER, 0, 0, DPC], CODE + 0x30)
    });
    h.code(0x30, |cpu, ctx, _| {
        let count = ctx.memory().u32(DPC_COUNT)?;
        log(ctx, count)?;
        if count == 5 {
            cpu.eax = 0;
            return ctx.memory().ret(cpu, 0);
        }
        guest_call(cpu, ctx, token(238), &[], CODE + 0x30)
    });
    let outcome = machine.run(100).unwrap();
    assert!(
        matches!(outcome, RunOutcome::Complete { .. }),
        "{outcome:?}"
    );
    assert_eq!(logged(&h), [1, 2, 3, 4, 5]);
}

// Review probes (kernel-r5 rework): priorities must not freeze logical
// time, and finite DPC chains drain before the thread continues.
/// An unused low-priority ready thread must not stop interrupt time.
#[test]
fn review_priority_pollers_must_not_freeze_periodic_timer() {
    let mut h = Harness::new();
    let mut machine = machine(&h);
    h.code(0x300, |cpu, ctx, _| {
        ctx.memory().set_u32(DPC_COUNT, 1)?;
        ctx.memory().ret(cpu, 16)
    });
    h.success(113, &[TIMER, 1]);
    h.success(107, &[DPC, CODE + 0x300, 0]);
    let (lo, hi) = relative(20);
    h.success(150, &[TIMER, lo, hi, 20, DPC]);
    h.code(0, |cpu, ctx, _| {
        create(cpu, ctx, 0, CODE + 0x2000, CODE + 0x10)
    });
    h.code(0x10, |cpu, ctx, _| {
        create(cpu, ctx, 1, CODE + 0x1000, CODE + 0x20)
    });
    set_priority(&h, 0x20, 0, -2, CODE + 0x50);
    set_priority(&h, 0x50, 1, 2, CODE + 0x1000);
    h.code(0x1000, |cpu, ctx, _| {
        if ctx.memory().u32(DPC_COUNT)? != 0 {
            cpu.eax = 0;
            return ctx.memory().ret(cpu, 0);
        }
        guest_call(cpu, ctx, token(238), &[], CODE + 0x1000)
    });
    h.code(0x2000, |cpu, ctx, _| {
        cpu.eax = 0;
        ctx.memory().ret(cpu, 0)
    });
    let outcome = machine.run(100).unwrap();
    assert!(
        matches!(outcome, RunOutcome::Complete { .. }),
        "{outcome:?}; timer DPCs={}, clock={}, schedule={:?}",
        h.get(DPC_COUNT),
        h.kernel.clock().counter(),
        machine.schedule()
    );
}

/// A single high-priority SwitchToThread poller and a low flag setter.
#[test]
fn review_single_high_yielder_allows_low_flag_setter() {
    let h = Harness::new();
    let mut machine = machine(&h);
    h.code(0, |cpu, ctx, _| {
        create(cpu, ctx, 0, CODE + 0x2000, CODE + 0x10)
    });
    set_priority(&h, 0x10, 0, -2, CODE + 0x1000);
    h.code(0x1000, |cpu, ctx, _| {
        if ctx.memory().u32(DPC_COUNT)? != 0 {
            cpu.eax = 0;
            return ctx.memory().ret(cpu, 0);
        }
        guest_call(cpu, ctx, token(238), &[], CODE + 0x1000)
    });
    h.code(0x2000, |cpu, ctx, _| {
        ctx.memory().set_u32(DPC_COUNT, 1)?;
        cpu.eax = 0;
        ctx.memory().ret(cpu, 0)
    });
    assert!(matches!(
        machine.run(100).unwrap(),
        RunOutcome::Complete { .. }
    ));
}

/// A single high yielder must let a low thread finish a timed wait.
#[test]
fn review_single_high_yielder_allows_low_timed_wait() {
    let mut h = Harness::new();
    let mut machine = machine(&h);
    h.success(108, &[TIMER, 0, 0]); // notification event, unsignalled
    let (lo, hi) = relative(10);
    h.set(DATA + 0x300, lo);
    h.set(DATA + 0x304, hi);
    h.code(0, |cpu, ctx, _| {
        create(cpu, ctx, 0, CODE + 0x2000, CODE + 0x10)
    });
    set_priority(&h, 0x10, 0, -2, CODE + 0x1000);
    h.code(0x1000, |cpu, ctx, _| {
        if ctx.memory().u32(DPC_COUNT)? != 0 {
            cpu.eax = 0;
            return ctx.memory().ret(cpu, 0);
        }
        guest_call(cpu, ctx, token(238), &[], CODE + 0x1000)
    });
    h.code(0x2000, |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            token(159),
            &[TIMER, 0, 0, 0, DATA + 0x300],
            CODE + 0x2010,
        )
    });
    h.code(0x2010, |cpu, ctx, _| {
        assert_eq!(cpu.eax, status::TIMEOUT);
        ctx.memory().set_u32(DPC_COUNT, 1)?;
        cpu.eax = 0;
        ctx.memory().ret(cpu, 0)
    });
    assert!(matches!(
        machine.run(100).unwrap(),
        RunOutcome::Complete { .. }
    ));
    assert_eq!(h.kernel.clock().counter(), 100_000);
}

/// A finite A -> B DPC chain should complete before returning to PASSIVE_LEVEL.
#[test]
fn review_distinct_nested_dpc_precedes_guest_continuation() {
    let h = Harness::new();
    let mut machine = machine(&h);
    h.code(0, |cpu, ctx, _| {
        guest_call(cpu, ctx, token(113), &[TIMER, 0], CODE + 0x10)
    });
    h.code(0x10, |cpu, ctx, _| {
        guest_call(cpu, ctx, token(113), &[TIMER + 0x400, 0], CODE + 0x20)
    });
    h.code(0x20, |cpu, ctx, _| {
        guest_call(cpu, ctx, token(107), &[DPC, CODE + 0x300, 0], CODE + 0x30)
    });
    h.code(0x30, |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            token(107),
            &[DPC + 0x400, CODE + 0x400, 0],
            CODE + 0x40,
        )
    });
    h.code(0x40, |cpu, ctx, _| {
        guest_call(cpu, ctx, token(149), &[TIMER, 0, 0, DPC], CODE + 0x50)
    });
    h.code(0x50, |cpu, ctx, _| {
        let second_dpc_ran = ctx.memory().u32(DPC_COUNT)?;
        assert_eq!(
            second_dpc_ran, 1,
            "guest continuation ran before distinct DPC B"
        );
        cpu.eax = 0;
        ctx.memory().ret(cpu, 0)
    });
    h.code(0x300, |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            token(149),
            &[TIMER + 0x400, 0, 0, DPC + 0x400],
            CODE + 0x340,
        )
    });
    h.code(0x340, |cpu, ctx, _| ctx.memory().ret(cpu, 16));
    h.code(0x400, |cpu, ctx, _| {
        ctx.memory().set_u32(DPC_COUNT, 1)?;
        ctx.memory().ret(cpu, 16)
    });
    let outcome = machine.run(20).unwrap();
    assert!(
        matches!(outcome, RunOutcome::Complete { .. }),
        "{outcome:?}"
    );
}

#[test]
fn review_eight_equal_yielders_share_periodic_time() {
    let mut h = Harness::new();
    let mut machine = machine(&h);
    h.code(0, |_, _, _| Ok(TransferAbi::jump(CODE + 0x1000)));
    h.code(0x300, |cpu, ctx, _| {
        let mut m = ctx.memory();
        let n = m.u32(DPC_COUNT)?;
        m.set_u32(DPC_COUNT, n + 1)?;
        m.ret(cpu, 16)
    });
    for slot in 0..7 {
        h.success(
            255,
            &[
                HANDLES + slot * 4,
                0,
                0x4000,
                0,
                0,
                CODE + 0x1000,
                0,
                0,
                0,
                0,
            ],
        );
    }
    h.success(113, &[TIMER, 1]);
    h.success(107, &[DPC, CODE + 0x300, 0]);
    let (lo, hi) = relative(20);
    h.success(150, &[TIMER, lo, hi, 20, DPC]);
    yielder(&h, 0x1000, 5, status::SUCCESS);
    let outcome = machine.run(200).unwrap();
    assert!(
        matches!(outcome, RunOutcome::Complete { .. }),
        "{outcome:?}"
    );
    assert_eq!(h.get(DPC_COUNT), 5);
    assert_eq!(logged(&h), (0..6).flat_map(|_| 1..=8).collect::<Vec<_>>());
}

#[test]
fn review_sleep_zero_must_not_freeze_periodic_timer() {
    let mut h = Harness::new();
    let mut machine = machine(&h);
    h.code(0x300, |cpu, ctx, _| {
        ctx.memory().set_u32(DPC_COUNT, 1)?;
        ctx.memory().ret(cpu, 16)
    });
    h.success(113, &[TIMER, 1]);
    h.success(107, &[DPC, CODE + 0x300, 0]);
    let (lo, hi) = relative(20);
    h.success(150, &[TIMER, lo, hi, 20, DPC]);
    h.code(0, |cpu, ctx, _| {
        create(cpu, ctx, 0, CODE + 0x2000, CODE + 0x10)
    });
    set_priority(&h, 0x10, 0, -2, CODE + 0x1000);
    h.code(0x1000, |cpu, ctx, _| {
        if ctx.memory().u32(DPC_COUNT)? != 0 {
            cpu.eax = 0;
            return ctx.memory().ret(cpu, 0);
        }
        guest_call(cpu, ctx, token(99), &[0, 0, DATA + 0x300], CODE + 0x1000)
    });
    h.code(0x2000, |cpu, ctx, _| {
        cpu.eax = 0;
        ctx.memory().ret(cpu, 0)
    });
    let outcome = machine.run(100).unwrap();
    assert!(
        matches!(outcome, RunOutcome::Complete { .. }),
        "{outcome:?}; timer DPCs={}, clock={}",
        h.get(DPC_COUNT),
        h.kernel.clock().counter()
    );
}

#[test]
fn review_timeout_wakeup_must_not_stop_later_timer() {
    let mut h = Harness::new();
    let mut machine = machine(&h);
    const EVENT: u32 = DATA + 0x600;
    h.success(108, &[EVENT, 0, 0]);
    h.code(0x300, |cpu, ctx, _| {
        ctx.memory().set_u32(DPC_COUNT, 1)?;
        ctx.memory().ret(cpu, 16)
    });
    h.success(113, &[TIMER, 1]);
    h.success(107, &[DPC, CODE + 0x300, 0]);
    let (lo, hi) = relative(20);
    h.success(150, &[TIMER, lo, hi, 20, DPC]);
    let (lo, hi) = relative(10);
    h.set(DATA + 0x300, lo);
    h.set(DATA + 0x304, hi);
    h.code(0, |cpu, ctx, _| {
        create(cpu, ctx, 0, CODE + 0x1000, CODE + 0x10)
    });
    h.code(0x10, |cpu, ctx, _| {
        create(cpu, ctx, 1, CODE + 0x1000, CODE + 0x20)
    });
    set_priority(&h, 0x20, 1, 2, CODE + 0x50);
    let root = machine.thread(1).unwrap().object;
    h.code(0x50, move |cpu, ctx, _| {
        guest_call(cpu, ctx, token(143), &[root, (-2i32) as u32], CODE + 0x70)
    });
    h.code(0x70, |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            token(159),
            &[EVENT, 0, 0, 0, DATA + 0x300],
            CODE + 0x80,
        )
    });
    h.code(0x80, |cpu, ctx, _| {
        assert_eq!(cpu.eax, status::TIMEOUT);
        cpu.eax = 0;
        ctx.memory().ret(cpu, 0)
    });
    h.code(0x1000, |cpu, ctx, _| {
        if ctx.memory().u32(DPC_COUNT)? != 0 {
            cpu.eax = 0;
            return ctx.memory().ret(cpu, 0);
        }
        guest_call(cpu, ctx, token(238), &[], CODE + 0x1000)
    });
    let outcome = machine.run(100).unwrap();
    assert!(
        matches!(outcome, RunOutcome::Complete { .. }),
        "{outcome:?}; timer DPCs={}, clock={}",
        h.get(DPC_COUNT),
        h.kernel.clock().counter()
    );
}

#[test]
fn review_alertable_boundaries_stop_explicitly() {
    let mut h = Harness::new();
    let _machine = machine(&h);
    h.success(108, &[TIMER, 0, 0]);
    h.stopped(159, &[TIMER, 0, 0, 1, 0]);
    h.stopped(99, &[0, 1, DATA + 0x300]);
}

// Recheck probes (kernel-r5): priority boundaries for the idle advance, and
// DPC cycles mixed with finite DPC work.
fn recheck_priority(h: &mut Harness, slot: u32, increment: i32) {
    let handle = h.get(HANDLES + slot * 4);
    h.success(246, &[handle, 0, OBJECT]);
    let object = h.get(OBJECT);
    h.success(143, &[object, increment as u32]);
}
fn recheck_boundary(kind: u32) {
    let mut h = Harness::new();
    let mut machine = machine(&h);
    h.success(113, &[TIMER, 1]);
    let (lo, hi) = relative(20);
    h.success(149, &[TIMER, lo, hi, 0]);
    h.code(0, |cpu, ctx, _| {
        create(cpu, ctx, 0, CODE + 0x1000, CODE + 0x10)
    });
    set_priority(&h, 0x10, 0, 2, CODE + 0x1000);
    h.code(0x1000, |cpu, ctx, _| {
        if ctx.memory().u32(DPC_COUNT)? != 0 {
            cpu.eax = 0;
            return ctx.memory().ret(cpu, 0);
        }
        guest_call(cpu, ctx, token(238), &[], CODE + 0x1000)
    });
    let clock_kernel = h.kernel.clone();
    h.code(0x2000, move |cpu, ctx, _| {
        assert_eq!(
            clock_kernel.clock().counter(),
            0,
            "real work must precede timer expiry"
        );
        assert_eq!(ctx.runtime().memory().u32(DPC_COUNT)?, 0);
        ctx.memory().set_u32(DPC_COUNT, 1)?;
        cpu.eax = 0;
        ctx.memory().ret(cpu, 0)
    });
    assert_eq!(machine.run(2).unwrap(), RunOutcome::SliceLimit);
    assert_eq!(machine.schedule(), &[1, 2]);
    assert_eq!(h.kernel.clock().counter(), 0);
    // Both existing threads are now marked polling. Introduce real work.
    let suspended = u32::from(kind == 2);
    h.success(
        255,
        &[
            HANDLES + 4,
            0,
            0x4000,
            0,
            0,
            CODE + 0x2000,
            0,
            suspended,
            0,
            0,
        ],
    );
    match kind {
        0 => recheck_priority(&mut h, 1, 2), // same priority as high poller
        1 => recheck_priority(&mut h, 1, 4), // newly created higher priority
        2 => {
            recheck_priority(&mut h, 1, 4);
            let handle = h.get(HANDLES + 4);
            h.success(224, &[handle, 0]);
        }
        3 => {
            // boost ready work from below the old scheduling floor
            recheck_priority(&mut h, 1, -2);
            recheck_priority(&mut h, 1, 4);
        }
        4 => {
            // lowering the high poller changes the next pick/floor
            recheck_priority(&mut h, 0, -4);
        }
        5 => {}
        _ => unreachable!(),
    }
    if kind == 5 {
        assert_eq!(machine.run(1).unwrap(), RunOutcome::SliceLimit);
        assert_eq!(
            h.kernel.clock().counter(),
            0,
            "next pick floor must include its peers"
        );
        return;
    }
    assert!(matches!(
        machine.run(30).unwrap(),
        RunOutcome::Complete { .. }
    ));
}
#[test]
fn recheck_same_priority_real_work() {
    recheck_boundary(0);
}
#[test]
fn recheck_new_higher_priority_real_work() {
    recheck_boundary(1);
}
#[test]
fn recheck_resumed_higher_priority_real_work() {
    recheck_boundary(2);
}
#[test]
fn recheck_mid_poll_priority_boost() {
    recheck_boundary(3);
}
#[test]
fn recheck_mid_poll_priority_drop() {
    recheck_boundary(4);
}

fn recheck_cycle(ready: bool) {
    let mut h = Harness::new();
    let mut machine = machine(&h);
    h.success(113, &[TIMER, 0]);
    h.success(113, &[TIMER + 0x400, 0]);
    h.success(107, &[DPC, CODE + 0x300, 0]);
    h.success(107, &[DPC + 0x400, CODE + 0x400, 0]);
    h.code(0x300, |cpu, ctx, _| {
        log(ctx, 10)?;
        guest_call(
            cpu,
            ctx,
            token(149),
            &[TIMER + 0x400, 0, 0, DPC + 0x400],
            CODE + 0x340,
        )
    });
    h.code(0x340, |cpu, ctx, _| ctx.memory().ret(cpu, 16));
    h.code(0x400, |cpu, ctx, _| {
        log(ctx, 20)?;
        if ctx.memory().u32(LOG)? >= 4 {
            return ctx.memory().ret(cpu, 16);
        }
        guest_call(cpu, ctx, token(149), &[TIMER, 0, 0, DPC], CODE + 0x440)
    });
    h.code(0x440, |cpu, ctx, _| ctx.memory().ret(cpu, 16));
    if ready {
        h.code(0, |cpu, ctx, _| {
            guest_call(cpu, ctx, token(149), &[TIMER, 0, 0, DPC], CODE + 0x10)
        });
        h.code(0x10, |cpu, ctx, _| {
            assert_eq!(ctx.memory().u32(LOG)?, 2);
            log(ctx, 1)?;
            guest_call(cpu, ctx, token(238), &[], CODE + 0x20)
        });
        h.code(0x20, |cpu, ctx, _| {
            assert_eq!(ctx.memory().u32(LOG)?, 5);
            cpu.eax = 0;
            ctx.memory().ret(cpu, 0)
        });
    } else {
        h.success(108, &[DATA + 0x600, 0, 0]);
        h.code(0, |cpu, ctx, _| {
            guest_call(
                cpu,
                ctx,
                token(159),
                &[DATA + 0x600, 0, 0, 0, 0],
                CODE + 0x10,
            )
        });
        h.code(0x10, |cpu, ctx, _| {
            cpu.eax = 0;
            ctx.memory().ret(cpu, 0)
        });
        assert_eq!(machine.run(1).unwrap(), RunOutcome::SliceLimit);
        h.success(149, &[TIMER, 0, 0, DPC]);
    }
    let outcome = machine.run(20).unwrap();
    if ready {
        assert!(
            matches!(outcome, RunOutcome::Complete { .. }),
            "{outcome:?}"
        );
        assert_eq!(logged(&h), [10, 20, 1, 10, 20]);
    } else {
        assert!(
            matches!(outcome, RunOutcome::Deadlock { .. }),
            "{outcome:?}"
        );
        assert_eq!(logged(&h), [10, 20, 10, 20]);
    }
}
#[test]
fn recheck_dpc_cycle_with_ready_thread() {
    recheck_cycle(true);
}
#[test]
fn recheck_dpc_cycle_without_ready_thread() {
    recheck_cycle(false);
}

#[test]
fn recheck_recurrent_head_must_not_defer_distinct_dpc() {
    let mut h = Harness::new();
    let mut machine = machine(&h);
    h.success(113, &[TIMER, 0]);
    h.success(113, &[TIMER + 0x400, 0]);
    h.success(107, &[DPC, CODE + 0x300, 0]);
    h.success(107, &[DPC + 0x400, CODE + 0x400, 0]);
    h.code(0, |cpu, ctx, _| {
        guest_call(cpu, ctx, token(149), &[TIMER, 0, 0, DPC], CODE + 0x10)
    });
    h.code(0x10, |cpu, ctx, _| {
        assert_eq!(
            ctx.memory().u32(DPC_COUNT)?,
            1,
            "distinct DPC B was deferred behind recurring A"
        );
        cpu.eax = 0;
        ctx.memory().ret(cpu, 0)
    });
    h.code(0x300, |cpu, ctx, _| {
        // Requeue A first, then enqueue distinct finite work B behind it.
        guest_call(cpu, ctx, token(149), &[TIMER, 0, 0, DPC], CODE + 0x310)
    });
    h.code(0x310, |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            token(149),
            &[TIMER + 0x400, 0, 0, DPC + 0x400],
            CODE + 0x340,
        )
    });
    h.code(0x340, |cpu, ctx, _| ctx.memory().ret(cpu, 16));
    h.code(0x400, |cpu, ctx, _| {
        ctx.memory().set_u32(DPC_COUNT, 1)?;
        ctx.memory().ret(cpu, 16)
    });
    assert!(matches!(
        machine.run(20).unwrap(),
        RunOutcome::Complete { .. }
    ));
}

#[test]
fn recheck_yielder_exclusion_floor() {
    recheck_boundary(5);
}
