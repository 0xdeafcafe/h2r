mod common;
use common::*;
use kernel::{Machine, RunOutcome, status};
use runtime::Event;

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
fn startup_255_system_routine_tls_delay_thread_return_and_join() {
    let tls = xbe::TlsDirectory {
        raw_start: DATA + 0x100,
        raw_end: DATA + 0x104,
        index_addr: DATA + 0x108,
        callbacks_addr: 0,
        zero_fill: 0x8c,
        characteristics: 0,
    };
    let mut config = kernel::Config {
        tls: Some(tls),
        ..kernel::Config::default()
    };
    config.data_exports.insert(156, DATA + 0x110);
    let h = Harness::config(config);
    h.bytes(DATA + 0x100, &[1, 2, 3, 4]);
    h.set(tls.index_addr, 0xffff_ffdb);
    h.runtime
        .memory()
        .set_u64(DATA + 0x120, (-20_000i64) as u64)
        .unwrap();
    let mut machine = attach(&h);
    // Attaching must preserve the guest's already computed index, not replace it.
    assert_eq!(h.get(tls.index_addr), 0xffff_ffdb);
    let entry_tls = machine.thread(1).unwrap().tls;
    assert_eq!(h.get(entry_tls), entry_tls + 4);
    assert_eq!((entry_tls + 4) % 16, 0);
    h.code(0, |cpu, ctx, _| {
        cpu.ebx = 0xabcdef;
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(255),
            &[
                DATA,
                0,
                0x4000,
                0x94,
                DATA + 4,
                CODE + 0x300,
                DATA + 0x80,
                0,
                0,
                CODE + 0x200,
            ],
            CODE + 0x100,
        )
    });
    h.code(0x100, |cpu, ctx, _| {
        assert_eq!(cpu.eax, status::SUCCESS);
        assert_eq!(cpu.ebx, 0xabcdef);
        let handle = ctx.memory().u32(DATA)?;
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(233),
            &[handle, 0, 0],
            CODE + 0x110,
        )
    });
    h.code(0x110, |cpu, ctx, _| {
        assert_eq!(cpu.eax, 0);
        assert_eq!(cpu.ebx, 0xabcdef);
        assert_eq!(ctx.memory().u32(DATA + 0x80)?, 42);
        cpu.eax = 0;
        ctx.memory().ret(cpu, 0)
    });
    h.code(0x200, |cpu, ctx, _| {
        assert_eq!(ctx.thread_id, 2);
        assert_eq!(cpu.ebx, 0);
        assert_eq!(cpu.x87.control, 0x027f);
        assert_eq!(cpu.mxcsr, 0x1f80);
        let (start, context) = {
            let m = ctx.memory();
            let object = m.u32(cpu.fs_base + 0x28)?;
            let tls = m.u32(object + 0x28)?;
            let payload = m.u32(tls)?;
            assert_eq!(payload, tls + 4);
            assert_eq!(payload % 16, 0);
            assert_eq!(m.read::<4>(payload)?, [1, 2, 3, 4]);
            assert_eq!(m.u32(payload + 4)?, 0);
            let tib_top = m.u32(cpu.fs_base + 4)?;
            assert_eq!(tib_top.wrapping_add(0xffff_ffdbu32.wrapping_mul(4)), tls);
            assert_eq!(m.u32(cpu.fs_base)?, u32::MAX);
            (m.u32(cpu.esp + 4)?, m.u32(cpu.esp + 8)?)
        };
        guest_call(cpu, ctx, start, &[context], CODE + 0x210)
    });
    h.code(0x210, |cpu, ctx, _| ctx.memory().ret(cpu, 8));
    h.code(0x300, |cpu, ctx, _| {
        assert_eq!(ctx.memory().u32(cpu.esp + 4)?, DATA + 0x80);
        cpu.ebx = 0x123456;
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(99),
            &[0, 0, DATA + 0x120],
            CODE + 0x310,
        )
    });
    h.code(0x310, |cpu, ctx, _| {
        assert_eq!(cpu.eax, 0);
        assert_eq!(cpu.ebx, 0x123456);
        let context = ctx.memory().u32(cpu.esp + 4)?;
        ctx.memory().set_u32(context, 42)?;
        ctx.memory().ret(cpu, 4)
    });
    let result = machine.run(20).unwrap();
    assert_eq!(
        result,
        RunOutcome::Complete {
            exits: [(1, 0), (2, 0)].into()
        }
    );
    assert_eq!(machine.schedule(), &[1, 2, 2, 1]);
    assert_eq!(h.kernel.clock().counter(), 20_000);
    assert_eq!(h.get(DATA + 0x110), 2);
    let object = machine.thread(2).unwrap().object;
    assert_eq!(h.get(object + 4), 1);
    assert_eq!(h.get(object + 0x120), 0);
    assert_ne!(
        machine.thread(1).unwrap().tls,
        machine.thread(2).unwrap().tls
    );
    assert_eq!(machine.thread(1).unwrap().cpu.ebx, 0xabcdef);
}

