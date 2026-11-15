//! Regression probes from the independent kernel review.
mod common;
use common::*;
use kernel::{Machine, RunOutcome, status};

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

#[test]
fn zeroed_ram_and_wrong_header_sizes_are_not_dispatcher_objects() {
    let mut h = Harness::new();
    let _machine = attach(&h);
    h.stopped(159, &[DATA, 0, 0, 0, 0]);
    h.stopped(145, &[DATA, 0, 0]);
    h.stopped(138, &[DATA]);
    for kind in [0, 1, 5] {
        h.bytes(DATA, &[kind, 0, 3, 0]);
        h.set(DATA + 4, 1);
        h.set(DATA + 16, 3);
        h.stopped(159, &[DATA, 0, 0, 0, 0]);
        h.stopped(145, &[DATA, 0, 0]);
    }
}

#[test]
fn inline_reinitialization_does_not_consume_a_notification_event_as_a_semaphore() {
    let mut h = Harness::new();
    h.success(112, &[DATA, 1, 3]);
    h.bytes(DATA, &[0, 0, 4, 0]);
    h.set(DATA + 4, 1);
    h.runtime.memory().set_u64(DATA + 0x100, 0).unwrap();
    assert_eq!(h.success(159, &[DATA, 0, 0, 0, DATA + 0x100]), 0);
    assert_eq!(h.get(DATA + 4), 1);
    h.set(DATA + 4, 1);
    h.bytes(DATA, &[0, 0, 0, 0]);
    h.stopped(138, &[DATA]);
}

#[test]
fn ke_release_semaphore_errors_never_return_ntstatus_as_a_count() {
    let mut h = Harness::new();
    h.success(108, &[DATA, 0, 0]);
    h.stopped(132, &[DATA, 0, 1, 0]);
    h.success(112, &[DATA, 3, 3]);
    h.stopped(132, &[DATA, 0, 1, 0]);
    assert_eq!(h.get(DATA + 4), 3);
}

#[test]
fn wait_any_duplicates_choose_the_first_index_but_wait_all_rejects_them() {
    let mut h = Harness::new();
    h.success(108, &[DATA, 1, 1]);
    h.set(DATA + 0x40, DATA);
    h.set(DATA + 0x44, DATA);
    h.runtime.memory().set_u64(DATA + 0x80, 0).unwrap();
    assert_eq!(
        h.success(158, &[2, DATA + 0x40, 1, 0, 0, 0, DATA + 0x80, 0]),
        0
    );
    assert_eq!(h.get(DATA + 4), 0);
    assert_eq!(
        h.success(158, &[2, DATA + 0x40, 0, 0, 0, 0, DATA + 0x80, 0]),
        status::INVALID_PARAMETER_MIX
    );
}

#[test]
fn current_thread_pseudo_handle_can_be_duplicated_and_closed_without_terminating() {
    let mut h = Harness::new();
    let machine = attach(&h);
    assert_eq!(h.success(197, &[0xffff_fffe, DATA, 1]), 0);
    let handle = h.get(DATA);
    assert_ne!(handle, 0xffff_fffe);
    assert_eq!(h.success(224, &[handle, DATA + 4]), 0);
    assert_eq!(h.get(DATA + 4), 0);
    assert_eq!(h.success(187, &[0xffff_fffe]), 0);
    assert_eq!(h.success(104, &[]), machine.thread(1).unwrap().object);
    assert_eq!(h.success(187, &[handle]), 0);
}

#[test]
fn critical_section_owner_is_the_kthread_pointer() {
    let mut h = Harness::new();
    let machine = attach(&h);
    h.success(291, &[DATA]);
    h.success(277, &[DATA]);
    assert_eq!(h.get(DATA + 24), machine.thread(1).unwrap().object);
    h.success(294, &[DATA]);
    assert_eq!(h.get(DATA + 24), 0);
}

#[test]
fn unsupported_valid_allocation_flags_stop_instead_of_reporting_invalid_parameter() {
    let mut h = Harness::new();
    h.set(DATA, 0);
    h.set(DATA + 4, 4096);
    // MEM_NOZERO. MEM_TOP_DOWN (0x100000) is implemented; see tests/memory.rs.
    h.stopped(184, &[DATA, 0, DATA + 4, 0x3000 | 0x800000, 4]);
    assert_eq!(h.get(DATA), 0);
    assert_eq!(h.get(DATA + 4), 4096);
}

