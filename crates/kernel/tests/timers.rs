mod common;
use common::*;
use kernel::{Machine, RunOutcome, threads::DPC_THREAD_ID};

const TIMER: u32 = DATA + 0x100;
const DPC: u32 = DATA + 0x140;
const RECORD: u32 = DATA + 0x400;
const INTERVAL: u32 = DATA + 0x500;
const IRQL: u32 = 0x24;

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
/// A DPC routine that records its four arguments, its IRQL and a call count.
fn dpc_routine(h: &Harness) {
    h.code(0x300, |cpu, ctx, _| {
        let mut m = ctx.memory();
        for i in 0..4 {
            let v = m.u32(cpu.esp + 4 + i * 4)?;
            m.set_u32(RECORD + i * 4, v)?;
        }
        let irql = m.u8(cpu.fs_base + IRQL)?;
        m.set_u32(RECORD + 16, irql as u32)?;
        let count = m.u32(RECORD + 20)?;
        m.set_u32(RECORD + 20, count + 1)?;
        m.ret(cpu, 16)
    });
}
fn relative(ms: i64) -> (u32, u32) {
    let v = (-ms * 10_000) as u64;
    (v as u32, (v >> 32) as u32)
}

#[test]
fn timer_and_dpc_objects_have_the_documented_layout() {
    let mut h = Harness::new();
    h.bytes(TIMER, &[0xcc; 0x28]);
    assert_eq!(h.success(113, &[TIMER, 1]), 0xfeedface);
    let m = h.runtime.memory();
    assert_eq!(m.read::<4>(TIMER).unwrap(), [9, 0, 10, 0]);
    assert_eq!(m.u32(TIMER + 4).unwrap(), 0);
    assert_eq!(m.u32(TIMER + 8).unwrap(), TIMER + 8);
    assert_eq!(m.u64(TIMER + 0x10).unwrap(), 0);
    assert_eq!(m.u32(TIMER + 0x20).unwrap(), 0);
    assert_eq!(m.u32(TIMER + 0x24).unwrap(), 0);
    drop(m);
    h.bytes(DPC, &[0xcc; 0x1c]);
    h.success(107, &[DPC, CODE + 0x300, 0x1234]);
    let m = h.runtime.memory();
    assert_eq!(m.u16(DPC).unwrap(), 19);
    assert_eq!(m.u8(DPC + 2).unwrap(), 0);
    assert_eq!(m.u32(DPC + 0xc).unwrap(), CODE + 0x300);
    assert_eq!(m.u32(DPC + 0x10).unwrap(), 0x1234);
    assert_eq!(m.u32(DPC + 0x14).unwrap(), 0);
    drop(m);
    h.stopped(113, &[TIMER, 2]);
    // The retail auto power-down call: six hours relative, with a DPC.
    let due = (-216_000_000_000i64) as u64;
    assert_eq!(
        h.success(149, &[TIMER, due as u32, (due >> 32) as u32, DPC]),
        0
    );
    assert_eq!(h.runtime.memory().u8(TIMER + 3).unwrap(), 1);
    assert_eq!(
        h.runtime.memory().u64(TIMER + 0x10).unwrap(),
        216_000_000_000
    );
    assert_eq!(h.get(TIMER + 0x20), DPC);
    // Re-setting an inserted timer reports it was inserted.
    assert_eq!(
        h.success(149, &[TIMER, due as u32, (due >> 32) as u32, 0]),
        1
    );
    assert_eq!(h.success(97, &[TIMER]), 1);
    assert_eq!(h.runtime.memory().u8(TIMER + 3).unwrap(), 0);
    assert_eq!(h.success(97, &[TIMER]), 0);
    // Not a timer / not a DPC: diagnostic stops, frames intact.
    h.stopped(97, &[DATA + 0x600]);
    h.stopped(149, &[TIMER, due as u32, (due >> 32) as u32, DATA + 0x600]);
}