#[test]
fn suspended_start_resume_yield_and_noreturn_termination() {
    let h = Harness::new();
    let mut machine = attach(&h);
    h.code(0, |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(255),
            &[DATA, 0, 0x4000, 0, DATA + 4, CODE + 0x200, 0, 1, 0, 0],
            CODE + 0x100,
        )
    });
    h.code(0x100, |cpu, ctx, _| {
        assert_eq!(ctx.memory().u32(DATA + 0x80)?, 0);
        let handle = ctx.memory().u32(DATA)?;
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(224),
            &[handle, DATA + 8],
            CODE + 0x110,
        )
    });
    h.code(0x110, |cpu, ctx, _| {
        assert_eq!(ctx.memory().u32(DATA + 8)?, 1);
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(238),
            &[],
            CODE + 0x120,
        )
    });
    h.code(0x120, |cpu, ctx, _| {
        assert_eq!(cpu.eax, 0);
        assert_eq!(ctx.memory().u32(DATA + 0x80)?, 1);
        let handle = ctx.memory().u32(DATA)?;
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(234),
            &[handle, 0, 0, 0],
            CODE + 0x130,
        )
    });
    h.code(0x130, |cpu, ctx, _| {
        assert_eq!(cpu.eax, 0);
        cpu.eax = 0;
        ctx.memory().ret(cpu, 0)
    });
    h.code(0x200, |cpu, ctx, _| {
        ctx.memory().set_u32(DATA + 0x80, 1)?;
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(258),
            &[42],
            CODE + 0x210,
        )
    });
    h.code(0x210, |_, _, _| panic!("PsTerminateSystemThread returned"));
    assert_eq!(
        machine.run(20).unwrap(),
        RunOutcome::Complete {
            exits: [(1, 0), (2, 42)].into()
        }
    );
    assert_eq!(machine.schedule(), &[1, 2, 1]);
    assert_eq!(h.get(machine.thread(2).unwrap().object + 0x120), 42);
}

#[test]
fn thread_creation_and_blocking_failures_preserve_stops_and_outputs() {
    let mut h = Harness::new();
    h.stopped(255, &[DATA, 0, 0x4000, 0, 0, CODE, 0, 0, 0, 0]);
    h.stopped(104, &[]);
    h.stopped(258, &[0]);
    h.stopped(238, &[]);
    let mut cpu = h.cpu;
    cpu.esp = STACK_TOP - 4;
    cpu.eip = CODE;
    h.set(cpu.esp, runtime::exec::HARNESS_EXIT);
    let _machine = Machine::new(
        h.runtime.clone(),
        h.kernel.clone(),
        cpu,
        STACK_BOTTOM,
        STACK_TOP,
    )
    .unwrap();
    h.set(DATA, 0x1234);
    h.stopped(255, &[DATA, 0, 0x4000, 0, 0, CODE, 0, 0, 1, 0]);
    h.stopped(255, &[DATA, 1, 0x4000, 0, 0, CODE, 0, 0, 0, 0]);
    assert_eq!(h.get(DATA), 0x1234);
    assert_eq!(
        h.success(255, &[DATA, 0, 0x4000, 0, 0, 0, 0, 0, 0, 0]),
        status::INVALID_PARAMETER
    );
    assert_eq!(
        h.success(255, &[DATA, 0, u32::MAX, 0, 0, CODE, 0, 0, 0, 0]),
        status::NO_MEMORY
    );
    let t = h.call(255, &[0, 0, 0x4000, 0, 0, CODE, 0, 0, 0, 0]);
    assert!(matches!(
        h.ctx.event(t.event_id),
        Some(Event::MemoryFault { .. })
    ));
    assert_eq!(h.success(104, &[]), _machine.thread(1).unwrap().object);
    assert_eq!(h.success(238, &[]), status::NO_YIELD_PERFORMED);
}

