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