#[test]
fn an_expiring_timer_wakes_its_waiter_and_runs_its_dpc_at_dispatch_level() {
    let mut h = Harness::new();
    let mut machine = machine(&h);
    dpc_routine(&h);
    h.success(113, &[TIMER, 1]); // synchronization timer
    h.success(107, &[DPC, CODE + 0x300, 0x1234]);
    let (lo, hi) = relative(10);
    assert_eq!(h.success(149, &[TIMER, lo, hi, DPC]), 0);
    h.code(0, |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(159),
            &[TIMER, 0, 0, 0, 0],
            CODE + 0x100,
        )
    });
    h.code(0x100, |cpu, ctx, _| ctx.memory().ret(cpu, 0));
    let outcome = machine.run(10).unwrap();
    assert!(
        matches!(outcome, RunOutcome::Complete { .. }),
        "{outcome:?}"
    );
    assert_eq!(h.kernel.clock().counter(), 100_000);
    // KiTimerExpiration: Dpc, context, expiry system time low/high.
    assert_eq!(h.get(RECORD), DPC);
    assert_eq!(h.get(RECORD + 4), 0x1234);
    assert_eq!(h.get(RECORD + 8), 100_000);
    assert_eq!(h.get(RECORD + 12), 0);
    assert_eq!(h.get(RECORD + 16), 2, "DPC runs at DISPATCH_LEVEL");
    assert_eq!(h.get(RECORD + 20), 1);
    assert_eq!(h.runtime.memory().u8(h.kernel.kpcr() + IRQL).unwrap(), 0);
    // The synchronization timer's signal was consumed by the wait.
    assert_eq!(h.get(TIMER + 4), 0);
    assert_eq!(h.runtime.memory().u8(TIMER + 3).unwrap(), 0);
    assert_eq!(h.runtime.memory().u8(DPC + 2).unwrap(), 0);
    // The DPC ran before the woken thread's slice.
    let schedule = machine.schedule();
    let dpc = schedule.iter().position(|t| *t == DPC_THREAD_ID).unwrap();
    assert_eq!(schedule[dpc + 1..], [1]);
}

#[test]
fn periodic_timers_rearm_and_due_timers_expire_immediately() {
    let mut h = Harness::new();
    let mut machine = machine(&h);
    dpc_routine(&h);
    h.success(113, &[TIMER, 0]); // notification timer
    h.success(107, &[DPC, CODE + 0x300, 0]);
    let (lo, hi) = relative(5);
    // KeSetTimerEx: 5 ms due, 5 ms period.
    assert_eq!(h.success(150, &[TIMER, lo, hi, 5, DPC]), 0);
    h.runtime
        .memory()
        .set_u64(INTERVAL, (-120_000i64) as u64)
        .unwrap();
    h.code(0, |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(99),
            &[0, 0, INTERVAL],
            CODE + 0x100,
        )
    });
    h.code(0x100, |cpu, ctx, _| ctx.memory().ret(cpu, 0));
    let outcome = machine.run(20).unwrap();
    assert!(
        matches!(outcome, RunOutcome::Complete { .. }),
        "{outcome:?}"
    );
    // Fired at 5 and 10 ms before the 12 ms sleep ended; next due at 15 ms.
    assert_eq!(h.get(RECORD + 20), 2);
    assert_eq!(h.get(TIMER + 4), 1, "notification timers stay signalled");
    assert_eq!(h.runtime.memory().u8(TIMER + 3).unwrap(), 1);
    assert_eq!(h.runtime.memory().u64(TIMER + 0x10).unwrap(), 150_000);
    assert_eq!(h.success(97, &[TIMER]), 1);
    // A due time already passed expires during the call itself.
    assert_eq!(h.success(149, &[TIMER, 0, 0, 0]), 0);
    assert_eq!(h.get(TIMER + 4), 1);
    assert_eq!(h.runtime.memory().u8(TIMER + 3).unwrap(), 0);
}

#[test]
fn a_dpc_that_blocks_stops_with_its_own_context() {
    let mut h = Harness::new();
    let mut machine = machine(&h);
    h.success(113, &[TIMER, 0]);
    h.success(107, &[DPC, CODE + 0x300, 0]);
    // Waiting is illegal at DISPATCH_LEVEL: the routine's wait must stop.
    h.code(0x300, |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(159),
            &[TIMER + 0x80, 0, 0, 0, 0],
            CODE + 0x380,
        )
    });
    h.success(108, &[TIMER + 0x80, 0, 0]);
    let (lo, hi) = relative(1);
    h.success(149, &[TIMER, lo, hi, DPC]);
    h.runtime
        .memory()
        .set_u64(INTERVAL, (-50_000i64) as u64)
        .unwrap();
    h.code(0, |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(99),
            &[0, 0, INTERVAL],
            CODE + 0x100,
        )
    });
    let outcome = machine.run(10).unwrap();
    let RunOutcome::Stopped { thread, .. } = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(thread, DPC_THREAD_ID);
    assert!(machine.dpc_thread().is_some());
    assert!(machine.retry_kernel_call().is_err());
}