#[test]
fn contended_critical_sections_park_only_the_calling_thread() {
    let mut h = Harness::new();
    let mut machine = attach(&h);
    h.success(291, &[DATA]);
    h.success(277, &[DATA]); // entry thread owns it
    h.success(
        255,
        &[DATA + 0x40, 0, 0x4000, 0, 0, CODE + 0x200, 0, 0, 0, 0],
    );
    h.code(0, |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(238),
            &[],
            CODE + 0x100,
        )
    });
    h.code(0x100, |cpu, ctx, _| {
        assert_eq!(ctx.memory().u32(DATA + 0x80)?, 0);
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(294),
            &[DATA],
            CODE + 0x110,
        )
    });
    h.code(0x110, |cpu, ctx, _| {
        let handle = ctx.memory().u32(DATA + 0x40)?;
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(233),
            &[handle, 0, 0],
            CODE + 0x120,
        )
    });
    h.code(0x120, |cpu, ctx, _| {
        assert_eq!(ctx.memory().u32(DATA + 0x80)?, 1);
        cpu.eax = 0;
        ctx.memory().ret(cpu, 0)
    });
    h.code(0x200, |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(277),
            &[DATA],
            CODE + 0x210,
        )
    });
    h.code(0x210, |cpu, ctx, _| {
        let object = ctx.memory().u32(cpu.fs_base + 0x28)?;
        assert_eq!(ctx.memory().u32(DATA + 24)?, object);
        assert_eq!(ctx.memory().u32(DATA + 20)?, 1);
        ctx.memory().set_u32(DATA + 0x80, 1)?;
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(294),
            &[DATA],
            CODE + 0x220,
        )
    });
    h.code(0x220, |cpu, ctx, _| ctx.memory().ret(cpu, 4));
    assert_eq!(
        machine.run(20).unwrap(),
        RunOutcome::Complete {
            exits: [(1, 0), (2, 0)].into()
        }
    );
    assert_eq!(h.get(DATA + 16), u32::MAX);
}

#[test]
fn event_wait_wakes_fifo_and_deadlocks_are_diagnostics() {
    let mut h = Harness::new();
    let mut machine = attach(&h);
    h.success(108, &[DATA, 1, 0]);
    h.success(
        255,
        &[DATA + 0x40, 0, 0x4000, 0, 0, CODE + 0x200, 0, 0, 0, 0],
    );
    h.code(0, |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(159),
            &[DATA, 0, 0, 0, 0],
            CODE + 0x100,
        )
    });
    h.code(0x100, |cpu, ctx, _| {
        assert_eq!(cpu.eax, 0);
        cpu.eax = 0;
        ctx.memory().ret(cpu, 0)
    });
    h.code(0x200, |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(145),
            &[DATA, 0, 0],
            CODE + 0x210,
        )
    });
    h.code(0x210, |cpu, ctx, _| ctx.memory().ret(cpu, 4));
    assert_eq!(
        machine.run(20).unwrap(),
        RunOutcome::Complete {
            exits: [(1, 0), (2, 0)].into()
        }
    );
    assert_eq!(h.get(DATA + 4), 0);
    let mut h = Harness::new();
    let mut machine = attach(&h);
    h.success(108, &[DATA, 0, 0]);
    h.code(0, |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(159),
            &[DATA, 0, 0, 0, 0],
            CODE + 0x100,
        )
    });
    assert_eq!(
        machine.run(5).unwrap(),
        RunOutcome::Deadlock { threads: vec![1] }
    );
}

