//! Periodic host interrupts (`kernel::periodic`), wall pacing
//! (`kernel::pacer`) and replay of the clock's coalescing mode. The
//! generated-code waits on a vblank counter and a swap are in preempt.rs.
mod common;
use common::*;
use kernel::clock::Clock;
use kernel::periodic::{Rate, Source};
use kernel::{Machine, RunOutcome};
use runtime::{CallConvention, GuestInterrupt};
use std::sync::{Arc, Mutex};

/// A Rust stand-in for the guest callback: counts calls at `COUNT` and
/// keeps the record's first word at `LAST`.
const CALLBACK: u32 = 0x800;
const COUNT: u32 = DATA + 0x100;
const LAST: u32 = DATA + 0x104;
const DELAY: u32 = DATA + 0x120;

fn attach(h: &Harness) -> Machine {
    Machine::new(
        h.runtime.clone(),
        h.kernel.clone(),
        h.cpu,
        STACK_BOTTOM,
        STACK_TOP,
    )
    .unwrap()
}

fn source(convention: CallConvention, calls: Arc<Mutex<u32>>) -> Source {
    Source {
        name: "test vblank",
        rate: Rate::HZ_60,
        max_catch_up: 4,
        handler: Box::new(move |guest, scratch, _missed| {
            let mut n = calls.lock().unwrap();
            *n += 1;
            guest.set_u32(scratch, *n)?;
            Ok(Some(GuestInterrupt {
                target: CODE + CALLBACK,
                args: vec![scratch],
                convention,
                thread: None,
                coalesce: false,
                name: "test vblank",
            }))
        }),
    }
}

/// Thread 1 sleeps `ticks`, then exits with the callback count.
fn sleeper(ticks: u64) -> Harness {
    let h = Harness::new();
    h.runtime
        .memory()
        .set_u64(DELAY, (ticks as i64).wrapping_neg() as u64)
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
        cpu.eax = ctx.memory().u32(COUNT)?;
        ctx.memory().ret(cpu, 0)
    });
    // cdecl callback(record): ret.
    h.code(CALLBACK, |cpu, ctx, _| {
        // The callback runs on the DPC context.
        assert_eq!(ctx.thread_id, kernel::threads::DPC_THREAD_ID);
        let mut m = ctx.memory();
        let record = m.u32(cpu.esp + 4)?;
        let first = m.u32(record)?;
        m.set_u32(LAST, first)?;
        let n = m.u32(COUNT)?;
        m.set_u32(COUNT, n + 1)?;
        m.ret(cpu, 0)
    });
    h
}

#[test]
fn a_sleeping_machine_steps_through_each_vblank_to_its_deadline() {
    // 100 ms asleep: vblanks at ceil(n * 166666.67) ticks for n = 1..=6,
    // each delivered at its own deadline, then the sleep ends exactly.
    let h = sleeper(1_000_000);
    let calls = Arc::new(Mutex::new(0));
    let id = h
        .kernel
        .add_periodic(source(CallConvention::Cdecl, calls.clone()))
        .unwrap();
    let mut m = attach(&h);
    let outcome = m.run(100).unwrap();
    assert_eq!(
        outcome,
        RunOutcome::Complete {
            exits: [(1, 6)].into()
        }
    );
    assert_eq!(h.get(LAST), 6);
    let clock = m.kernel.clock();
    assert_eq!(clock.counter(), 1_000_000);
    let deadlines: Vec<u64> = (1..=6)
        .map(|n: u64| (n * 10_000_000).div_ceil(60))
        .collect();
    assert_eq!(&clock.advances()[..6], &deadlines[..]);
    let stats = m.kernel.periodic_stats(id).unwrap();
    assert_eq!((stats.delivered, stats.dropped), (6, 0));
    // The IRQL the callback raised was restored for the thread.
    assert_eq!(
        h.runtime
            .memory()
            .u8(h.kernel.kpcr() + kernel::layout::kpcr::IRQL)
            .unwrap(),
        0
    );
}