#[test]
fn the_dpc_expiry_time_carries_the_system_time_high_word() {
    // An epoch past 2^32 ticks, as every real NT system time is.
    let epoch = 0x01d9_0000_1234_5678u64;
    let mut h = Harness::config(kernel::Config {
        epoch_100ns: epoch,
        ..kernel::Config::default()
    });
    let mut machine = machine(&h);
    dpc_routine(&h);
    h.success(113, &[TIMER, 0]);
    h.success(107, &[DPC, CODE + 0x300, 0]);
    let (lo, hi) = relative(10);
    assert_eq!(h.success(149, &[TIMER, lo, hi, DPC]), 0);
    h.code(0, |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(159),
            &[TIMER, 0, 0, 0, 0],
            CODE + 0x100,
        )
    });
    h.code(0x100, |cpu, ctx, _| ctx.memory().ret(cpu, 0));
    let outcome = machine.run(10).unwrap();
    assert!(
        matches!(outcome, RunOutcome::Complete { .. }),
        "{outcome:?}"
    );
    let expiry = epoch + 100_000;
    assert_eq!(h.get(RECORD + 8), expiry as u32);
    assert_eq!(h.get(RECORD + 12), (expiry >> 32) as u32);
    assert_ne!(h.get(RECORD + 12), 0);
}

/// Two notification timers at `TIMER` and `TIMER + 0x40`, a WaitAny on both
/// from thread 1 with an optional timeout, and the host clock jumped to
/// `advance_ms` while the thread is blocked. Returns the wait's status.
fn wait_any_across_a_time_jump(due_ms: [i64; 2], timeout_ms: Option<i64>, advance_ms: u64) -> u32 {
    let mut h = Harness::new();
    let mut machine = machine(&h);
    let timers = [TIMER, TIMER + 0x40];
    for (timer, ms) in timers.into_iter().zip(due_ms) {
        h.success(113, &[timer, 0]);
        let (lo, hi) = relative(ms);
        h.success(149, &[timer, lo, hi, 0]);
    }
    h.set(DATA, timers[0]);
    h.set(DATA + 4, timers[1]);
    let timeout = match timeout_ms {
        Some(ms) => {
            h.runtime
                .memory()
                .set_u64(INTERVAL, (-ms * 10_000) as u64)
                .unwrap();
            INTERVAL
        }
        None => 0,
    };
    h.code(0, move |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(158),
            &[2, DATA, 1, 0, 0, 0, timeout, 0],
            CODE + 0x100,
        )
    });
    h.code(0x100, |cpu, ctx, _| {
        ctx.memory().set_u32(DATA + 8, cpu.eax)?;
        ctx.memory().ret(cpu, 0)
    });
    assert_eq!(machine.run(1).unwrap(), RunOutcome::SliceLimit);
    h.kernel
        .advance_time(&h.runtime, advance_ms * 10_000)
        .unwrap();
    let outcome = machine.run(5).unwrap();
    assert!(
        matches!(outcome, RunOutcome::Complete { .. }),
        "{outcome:?}"
    );
    h.get(DATA + 8)
}

#[test]
fn wait_any_is_satisfied_by_the_earliest_expiry_when_time_jumps_past_several() {
    // Index 1 (10 ms) expired before index 0 (20 ms).
    assert_eq!(wait_any_across_a_time_jump([20, 10], None, 30), 1);
    assert_eq!(wait_any_across_a_time_jump([10, 20], None, 30), 0);
    // A 15 ms timeout falls between the two expiries: the 10 ms timer wins.
    assert_eq!(wait_any_across_a_time_jump([20, 10], Some(15), 30), 1);
    // A 5 ms timeout precedes both timers, so the wait times out.
    assert_eq!(
        wait_any_across_a_time_jump([20, 10], Some(5), 30),
        kernel::status::TIMEOUT
    );
}