#[test]
fn critical_regions_track_per_thread_apc_disable_and_unsupported_stops_persist() {
    let mut h = Harness::new();
    h.stopped(101, &[]);
    h.stopped(122, &[]);
    h.cpu.esp = STACK_TOP - 4;
    h.cpu.eip = CODE;
    h.set(h.cpu.esp, runtime::exec::HARNESS_EXIT);
    let mut machine = attach(&h);
    let object = machine.thread(1).unwrap().object;
    h.success(101, &[]);
    h.success(101, &[]);
    assert_eq!(h.get(object + 0x68), (-2i32) as u32);
    h.success(122, &[]);
    h.success(122, &[]);
    assert_eq!(h.get(object + 0x68), 0);
    h.stopped(122, &[]);
    assert_eq!(h.get(object + 0x68), 0);
    h.code(0, |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(95),
            &[0x123],
            CODE + 0x100,
        )
    });
    let result = machine.run(5).unwrap();
    let RunOutcome::Stopped { thread, transfer } = result else {
        panic!("bugcheck must stop")
    };
    assert_eq!(thread, 1);
    assert!(
        matches!(machine.thread(thread).unwrap().ctx.event(transfer.event_id),Some(Event::KernelCall(call)) if call.ordinal==95)
    );
    assert_eq!(machine.run(5).unwrap(), result);
}

#[test]
fn timed_wait_returns_timeout_without_signaling_and_irql_survives_a_delay() {
    let mut h = Harness::new();
    let mut machine = attach(&h);
    h.success(108, &[DATA, 0, 0]);
    h.runtime
        .memory()
        .set_u64(DATA + 0x40, (-10000i64) as u64)
        .unwrap();
    h.code(0, |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(159),
            &[DATA, 0, 0, 0, DATA + 0x40],
            CODE + 0x100,
        )
    });
    h.code(0x100, |cpu, ctx, _| {
        assert_eq!(cpu.eax, status::TIMEOUT);
        assert_eq!(ctx.memory().u32(DATA + 4)?, 0);
        cpu.ecx = 1;
        ctx.memory()
            .call(cpu, runtime::kernel::function_token(160), CODE + 0x110)
    });
    h.code(0x110, |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(99),
            &[0, 0, DATA + 0x40],
            CODE + 0x120,
        )
    });
    h.code(0x120, |cpu, ctx, _| {
        assert_eq!(cpu.eax, 0);
        assert_eq!(ctx.memory().u8(cpu.fs_base + 0x24)?, 1);
        cpu.ecx = 0;
        ctx.memory()
            .call(cpu, runtime::kernel::function_token(161), CODE + 0x130)
    });
    h.code(0x130, |cpu, ctx, _| {
        cpu.eax = 0;
        ctx.memory().ret(cpu, 0)
    });
    assert_eq!(
        machine.run(10).unwrap(),
        RunOutcome::Complete {
            exits: [(1, 0)].into()
        }
    );
    assert_eq!(h.kernel.clock().counter(), 20_000);
}

#[test]
fn owned_thread_start_completion_survives_an_unsupported_kernel_stop_and_retry() {
    let h = Harness::new();
    let mut machine = attach(&h);
    h.code(0, |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(255),
            &[DATA, 0, 0x4000, 0, 0, CODE + 0x200, 0, 0, 0, 0],
            CODE + 0x100,
        )
    });
    h.code(0x100, |cpu, ctx, _| {
        let handle = ctx.memory().u32(DATA)?;
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(233),
            &[handle, 0, 0],
            CODE + 0x110,
        )
    });
    h.code(0x110, |cpu, ctx, _| {
        cpu.eax = 0;
        ctx.memory().ret(cpu, 0)
    });
    h.code(0x200, |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(346),
            &[DATA, 4],
            CODE + 0x210,
        )
    });
    h.code(0x210, |cpu, ctx, _| {
        assert_eq!(cpu.eax, 4);
        ctx.memory().ret(cpu, 4)
    });
    let result = machine.run(10).unwrap();
    let RunOutcome::Stopped {
        thread: 2,
        transfer,
    } = result
    else {
        panic!("expected child kernel boundary")
    };
    assert!(
        matches!(machine.thread(2).unwrap().ctx.event(transfer.event_id),Some(Event::KernelCall(call)) if call.ordinal==346)
    );
    h.runtime.edit_dispatch(|d| {
        kernel::abi::register(
            d,
            kernel::abi::Export::stdcall(346, "synthetic comparison", 2),
            |args, memory| {
                assert_eq!(args.get(0), DATA);
                assert_eq!(args.get(1), 4);
                let _ = memory.u32(args.get(0))?;
                Ok(kernel::abi::Value::U32(4))
            },
        )
    });
    machine.retry_kernel_call().unwrap();
    assert_eq!(
        machine.run(10).unwrap(),
        RunOutcome::Complete {
            exits: [(1, 0), (2, 0)].into()
        }
    );
}