#[test]
fn child_stack_has_an_unmapped_guard_that_other_allocations_cannot_fill() {
    let mut h = Harness::new();
    let mut machine = attach(&h);
    h.code(0, |cpu, ctx, _| {
        cpu.eax = 0;
        ctx.memory().ret(cpu, 0)
    });
    h.success(255, &[DATA, 0, 0x4000, 0, 0, CODE + 0x200, 0, 1, 0, 0]);
    assert_eq!(
        machine.run(10).unwrap(),
        RunOutcome::Deadlock { threads: vec![2] }
    );
    let child = machine.thread(2).unwrap();
    let bottom = h.get(child.object + 0x20);
    let before = h.runtime.memory().read::<320>(child.object).unwrap();
    assert!(h.runtime.memory().set_u32(bottom - 4, 0xbad).is_err());
    for _ in 0..8 {
        assert_ne!(h.success(14, &[16]), 0);
    }
    assert!(h.runtime.memory().u32(bottom - 4).is_err());
    assert_eq!(
        h.runtime.memory().read::<320>(child.object).unwrap(),
        before
    );
}

fn polling_probe(use_yield: bool) {
    let h = Harness::new();
    let mut machine = attach(&h);
    h.runtime.memory().set_u64(DATA + 0x100, 0).unwrap();
    h.runtime
        .memory()
        .set_u64(DATA + 0x108, (-10000i64) as u64)
        .unwrap();
    h.code(0, |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(255),
            &[DATA, 0, 0x4000, 0, 0, CODE + 0x200, 0, 0, 0, 0],
            CODE + 0x100,
        )
    });
    h.code(0x100, move |cpu, ctx, _| {
        if ctx.memory().u32(DATA + 0x80)? != 0 {
            cpu.eax = 0;
            return ctx.memory().ret(cpu, 0);
        }
        if use_yield {
            guest_call(
                cpu,
                ctx,
                runtime::kernel::function_token(238),
                &[],
                CODE + 0x100,
            )
        } else {
            guest_call(
                cpu,
                ctx,
                runtime::kernel::function_token(99),
                &[0, 0, DATA + 0x100],
                CODE + 0x100,
            )
        }
    });
    h.code(0x200, |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(99),
            &[0, 0, DATA + 0x108],
            CODE + 0x210,
        )
    });
    h.code(0x210, |cpu, ctx, _| {
        ctx.memory().set_u32(DATA + 0x80, 1)?;
        ctx.memory().ret(cpu, 4)
    });
    assert_eq!(
        machine.run(30).unwrap(),
        RunOutcome::Complete {
            exits: [(1, 0), (2, 0)].into()
        }
    );
    assert_eq!(h.kernel.clock().counter(), 10000);
}
#[test]
fn zero_delay_polling_does_not_starve_a_timed_child() {
    polling_probe(false);
}
#[test]
fn yield_polling_does_not_starve_a_timed_child() {
    polling_probe(true);
}

#[test]
fn clock_snapshots_share_history_and_advance_history_is_bounded() {
    let h = Harness::new();
    h.kernel.advance_time(&h.runtime, 1).unwrap();
    let a = h.kernel.clock();
    let b = h.kernel.clock();
    assert_eq!(a.advances().as_ptr(), b.advances().as_ptr());
    let mut clock = kernel::clock::Clock::new(0);
    for _ in 0..kernel::clock::MAX_CLOCK_ADVANCES * 3 {
        clock.advance(1).unwrap();
    }
    assert!(clock.advances().len() <= kernel::clock::MAX_CLOCK_ADVANCES);
    assert!(clock.dropped_advances() > 0);
    assert_eq!(
        kernel::clock::Clock::replay(0, clock.advances())
            .unwrap()
            .counter(),
        clock.counter()
    );
    h.kernel.advance_time(&h.runtime, 1).unwrap();
    assert_eq!(a.counter(), 1);
    assert_eq!(a.advances(), &[1]);
}

#[test]
fn cache_attributes_use_coherent_ram_but_remain_visible_in_protection_queries() {
    let mut h = Harness::new();
    for protection in [0x204, 0x404] {
        let address = h.success(166, &[4096, 0, u32::MAX, 4096, protection]);
        assert_ne!(address, 0);
        h.set(address, 123);
        assert_eq!(h.get(address), 123);
        assert_eq!(h.success(179, &[address]), protection);
        h.set(DATA, 0);
        h.set(DATA + 4, 4096);
        assert_eq!(h.success(184, &[DATA, 0, DATA + 4, 0x3000, protection]), 0);
        let vm = h.get(DATA);
        h.set(vm, 456);
        assert_eq!(h.get(vm), 456);
        assert_eq!(h.success(179, &[vm]), protection);
    }
}
