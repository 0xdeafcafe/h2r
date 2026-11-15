mod common;
use common::*;
use kernel::status;
use runtime::Event;

#[test]
fn event_waits_preserve_notification_and_consume_synchronization_state() {
    let mut h = Harness::new();
    h.set(DATA + 0x100, 0);
    h.set(DATA + 0x104, 0); // zero LARGE_INTEGER timeout
    let timeout = DATA + 0x100;
    for kind in [0, 1] {
        h.success(108, &[DATA, kind, 0]);
        assert_eq!(h.success(159, &[DATA, 0, 0, 0, timeout]), status::TIMEOUT);
        assert_eq!(h.success(145, &[DATA, 0, 0]), 0);
        assert_eq!(h.success(159, &[DATA, 0, 0, 0, timeout]), 0);
        assert_eq!(h.get(DATA + 4), if kind == 0 { 1 } else { 0 });
        let previous = h.success(138, &[DATA]);
        assert_eq!(previous, if kind == 0 { 1 } else { 0 });
        h.stopped(145, &[DATA, 0, 1]);
        assert_eq!(h.get(DATA + 4), 0);
        h.stopped(159, &[DATA, 0, 0, 1, timeout]);
        // Immediate polling works without Machine; an actual block does not.
        h.stopped(159, &[DATA, 0, 0, 0, 0]);
    }
}

#[test]
fn semaphores_enforce_limits_and_wait_all_is_atomic() {
    let mut h = Harness::new();
    h.success(112, &[DATA, 1, 3]);
    h.success(108, &[DATA + 0x20, 1, 0]);
    h.set(DATA + 0x100, DATA);
    h.set(DATA + 0x104, DATA + 0x20);
    h.runtime.memory().set_u64(DATA + 0x110, 0).unwrap();
    let args = [2, DATA + 0x100, 0, 0, 0, 0, DATA + 0x110, 0];
    assert_eq!(h.success(158, &args), status::TIMEOUT);
    assert_eq!(h.get(DATA + 4), 1);
    h.success(145, &[DATA + 0x20, 0, 0]);
    assert_eq!(h.success(158, &args), 0);
    assert_eq!(h.get(DATA + 4), 0);
    assert_eq!(h.get(DATA + 0x24), 0);
    assert_eq!(h.success(132, &[DATA, 0, 2, 0]), 0);
    assert_eq!(h.get(DATA + 4), 2);
    h.stopped(132, &[DATA, 0, 2, 0]);
    assert_eq!(h.get(DATA + 4), 2);
    h.stopped(132, &[DATA, 0, 1, 1]);
    assert_eq!(h.get(DATA + 4), 2);
    assert_eq!(h.success(159, &[DATA, 0, 0, 0, DATA + 0x110]), 0);
    assert_eq!(h.get(DATA + 4), 1);
    h.set(DATA + 0x104, DATA);
    assert_eq!(h.success(158, &args), status::INVALID_PARAMETER_MIX);
    assert_eq!(h.get(DATA + 4), 1);
    assert_eq!(
        h.success(158, &[0, DATA, 0, 0, 0, 0, 0, 0]),
        status::INVALID_PARAMETER
    );
}