#[test]
fn suspended_thread_keeps_a_canonical_stack_before_its_first_schedule() {
    let h = Harness::new();
    let mut machine = attach(&h);
    h.code(0, |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(255),
            &[DATA, 0, 0x4000, 0, 0, CODE + 0x200, 0, 1, 0, 0],
            CODE + 0x100,
        )
    });
    h.code(0x100, |cpu, ctx, _| {
        cpu.eax = 0;
        ctx.memory().ret(cpu, 0)
    });
    assert_eq!(
        machine.run(10).unwrap(),
        RunOutcome::Deadlock { threads: vec![2] }
    );
    let child = machine.thread(2).unwrap();
    assert_eq!(h.runtime.memory().u8(child.object + 0x2c).unwrap(), 5);
    assert_eq!(h.get(child.object + 0x24), child.cpu.esp);
    assert_eq!(machine.schedule(), &[1]);
}

#[test]
fn thread_creation_with_a_full_handle_table_maps_nothing() {
    let mut h = Harness::new();
    let _machine = attach(&h);
    h.fill_handles();
    let before = h.runtime.memory().mappings().len();
    for _ in 0..3 {
        assert_eq!(
            h.success(255, &[DATA, 0, 0x4000, 0, 0, CODE + 0x200, 0, 1, 0, 0]),
            status::INSUFFICIENT_RESOURCES
        );
    }
    assert_eq!(
        h.runtime.memory().mappings().len(),
        before,
        "a failed creation left its stack, guard or object mapped"
    );
}

mod object_references {
    use super::*;
    use runtime::kernel::function_token as token;

    const THREAD_TYPE: u32 = DATA + 0xa00;
    const EVENT_TYPE: u32 = DATA + 0xa10;
    const OBJECT: u32 = DATA + 0xa20;
    const EVENT: u32 = DATA + 0xa24;
    const CURRENT: u32 = 0xffff_fffe;

    fn harness() -> Harness {
        let mut config = kernel::Config::default();
        config.data_exports.insert(259, THREAD_TYPE); // PsThreadObjectType
        config.data_exports.insert(16, EVENT_TYPE); // ExEventObjectType
        Harness::config(config)
    }