#[test]
fn a_periodic_source_alone_does_not_hide_a_deadlock() {
    // An infinite wait nobody can end: vblanks would tick forever, so the
    // machine reports the deadlock instead of advancing for them.
    let mut h = Harness::new();
    let calls = Arc::new(Mutex::new(0));
    h.kernel
        .add_periodic(source(CallConvention::Cdecl, calls.clone()))
        .unwrap();
    let mut m = attach(&h);
    h.success(108, &[DATA, 0, 0]); // KeInitializeEvent
    h.code(0, |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(159), // KeWaitForSingleObject
            &[DATA, 0, 0, 0, 0],
            CODE + 0x100,
        )
    });
    assert_eq!(
        m.run(10).unwrap(),
        RunOutcome::Deadlock { threads: vec![1] }
    );
    assert_eq!(*calls.lock().unwrap(), 0);
    assert_eq!(m.kernel.clock().counter(), 0);
}

#[test]
fn a_callback_that_unbalances_the_stack_stops_on_the_dpc_context() {
    // Declared stdcall, the callback returns with a plain ret.
    let h = sleeper(1_000_000);
    let calls = Arc::new(Mutex::new(0));
    h.kernel
        .add_periodic(source(CallConvention::Stdcall, calls))
        .unwrap();
    let mut m = attach(&h);
    let RunOutcome::Stopped { thread, transfer } = m.run(100).unwrap() else {
        panic!("expected a stop");
    };
    assert_eq!(thread, kernel::threads::DPC_THREAD_ID);
    let dpc = m.dpc_thread().unwrap();
    assert!(matches!(
        dpc.ctx.event(transfer.event_id),
        Some(runtime::Event::InterruptImbalance {
            name: "test vblank",
            ..
        })
    ));
}

#[test]
fn removing_a_source_drops_what_it_owed_and_rejects_bad_rates() {
    let h = sleeper(1_000_000);
    let calls = Arc::new(Mutex::new(0));
    let id = h
        .kernel
        .add_periodic(source(CallConvention::Cdecl, calls.clone()))
        .unwrap();
    h.kernel.advance_time(&h.runtime, 400_000).unwrap();
    assert!(h.kernel.remove_periodic(id));
    assert!(!h.kernel.remove_periodic(id));
    assert_eq!(h.kernel.periodic_stats(id), None);
    let mut m = attach(&h);
    assert_eq!(
        m.run(100).unwrap(),
        RunOutcome::Complete {
            exits: [(1, 0)].into()
        }
    );
    for rate in [
        Rate { num: 0, den: 1 },
        Rate { num: 1, den: 0 },
        Rate {
            num: 20_000_000,
            den: 1,
        },
    ] {
        let mut s = source(CallConvention::Cdecl, calls.clone());
        s.rate = rate;
        assert!(h.kernel.add_periodic(s).is_err(), "{rate:?}");
    }
}

#[test]
fn a_paced_machine_waits_for_wall_time_without_changing_its_schedule() {
    // 100 ms of logical sleep takes about 100 ms of wall time paced, and
    // ends in the same state as unpaced.
    let run = |paced: bool| {
        let h = sleeper(1_000_000);
        h.kernel
            .add_periodic(source(CallConvention::Cdecl, Arc::new(Mutex::new(0))))
            .unwrap();
        let mut m = attach(&h);
        if paced {
            m.pacer = Some(kernel::pacer::Pacer::real_time());
        }
        let start = std::time::Instant::now();
        let outcome = m.run(100).unwrap();
        (
            start.elapsed(),
            outcome,
            m.kernel.clock(),
            m.schedule().to_vec(),
        )
    };
    let (fast, outcome, clock, schedule) = run(false);
    let (slow, paced_outcome, paced_clock, paced_schedule) = run(true);
    assert_eq!(
        (outcome, clock, schedule),
        (paced_outcome, paced_clock, paced_schedule)
    );
    assert!(slow >= std::time::Duration::from_millis(95), "{slow:?}");
    assert!(fast < slow);
}

#[test]
fn a_pacer_that_falls_behind_starts_counting_again() {
    let mut p = kernel::pacer::Pacer::real_time();
    p.wait(0);
    // A 150 ms host stall leaves logical time more than `max_lag` behind:
    // the pacer counts from the present instead of letting the clock hurry
    // to make the stall up, and paces the next 20 ms normally.
    std::thread::sleep(std::time::Duration::from_millis(150));
    p.wait(1);
    assert_eq!(p.reanchors(), 1);
    let start = std::time::Instant::now();
    p.wait(1 + 200_000); // 20 ms past the new origin
    let waited = start.elapsed();
    // Without the re-anchor the old origin is long past and this would not
    // sleep at all, so a lower bound proves the new origin. There is no upper
    // bound: how long a sleep overshoots is up to the host scheduler.
    assert!(waited >= std::time::Duration::from_millis(15), "{waited:?}");
    assert_eq!(p.reanchors(), 1, "the paced wait did not re-anchor again");
}