#[test]
fn handle_events_semaphores_duplicates_and_type_errors() {
    let mut h = Harness::new();
    h.runtime.memory().set_u64(DATA + 0x100, 0).unwrap();
    assert_eq!(h.success(189, &[DATA, 0, 1, 0]), 0);
    let event = h.get(DATA);
    assert_eq!(h.success(225, &[event, DATA + 0x20]), 0);
    assert_eq!(h.get(DATA + 0x20), 0);
    assert_eq!(h.success(233, &[event, 0, DATA + 0x100]), 0);
    assert_eq!(
        h.success(234, &[event, 0, 0, DATA + 0x100]),
        status::TIMEOUT
    );
    h.success(225, &[event, 0]);
    assert_eq!(h.success(197, &[event, DATA + 4, 2]), 0);
    let duplicate = h.get(DATA + 4);
    assert_ne!(event, duplicate);
    assert_eq!(h.success(187, &[event]), 0);
    assert_eq!(h.success(233, &[event, 0, 0]), status::INVALID_HANDLE);
    assert_eq!(h.success(233, &[duplicate, 0, DATA + 0x100]), 0);
    assert_eq!(h.success(193, &[DATA + 8, 0, 1, 3]), 0);
    let semaphore = h.get(DATA + 8);
    assert_eq!(
        h.success(225, &[semaphore, 0]),
        status::OBJECT_TYPE_MISMATCH
    );
    assert_eq!(
        h.success(222, &[duplicate, 1, 0]),
        status::OBJECT_TYPE_MISMATCH
    );
    assert_eq!(h.success(222, &[semaphore, 2, DATA + 0x20]), 0);
    assert_eq!(h.get(DATA + 0x20), 1);
    assert_eq!(
        h.success(222, &[semaphore, 1, 0]),
        status::SEMAPHORE_LIMIT_EXCEEDED
    );
    assert_eq!(
        h.success(222, &[semaphore, 0, 0]),
        status::INVALID_PARAMETER
    );
    h.set(DATA + 0x80, duplicate);
    h.set(DATA + 0x84, semaphore);
    assert_eq!(h.success(235, &[2, DATA + 0x80, 1, 0, 0, DATA + 0x100]), 1);
    assert_eq!(h.success(197, &[duplicate, DATA + 12, 1]), 0);
    assert_eq!(h.success(187, &[duplicate]), status::INVALID_HANDLE);
    assert_eq!(h.success(187, &[h.get(DATA + 12)]), 0);
    assert_eq!(h.success(187, &[semaphore]), 0);
    assert_eq!(h.success(224, &[semaphore, 0]), status::INVALID_HANDLE);
    assert_eq!(h.success(225, &[event, 0]), status::INVALID_HANDLE);
    assert_eq!(h.success(197, &[event, DATA, 0]), status::INVALID_HANDLE);
    assert_eq!(h.success(189, &[DATA, 0, 9, 0]), status::INVALID_PARAMETER);
    assert_eq!(h.success(193, &[DATA, 0, 4, 3]), status::INVALID_PARAMETER);
    h.set(DATA + 0x50, 0);
    h.set(DATA + 0x54, DATA + 0x60);
    h.set(DATA + 0x58, 0);
    h.stopped(189, &[DATA, DATA + 0x50, 0, 0]);
}

#[test]
fn critical_sections_mirror_guest_layout_and_enforce_recursive_ownership() {
    let mut h = Harness::new();
    let machine = kernel::Machine::new(
        h.runtime.clone(),
        h.kernel.clone(),
        h.cpu,
        STACK_BOTTOM,
        STACK_TOP,
    )
    .unwrap();
    h.success(291, &[DATA]);
    assert_eq!(h.get(DATA + 16), u32::MAX);
    h.success(277, &[DATA]);
    h.success(277, &[DATA]);
    assert_eq!(h.get(DATA + 16), 1);
    assert_eq!(h.get(DATA + 20), 2);
    assert_eq!(h.get(DATA + 24), machine.thread(1).unwrap().object);
    h.ctx.thread_id = 2;
    assert_eq!(h.success(306, &[DATA]), 0);
    h.stopped(294, &[DATA]);
    h.stopped(291, &[DATA]);
    h.ctx.thread_id = 1;
    h.success(294, &[DATA]);
    assert_eq!(h.get(DATA + 20), 1);
    h.success(294, &[DATA]);
    assert_eq!(h.get(DATA + 16), u32::MAX);
    assert_eq!(h.get(DATA + 24), 0);
    assert_eq!(h.success(306, &[DATA]), 1);
    h.success(294, &[DATA]);
    let before = h.get(DATA + 16);
    let t = h.call(291, &[0xdead0000]);
    assert!(matches!(
        h.ctx.event(t.event_id),
        Some(Event::MemoryFault { .. })
    ));
    assert_eq!(h.get(DATA + 16), before);
}