    /// XAPI SetThreadPriority (0x2d1fb6): reference, set, dereference.
    #[test]
    fn set_thread_priority_references_the_thread_and_records_its_base_priority() {
        let h = harness();
        h.code(0, |cpu, ctx, _| {
            guest_call(
                cpu,
                ctx,
                token(246),
                &[CURRENT, THREAD_TYPE, OBJECT],
                CODE + 0x10,
            )
        });
        h.code(0x10, |cpu, ctx, _| {
            assert_eq!(cpu.eax, 0);
            let thread = ctx.memory().u32(OBJECT)?;
            assert_eq!(thread, ctx.memory().u32(cpu.fs_base + 0x28)?);
            {
                let m = ctx.memory();
                // A new thread starts at the process base priority, 8.
                assert_eq!((m.u8(thread + 0x70)?, m.u8(thread + 0x32)?), (8, 8));
            }
            // THREAD_PRIORITY_TIME_CRITICAL (15) becomes 16: saturation.
            guest_call(cpu, ctx, token(143), &[thread, 16], CODE + 0x20)
        });
        h.code(0x20, |cpu, ctx, _| {
            assert_eq!(cpu.eax, 0, "old increment");
            let thread = ctx.memory().u32(OBJECT)?;
            {
                let m = ctx.memory();
                assert_eq!(m.u8(thread + 0x31)?, 1, "saturated");
                assert_eq!(m.u8(thread + 0x32)?, 15, "top of the variable class");
                assert_eq!(m.u8(thread + 0x70)?, 15);
            }
            guest_call(cpu, ctx, token(143), &[thread, (-2i32) as u32], CODE + 0x30)
        });
        h.code(0x30, |cpu, ctx, _| {
            assert_eq!(cpu.eax, 16, "a saturated thread reports +16");
            let thread = ctx.memory().u32(OBJECT)?;
            {
                let m = ctx.memory();
                assert_eq!(m.u8(thread + 0x31)?, 0);
                assert_eq!((m.u8(thread + 0x70)?, m.u8(thread + 0x32)?), (6, 6));
            }
            guest_call(
                cpu,
                ctx,
                token(143),
                &[thread, (-16i32) as u32],
                CODE + 0x40,
            )
        });
        h.code(0x40, |cpu, ctx, _| {
            assert_eq!(cpu.eax, (-2i32) as u32);
            let thread = ctx.memory().u32(OBJECT)?;
            assert_eq!(ctx.memory().u8(thread + 0x31)?, 0xff, "saturated low");
            assert_eq!(ctx.memory().u8(thread + 0x32)?, 1, "idle");
            cpu.ecx = thread;
            guest_call(cpu, ctx, token(250), &[], CODE + 0x50)
        });
        h.code(0x50, |cpu, ctx, _| {
            // Wrong type: an event handle as a thread.
            guest_call(cpu, ctx, token(189), &[EVENT, 0, 0, 0], CODE + 0x60)
        });
        h.code(0x60, |cpu, ctx, _| {
            let event = ctx.memory().u32(EVENT)?;
            guest_call(
                cpu,
                ctx,
                token(246),
                &[event, THREAD_TYPE, OBJECT],
                CODE + 0x70,
            )
        });
        h.code(0x70, |cpu, ctx, _| {
            assert_eq!(cpu.eax, status::OBJECT_TYPE_MISMATCH);
            let event = ctx.memory().u32(EVENT)?;
            guest_call(
                cpu,
                ctx,
                token(246),
                &[event, EVENT_TYPE, OBJECT],
                CODE + 0x80,
            )
        });
        h.code(0x80, |cpu, ctx, _| {
            assert_eq!(cpu.eax, 0);
            // Closing the only handle leaves the referenced event alive.
            let event = ctx.memory().u32(EVENT)?;
            guest_call(cpu, ctx, token(187), &[event], CODE + 0x90)
        });
        h.code(0x90, |cpu, ctx, _| {
            let object = ctx.memory().u32(OBJECT)?;
            // Still a valid event: KeSetEvent works on the referenced body.
            guest_call(cpu, ctx, token(145), &[object, 0, 0], CODE + 0xa0)
        });
        h.code(0xa0, |cpu, ctx, _| {
            assert_eq!(cpu.eax, 0);
            cpu.ecx = ctx.memory().u32(OBJECT)?;
            guest_call(cpu, ctx, token(250), &[], CODE + 0xb0)
        });
        h.code(0xb0, |cpu, ctx, _| {
            cpu.eax = 0;
            ctx.memory().ret(cpu, 0)
        });
        let mut machine = attach(&h);
        let outcome = machine.run(20).unwrap();
        assert!(
            matches!(outcome, RunOutcome::Complete { .. }),
            "{outcome:?}"
        );
        assert!(h.kernel.stop_reasons().is_empty());
    }

    #[test]
    fn object_references_reject_bad_handles_unknown_types_and_extra_dereferences() {
        let mut h = harness();
        assert_eq!(h.success(246, &[0x7ff0, 0, OBJECT]), status::INVALID_HANDLE);
        assert_eq!(h.success(189, &[EVENT, 0, 0, 0]), 0);
        let event = h.get(EVENT);
        // An OBJECT_TYPE address that is not an imported type export.
        h.stopped(246, &[event, DATA + 0xb00, OBJECT]);
        // A null type accepts any dispatcher object.
        assert_eq!(h.success(246, &[event, 0, OBJECT]), 0);
        let object = h.get(OBJECT);
        h.success(250, &[object]);
        // More dereferences than references stop.
        h.stopped(250, &[object]);
        // The pseudo-handle and KeSetBasePriorityThread need Machine threads.
        h.stopped(246, &[CURRENT, THREAD_TYPE, OBJECT]);
        h.stopped(143, &[object, 1]);
    }
}