#[test]
fn resume_keeps_the_coalescing_mode_that_replay_drops() {
    // The backedge review's P3: two charges, replay, one more charge.
    let mut original = Clock::new(0);
    original.charge(2);
    original.charge(3);
    assert_eq!(original.advances(), [5]);
    assert!(original.charging());
    let mut replayed = Clock::replay(0, original.advances()).unwrap();
    let mut resumed = Clock::resume(0, original.advances(), original.charging()).unwrap();
    assert_eq!(resumed, original);
    original.charge(4);
    replayed.charge(4);
    resumed.charge(4);
    assert_eq!(original.advances(), [9]);
    assert_eq!(resumed.advances(), [9]);
    assert_eq!(resumed, original);
    // replay's narrower contract: the time, not the grouping.
    assert_eq!(replayed.counter(), 9);
    assert_eq!(replayed.advances(), [5, 9]);
    // After an explicit advance there is nothing to coalesce with.
    let mut explicit = Clock::new(0);
    explicit.advance_to(7).unwrap();
    assert!(!explicit.charging());
    let resumed = Clock::resume(0, explicit.advances(), explicit.charging()).unwrap();
    assert_eq!(resumed, explicit);
    // An empty history cannot be charging.
    assert!(!Clock::resume(0, &[], true).unwrap().charging());
}

#[test]
fn work_after_a_paced_sleep_waits_for_wall_time() {
    // A lone thread sleeps 200 ms of logical time, then acts. Paced, the
    // act (and each vblank callback on the way) happens no earlier in wall
    // time than in logical time, even when run is entered one slice at a
    // time.
    let h = sleeper(2_000_000);
    let start = std::time::Instant::now();
    let woke = Arc::new(Mutex::new(None));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let w = woke.clone();
    h.code(0x100, move |cpu, ctx, _| {
        *w.lock().unwrap() = Some(start.elapsed());
        ctx.memory().ret(cpu, 0)
    });
    let c = calls.clone();
    h.code(CALLBACK, move |cpu, ctx, _| {
        c.lock().unwrap().push(start.elapsed());
        ctx.memory().ret(cpu, 0)
    });
    h.kernel
        .add_periodic(source(CallConvention::Cdecl, Arc::new(Mutex::new(0))))
        .unwrap();
    let mut m = attach(&h);
    m.pacer = Some(kernel::pacer::Pacer::real_time());
    while !matches!(m.run(1).unwrap(), RunOutcome::Complete { .. }) {}
    let woke = woke.lock().unwrap().expect("the thread woke");
    assert!(woke >= std::time::Duration::from_millis(198), "{woke:?}");
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 12);
    for (i, at) in calls.iter().enumerate() {
        let due = std::time::Duration::from_nanos((i as u64 + 1) * 1_000_000_000 / 60);
        assert!(
            *at + std::time::Duration::from_millis(2) >= due,
            "vblank {i}: {at:?}"
        );
    }
}

#[test]
fn a_debt_past_ten_seconds_is_dropped_and_the_rest_delivered() {
    // Twenty seconds pass before the machine runs: 1200 periods due, the
    // newest 600 (ten seconds) kept and all delivered, the older 600
    // dropped and reported once. Then six more while the thread sleeps.
    let h = sleeper(1_000_000);
    let calls = Arc::new(Mutex::new(0));
    let id = h
        .kernel
        .add_periodic(source(CallConvention::Cdecl, calls.clone()))
        .unwrap();
    h.kernel.advance_time(&h.runtime, 200_000_000).unwrap();
    let mut m = attach(&h);
    assert_eq!(
        m.run(10_000).unwrap(),
        RunOutcome::Complete {
            exits: [(1, 606)].into()
        }
    );
    let stats = m.kernel.periodic_stats(id).unwrap();
    assert_eq!((stats.delivered, stats.dropped, stats.owed), (606, 600, 0));
    assert_eq!(m.kernel.clock().counter(), 201_000_000);
}