#[test]
fn closing_last_object_handle_reuses_backing_without_reusing_handle_identity() {
    let mut h = Harness::new();
    let mut last = 0;
    h.success(189, &[DATA, 0, 0, 0]);
    let first = h.get(DATA);
    h.success(187, &[first]);
    let mapping_count = h.runtime.memory().mappings().len();
    for _ in 0..100 {
        assert_eq!(h.success(193, &[DATA, 0, 0, 1]), 0);
        let handle = h.get(DATA);
        assert!(handle > last);
        last = handle;
        assert_eq!(h.success(187, &[handle]), 0);
    }
    assert_eq!(h.runtime.memory().mappings().len(), mapping_count);
    assert_eq!(h.success(187, &[first]), status::INVALID_HANDLE);
}

#[test]
fn semaphore_header_at_the_end_of_the_address_space_faults_instead_of_overflowing() {
    let mut h = Harness::new();
    h.runtime
        .memory()
        .map(0xffff_f000, 4096, 0x100000, runtime::guest::Permissions::RW)
        .unwrap();
    // A readable 16-byte semaphore header whose limit word would be at 2^32.
    h.bytes(0xffff_fff0, &[5, 0, 5, 0]);
    let t = h.call(132, &[0xffff_fff0, 0, 1, 0]);
    assert!(matches!(
        h.ctx.event(t.event_id),
        Some(Event::MemoryFault { .. })
    ));
    let t = h.call(159, &[0xffff_fff0, 0, 0, 0, 0]);
    assert!(matches!(
        h.ctx.event(t.event_id),
        Some(Event::MemoryFault { .. })
    ));
}

mod mutants {
    use super::*;
    use kernel::{Machine, RunOutcome};
    use runtime::kernel::function_token as token;

    const MUTANT: u32 = DATA + 0x900;
    const PREVIOUS: u32 = DATA + 0x904;
    const CHILD: u32 = DATA + 0x908;
    const OBJECT: u32 = DATA + 0x90c;
    const LOG: u32 = DATA + 0x910;
    const CURRENT_THREAD: u32 = 0x28;

    /// The KTHREAD of the running thread (FS:[0x28]).
    fn current(cpu: &runtime::CpuAbi, ctx: &mut runtime::ExecCtx) -> u32 {
        ctx.memory().u32(cpu.fs_base + CURRENT_THREAD).unwrap()
    }
    fn mutant(ctx: &mut runtime::ExecCtx) -> (i32, u32, u8, u32) {
        let m = ctx.memory();
        let at = m.u32(OBJECT).unwrap();
        (
            m.u32(at + 4).unwrap() as i32,
            m.u32(at + 0x18).unwrap(),
            m.u8(at + 0x1c).unwrap(),
            at,
        )
    }

