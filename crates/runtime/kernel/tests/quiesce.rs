//! Machine::quiescent over synthetic guests that really run: what a
//! snapshot probe sees between slices (docs/runtime/snapshots.md).
mod common;
use common::*;
use kernel::quiesce::{Blocker, THREAD_RETURN};
use kernel::{Machine, Quiescence, RunOutcome};

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

/// Run one slice at a time, asking after each, until the machine ends.
fn observe(machine: &mut Machine) -> (Vec<Quiescence>, RunOutcome) {
    let mut seen = Vec::new();
    for _ in 0..200 {
        let outcome = machine.run(1).unwrap();
        if !matches!(outcome, RunOutcome::SliceLimit) {
            return (seen, outcome);
        }
        seen.push(machine.quiescent());
    }
    panic!("machine did not finish");
}

/// KeDelayExecutionThread(KernelMode, FALSE, &interval) returning to `then`.
fn delay(
    cpu: &mut runtime::CpuAbi,
    ctx: &mut runtime::ExecCtx,
    then: u32,
) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    guest_call(
        cpu,
        ctx,
        runtime::kernel::function_token(99),
        &[0, 0, DATA + 0x120],
        then,
    )
}

fn harness() -> Harness {
    let h = Harness::new();
    // A 2 ms relative interval.
    h.runtime
        .memory()
        .set_u64(DATA + 0x120, (-20_000i64) as u64)
        .unwrap();
    h
}

#[test]
fn threads_waiting_in_the_kernel_are_quiescent() {
    let h = harness();
    let mut machine = attach(&h);
    // Thread 1 creates thread 2 (PsCreateSystemThreadEx), then both wait.
    h.code(0, |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(255),
            &[DATA, 0, 0x4000, 0, DATA + 4, CODE + 0x200, 0, 0, 0, 0],
            CODE + 0x100,
        )
    });
    h.code(0x100, |cpu, ctx, _| delay(cpu, ctx, CODE + 0x110));
    h.code(0x110, |cpu, ctx, _| delay(cpu, ctx, CODE + 0x120));
    h.code(0x120, |cpu, ctx, _| {
        cpu.eax = 0;
        ctx.memory().ret(cpu, 0)
    });
    h.code(0x200, |cpu, ctx, _| delay(cpu, ctx, CODE + 0x210));
    // PsTerminateSystemThread.
    h.code(0x210, |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(258),
            &[0],
            CODE + 0x220,
        )
    });
    let (seen, outcome) = observe(&mut machine);
    assert!(
        matches!(outcome, RunOutcome::Complete { .. }),
        "{outcome:?}"
    );
    assert!(!seen.is_empty());
    for q in &seen {
        assert!(q.is_quiescent(), "{q:?}");
    }
    // While thread 2 lived, its return continuation was listed as
    // rebuildable, not as a blocker.
    assert!(seen.iter().any(|q| {
        q.rebuildable
            .iter()
            .any(|(t, c)| *t == 2 && c.origin == THREAD_RETURN)
    }));
}

#[test]
fn a_thread_blocked_inside_an_override_callback_is_a_blocker_until_it_returns() {
    let h = harness();
    let mut machine = attach(&h);
    // An override that calls guest code and waits for it, as a feature's
    // `after` hook or an HLE callback does; the guest code blocks.
    h.code(0, |cpu, ctx, _| {
        let rt = std::sync::Arc::clone(ctx.runtime());
        ctx.call_guest_named(
            cpu,
            &mut rt.memory(),
            CODE + 0x400,
            "test: override callback",
            |cpu, ctx| {
                cpu.eax = 0;
                ctx.memory().ret(cpu, 0)
            },
        )
    });
    h.code(0x400, |cpu, ctx, _| delay(cpu, ctx, CODE + 0x410));
    h.code(0x410, |cpu, ctx, _| ctx.memory().ret(cpu, 0));
    let (seen, outcome) = observe(&mut machine);
    assert!(
        matches!(outcome, RunOutcome::Complete { .. }),
        "{outcome:?}"
    );
    let blocked: Vec<_> = seen.iter().filter(|q| !q.is_quiescent()).collect();
    assert!(!blocked.is_empty(), "{seen:?}");
    for q in blocked {
        assert_eq!(
            q.blockers,
            vec![Blocker::Continuation {
                thread: 1,
                origin: "test: override callback",
                target: CODE + 0x400,
            }]
        );
    }
}