    #[test]
    fn mutants_are_recursive_owned_released_and_abandoned_on_thread_exit() {
        let h = Harness::new();
        // Thread 1: create owned, inspect, recurse, start a contender.
        h.code(0, |cpu, ctx, _| {
            guest_call(cpu, ctx, token(192), &[MUTANT, 0, 1], CODE + 0x10)
        });
        h.code(0x10, |cpu, ctx, _| {
            assert_eq!(cpu.eax, 0);
            let handle = ctx.memory().u32(MUTANT)?;
            guest_call(cpu, ctx, token(246), &[handle, 0, OBJECT], CODE + 0x20)
        });
        h.code(0x20, |cpu, ctx, _| {
            assert_eq!(cpu.eax, 0);
            let (signal, owner, abandoned, at) = mutant(ctx);
            let me = current(cpu, ctx);
            {
                let m = ctx.memory();
                assert_eq!(m.read::<4>(at)?, [2, 0, 8, 0], "KMUTANT header");
                assert_eq!((signal, owner, abandoned), (0, me, 0));
                // Linked into the owner's MutantListHead.
                assert_eq!(m.u32(me + 0x10)?, at + 0x10);
                assert_eq!(m.u32(at + 0x10)?, me + 0x10);
            }
            cpu.ecx = at;
            guest_call(cpu, ctx, token(250), &[], CODE + 0x30)
        });
        h.code(0x30, |cpu, ctx, _| {
            let handle = ctx.memory().u32(MUTANT)?;
            guest_call(cpu, ctx, token(234), &[handle, 0, 0, 0], CODE + 0x40)
        });
        h.code(0x40, |cpu, ctx, _| {
            assert_eq!(cpu.eax, 0, "the owner reacquires without blocking");
            assert_eq!(mutant(ctx).0, -1);
            guest_call(
                cpu,
                ctx,
                token(255),
                &[CHILD, 0, 0x4000, 0, 0, CODE + 0x300, 0, 0, 0, 0],
                CODE + 0x50,
            )
        });
        h.code(0x50, |cpu, ctx, _| {
            assert_eq!(cpu.eax, 0);
            let handle = ctx.memory().u32(MUTANT)?;
            guest_call(cpu, ctx, token(221), &[handle, PREVIOUS], CODE + 0x60)
        });
        h.code(0x60, |cpu, ctx, _| {
            assert_eq!(cpu.eax, 0);
            assert_eq!(ctx.memory().u32(PREVIOUS)? as i32, -1);
            guest_call(cpu, ctx, token(238), &[], CODE + 0x70)
        });
        h.code(0x70, |cpu, ctx, _| {
            // The child tried a release (not owner) and now waits.
            assert_eq!(ctx.memory().u32(LOG)?, 1);
            let handle = ctx.memory().u32(MUTANT)?;
            guest_call(cpu, ctx, token(221), &[handle, PREVIOUS], CODE + 0x80)
        });
        h.code(0x80, |cpu, ctx, _| {
            assert_eq!(cpu.eax, 0);
            assert_eq!(ctx.memory().u32(PREVIOUS)?, 0);
            // The final release handed it straight to the waiting child.
            let (signal, owner, _, _) = mutant(ctx);
            assert_eq!(signal, 0);
            assert_ne!(owner, current(cpu, ctx));
            assert_ne!(owner, 0);
            let child = ctx.memory().u32(CHILD)?;
            guest_call(cpu, ctx, token(233), &[child, 0, 0], CODE + 0x90)
        });
        h.code(0x90, |cpu, ctx, _| {
            assert_eq!(cpu.eax, 0);
            // The child exited owning it: free, and marked abandoned.
            let (signal, owner, abandoned, _) = mutant(ctx);
            assert_eq!((signal, owner, abandoned), (1, 0, 1));
            let handle = ctx.memory().u32(MUTANT)?;
            guest_call(cpu, ctx, token(234), &[handle, 0, 0, 0], CODE + 0xa0)
        });
        h.code(0xa0, |cpu, ctx, _| {
            assert_eq!(cpu.eax, 0x80, "STATUS_ABANDONED_WAIT_0");
            let (signal, owner, abandoned, _) = mutant(ctx);
            assert_eq!((signal, owner, abandoned), (0, current(cpu, ctx), 0));
            let handle = ctx.memory().u32(MUTANT)?;
            guest_call(cpu, ctx, token(221), &[handle, 0], CODE + 0xb0)
        });
        h.code(0xb0, |cpu, ctx, _| {
            assert_eq!(cpu.eax, 0);
            let (signal, owner, _, _) = mutant(ctx);
            assert_eq!((signal, owner), (1, 0));
            let me = current(cpu, ctx);
            assert_eq!(ctx.memory().u32(me + 0x10)?, me + 0x10, "list empty again");
            cpu.eax = 0;
            ctx.memory().ret(cpu, 0)
        });
        // Thread 2: StartRoutine(Context).
        h.code(0x300, |cpu, ctx, _| {
            let handle = ctx.memory().u32(MUTANT)?;
            guest_call(cpu, ctx, token(221), &[handle, 0], CODE + 0x310)
        });
        h.code(0x310, |cpu, ctx, _| {
            assert_eq!(cpu.eax, status::MUTANT_NOT_OWNED);
            ctx.memory().set_u32(LOG, 1)?;
            let handle = ctx.memory().u32(MUTANT)?;
            guest_call(cpu, ctx, token(234), &[handle, 0, 0, 0], CODE + 0x320)
        });
        h.code(0x320, |cpu, ctx, _| {
            assert_eq!(cpu.eax, 0);
            ctx.memory().ret(cpu, 4)
        });
        let mut machine = Machine::new(
            h.runtime.clone(),
            h.kernel.clone(),
            h.cpu,
            STACK_BOTTOM,
            STACK_TOP,
        )
        .unwrap();
        let outcome = machine.run(50).unwrap();
        assert!(
            matches!(outcome, RunOutcome::Complete { .. }),
            "{outcome:?}"
        );
        assert!(h.kernel.stop_reasons().is_empty());
    }

    /// MutantListEntry links are guest memory. A Flink whose Blink field
    /// would sit past 0xffffffff faults the release instead of wrapping, and
    /// the mutant stays owned with its count unchanged.
    #[test]
    fn mutant_release_with_an_overflowing_list_link_faults_without_changes() {
        let h = Harness::new();
        h.code(0, |cpu, ctx, _| {
            guest_call(cpu, ctx, token(192), &[MUTANT, 0, 1], CODE + 0x10)
        });
        h.code(0x10, |cpu, ctx, _| {
            assert_eq!(cpu.eax, 0);
            let handle = ctx.memory().u32(MUTANT)?;
            guest_call(cpu, ctx, token(246), &[handle, 0, OBJECT], CODE + 0x20)
        });
        h.code(0x20, |cpu, ctx, _| {
            assert_eq!(cpu.eax, 0);
            let at = mutant(ctx).3;
            ctx.memory().set_u32(at + 0x10, 0xffff_ffff)?;
            let handle = ctx.memory().u32(MUTANT)?;
            guest_call(cpu, ctx, token(221), &[handle, PREVIOUS], CODE + 0x30)
        });
        h.code(0x30, |_, _, _| panic!("the release must fault"));
        let mut machine = Machine::new(
            h.runtime.clone(),
            h.kernel.clone(),
            h.cpu,
            STACK_BOTTOM,
            STACK_TOP,
        )
        .unwrap();
        let outcome = machine.run(20).unwrap();
        let RunOutcome::Stopped { thread, transfer } = outcome else {
            panic!("{outcome:?}");
        };
        assert!(matches!(
            machine.thread(thread).unwrap().ctx.event(transfer.event_id),
            Some(Event::MemoryFault { .. })
        ));
        let m = h.runtime.memory();
        let at = m.u32(OBJECT).unwrap();
        let owner = m.u32(at + 0x18).unwrap();
        assert_ne!(owner, 0, "still owned");
        assert_eq!(m.u32(at + 4).unwrap(), 0, "count unchanged");
        assert_eq!(m.u32(at + 0x10).unwrap(), 0xffff_ffff);
        assert_eq!(m.u32(owner + 0x10).unwrap(), at + 0x10, "list unchanged");
    }

    #[test]
    fn mutant_handles_type_checks_and_unsupported_forms() {
        let mut h = Harness::new();
        // Unowned creation needs no thread; release by a non-owner fails.
        assert_eq!(h.success(192, &[MUTANT, 0, 0]), 0);
        let handle = h.get(MUTANT);
        assert_eq!(
            h.success(221, &[handle, PREVIOUS]),
            status::MUTANT_NOT_OWNED
        );
        // An unowned mutant is signalled: an immediate wait with no thread
        // cannot own it, so it stops rather than inventing an owner.
        h.set(DATA + 0x100, 0);
        h.set(DATA + 0x104, 0);
        h.stopped(234, &[handle, 0, 0, DATA + 0x100]);
        // Initial ownership needs a KTHREAD; names need a namespace.
        h.stopped(192, &[MUTANT, 0, 1]);
        h.set(DATA + 0x200, 0);
        h.set(DATA + 0x204, DATA + 0x210);
        h.set(DATA + 0x208, 0x40);
        h.stopped(192, &[MUTANT, DATA + 0x200, 0]);
        // Wrong object types.
        assert_eq!(h.success(189, &[DATA + 0x300, 0, 0, 0]), 0);
        let event = h.get(DATA + 0x300);
        assert_eq!(h.success(221, &[event, 0]), status::OBJECT_TYPE_MISMATCH);
        assert_eq!(
            h.success(222, &[handle, 1, 0]),
            status::OBJECT_TYPE_MISMATCH
        );
        assert_eq!(h.success(221, &[0x7ff0, 0]), status::INVALID_HANDLE);
        // RtlNtStatusToDosError knows ERROR_NOT_OWNER.
        assert_eq!(h.success(301, &[status::MUTANT_NOT_OWNED]), 288);
        assert_eq!(h.success(187, &[handle]), 0);
    }
}
